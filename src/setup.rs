//! `conductor init` and `conductor doctor`: getting a repository ready, and checking it is.

use anyhow::Result;
use conductor_model::Workflow;
use conductor_model::workflow::Gate;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, ExitCode};

const WORKFLOW: &str = r#"# One agent writes failing tests for the work; another makes them pass without being able
# to touch them. Every check is run by conductor, not by the agents.
#
# Conductor reads this file from the commit a run starts at, never from your working tree:
# commit changes to it before they take effect.
id: build
version: 1
kind: build

defaults:
  # A failed check is sent back to the agent; the second retry starts from a clean tree.
  retries: { max: 2, ladder: [in_context, fresh] }

budget:
  max_stage_wall_clock_sec: 900
  max_mutation_wall_clock_sec: 600

# Turn a passed run into a draft pull request with its receipt (`conductor deliver`).
# deliver: { base: main }

stages:
  - id: tests
    agent:
      kind: claude
      allowed_tools: ["Bash(cargo test:*)", "Bash(cargo build:*)"]
    prompt_file: .conductor/prompts/tests.md
    # Tests, plus `todo!()` stubs in src/ so tests of new code compile.
    scope: { write: ["tests/**", "src/**"] }
    gates:
      - { type: scope }
      # The new tests must compile, and every one of them must fail for the right reason
      # (an assertion or a `todo!()`, not a trivially false test). A stub that quietly
      # implements the work would make some pass, and fail this check.
      - type: command_assert
        command: ["cargo", "test", "--no-fail-fast", "--message-format", "json"]
        parser: cargo_json
        assert:
          - "compiled == true"
          - "tests_new > 0"
          - "tests_failed == tests_new"
          - "failures.kind all != 'trivial'"

  - id: implement
    depends_on: [tests]
    agent:
      kind: claude
      allowed_tools: ["Bash(cargo test:*)", "Bash(cargo build:*)"]
    prompt_file: .conductor/prompts/implement.md
    scope:
      write: ["src/**"]
      frozen: ["tests/**"]      # the tests from the stage above are locked
    gates:
      - { type: scope }
      - type: command_assert
        command: ["cargo", "test", "--message-format", "json"]
        parser: cargo_json
        reruns: 1               # passing twice rules out a lucky run
        # tests_new == 0: the implementer adds no tests of its own, even inside src/.
        assert: ["tests_failed == 0", "tests_run > 0", "tests_new == 0"]
      # Bugs injected into the changed code must be caught by the tests.
      - { type: mutation, tool: cargo_mutants, in_diff: true, min_score: 0.6 }
      # If your CI requires formatting and lints, check them here too, so a run can't
      # pass what CI would fail (the repository must already be clean at its base), and
      # let the agent run them: add "Bash(cargo fmt:*)" and "Bash(cargo clippy:*)" to
      # allowed_tools above.
      # - { type: command_assert, command: ["cargo", "fmt", "--all", "--check"], parser: exit }
      # - { type: command_assert, command: ["cargo", "clippy", "--all-targets", "--", "-D", "warnings"], parser: exit }
"#;

const TESTS_PROMPT: &str = "\
You write tests only. Write Rust integration tests under tests/ for the work described
below. Cover normal cases, boundaries and malformed input.

If the tests call something that does not exist yet, add it under src/ with its real
signature and a body of `todo!()`, and export it, so the tests compile. Nothing more: do
not implement anything. Every test you add must fail. Run `cargo test` to confirm they
compile and fail.
";

const IMPLEMENT_PROMPT: &str = "\
You implement. Make the tests under tests/ pass by changing src/ only. The tests are
locked: do not edit them. Run `cargo test` to check your work.
";

const POLICY: &str = "\
# Repository policy, read from the commit a run starts at.
#
# Paths no agent may change, in any workflow. conductor always protects its own config
# (.conductor/*.yaml, .conductor/workflows/**, .conductor/prompts/**) and .github/**.
protected: []

# Files build tools rewrite, which any stage may change. They are listed on the receipt as
# not reviewed. Leave unset for the usual lockfiles (Cargo.lock, package-lock.json, …);
# `generated: []` allows none.
# generated: [Cargo.lock]
";

const TASK: &str = "\
# What to build

Describe the change here: what it should do, with examples of inputs and outputs, and
anything it must not do. Both agents get this text. Then run:

    conductor run .conductor/workflows/build.yaml --spec .conductor/task.md
";

/// A test runner `init` knows how to start a non-Rust repository with. Everything else in
/// the workflow is the same for every language: the checks read JUnit XML.
struct Stack {
    name: String,
    /// The test command as a YAML flow sequence, and where its report goes.
    command: String,
    report: Option<&'static str>,
    allowed: Vec<String>,
    tests: Vec<&'static str>,
    sources: Vec<&'static str>,
    /// How a test-writer makes new code exist without implementing it.
    stub: &'static str,
    /// The `setup:` a run's fresh checkout needs, as a YAML flow sequence of commands.
    setup: Option<&'static str>,
    /// Paths the runner writes into the repository, for .gitignore.
    ignore: Vec<&'static str>,
    /// Things to do before the first run, printed after init.
    notes: Vec<String>,
}

fn quoted(items: &[impl AsRef<str>]) -> String {
    items
        .iter()
        .map(|i| format!("{:?}", i.as_ref()))
        .collect::<Vec<_>>()
        .join(", ")
}

impl Stack {
    /// The project type at `repo`'s root, or a generic starter to edit.
    fn detect(repo: &Path) -> Stack {
        let has = |f: &str| repo.join(f).exists();
        let src_or = |fallback: &'static str| {
            if has("src") { "src/**" } else { fallback }
        };
        if has("package.json") {
            let pkg = std::fs::read_to_string(repo.join("package.json")).unwrap_or_default();
            let tests = vec![
                "**/*.test.*",
                "**/*.spec.*",
                "**/__tests__/**",
                "test/**",
                "tests/**",
            ];
            let stub = "a body that throws `new Error(\"not implemented\")`";
            if pkg.contains("\"jest\"") && !pkg.contains("\"vitest\"") {
                let mut notes = vec![];
                if !pkg.contains("jest-junit") {
                    notes.push(
                        "install the JUnit reporter the checks read: `npm i -D jest-junit`".into(),
                    );
                }
                return Stack {
                    name: "JavaScript (jest)".into(),
                    command: r#"["npx", "jest", "--ci", "--reporters=default", "--reporters=jest-junit"]"#.into(),
                    report: Some("junit.xml"),
                    allowed: vec!["Bash(npx jest:*)".into(), "Bash(npm test:*)".into()],
                    tests,
                    sources: vec![src_or("**")],
                    stub,
                    setup: node_setup(repo),
                    ignore: vec!["/junit.xml"],
                    notes,
                };
            }
            let mut notes = vec![];
            if !pkg.contains("\"vitest\"") {
                notes.push("no test runner found in package.json; the checks assume vitest (`npm i -D vitest`)".into());
            }
            return Stack {
                name: "JavaScript (vitest)".into(),
                command: r#"["npx", "vitest", "run", "--reporter=default", "--reporter=junit", "--outputFile.junit={{report}}"]"#.into(),
                report: None,
                allowed: vec!["Bash(npx vitest:*)".into(), "Bash(npm test:*)".into()],
                tests,
                sources: vec![src_or("**")],
                stub,
                setup: node_setup(repo),
                ignore: vec![],
                notes,
            };
        }
        if [
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
            "Pipfile",
        ]
        .iter()
        .any(|f| has(f))
        {
            return Stack {
                name: "Python (pytest)".into(),
                // -B and no:cacheprovider: pytest writes nothing into the repository.
                // With a src/ layout, pythonpath=src makes the run's own code win over an
                // editable install, which points at your checkout instead.
                command: if has("src") {
                    r#"["python3", "-B", "-m", "pytest", "-p", "no:cacheprovider", "-o", "pythonpath=src", "--junitxml={{report}}"]"#
                } else {
                    r#"["python3", "-B", "-m", "pytest", "-p", "no:cacheprovider", "--junitxml={{report}}"]"#
                }
                .into(),
                report: None,
                allowed: vec!["Bash(python3 -B -m pytest:*)".into(), "Bash(python3 -m pytest:*)".into(), "Bash(pytest:*)".into()],
                tests: vec!["tests/**", "test/**", "**/test_*.py", "**/*_test.py", "**/conftest.py"],
                sources: vec![src_or("**/*.py")],
                stub: "a body of `raise NotImplementedError`",
                setup: None,
                ignore: vec![],
                notes: vec![
                    "the checks run `python3` from PATH: run conductor with the project's virtualenv active".into(),
                ],
            };
        }
        if has("go.mod") {
            return Stack {
                name: "Go (gotestsum)".into(),
                command: r#"["gotestsum", "--junitfile", "{{report}}"]"#.into(),
                report: None,
                allowed: vec!["Bash(gotestsum:*)".into(), "Bash(go test:*)".into(), "Bash(go build:*)".into(), "Bash(go vet:*)".into()],
                tests: vec!["**/*_test.go", "**/testdata/**"],
                sources: vec!["**/*.go"],
                // A panic stops the whole test binary, so the tests after it never report.
                stub: "a body that returns zero values (not `panic`: a panic stops the whole test binary and hides the tests after it)",
                setup: None,
                ignore: vec![],
                notes: vec!["the checks read JUnit XML through gotestsum: `go install gotest.tools/gotestsum@latest`".into()],
            };
        }
        let java_tests = vec!["**/src/test/**"];
        let java_stub =
            "a body that throws `new UnsupportedOperationException(\"not implemented\")`";
        if has("pom.xml") {
            return Stack {
                name: "Java (Maven)".into(),
                command: r#"["mvn", "-B", "-q", "-fae", "test"]"#.into(),
                report: Some("**/target/surefire-reports/TEST-*.xml"),
                allowed: vec!["Bash(mvn:*)".into()],
                tests: java_tests,
                sources: vec!["**/src/main/**"],
                stub: java_stub,
                setup: None,
                ignore: vec![],
                notes: vec![],
            };
        }
        if [
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ]
        .iter()
        .any(|f| has(f))
        {
            let gradle = if has("gradlew") {
                "./gradlew"
            } else {
                "gradle"
            };
            return Stack {
                name: "JVM (Gradle)".into(),
                command: format!(r#"["{gradle}", "test", "--continue"]"#),
                report: Some("**/build/test-results/test/*.xml"),
                allowed: vec![format!("Bash({gradle}:*)")],
                tests: java_tests,
                sources: vec!["**/src/main/**"],
                stub: java_stub,
                setup: None,
                ignore: vec![],
                notes: vec![],
            };
        }
        Stack {
            name: "an unknown stack".into(),
            command: r#"["./run-tests", "{{report}}"]"#.into(),
            report: None,
            allowed: vec!["Bash(./run-tests:*)".into()],
            tests: vec!["tests/**", "test/**"],
            sources: vec![src_or("**")],
            stub: "a body that fails with a \"not implemented\" error",
                setup: None,
            ignore: vec![],
            notes: vec![
                "set the test command in .conductor/workflows/build.yaml: any runner that writes JUnit XML works; put `{{report}}` where it takes the report's path".into(),
                "check the tests and source paths in the stages' `scope`".into(),
            ],
        }
    }

    fn workflow(&self) -> String {
        let setup = self
            .setup
            .map(|c| {
                format!(
                    "\n# Runs happen in a fresh checkout: install what it doesn't carry, once per run.\nsetup: {c}\n"
                )
            })
            .unwrap_or_default();
        let report = self
            .report
            .map(|r| format!("\n        report: {r:?}"))
            .unwrap_or_default();
        let command = &self.command;
        let allowed = quoted(&self.allowed);
        let tests = quoted(&self.tests);
        let all = quoted(&[self.tests.clone(), self.sources.clone()].concat());
        let sources = quoted(&self.sources);
        format!(
            r#"# One agent writes failing tests for the work; another makes them pass without being able
# to touch them. Every check is run by conductor, not by the agents, and reads the test
# runner's JUnit XML report ({name}).
#
# Conductor reads this file from the commit a run starts at, never from your working tree:
# commit changes to it before they take effect.
id: build
version: 1
kind: build

defaults:
  # A failed check is sent back to the agent; the second retry starts from a clean tree.
  retries: {{ max: 2, ladder: [in_context, fresh] }}

budget:
  max_stage_wall_clock_sec: 900

# Turn a passed run into a draft pull request with its receipt (`conductor deliver`).
# deliver: {{ base: main }}
{setup}
stages:
  - id: tests
    agent:
      kind: claude
      allowed_tools: [{allowed}]
    prompt_file: .conductor/prompts/tests.md
    # Tests, plus stubs of new code so the tests can load it.
    scope: {{ write: [{all}] }}
    gates:
      - {{ type: scope }}
      # The new tests must load, and every one of them must fail for the right reason (an
      # assertion or a "not implemented" stub, not a trivially false test). A stub that
      # quietly implements the work would make some pass, and fail this check.
      - type: command_assert
        command: {command}
        parser: junit_xml{report}
        assert:
          - "compiled == true"
          - "tests_new > 0"
          - "tests_failed == tests_new"
          - "failures.kind all != 'trivial'"

  - id: implement
    depends_on: [tests]
    agent:
      kind: claude
      allowed_tools: [{allowed}]
    prompt_file: .conductor/prompts/implement.md
    scope:
      write: [{sources}]
      frozen: [{tests}]      # the tests from the stage above are locked
    gates:
      - {{ type: scope }}
      - type: command_assert
        command: {command}
        parser: junit_xml{report}
        reruns: 1               # passing twice rules out a lucky run
        # tests_new == 0: the implementer adds no tests of its own.
        assert: ["tests_failed == 0", "tests_run > 0", "tests_new == 0"]
      # If your CI requires formatting, lints or type checks, check them here too, so a run
      # can't pass what CI would fail: a command that must exit 0 is
      # `{{ type: command_assert, command: [...], parser: exit }}`. Add it to allowed_tools
      # above so the agent can run it. (Mutation testing is Rust-only for now.)
"#,
            name = self.name,
        )
    }

    fn tests_prompt(&self) -> String {
        format!(
            "\
You write tests only. Write tests for the work described below, where this project keeps
its tests. Cover normal cases, boundaries and malformed input.

If the tests call something that does not exist yet, add it with its real signature and
{stub}, so the tests load. Nothing more: do not implement anything. Every test you add must
fail. Run the tests to confirm they load and fail.
",
            stub = self.stub
        )
    }
}

/// Installing a JavaScript project's packages, with the lockfile it has.
fn node_setup(repo: &Path) -> Option<&'static str> {
    Some(if repo.join("pnpm-lock.yaml").exists() {
        r#"[["pnpm", "install", "--frozen-lockfile"]]"#
    } else if repo.join("yarn.lock").exists() {
        r#"[["yarn", "install", "--frozen-lockfile"]]"#
    } else if repo.join("package-lock.json").exists() {
        r#"[["npm", "ci"]]"#
    } else {
        r#"[["npm", "install"]]"#
    })
}

const IMPLEMENT_PROMPT_ANY: &str = "\
You implement. Make the new tests pass by changing the source code only. The tests are
locked: do not edit them, and do not add tests of your own. Run the tests to check your work.
";

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn init(repo: &Path) -> Result<ExitCode> {
    let (workflow, tests, implement, stack) = if repo.join("Cargo.toml").is_file() {
        let (w, t, i) = (
            WORKFLOW.to_owned(),
            TESTS_PROMPT.to_owned(),
            IMPLEMENT_PROMPT.to_owned(),
        );
        (w, t, i, None)
    } else {
        let s = Stack::detect(repo);
        (
            s.workflow(),
            s.tests_prompt(),
            IMPLEMENT_PROMPT_ANY.to_owned(),
            Some(s),
        )
    };
    println!(
        "  found    {}",
        stack
            .as_ref()
            .map_or("Rust (cargo test)", |s| s.name.as_str())
    );
    let files = [
        (".conductor/workflows/build.yaml", workflow.as_str()),
        (".conductor/prompts/tests.md", tests.as_str()),
        (".conductor/prompts/implement.md", implement.as_str()),
        (".conductor/policy.yaml", POLICY),
        (".conductor/task.md", TASK),
    ];
    for (rel, text) in files {
        let path = repo.join(rel);
        if path.exists() {
            println!("  kept     {rel} (already there)");
            continue;
        }
        std::fs::create_dir_all(path.parent().unwrap_or(repo))?;
        std::fs::write(&path, text)?;
        println!("  wrote    {rel}");
    }
    let ignore = repo.join(".gitignore");
    // What the checks themselves write must be ignored, or the next attempt's scope check
    // sees it as the agent's: cargo's target/ stopped two tries of one run before this.
    let mut wanted = vec!["/.conductor/runs/"];
    match &stack {
        Some(s) => wanted.extend(s.ignore.clone()),
        None => wanted.push("/target/"),
    }
    for line in wanted {
        let current = std::fs::read_to_string(&ignore).unwrap_or_default();
        let bare = line.trim_matches('/');
        if current.lines().any(|l| l.trim().trim_matches('/') == bare) {
            println!("  kept     .gitignore (already ignores {bare})");
            continue;
        }
        let sep = if current.is_empty() || current.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(&ignore, format!("{current}{sep}{line}\n"))?;
        if bare == ".conductor/runs" {
            println!("  updated  .gitignore: run records stay out of git");
        } else {
            println!("  updated  .gitignore: {bare}, which the checks write, stays out of git");
        }
    }
    if let Some(s) = &stack {
        for n in &s.notes {
            println!("  todo     {n}");
        }
    }
    println!(
        "\nNext:\n  1. describe the work in .conductor/task.md\n  2. commit .conductor/ and .gitignore (runs read the workflow from a commit)\n  3. conductor doctor\n  4. conductor run .conductor/workflows/build.yaml --spec .conductor/task.md"
    );
    Ok(ExitCode::SUCCESS)
}

struct Report {
    failed: bool,
}

impl Report {
    fn ok(&self, what: &str, detail: &str) {
        println!("  ✓  {what:<22} {detail}");
    }
    fn warn(&self, what: &str, detail: &str) {
        println!("  !  {what:<22} {detail}");
    }
    fn fail(&mut self, what: &str, detail: &str) {
        self.failed = true;
        println!("  ✗  {what:<22} {detail}");
    }
}

fn version(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_owned()
    })
}

pub fn doctor(repo: &Path, herdr: conductor_herdr::Herdr) -> Result<ExitCode> {
    let mut r = Report { failed: false };
    println!("conductor doctor · {}\n", repo.display());

    let Some(head) = git(repo, &["rev-parse", "--short", "HEAD"]) else {
        r.fail("git", "no commit yet: runs start from a commit");
        return Ok(ExitCode::FAILURE);
    };
    r.ok("git", &format!("HEAD at {head}"));

    // Workflows as a run will see them: at HEAD.
    let listed = git(
        repo,
        &["ls-tree", "--name-only", "HEAD", ".conductor/workflows/"],
    )
    .unwrap_or_default();
    let paths: Vec<&str> = listed
        .lines()
        .filter(|p| p.ends_with(".yaml") || p.ends_with(".yml"))
        .collect();
    let mut workflows = Vec::new();
    if paths.is_empty() {
        if repo.join(".conductor/workflows").is_dir() {
            r.fail(
                "workflows",
                "none committed; runs read workflows from a commit, so commit .conductor/",
            );
        } else {
            r.fail("workflows", "none yet; `conductor init` writes a starter");
        }
    }
    for p in &paths {
        let Some(text) = git(repo, &["show", &format!("HEAD:{p}")]) else {
            continue;
        };
        match Workflow::parse(&text) {
            Ok(wf) => {
                let v = wf.validate();
                if v.is_ok() {
                    r.ok(
                        "workflow",
                        &format!("{p}: `{}`, {} stages", wf.id, wf.stages.len()),
                    );
                } else {
                    r.fail(
                        "workflow",
                        &format!("{p}: {} (`conductor validate {p}`)", v.errors[0].message),
                    );
                }
                workflows.push(wf);
            }
            Err(e) => r.fail("workflow", &format!("{p}: {e}")),
        }
    }
    if git(
        repo,
        &[
            "status",
            "--porcelain",
            "--",
            ".conductor/workflows",
            ".conductor/prompts",
            ".conductor/policy.yaml",
        ],
    )
    .is_some_and(|s| !s.is_empty())
    {
        r.warn(
            "uncommitted config",
            "changes under .conductor/ are not used until committed",
        );
    }

    // What the workflows need on this machine.
    let kinds: BTreeSet<&str> = workflows
        .iter()
        .flat_map(|w| w.stages.iter().map(|s| s.agent.kind.as_str()))
        .collect();
    for k in &kinds {
        match *k {
            "claude" => match version("claude", &["--version"]) {
                Some(v) => r.ok("agent claude", &v),
                None => r.fail(
                    "agent claude",
                    "`claude` is not on PATH; install Claude Code",
                ),
            },
            "script" => {}
            other => r.fail(
                &format!("agent {other}"),
                "only `claude` and `script` agents run in 0.1",
            ),
        }
    }
    let gates: Vec<&Gate> = workflows
        .iter()
        .flat_map(|w| w.stages.iter().flat_map(|s| s.all_gates()))
        .collect();
    let programs: BTreeSet<&str> = gates
        .iter()
        .filter_map(|g| match g {
            Gate::CommandAssert { command, .. } => command.first().map(String::as_str),
            _ => None,
        })
        .chain(
            workflows
                .iter()
                .flat_map(|w| w.setup.iter().filter_map(|c| c.first().map(String::as_str))),
        )
        .collect();
    for p in programs {
        match version(p, &["--version"]) {
            Some(v) => r.ok(&format!("check tool {p}"), &v),
            None => r.fail(
                &format!("check tool {p}"),
                "not on PATH; checks that run it would fail",
            ),
        }
    }
    if gates.iter().any(|g| matches!(g, Gate::Mutation { .. })) {
        match version("cargo", &["mutants", "--version"]) {
            Some(v) => r.ok("mutation testing", &v),
            None => r.fail(
                "mutation testing",
                "cargo-mutants is missing: `cargo install --locked cargo-mutants`",
            ),
        }
    }

    // Where runs will happen.
    let in_herdr = std::env::var("HERDR_ENV").as_deref() == Ok("1");
    match herdr.check_protocol() {
        Ok(p) if in_herdr => r.ok("herdr", &format!("protocol {p}; runs open a tab here")),
        Ok(p) => r.ok(
            "herdr",
            &format!("protocol {p}; use `--executor herdr` to run in it"),
        ),
        Err(conductor_herdr::HerdrError::Missing(_)) => {
            r.warn("herdr", "not installed; runs are headless (fine for CI)")
        }
        Err(e) if in_herdr => r.fail("herdr", &e.to_string()),
        Err(_) => r.warn("herdr", "no server reachable from here; runs are headless"),
    }
    match conductor_engine::otlp::Config::from_env() {
        Some(c) => r.ok("telemetry", &format!("runs are exported to {}", c.endpoint)),
        None => r.warn(
            "telemetry",
            "off; set CONDUCTOR_OTLP_ENDPOINT to export traces (`conductor trace` works without)",
        ),
    }
    let ignored = git(repo, &["check-ignore", "-q", ".conductor/runs/x"]).is_some();
    if ignored {
        r.ok("run records", "ignored by git");
    } else {
        r.warn("run records", "add /.conductor/runs/ to .gitignore");
    }

    println!();
    if r.failed {
        println!("Fix the ✗ lines before running.");
        Ok(ExitCode::FAILURE)
    } else {
        println!("Ready. Start a run with `conductor run <workflow> --spec <file>`.");
        Ok(ExitCode::SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_starter_workflow_is_valid() {
        let wf = Workflow::parse(WORKFLOW).unwrap();
        let v = wf.validate();
        assert!(v.is_ok(), "{:?}", v.errors);
        assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    }

    #[test]
    fn every_stack_starter_is_valid() {
        let d = tempfile::tempdir().unwrap();
        let markers: &[(&str, &str)] = &[
            ("package.json", r#"{"devDependencies":{"vitest":"1"}}"#),
            ("package.json", r#"{"devDependencies":{"jest":"29"}}"#),
            ("pyproject.toml", ""),
            ("go.mod", "module x\n"),
            ("pom.xml", "<project/>"),
            ("build.gradle", ""),
            ("README", ""),
        ];
        let mut names = BTreeSet::new();
        for (file, text) in markers {
            let d = d.path().join(names.len().to_string());
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), text).unwrap();
            let s = Stack::detect(&d);
            let wf = Workflow::parse(&s.workflow()).unwrap_or_else(|e| panic!("{}: {e}", s.name));
            let v = wf.validate();
            assert!(v.is_ok(), "{}: {:?}", s.name, v.errors);
            assert!(v.warnings.is_empty(), "{}: {:?}", s.name, v.warnings);
            names.insert(s.name);
        }
        assert_eq!(names.len(), markers.len(), "{names:?}");
    }

    #[test]
    fn the_starter_policy_parses() {
        conductor_model::workflow::Policy::parse(POLICY).unwrap();
    }
}
