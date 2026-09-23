//! Turns a workflow gate into a verdict conductor witnessed itself.
//!
//! Every path out of here is one of: passed, failed, flaky (mixed results across reruns), or
//! unwitnessed (the check could not be carried out, and the reason says why). A gate never
//! passes by default, and an unparseable assertion is never read as `false`.

use crate::assert::{self, Evaluated, Fact, Facts};
use crate::cargo_test::{self, Outcome, TestReport};
use crate::git;
use crate::mutants::{self, MutationReport};
use crate::runner::{self, Execution};
use crate::scope::{self, ScopeReport};
use conductor_model::Verdict;
use conductor_model::workflow::{Gate, MutationTool, Parser};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What a gate needs to know about the stage it checks. Paths and patterns arrive already
/// resolved: no `{{…}}` templates.
#[derive(Debug, Clone)]
pub struct Context<'a> {
    pub worktree: &'a Path,
    /// The commit the stage started from.
    pub base: &'a str,
    pub run_id: &'a str,
    pub timeout: Duration,
    pub write: &'a [String],
    pub frozen: &'a [String],
    pub protected: &'a [String],
    /// Files build tools rewrite, allowed in any stage (the repository policy's `generated`).
    pub generated: &'a [String],
    /// Output id → path relative to the worktree.
    pub outputs: &'a BTreeMap<String, String>,
    /// Test names that existed before this stage, so `tests_new` can be counted.
    pub baseline_tests: Option<&'a BTreeSet<String>>,
    /// Where scratch files for this gate may go (a diff for cargo-mutants, its output).
    pub scratch: &'a Path,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GateResult {
    pub gate: &'static str,
    pub verdict: Verdict,
    /// One plain sentence for the receipt.
    pub detail: String,
    pub executions: Vec<Execution>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<AssertionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<ScopeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutation: Option<MutationReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestReport>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AssertionResult {
    pub expr: String,
    pub run: usize,
    pub holds: bool,
    pub observed: String,
}

impl From<(usize, Evaluated)> for AssertionResult {
    fn from((run, e): (usize, Evaluated)) -> Self {
        AssertionResult {
            expr: e.expr,
            run,
            holds: e.holds,
            observed: e.observed,
        }
    }
}

impl GateResult {
    fn new(gate: &'static str, verdict: Verdict, detail: impl Into<String>) -> Self {
        GateResult {
            gate,
            verdict,
            detail: detail.into(),
            executions: vec![],
            assertions: vec![],
            scope: None,
            mutation: None,
            tests: None,
        }
    }

    fn unwitnessed(gate: &'static str, why: impl Into<String>) -> Self {
        Self::new(gate, Verdict::Unwitnessed, why)
    }
}

pub fn evaluate(gate: &Gate, ctx: &Context) -> GateResult {
    match gate {
        Gate::Scope => scope_gate(ctx),
        Gate::FileNonempty { output } => file_nonempty(output, ctx),
        Gate::Schema { .. } => GateResult::unwitnessed("schema", "schema gates arrive in v0.2"),
        Gate::CommandAssert {
            command,
            parser,
            assert,
            reruns,
        } => command_assert(command, *parser, assert, *reruns, ctx),
        Gate::Mutation {
            tool,
            in_diff,
            min_score,
        } => mutation(*tool, *in_diff, *min_score, ctx),
    }
}

fn scope_gate(ctx: &Context) -> GateResult {
    let changed = match git::changed_files(ctx.worktree, ctx.base) {
        Ok(c) => c,
        Err(e) => {
            return GateResult::unwitnessed("scope", format!("could not list changed files: {e}"));
        }
    };
    let report = match scope::check(
        &changed,
        ctx.write,
        ctx.frozen,
        ctx.protected,
        ctx.generated,
    ) {
        Ok(r) => r,
        Err(e) => return GateResult::unwitnessed("scope", e.to_string()),
    };
    let (verdict, detail) = if report.passed() {
        (
            Verdict::Passed,
            match report.generated.len() {
                0 => format!("{} file(s) changed, all within scope", report.changed.len()),
                n => format!(
                    "{} file(s) changed, all within scope; {n} written by tooling",
                    report.changed.len()
                ),
            },
        )
    } else {
        let v = &report.violations[0];
        let more = report.violations.len() - 1;
        let more = if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        };
        (
            Verdict::Failed,
            format!("{} is {}{more}", v.path, v.breach.describe()),
        )
    };
    GateResult {
        scope: Some(report),
        ..GateResult::new("scope", verdict, detail)
    }
}

fn file_nonempty(output: &str, ctx: &Context) -> GateResult {
    let Some(rel) = ctx.outputs.get(output) else {
        return GateResult::unwitnessed(
            "file_nonempty",
            format!("the stage declares no output `{output}`"),
        );
    };
    let path = ctx.worktree.join(rel);
    match std::fs::metadata(&path) {
        Ok(m) if m.is_file() && m.len() > 0 => GateResult::new(
            "file_nonempty",
            Verdict::Passed,
            format!("{rel} written ({} bytes)", m.len()),
        ),
        Ok(_) => GateResult::new("file_nonempty", Verdict::Failed, format!("{rel} is empty")),
        Err(_) => GateResult::new(
            "file_nonempty",
            Verdict::Failed,
            format!("{rel} was not written"),
        ),
    }
}

/// Facts a `command_assert` over test output may name.
pub fn test_facts(
    report: &TestReport,
    exit_code: Option<i32>,
    baseline: Option<&BTreeSet<String>>,
) -> Facts {
    let n = |x: usize| Fact::Num(x as f64);
    let mut f = Facts::new();
    if let Some(c) = report.compiled {
        f.insert("compiled".into(), Fact::Bool(c));
    }
    f.insert("compile_errors".into(), n(report.compile_errors));
    f.insert("tests_run".into(), n(report.tests_run()));
    f.insert("tests_passed".into(), n(report.count(Outcome::Passed)));
    f.insert("tests_failed".into(), n(report.count(Outcome::Failed)));
    f.insert("tests_ignored".into(), n(report.count(Outcome::Ignored)));
    if let Some(code) = exit_code {
        f.insert("exit_code".into(), Fact::Num(f64::from(code)));
    }
    let record = |t: &cargo_test::TestCase| {
        let mut r = BTreeMap::new();
        r.insert("name".into(), Fact::Text(t.name.clone()));
        r.insert(
            "kind".into(),
            Fact::Text(t.failure.map_or("none", |k| k.name()).into()),
        );
        r
    };
    f.insert(
        "failures".into(),
        Fact::List(
            report
                .tests
                .iter()
                .filter(|t| t.outcome == Outcome::Failed)
                .map(record)
                .collect(),
        ),
    );
    if let Some(base) = baseline {
        let new: Vec<_> = report
            .tests
            .iter()
            .filter(|t| !base.contains(&t.name))
            .collect();
        f.insert("tests_new".into(), n(new.len()));
        f.insert(
            "new_tests".into(),
            Fact::List(
                new.iter()
                    .map(|t| {
                        let mut r = record(t);
                        let outcome = match t.outcome {
                            Outcome::Passed => "passed",
                            Outcome::Failed => "failed",
                            Outcome::Ignored => "ignored",
                        };
                        r.insert("outcome".into(), Fact::Text(outcome.into()));
                        r
                    })
                    .collect(),
            ),
        );
    }
    f
}

fn command_assert(
    command: &[String],
    parser: Parser,
    asserts: &[String],
    reruns: u32,
    ctx: &Context,
) -> GateResult {
    const NAME: &str = "command_assert";
    if parser == Parser::JunitXml {
        return GateResult::unwitnessed(NAME, "the junit_xml parser arrives in v0.2");
    }
    let mut result = GateResult::new(NAME, Verdict::Pending, "");
    let mut outcomes = Vec::new();
    for run in 0..=reruns as usize {
        let exec = runner::run(&runner::Spec {
            argv: command,
            cwd: ctx.worktree,
            timeout: ctx.timeout,
            pass_env: &[],
            run_id: ctx.run_id,
            inherit_env: false,
            set_env: &[],
        });
        if !exec.complete() {
            let why = exec
                .reason
                .clone()
                .unwrap_or_else(|| "the command did not finish".into());
            result.executions.push(exec);
            result.verdict = Verdict::Unwitnessed;
            result.detail = format!("`{}` could not be witnessed: {why}", command.join(" "));
            return result;
        }
        let report = cargo_test::parse(&exec.stdout_text());
        let facts = test_facts(&report, exec.exit_code, ctx.baseline_tests);
        let mut all_hold = true;
        for a in asserts {
            match assert::evaluate(a, &facts) {
                Ok(e) => {
                    all_hold &= e.holds;
                    result.assertions.push((run, e).into());
                }
                Err(err) => {
                    result.executions.push(exec);
                    result.verdict = Verdict::Unwitnessed;
                    result.detail = err.to_string();
                    return result;
                }
            }
        }
        if asserts.is_empty() {
            all_hold = exec.exit_code == Some(0);
        }
        outcomes.push(all_hold);
        result.tests = Some(report);
        result.executions.push(exec);
    }
    let runs = outcomes.len();
    let passed = outcomes.iter().filter(|b| **b).count();
    let failing: Vec<&str> = result
        .assertions
        .iter()
        .filter(|a| !a.holds)
        .map(|a| a.expr.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let tests = result
        .tests
        .as_ref()
        .map(|t| {
            format!(
                "{} passed, {} failed",
                t.count(Outcome::Passed),
                t.count(Outcome::Failed)
            )
        })
        .unwrap_or_default();
    let times = if runs > 1 {
        format!(" on all {runs} runs")
    } else {
        String::new()
    };
    (result.verdict, result.detail) = if passed == runs {
        (
            Verdict::Passed,
            format!("{tests}; every assertion held{times}"),
        )
    } else if passed == 0 {
        (
            Verdict::Failed,
            format!("{tests}; did not hold: {}", failing.join(", ")),
        )
    } else {
        (
            Verdict::Flaky,
            format!("held on {passed} of {runs} runs, so it counts as flaky, not passed"),
        )
    };
    result
}

fn mutation(tool: MutationTool, in_diff: bool, min_score: f64, ctx: &Context) -> GateResult {
    const NAME: &str = "mutation";
    let MutationTool::CargoMutants = tool;
    let out_dir: PathBuf = ctx.scratch.join("mutants");
    let mut argv: Vec<String> = vec![
        "cargo".into(),
        "mutants".into(),
        "--no-shuffle".into(),
        "--output".into(),
        out_dir.display().to_string(),
    ];
    if in_diff {
        let diff = match std::process::Command::new("git")
            .arg("-C")
            .arg(ctx.worktree)
            .args(["diff", ctx.base])
            .output()
        {
            Ok(o) if o.status.success() => o.stdout,
            Ok(o) => {
                return GateResult::unwitnessed(
                    NAME,
                    format!(
                        "could not diff against the base: {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    ),
                );
            }
            Err(e) => return GateResult::unwitnessed(NAME, format!("could not run git: {e}")),
        };
        if diff.is_empty() {
            return GateResult::unwitnessed(
                NAME,
                "the stage changed nothing, so there is nothing to mutate",
            );
        }
        let diff_path = ctx.scratch.join("stage.diff");
        if let Err(e) =
            std::fs::create_dir_all(ctx.scratch).and_then(|_| std::fs::write(&diff_path, &diff))
        {
            return GateResult::unwitnessed(NAME, format!("could not write the diff: {e}"));
        }
        argv.push("--in-diff".into());
        argv.push(diff_path.display().to_string());
    }
    let exec = runner::run(&runner::Spec {
        argv: &argv,
        cwd: ctx.worktree,
        timeout: ctx.timeout,
        pass_env: &[],
        run_id: ctx.run_id,
        inherit_env: false,
        set_env: &[],
    });
    let mut result = GateResult::new(NAME, Verdict::Unwitnessed, "");
    let outcomes = out_dir.join("mutants.out").join("outcomes.json");
    let read = std::fs::read_to_string(&outcomes);
    let complete = exec.complete();
    let exit = exec.exit_code;
    result.executions.push(exec);
    if !complete {
        result.detail = "cargo-mutants did not finish".into();
        return result;
    }
    // 0: every mutant caught; 2: some missed; 3: some timed out. Anything else means the
    // baseline failed or the tool couldn't run, and nothing about the tests was learned.
    if !matches!(exit, Some(0 | 2 | 3)) {
        result.detail = format!(
            "cargo-mutants exited with {exit:?}; is it installed, and do the tests pass unmutated?"
        );
        return result;
    }
    let report = match read
        .map_err(|e| e.to_string())
        .and_then(|j| mutants::parse(&j).map_err(|e| e.to_string()))
    {
        Ok(r) => r,
        Err(e) => {
            result.detail = format!("could not read cargo-mutants output: {e}");
            return result;
        }
    };
    match report.score() {
        None => {
            result.detail = "no viable mutants in this change, so the tests weren't tested".into()
        }
        Some(score) => {
            let caught = report.caught + report.timeout;
            let tested = caught + report.missed;
            result.verdict = if score + 1e-9 >= min_score {
                Verdict::Passed
            } else {
                Verdict::Failed
            };
            result.detail = format!(
                "caught {caught} of {tested} injected bugs · {score:.2}, needs {min_score:.2}"
            );
        }
    }
    result.mutation = Some(report);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn sh(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A tiny crate in a git repository, committed as the base.
    fn project(lib: &str) -> (tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::create_dir_all(p.join("src")).unwrap();
        fs::write(
            p.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        )
        .unwrap();
        fs::write(p.join("src/lib.rs"), lib).unwrap();
        fs::write(p.join(".gitignore"), "/target\n").unwrap();
        sh(p, &["init", "-q", "-b", "main"]);
        sh(p, &["config", "user.email", "t@example.com"]);
        sh(p, &["config", "user.name", "t"]);
        sh(p, &["add", "."]);
        sh(p, &["commit", "-q", "-m", "base"]);
        let base = git::head(p).unwrap();
        (d, base)
    }

    struct Owned {
        write: Vec<String>,
        frozen: Vec<String>,
        protected: Vec<String>,
        generated: Vec<String>,
        outputs: BTreeMap<String, String>,
        scratch: tempfile::TempDir,
    }

    fn owned() -> Owned {
        Owned {
            write: vec!["src/**".into()],
            frozen: vec!["tests/locked.rs".into()],
            protected: vec!["Cargo.lock".into()],
            generated: vec![],
            outputs: BTreeMap::from([("spec".into(), "spec.md".into())]),
            scratch: tempfile::tempdir().unwrap(),
        }
    }

    fn ctx<'a>(dir: &'a Path, base: &'a str, o: &'a Owned) -> Context<'a> {
        Context {
            worktree: dir,
            base,
            run_id: "r1",
            timeout: Duration::from_secs(240),
            write: &o.write,
            frozen: &o.frozen,
            protected: &o.protected,
            generated: &o.generated,
            outputs: &o.outputs,
            baseline_tests: None,
            scratch: o.scratch.path(),
        }
    }

    const LIB: &str = "pub fn clamp(x: i32) -> i32 { if x > 10 { 10 } else { x } }\n";

    #[test]
    fn scope_passes_inside_and_fails_on_a_locked_file() {
        let (d, base) = project(LIB);
        let o = owned();
        fs::write(d.path().join("src/lib.rs"), format!("{LIB}// edit\n")).unwrap();
        assert_eq!(
            evaluate(&Gate::Scope, &ctx(d.path(), &base, &o)).verdict,
            Verdict::Passed
        );
        fs::create_dir_all(d.path().join("tests")).unwrap();
        fs::write(d.path().join("tests/locked.rs"), "").unwrap();
        let r = evaluate(&Gate::Scope, &ctx(d.path(), &base, &o));
        assert_eq!(r.verdict, Verdict::Failed);
        assert!(
            r.detail
                .contains("tests/locked.rs is locked by an earlier stage"),
            "{}",
            r.detail
        );
    }

    #[test]
    fn file_nonempty_needs_real_content() {
        let (d, base) = project(LIB);
        let o = owned();
        let g = Gate::FileNonempty {
            output: "spec".into(),
        };
        assert_eq!(
            evaluate(&g, &ctx(d.path(), &base, &o)).verdict,
            Verdict::Failed
        );
        fs::write(d.path().join("spec.md"), "").unwrap();
        assert_eq!(
            evaluate(&g, &ctx(d.path(), &base, &o)).verdict,
            Verdict::Failed
        );
        fs::write(d.path().join("spec.md"), "# spec").unwrap();
        assert_eq!(
            evaluate(&g, &ctx(d.path(), &base, &o)).verdict,
            Verdict::Passed
        );
    }

    fn cargo_test_gate(asserts: &[&str], reruns: u32) -> Gate {
        Gate::CommandAssert {
            command: [
                "cargo",
                "test",
                "--no-fail-fast",
                "--message-format",
                "json",
            ]
            .map(String::from)
            .to_vec(),
            parser: Parser::CargoJson,
            assert: asserts.iter().map(|s| s.to_string()).collect(),
            reruns,
        }
    }

    #[test]
    fn red_for_the_right_reason_is_told_apart_from_assert_false() {
        let honest = format!(
            "{LIB}\n#[cfg(test)] mod t {{ use super::*; #[test] fn caps() {{ assert_eq!(clamp(50), 9); }} }}\n"
        );
        let (d, base) = project(&honest);
        let o = owned();
        let gate = cargo_test_gate(
            &[
                "compiled == true",
                "tests_failed > 0",
                "failures.kind all == 'assertion'",
            ],
            0,
        );
        let r = evaluate(&gate, &ctx(d.path(), &base, &o));
        assert_eq!(r.verdict, Verdict::Passed, "{}", r.detail);

        fs::write(
            d.path().join("src/lib.rs"),
            format!("{LIB}\n#[cfg(test)] mod t {{ #[test] fn fake() {{ assert!(false); }} }}\n"),
        )
        .unwrap();
        let r = evaluate(&gate, &ctx(d.path(), &base, &o));
        assert_eq!(r.verdict, Verdict::Failed, "{}", r.detail);
        assert!(
            r.detail.contains("failures.kind all == 'assertion'"),
            "{}",
            r.detail
        );
    }

    #[test]
    fn green_tests_pass_on_every_rerun() {
        let lib = format!(
            "{LIB}\n#[cfg(test)] mod t {{ use super::*; #[test] fn caps() {{ assert_eq!(clamp(50), 10); }} }}\n"
        );
        let (d, base) = project(&lib);
        let o = owned();
        let r = evaluate(
            &cargo_test_gate(&["tests_failed == 0", "tests_run > 0"], 1),
            &ctx(d.path(), &base, &o),
        );
        assert_eq!(r.verdict, Verdict::Passed, "{}", r.detail);
        assert_eq!(r.executions.len(), 2);
        assert!(r.detail.contains("on all 2 runs"), "{}", r.detail);
    }

    #[test]
    fn an_assertion_nobody_can_evaluate_is_unwitnessed_not_failed() {
        let (d, base) = project(LIB);
        let o = owned();
        let r = evaluate(
            &cargo_test_gate(&["coverage > 80"], 0),
            &ctx(d.path(), &base, &o),
        );
        assert_eq!(r.verdict, Verdict::Unwitnessed, "{}", r.detail);
    }

    #[test]
    fn a_flaky_test_is_flaky_not_passed() {
        // Fails on the first run, passes after: the marker file flips it.
        let lib = format!(
            "{LIB}\n#[cfg(test)] mod t {{ #[test] fn flips() {{ let p = std::path::Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"ran\"); let first = !p.exists(); std::fs::write(&p, \"\").unwrap(); assert!(!first, \"first run fails\"); }} }}\n"
        );
        let (d, base) = project(&lib);
        let o = owned();
        let r = evaluate(
            &cargo_test_gate(&["tests_failed == 0"], 1),
            &ctx(d.path(), &base, &o),
        );
        assert_eq!(r.verdict, Verdict::Flaky, "{}", r.detail);
    }

    #[test]
    fn mutation_scores_the_change_and_names_survivors() {
        if Command::new("cargo")
            .args(["mutants", "--version"])
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            eprintln!("cargo-mutants is not installed; skipping");
            return;
        }
        let (d, base) = project("pub fn noop() {}\n");
        let o = owned();
        fs::write(
            d.path().join("src/lib.rs"),
            format!("{LIB}\n#[cfg(test)] mod t {{ use super::*; #[test] fn caps() {{ assert_eq!(clamp(50), 10); }} }}\n"),
        )
        .unwrap();
        let g = Gate::Mutation {
            tool: MutationTool::CargoMutants,
            in_diff: true,
            min_score: 0.9,
        };
        let r = evaluate(&g, &ctx(d.path(), &base, &o));
        let m = r
            .mutation
            .as_ref()
            .unwrap_or_else(|| panic!("no report: {}", r.detail));
        assert!(
            m.missed > 0,
            "`>` vs `>=` at the boundary is untested: {m:?}"
        );
        assert_eq!(r.verdict, Verdict::Failed, "{}", r.detail);
        assert!(m.survivors.iter().any(|s| s.what.contains("clamp")));
    }

    #[test]
    fn mutation_of_no_change_is_unwitnessed() {
        let (d, base) = project(LIB);
        let o = owned();
        let g = Gate::Mutation {
            tool: MutationTool::CargoMutants,
            in_diff: true,
            min_score: 0.7,
        };
        assert_eq!(
            evaluate(&g, &ctx(d.path(), &base, &o)).verdict,
            Verdict::Unwitnessed
        );
    }
}
