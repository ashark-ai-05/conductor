//! The stage loop (SPEC §10): brief → dispatch → confirm → check → record, with the retry
//! ladder between failed attempts. Everything here is rule-based; the only model calls are
//! the agents themselves.

use crate::agent_panes;
use crate::ask;
use crate::executor::{self, AgentRun, AskChannel, Brief};
use crate::herdr_exec::PaneOutcome;
use crate::live::Live;
use crate::receipt::{self, StageRecord};
use crate::store::{Recorder, RunDir, new_run_id};
use crate::worktree;
use conductor_checks::gate::{self, GateResult, TestCommand, run_tests};
use conductor_model::view::Waiting;
use conductor_model::workflow::{AgentControl, Gate, Policy, REPORT, Rung, SPEC, Stage, Workflow};
use conductor_model::{Event, Receipt, Source, Verdict};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    Git(#[from] worktree::GitError),
    #[error("could not write the run record: {0}")]
    Io(#[from] std::io::Error),
    #[error("herdr is not usable: {0}")]
    Herdr(String),
    #[error("{path} at {base} is not a valid workflow: {why}")]
    Invalid {
        path: String,
        base: String,
        why: String,
    },
}

pub struct Options {
    pub repo: PathBuf,
    /// The workflow file, relative to the repository root. Read from `base`, never from
    /// the working tree.
    pub workflow: String,
    pub base: String,
    /// What to build: the spec file's contents or a one-line description.
    pub task: String,
    /// The spec file, relative to the repository, when there is one: the ticket a stage's
    /// `evidence: { to: "{{spec}}" }` appends to.
    pub spec_path: Option<String>,
    pub watcher: Option<Sender<Event>>,
    /// Where worktrees go; defaults to [`worktree::conductor_home`].
    pub home: Option<PathBuf>,
    pub mode: Mode,
    /// In herdr, the conductor binary to show the run's live view with, in the run's tab.
    pub status_ui: Option<PathBuf>,
}

/// Where agents run. The workflow and its receipt are the same either way.
pub enum Mode {
    /// No panes: CI, overnight, batch.
    Headless,
    /// A herdr tab per run, a pane per stage.
    Herdr(conductor_herdr::Herdr),
}

#[derive(Debug)]
pub struct Outcome {
    pub run_id: String,
    pub verdict: Verdict,
    pub receipt: Receipt,
    pub dir: RunDir,
    pub worktree: PathBuf,
    pub branch: String,
    /// The workflow's delivery settings, for the caller to act on once the run passed.
    pub deliver: Option<conductor_model::workflow::Deliver>,
    /// The OTLP export, when an endpoint is configured: the trace id, or why it failed.
    /// A failed export never fails the run.
    pub export: Option<Result<String, String>>,
}

/// Paths an agent may never change, whatever the workflow says: conductor's own config.
const ALWAYS_PROTECTED: &[&str] = &[
    ".conductor/*.yaml",
    ".conductor/workflows/**",
    ".conductor/prompts/**",
    ".github/**",
];

/// The repository policy at the base: (protected paths, files tooling writes).
fn policy_paths(repo: &Path, base: &str) -> (Vec<String>, Vec<String>) {
    let mut protected: Vec<String> = ALWAYS_PROTECTED.iter().map(|s| s.to_string()).collect();
    let policy = worktree::show(repo, base, ".conductor/policy.yaml")
        .ok()
        .and_then(|text| Policy::parse(&text).ok())
        .unwrap_or_default();
    let generated = policy.generated();
    protected.extend(policy.protected);
    (protected, generated)
}

fn order(stages: &[Stage]) -> Vec<&Stage> {
    let mut done: BTreeSet<&str> = BTreeSet::new();
    let mut out = Vec::new();
    while out.len() < stages.len() {
        let before = out.len();
        for s in stages {
            if !done.contains(s.id.as_str())
                && s.depends_on.iter().all(|d| done.contains(d.as_str()))
            {
                done.insert(s.id.as_str());
                out.push(s);
            }
        }
        if out.len() == before {
            break; // validation rejects cycles; this only guards against looping forever
        }
    }
    out
}

/// Variables `setup` commands get beyond a check's: how to reach the network, through a
/// proxy and a private certificate authority.
const SETUP_ENV: [&str; 12] = [
    "JAVA_TOOL_OPTIONS",
    "MAVEN_OPTS",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
];

/// How long a run waits for a person, unless the stage says otherwise.
const HUMAN_WAIT: Duration = Duration::from_secs(24 * 3600);

/// A `human` stage: the run pauses until the person decides, and their decision is the
/// stage's one check. No retries: a rejection ends the run with the reason on record.
#[allow(clippy::too_many_arguments)]
fn human_stage(
    rec: &mut Recorder,
    live: &mut Live,
    dir: &RunDir,
    stage: &Stage,
    question: &str,
    wt: &Path,
    evidence_to: Option<&str>,
    run_id: &str,
    stage_rec: &mut StageRecord,
    wait: Duration,
) -> Result<Verdict, EngineError> {
    let who = stage.agent.who.clone().unwrap_or_else(|| "someone".into());
    rec.record(
        Source::Witnessed,
        Some(&stage.id),
        format!(
            "waiting for {who}: `conductor approve {run_id}` or `conductor reject {run_id} -m <why>`"
        ),
    )?;
    live.stage(&stage.id, Verdict::Running, format!("waiting for {who}"));
    live.waiting(Some(Waiting {
        stage: stage.id.clone(),
        who: who.clone(),
        question: question.to_owned(),
        since: crate::store::now(),
        ask: None,
    }));
    live.save(rec);
    let started = Instant::now();
    let decision = loop {
        if let Some(d) = crate::decision::read(dir, &stage.id) {
            break Some(d);
        }
        if started.elapsed() > wait {
            break None;
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    live.waiting(None);
    let (verdict, detail) = match &decision {
        Some(d) => {
            let note = if d.note.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", d.note.trim())
            };
            rec.record(
                Source::Human,
                Some(&stage.id),
                format!("{} {}{note}", d.by, d.word()),
            )?;
            let v = if d.approved {
                Verdict::Passed
            } else {
                Verdict::Failed
            };
            (v, format!("{} by {}{note}", d.word(), d.by))
        }
        None => (
            Verdict::Unwitnessed,
            format!("no decision from {who} within {}s", wait.as_secs()),
        ),
    };
    let result = GateResult {
        gate: "decision",
        verdict,
        detail: detail.clone(),
        executions: vec![],
        assertions: vec![],
        scope: None,
        mutation: None,
        tests: None,
        claim: Some(format!("{who} approved the work so far")),
    };
    rec.record(
        Source::Witnessed,
        Some(&stage.id),
        format!("decision {}: {detail}", verdict.word()),
    )?;
    if let Some(to) = evidence_to
        && decision.is_some()
    {
        match append_evidence(
            wt,
            to,
            &stage.id,
            1,
            run_id,
            None,
            std::slice::from_ref(&result),
            None,
            &[],
        ) {
            Ok(_) => {
                let sha = worktree::commit_all(wt, &format!("conductor: {} ({run_id})", stage.id))?;
                rec.record(
                    Source::Witnessed,
                    Some(&stage.id),
                    format!("decision recorded in {to}; committed {}", &sha[..12]),
                )?;
            }
            Err(e) => rec
                .record(
                    Source::Witnessed,
                    Some(&stage.id),
                    format!("decision could not be written to {to}: {e}"),
                )
                .map(|_| ())?,
        }
    }
    stage_rec.finish(vec![result], 1);
    Ok(verdict)
}

/// How many lines of a check's output, or of a named file, the ticket gets.
const EVIDENCE_TAIL: usize = 40;

/// Appends one attempt's checks and files to `to`, under an `## Evidence` heading the
/// first time. Returns how many items were written.
/// How many lines of a change the ticket carries; the rest is on the run's branch.
const EVIDENCE_PATCH: usize = 200;

/// Appends this attempt's evidence to the ticket: the checks before the fix (first attempt
/// only), every check with its output, the files the stage names, and the change itself.
/// Returns how many items went in and the exact text appended, so the engine can tell
/// conductor's writes to the ticket from anyone else's.
#[allow(clippy::too_many_arguments)]
fn append_evidence(
    wt: &Path,
    to: &str,
    stage: &str,
    attempt: usize,
    run_id: &str,
    before: Option<(&str, &[GateResult])>,
    results: &[GateResult],
    patch: Option<&str>,
    files: &[String],
) -> std::io::Result<(usize, String)> {
    use std::fmt::Write as _;
    let path = wt.join(to);
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = String::new();
    if !current.ends_with('\n') && !current.is_empty() {
        out.push('\n');
    }
    if !current.contains("\n## Evidence") && !current.starts_with("## Evidence") {
        out.push_str("\n## Evidence\n");
    }
    let _ = writeln!(
        out,
        "\n### {stage} · try {attempt} · {} · run {run_id}\n",
        crate::store::now()
    );
    let mut n = 0;
    if let Some((base, before)) = before
        && !before.is_empty()
    {
        let _ = writeln!(
            out,
            "**Before the fix**, at commit {}:\n",
            base.get(..12).unwrap_or(base)
        );
        n += write_results(&mut out, before);
        out.push_str("\n**After the fix**:\n\n");
    }
    n += write_results(&mut out, results);
    if let Some(patch) = patch
        && !patch.trim().is_empty()
    {
        let files = patch
            .lines()
            .filter(|l| l.starts_with("diff --git "))
            .count();
        let added = patch
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .count();
        let removed = patch
            .lines()
            .filter(|l| l.starts_with('-') && !l.starts_with("---"))
            .count();
        let total = patch.lines().count();
        let _ = writeln!(
            out,
            "- **change**: {files} file(s), +{added} −{removed} (branch conductor/{run_id})"
        );
        out.push_str("  ```diff\n");
        for l in patch.lines().take(EVIDENCE_PATCH) {
            let _ = writeln!(out, "  {l}");
        }
        out.push_str("  ```\n");
        if total > EVIDENCE_PATCH {
            let _ = writeln!(
                out,
                "  (first {EVIDENCE_PATCH} of {total} lines; the whole change is on the branch)"
            );
        }
        n += 1;
    }
    for f in files {
        let p = wt.join(f);
        match std::fs::read(&p) {
            Ok(bytes) => {
                use sha2::Digest as _;
                let text = String::from_utf8_lossy(&bytes);
                let lines: Vec<&str> = text.lines().collect();
                let start = lines.len().saturating_sub(EVIDENCE_TAIL);
                let _ = writeln!(
                    out,
                    "- **{f}** ({} lines{}) sha256:{}",
                    lines.len(),
                    if start > 0 {
                        format!(", last {EVIDENCE_TAIL}")
                    } else {
                        String::new()
                    },
                    hex::encode(sha2::Sha256::digest(&bytes))
                );
                out.push_str("  ```\n");
                for l in &lines[start..] {
                    let _ = writeln!(out, "  {l}");
                }
                out.push_str("  ```\n");
            }
            Err(_) => {
                let _ = writeln!(out, "- **{f}**: not found");
            }
        }
        n += 1;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, current + &out)?;
    Ok((n, out))
}

/// One line per check, then each command's output and hashes.
fn write_results(out: &mut String, results: &[GateResult]) -> usize {
    use std::fmt::Write as _;
    let mut n = 0;
    for r in results {
        let _ = writeln!(
            out,
            "- {} **{}** {}: {}",
            r.verdict.glyph(),
            r.gate,
            r.verdict.word(),
            r.detail
        );
        n += 1;
        for e in &r.executions {
            let ran = format!(
                "`{}` exited {}",
                e.argv.join(" "),
                e.exit_code
                    .map_or("without a code".to_string(), |c| c.to_string())
            );
            // An exit-code check's detail is this same line: say it once.
            if !r.detail.contains(&ran) {
                let _ = writeln!(out, "  - {ran}");
            }
            let text = format!("{}\n{}", e.stdout_text(), e.stderr_tail);
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            if !lines.is_empty() {
                let start = lines.len().saturating_sub(EVIDENCE_TAIL);
                if start > 0 {
                    let _ = writeln!(out, "    (last {EVIDENCE_TAIL} of {} lines)", lines.len());
                }
                out.push_str("    ```\n");
                for l in &lines[start..] {
                    let _ = writeln!(out, "    {l}");
                }
                out.push_str("    ```\n");
            }
            let _ = writeln!(
                out,
                "    stdout {} · stderr {}",
                e.stdout_sha256, e.stderr_sha256
            );
        }
    }
    n
}

/// The checks the ticket will show, run once at the stage's base before any agent, so the
/// evidence carries the failure the fix is for and not only the pass after it.
#[allow(clippy::too_many_arguments)]
fn before_checks(
    stage: &Stage,
    wt: &Path,
    base: &str,
    dir: &RunDir,
    run_id: &str,
    timeout: Duration,
    paths: &StagePaths,
    protected: &[String],
    generated: &[String],
) -> Vec<GateResult> {
    let mut out = Vec::new();
    for (gi, g) in stage.all_gates().enumerate() {
        if !matches!(g, Gate::CommandAssert { .. }) {
            continue;
        }
        let scratch = dir.scratch(&stage.id, 0).join(format!("before-{gi}"));
        let ctx = gate::Context {
            worktree: wt,
            base,
            run_id,
            timeout,
            write: &paths.write,
            frozen: &paths.frozen,
            protected,
            generated,
            outputs: &paths.outputs,
            baseline_tests: None,
            scratch: &scratch,
        };
        out.push(gate::evaluate(&with_spec(g, paths.spec.as_deref()), &ctx));
    }
    out
}

fn uses_new_tests(stage: &Stage) -> bool {
    stage.all_gates().any(|g| matches!(g, Gate::CommandAssert { assert, .. } if assert.iter().any(|a| a.contains("tests_new") || a.contains("new_tests"))))
}

struct StagePaths {
    write: Vec<String>,
    frozen: Vec<String>,
    outputs: BTreeMap<String, String>,
    inputs: BTreeMap<String, String>,
    /// The ticket the run was given, for `{{spec}}` in a check's command.
    spec: Option<String>,
}

/// A check's command with `{{spec}}` filled in: the ticket the run was given, so one
/// script can check whichever ticket's acceptance criteria this run is for. Without a
/// ticket the placeholder stays, and the check says so when it fails.
fn with_spec<'g>(g: &'g Gate, spec: Option<&str>) -> std::borrow::Cow<'g, Gate> {
    match (g, spec) {
        (Gate::CommandAssert { command, .. }, Some(spec))
            if command.iter().any(|a| a.contains(SPEC)) =>
        {
            let mut g = g.clone();
            if let Gate::CommandAssert { command, .. } = &mut g {
                for a in command.iter_mut() {
                    *a = a.replace(SPEC, spec);
                }
            }
            std::borrow::Cow::Owned(g)
        }
        _ => std::borrow::Cow::Borrowed(g),
    }
}

/// Where a stage's evidence goes, if it declares any: `{{spec}}` is the ticket the run was
/// given. `None` when it names the spec and the run was given text, not a file.
fn evidence_target(s: &Stage, spec_path: Option<&str>) -> Option<String> {
    let e = s.evidence.as_ref()?;
    if e.to.trim() == SPEC {
        spec_path.map(str::to_owned)
    } else {
        Some(e.to.trim().to_owned())
    }
}

fn stage_paths(wf: &Workflow, s: &Stage, run_id: &str, spec_path: Option<&str>) -> StagePaths {
    // The evidence file is conductor's to write, so the scope check must allow it.
    let mut write: Vec<String> = s.scope.write.iter().map(|p| wf.render(p, run_id)).collect();
    if let Some(t) = evidence_target(s, spec_path) {
        write.push(t);
    }
    StagePaths {
        write,
        frozen: s
            .scope
            .frozen
            .iter()
            .map(|p| wf.render(p, run_id))
            .collect(),
        outputs: s
            .outputs
            .iter()
            .map(|o| (o.id.clone(), wf.render(&o.path, run_id)))
            .collect(),
        inputs: s
            .inputs
            .iter()
            .map(|(k, v)| (k.clone(), wf.render(v, run_id)))
            .collect(),
        spec: spec_path.map(str::to_owned),
    }
}

fn describe_gate(g: &Gate) -> String {
    match g {
        Gate::Scope => "only the files you may change were changed".into(),
        Gate::FileNonempty { output } => format!("output `{output}` exists and is not empty"),
        Gate::Schema { output, .. } => format!("output `{output}` matches its schema"),
        Gate::CommandAssert {
            command,
            assert,
            reruns,
            ..
        } => {
            let runs = if *reruns > 0 {
                format!(", run {} times", reruns + 1)
            } else {
                String::new()
            };
            // The agent may run the check itself: `{{report}}` is conductor's to fill, so the
            // brief names a file outside the repository, where it can't be mistaken for work.
            let shown = command
                .join(" ")
                .replace(REPORT, "/tmp/conductor-report.xml");
            if assert.is_empty() {
                format!("`{shown}` exits 0{runs}")
            } else {
                format!("`{shown}`{runs}: {}", assert.join("; "))
            }
        }
        Gate::Mutation { min_score, .. } => format!(
            "mutation testing of your change catches at least {:.0}% of injected bugs",
            min_score * 100.0
        ),
    }
}

/// How an agent opens panes beside its own, added to its brief when the workflow allows it.
fn pane_help() -> String {
    let bin = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "conductor".into());
    format!(
        "\n## Panes beside you\n\n\
         You run in a herdr pane in this run's tab. To run something alongside you (a server, \
         a watcher, a second shell), use:\n\
         - `{bin} pane split [--down]` prints the new pane's id\n\
         - `{bin} pane run <pane> <command>`\n\
         - `{bin} pane read <pane> [--lines N]`\n\
         - `{bin} pane close <pane>`\n\
         - `{bin} pane list`\n\n\
         You can reach only this run's tab, and run commands in and close only panes you \
         opened. Every action goes on the run's record.\n"
    )
}

fn brief_text(
    prompt: &str,
    task: &str,
    s: &Stage,
    p: &StagePaths,
    feedback: Option<&str>,
    pane_help: Option<&str>,
) -> String {
    let mut t = String::new();
    t.push_str(prompt.trim());
    t.push_str("\n\n## The work\n\n");
    t.push_str(task.trim());
    if !p.inputs.is_empty() {
        t.push_str("\n\n## Inputs\n\n");
        for (k, v) in &p.inputs {
            t.push_str(&format!("- {k}: {v}\n"));
        }
    }
    t.push_str("\n\n## Rules conductor will check after you finish\n\n");
    if !p.write.is_empty() {
        t.push_str(&format!("- You may change only: {}\n", p.write.join(", ")));
    }
    if !p.frozen.is_empty() {
        t.push_str(&format!(
            "- You must not change (locked by an earlier stage): {}\n",
            p.frozen.join(", ")
        ));
    }
    for (id, path) in &p.outputs {
        t.push_str(&format!("- Produce `{id}` at {path}\n"));
    }
    for g in s.all_gates() {
        t.push_str(&format!(
            "- Check: {}\n",
            describe_gate(&with_spec(g, p.spec.as_deref()))
        ));
    }
    if let Some(h) = pane_help {
        t.push_str(h);
    }
    if let Some(f) = feedback {
        t.push_str("\n## What failed on the last attempt\n\n");
        t.push_str(f.trim());
        t.push('\n');
    }
    t
}

/// Turns a failed gate into guidance for the next attempt: the check, and what it saw.
fn feedback(r: &GateResult) -> String {
    let mut f = format!("Check `{}` failed: {}\n", r.gate, r.detail);
    if let Some(s) = &r.scope {
        for v in &s.violations {
            f.push_str(&format!("- {} is {}\n", v.path, v.breach.describe()));
        }
    }
    if let Some(t) = &r.tests {
        if !t.compile_messages.is_empty() {
            f.push_str("The build failed:\n");
            for m in &t.compile_messages {
                f.push_str(&format!("- {m}\n"));
            }
        }
        for c in t.tests.iter().filter(|c| c.failure.is_some()).take(8) {
            f.push_str(&format!(
                "- test {} failed: {}\n",
                c.name,
                c.message
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
            ));
        }
    }
    // A check with no test report (a formatter, a linter) explains itself in its output.
    if r.tests.is_none()
        && r.scope.is_none()
        && r.mutation.is_none()
        && let Some(e) = r.executions.last()
    {
        let out = format!("{}\n{}", e.stdout_text(), e.stderr_tail);
        let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
        if !lines.is_empty() {
            f.push_str("Its output ended with:\n");
            for l in &lines[lines.len().saturating_sub(30)..] {
                f.push_str(&format!("    {l}\n"));
            }
        }
    }
    if let Some(m) = &r.mutation {
        f.push_str("These injected bugs were not caught by any test:\n");
        for s in m.survivors.iter().take(15) {
            f.push_str(&format!("- {}:{} {}\n", s.file, s.line, s.what));
        }
    }
    f
}

fn describe(c: &executor::ToolCall) -> String {
    match &c.target {
        Some(t) => format!("{} {t}", c.tool),
        None => c.tool.clone(),
    }
}

/// Records what the agent reported once done. Tool calls already recorded as they happened
/// are not recorded again.
fn record_agent(
    rec: &mut Recorder,
    stage: &str,
    a: &AgentRun,
    seen_live: bool,
) -> std::io::Result<()> {
    for n in &a.inferred {
        rec.record(Source::Inferred, Some(stage), n)?;
    }
    if !seen_live {
        for c in &a.tool_calls {
            rec.record(Source::Observed, Some(stage), describe(c))?;
        }
    }
    match &a.usage {
        Some(u) => {
            let cost = u
                .cost_usd
                .map(|c| format!(" · ${c:.2}"))
                .unwrap_or_default();
            rec.record(
                Source::Measured,
                Some(stage),
                format!("{} tokens{cost}", u.total()),
            )?;
        }
        None => {
            rec.record(
                Source::Witnessed,
                Some(stage),
                "token usage unavailable for this agent",
            )?;
        }
    }
    Ok(())
}

pub fn run(mut opts: Options) -> Result<Outcome, EngineError> {
    let started = Instant::now();
    let base = worktree::resolve(&opts.repo, &opts.base)?;
    let text = worktree::show(&opts.repo, &base, &opts.workflow)?;
    let invalid = |why: String| EngineError::Invalid {
        path: opts.workflow.clone(),
        base: base.clone(),
        why,
    };
    let wf = Workflow::parse(&text).map_err(|e| invalid(e.to_string()))?;
    let report = wf.validate();
    if !report.is_ok() {
        let why = report
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.at, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(invalid(why));
    }

    let run_id = new_run_id();
    let dir = RunDir::create(&opts.repo, &run_id)?;
    let mut rec = Recorder::open(&dir, opts.watcher.clone())?;
    rec.record(
        Source::Witnessed,
        None,
        format!("run started: {} at {}", wf.id, &base[..base.len().min(12)]),
    )?;
    let (wt, branch) = worktree::create(
        &opts.repo,
        &run_id,
        &base,
        &opts.home.clone().unwrap_or_else(worktree::conductor_home),
    )?;
    rec.record(
        Source::Witnessed,
        None,
        format!("worktree on branch {branch}"),
    )?;
    // Where `conductor pane` records what agents do, when the workflow lets them open panes.
    let actions_path = (wf.herdr.agent_control == AgentControl::OwnTab)
        .then(|| dir.root.join("agent-actions.jsonl"));
    let mut live = Live::new(&dir, &run_id, &opts.task, &wf, &order(&wf.stages));
    live.save(&rec);
    let herdr_run = match std::mem::replace(&mut opts.mode, Mode::Headless) {
        Mode::Headless => None,
        Mode::Herdr(h) => {
            let mut hr = crate::herdr_exec::HerdrRun::new(h, &run_id);
            if let Some(a) = &actions_path {
                hr = hr.with_agent_control(a.clone());
            }
            match hr
                .herdr
                .check_protocol()
                .and_then(|p| hr.open_tab(&wt).map(|t| (p, t)))
            {
                Ok((p, t)) => {
                    rec.record(
                        Source::Witnessed,
                        None,
                        format!("herdr protocol {p}; run tab {}", t.tab_id),
                    )?;
                    live.set_tab(&t.tab_id);
                    if let Some(bin) = &opts.status_ui {
                        let what = match hr.open_status_pane(bin, &opts.repo) {
                            Ok(p) => format!("status pane {p} shows the run live"),
                            Err(e) => format!("no status pane: {e}"),
                        };
                        rec.record(Source::Witnessed, None, what)?;
                    }
                    Some(hr)
                }
                Err(e) => {
                    rec.record(
                        Source::Witnessed,
                        None,
                        format!("halted: herdr is not usable: {e}"),
                    )?;
                    return Err(EngineError::Herdr(e.to_string()));
                }
            }
        }
    };
    let where_ran = if herdr_run.is_some() {
        "herdr"
    } else {
        "headless"
    };

    let (protected, generated) = policy_paths(&opts.repo, &base);
    let stage_timeout = Duration::from_secs(wf.budget.max_stage_wall_clock_sec.unwrap_or(900));
    let total_budget = wf.budget.max_total_wall_clock_sec.map(Duration::from_secs);
    let mut records: Vec<StageRecord> = Vec::new();
    let mut run_verdict = Verdict::Passed;
    let mut actions_seen = 0usize;
    // Time agents spent waiting on a person, which no clock counts.
    let mut waited_total = Duration::ZERO;
    let mut unmanaged_seen: BTreeSet<String> = BTreeSet::new();
    let pane_help = actions_path.as_ref().map(|_| pane_help());

    // What a checkout doesn't carry (installed packages), made once for the whole run: a
    // retry's reset keeps ignored files, so it survives. It gets the proxy settings on top of
    // a check's environment, since installing usually means downloading.
    let mut setup_failed = None;
    for argv in &wf.setup {
        let shown = argv.join(" ");
        let e = conductor_checks::runner::run(&conductor_checks::runner::Spec {
            argv,
            cwd: &wt,
            timeout: stage_timeout,
            pass_env: &SETUP_ENV.map(String::from),
            run_id: &run_id,
            inherit_env: false,
            set_env: &[],
        });
        let secs = e.duration_ms / 1000;
        if e.complete() && e.exit_code == Some(0) {
            rec.record(
                Source::Witnessed,
                None,
                format!("setup `{shown}` exited 0 in {secs}s"),
            )?;
            continue;
        }
        let tail = e
            .stderr_tail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| format!(": {}", l.trim()))
            .unwrap_or_default();
        let how = match (&e.reason, e.exit_code) {
            (Some(r), _) => r.clone(),
            (None, Some(c)) => format!("exited {c}"),
            (None, None) => "was killed".into(),
        };
        let why = format!("setup `{shown}` {how}{tail}");
        rec.record(Source::Witnessed, None, format!("halted: {why}"))?;
        setup_failed = Some(why);
        break;
    }
    // Setup's output must be invisible to the scope check, and survive a fresh retry's clean:
    // ignored by git, or a lockfile the policy calls generated.
    if setup_failed.is_none() && !wf.setup.is_empty() {
        let head = worktree::head(&wt)?;
        let left = conductor_checks::git::changed_files(&wt, &head).unwrap_or_default();
        let stray: Vec<String> = conductor_checks::scope::check(&left, &[], &[], &[], &generated)
            .map(|r| r.violations.into_iter().map(|v| v.path).collect())
            .unwrap_or_default();
        if !stray.is_empty() {
            let why = format!(
                "setup left files git doesn't ignore ({}); add them to .gitignore, or they count as the first stage's changes",
                stray.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            );
            rec.record(Source::Witnessed, None, format!("halted: {why}"))?;
            setup_failed = Some(why);
        }
    }

    for stage in order(&wf.stages) {
        if let Some(why) = &setup_failed {
            records.push(StageRecord::halted(stage, why));
            run_verdict = Verdict::Unwitnessed;
            break;
        }
        if let Some(limit) = total_budget
            && started.elapsed().saturating_sub(waited_total) > limit
        {
            rec.record(
                Source::Witnessed,
                Some(&stage.id),
                "halted: the run's wall-clock budget is spent",
            )?;
            run_verdict = Verdict::Failed;
            break;
        }
        let paths = stage_paths(&wf, stage, &run_id, opts.spec_path.as_deref());
        let evidence_to = evidence_target(stage, opts.spec_path.as_deref());
        let prompt = match &stage.prompt_file {
            Some(p) => worktree::show(&opts.repo, &base, p).unwrap_or_else(|_| {
                format!("You are the `{}` stage of a conductor workflow.", stage.id)
            }),
            None => format!("You are the `{}` stage of a conductor workflow.", stage.id),
        };
        let stage_base = worktree::head(&wt)?;
        let mut stage_rec = StageRecord::new(stage);
        let mut stage_verdict = Verdict::Failed;
        if stage.is_human() {
            let wait = stage
                .timeout_ms
                .map(Duration::from_millis)
                .unwrap_or(HUMAN_WAIT);
            // The decision is about the ticket, so it goes there unless the stage says
            // where else.
            stage_verdict = human_stage(
                &mut rec,
                &mut live,
                &dir,
                stage,
                &prompt,
                &wt,
                evidence_to.as_deref().or(opts.spec_path.as_deref()),
                &run_id,
                &mut stage_rec,
                wait,
            )?;
        } else {
            let exec: Result<Box<dyn executor::Executor + '_>, String> = match &herdr_run {
                Some(hr) => Ok(Box::new(crate::herdr_exec::HerdrExecutor {
                    run: hr,
                    kind: stage.agent.kind.clone(),
                })),
                None => executor::for_agent(&stage.agent.kind),
            };
            let exec = match exec {
                Ok(e) => e,
                Err(why) => {
                    rec.record(Source::Witnessed, Some(&stage.id), format!("halted: {why}"))?;
                    records.push(StageRecord::halted(stage, &why));
                    run_verdict = Verdict::Unwitnessed;
                    break;
                }
            };

            let baseline = if uses_new_tests(stage) {
                let scratch = dir.scratch(&stage.id, 0).join("baseline");
                Some(baseline_tests(stage, &wt, &scratch, &run_id, stage_timeout))
            } else {
                None
            };
            // A stage with evidence shows the checks failing before the fix as well as
            // passing after it.
            let before: Vec<GateResult> = if stage.evidence.is_some() {
                before_checks(
                    stage,
                    &wt,
                    &stage_base,
                    &dir,
                    &run_id,
                    stage_timeout,
                    &paths,
                    &protected,
                    &generated,
                )
            } else {
                Vec::new()
            };
            for r in &before {
                rec.record(
                    Source::Witnessed,
                    Some(&stage.id),
                    format!(
                        "before the fix: {} {}: {}",
                        r.gate,
                        r.verdict.word(),
                        r.detail
                    ),
                )?;
            }
            live.save(&rec);
            // The ticket as it is at the stage's base (the tree is at the base here, and the
            // checks above don't write it), and what conductor has appended to it since:
            // anything else in it is the agent's, and the agent's words are not evidence.
            let ticket_base = evidence_to
                .as_deref()
                .map(|to| std::fs::read_to_string(wt.join(to)).unwrap_or_default());
            let mut appended = String::new();

            let max_attempts = 1 + wf.defaults.retries.max as usize;
            let mut feedback_text: Option<String> = None;
            let mut session: Option<String> = None;

            for attempt in 1..=max_attempts {
                let rung = if attempt == 1 {
                    None
                } else {
                    wf.defaults.retries.ladder.get(attempt - 2).copied().or(wf
                        .defaults
                        .retries
                        .ladder
                        .last()
                        .copied())
                };
                let resume = match rung {
                    Some(Rung::InContext) => session.clone(),
                    Some(Rung::Fresh) => {
                        worktree::reset(&wt, &stage_base)?;
                        appended.clear();
                        None
                    }
                    Some(Rung::CrossKind) => {
                        rec.record(
                            Source::Witnessed,
                            Some(&stage.id),
                            "cross-kind retry skipped: no second agent kind is configured",
                        )?;
                        stage_rec.note("cross-kind retry skipped: no second agent kind configured");
                        break;
                    }
                    None => None,
                };
                let rung_name = match rung {
                    None => "first try",
                    Some(Rung::InContext) => "in-context retry",
                    Some(Rung::Fresh) => "fresh retry",
                    Some(Rung::CrossKind) => "cross-kind retry",
                };
                rec.record(
                    Source::Witnessed,
                    Some(&stage.id),
                    format!("attempt {attempt}: {rung_name} with {}", stage.agent.kind),
                )?;
                live.attempt(stage, attempt, rung_name);
                if herdr_run.is_some() {
                    live.pane(&stage.id, Verdict::Running, true, "conductor");
                }
                live.save(&rec);

                let brief = Brief {
                    stage: stage.id.clone(),
                    prompt: brief_text(
                        &prompt,
                        &opts.task,
                        stage,
                        &paths,
                        feedback_text.as_deref(),
                        pane_help.as_deref(),
                    ),
                    cwd: wt.clone(),
                    agent: stage.agent.clone(),
                    resume,
                    timeout: stage_timeout,
                    run_id: run_id.clone(),
                    attempt,
                    // In herdr the executor hands the grant to the pane; headless, it goes in the
                    // agent's environment.
                    env: match (&herdr_run, &actions_path) {
                        (None, Some(a)) => agent_panes::local_env(a, &stage.id),
                        _ => Vec::new(),
                    },
                    // A tool the agent asks for goes to the stage's `who`, else to whoever
                    // is running conductor.
                    ask: Some(AskChannel {
                        dir: ask::dir(&dir),
                        who: stage
                            .agent
                            .who
                            .clone()
                            .or_else(|| std::env::var("USER").ok())
                            .unwrap_or_else(|| "someone".into()),
                    }),
                };
                // What the agent does is recorded as it happens, so a person watching can
                // tell working from stuck, and a refusal shows the moment it lands.
                let mut seen_live = 0usize;
                let agent = {
                    let (rec, live) = (&mut rec, &mut live);
                    let stage_id = stage.id.as_str();
                    let who = brief
                        .ask
                        .as_ref()
                        .map(|c| c.who.clone())
                        .unwrap_or_default();
                    exec.run(&brief, &mut |o| {
                        seen_live += 1;
                        let (source, what) = match &o {
                            executor::Observed::Tool(c) => (Source::Observed, describe(c)),
                            executor::Observed::Refused { call, why } => {
                                live.view.note = Some(format!(
                                    "the agent asked to run `{}` and was refused; headless, nobody can approve it",
                                    call.target.as_deref().unwrap_or(&call.tool)
                                ));
                                (Source::Observed, format!("refused: {} ({why})", describe(call)))
                            }
                            // The agent is stopped on a question; so is the stage clock.
                            executor::Observed::Asked(a) => {
                                live.waiting(Some(Waiting {
                                    stage: stage_id.to_owned(),
                                    who: who.clone(),
                                    question: a.question(),
                                    since: a.since.clone(),
                                    ask: Some(a.id),
                                }));
                                (Source::Observed, format!("asked: {}", a.describe()))
                            }
                            executor::Observed::Blocked { why } => {
                                live.view.note = Some(why.clone());
                                (
                                    Source::Inferred,
                                    format!("the agent is waiting for a person in its pane: {why}"),
                                )
                            }
                            executor::Observed::Answered { ask, answer, waited } => {
                                live.waiting(None);
                                let note = if answer.note.trim().is_empty() {
                                    String::new()
                                } else {
                                    format!(": {}", answer.note.trim())
                                };
                                let _ = rec.record(
                                    Source::Human,
                                    Some(stage_id),
                                    format!("{} {} {}{note}", answer.by, answer.word(), ask.describe()),
                                );
                                (
                                    Source::Witnessed,
                                    format!(
                                        "the agent waited {}s for {}; the stage clock was stopped",
                                        waited.as_secs(),
                                        answer.by
                                    ),
                                )
                            }
                        };
                        if rec.record(source, Some(stage_id), what).is_ok() {
                            live.save(rec);
                        }
                    })
                };
                record_agent(&mut rec, &stage.id, &agent, seen_live > 0)?;
                if agent.asked > 0 {
                    waited_total += Duration::from_millis(agent.waited_ms);
                    stage_rec.note(&format!(
                        "the agent asked for {} tool(s), {} denied, and waited {}s for an answer; that time is not counted against the stage",
                        agent.asked,
                        agent.denied,
                        agent.waited_ms / 1000
                    ));
                } else if agent.waited_ms > 0 {
                    waited_total += Duration::from_millis(agent.waited_ms);
                    stage_rec.note(&format!(
                        "the agent waited {}s for a person at its pane; that time is not counted against the stage",
                        agent.waited_ms / 1000
                    ));
                }
                if !agent.refused.is_empty() {
                    stage_rec.note(&format!(
                        "{} tool call(s) refused: the agent asked and nobody could approve; widen allowed_tools",
                        agent.refused.len()
                    ));
                }
                if let Some(path) = &actions_path {
                    let actions = agent_panes::read_actions(path);
                    for a in actions.iter().skip(actions_seen) {
                        rec.record(Source::Observed, Some(&a.stage), a.describe())?;
                    }
                    actions_seen = actions.len();
                }
                live.save(&rec);
                // A session id inherited from whoever launched conductor is not this agent's.
                let parent = std::env::var("CLAUDE_CODE_SESSION_ID").ok();
                session = agent
                    .session_id
                    .clone()
                    .filter(|s| Some(s) != parent.as_ref())
                    .or(session);
                stage_rec.add_agent(&agent);

                if !agent.finished {
                    let why = agent
                        .reason
                        .clone()
                        .unwrap_or_else(|| "the agent did not finish".into());
                    rec.record(
                        Source::Witnessed,
                        Some(&stage.id),
                        format!("agent did not finish: {why}"),
                    )?;
                    feedback_text = Some(format!("Your last attempt did not finish: {why}"));
                    continue;
                }

                let missing: Vec<&String> = stage
                    .outputs
                    .iter()
                    .filter(|o| o.required)
                    .map(|o| &paths.outputs[&o.id])
                    .filter(|p| !wt.join(p).is_file())
                    .collect();
                if !missing.is_empty() {
                    let why = format!(
                        "declared output(s) missing: {}",
                        missing
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    rec.record(Source::Witnessed, Some(&stage.id), &why)?;
                    feedback_text = Some(why);
                    continue;
                }

                let mut results: Vec<GateResult> = Vec::new();
                let mut first_bad: Option<usize> = None;
                // Only conductor writes the ticket. If the agent changed it, its changes are
                // dropped and the attempt fails, before any other check.
                if let (Some(to), Some(base_text)) = (&evidence_to, &ticket_base) {
                    let expected = format!("{base_text}{appended}");
                    let actual = std::fs::read_to_string(wt.join(to)).unwrap_or_default();
                    if actual != expected {
                        std::fs::write(wt.join(to), &expected)?;
                        let detail = format!(
                            "the agent changed {to}; only conductor writes evidence there, so its changes were dropped"
                        );
                        rec.record(
                            Source::Witnessed,
                            Some(&stage.id),
                            format!("scope failed: {detail}"),
                        )?;
                        live.check(0, Verdict::Failed, &detail);
                        live.save(&rec);
                        results.push(GateResult {
                            gate: "scope",
                            verdict: Verdict::Failed,
                            detail,
                            executions: vec![],
                            assertions: vec![],
                            scope: None,
                            mutation: None,
                            tests: None,
                            claim: Some("only the files you may change were changed".into()),
                        });
                        first_bad = Some(0);
                    }
                }
                for (gi, g) in stage.all_gates().enumerate() {
                    if first_bad.is_some() {
                        break;
                    }
                    let scratch = dir.scratch(&stage.id, attempt).join(gi.to_string());
                    let timeout = match g {
                        Gate::Mutation { .. } => Duration::from_secs(
                            wf.budget.max_mutation_wall_clock_sec.unwrap_or(600),
                        ),
                        _ => stage_timeout,
                    };
                    let ctx = gate::Context {
                        worktree: &wt,
                        base: &stage_base,
                        run_id: &run_id,
                        timeout,
                        write: &paths.write,
                        frozen: &paths.frozen,
                        protected: &protected,
                        generated: &generated,
                        outputs: &paths.outputs,
                        baseline_tests: baseline.as_ref(),
                        scratch: &scratch,
                    };
                    let r = gate::evaluate(&with_spec(g, paths.spec.as_deref()), &ctx);
                    let hashes = r
                        .executions
                        .iter()
                        .map(|e| e.stdout_sha256.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let mut hashes = if hashes.is_empty() {
                        String::new()
                    } else {
                        format!(" [stdout {hashes}]")
                    };
                    if let Some(files) = r.tests.as_ref().map(|t| &t.report_files)
                        && !files.is_empty()
                    {
                        hashes.push_str(&format!(" [report {}]", files.join(", ")));
                    }
                    rec.record(
                        Source::Witnessed,
                        Some(&stage.id),
                        format!("{} {}: {}{hashes}", r.gate, r.verdict.word(), r.detail),
                    )?;
                    live.check(gi, r.verdict, &r.detail);
                    live.save(&rec);
                    let bad = r.verdict != Verdict::Passed;
                    results.push(r);
                    if bad {
                        first_bad = Some(results.len() - 1);
                        break;
                    }
                }

                // What this attempt changed, kept whatever the checks said, so a person can see
                // try 1 next to try 2. A fresh retry resets the tree and would lose it.
                let patch = worktree::diff_from(&wt, &stage_base, evidence_to.as_deref()).ok();
                if let Some(patch) = &patch {
                    let path = dir.attempt_diff(&stage.id, attempt);
                    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&dir.root));
                    let _ = std::fs::write(path, patch);
                }
                // The evidence, into the ticket, as it happened: the checks before the fix on
                // the first try, every check with its output, the change, then the files the
                // stage names. Written now, so a failed try is on record too.
                if let (Some(to), Some(ev)) = (&evidence_to, &stage.evidence) {
                    match append_evidence(
                        &wt,
                        to,
                        &stage.id,
                        attempt,
                        &run_id,
                        (attempt == 1).then_some((stage_base.as_str(), before.as_slice())),
                        &results,
                        patch.as_deref(),
                        &ev.files,
                    ) {
                        Ok((n, text)) => {
                            appended.push_str(&text);
                            rec.record(
                                Source::Witnessed,
                                Some(&stage.id),
                                format!("evidence: {n} item(s) appended to {to}"),
                            )?
                        }
                        Err(e) => rec.record(
                            Source::Witnessed,
                            Some(&stage.id),
                            format!("evidence could not be written to {to}: {e}"),
                        )?,
                    };
                }

                match first_bad {
                    None => {
                        stage_verdict = Verdict::Passed;
                        let sha = worktree::commit_all(
                            &wt,
                            &format!("conductor: {} ({run_id})", stage.id),
                        )?;
                        rec.record(
                            Source::Witnessed,
                            Some(&stage.id),
                            format!(
                                "stage passed on attempt {attempt}; committed {}",
                                &sha[..12]
                            ),
                        )?;
                        stage_rec.finish(results, attempt);
                        break;
                    }
                    Some(i) => {
                        let v = results[i].verdict;
                        if matches!(v, Verdict::Flaky | Verdict::Unwitnessed) {
                            // Retrying an agent can't fix a flaky or unrunnable check.
                            stage_verdict = v;
                            rec.record(
                                Source::Witnessed,
                                Some(&stage.id),
                                format!(
                                    "halted: {} is {}; no agent retry",
                                    results[i].gate,
                                    v.word()
                                ),
                            )?;
                            stage_rec.finish(results, attempt);
                            break;
                        }
                        feedback_text = Some(feedback(&results[i]));
                        stage_rec.finish(results, attempt);
                    }
                }
            }
        }
        stage_rec.verdict = stage_verdict;
        live.stage(
            &stage.id,
            stage_verdict,
            format!(
                "{} · {} after {} tr{}",
                stage.agent.kind,
                stage_verdict.word(),
                stage_rec.attempts,
                if stage_rec.attempts == 1 { "y" } else { "ies" }
            ),
        );
        records.push(stage_rec);
        // Headless, nobody is watching a background pane: stop what the agent left running,
        // whether or not the stage passed.
        if let (None, Some(path)) = (&herdr_run, &actions_path) {
            for (p, n) in agent_panes::stop_local(path, &stage.id) {
                if n > 0 {
                    rec.record(
                        Source::Witnessed,
                        Some(&stage.id),
                        format!("stopped background pane {p} the agent left running"),
                    )?;
                }
            }
        }
        if let Some(hr) = &herdr_run {
            let passed = stage_verdict == Verdict::Passed;
            for p in hr.unmanaged() {
                if unmanaged_seen.insert(p.clone()) {
                    if let Some(r) = records.last_mut() {
                        r.note(&format!(
                            "pane {p} appeared in the run's tab outside conductor; what ran there is unrecorded"
                        ));
                    }
                    rec.record(
                        Source::Inferred,
                        Some(&stage.id),
                        format!(
                            "pane {p} in the run's tab was not opened by conductor or through `conductor pane`"
                        ),
                    )?;
                }
            }
            if passed && wf.herdr.close_passed_panes {
                for (p, ok) in hr.close_agent_panes(&stage.id) {
                    let what = if ok {
                        format!("closed pane {p} the agent left open")
                    } else {
                        format!("herdr would not close pane {p} the agent left open")
                    };
                    rec.record(Source::Witnessed, Some(&stage.id), what)?;
                }
            }
            let outcome = hr.stage_done(&stage.id, passed, wf.herdr.close_passed_panes);
            live.pane(
                &stage.id,
                stage_verdict,
                matches!(outcome, PaneOutcome::LeftOpen | PaneOutcome::CloseFailed),
                "conductor",
            );
            let what = match outcome {
                PaneOutcome::Closed => "closed the stage's pane",
                PaneOutcome::Kept => "kept the stage's pane for the next stage",
                PaneOutcome::LeftOpen => "left the stage's pane open for you",
                PaneOutcome::CloseFailed => "herdr would not close the stage's pane",
            };
            rec.record(Source::Witnessed, Some(&stage.id), what)?;
        }
        live.save(&rec);
        if stage_verdict != Verdict::Passed {
            run_verdict = stage_verdict;
            rec.record(
                Source::Witnessed,
                Some(&stage.id),
                format!("run halted: stage {}", stage_verdict.word()),
            )?;
            break;
        }
    }

    rec.record(
        Source::Witnessed,
        None,
        format!(
            "run {} in {}s",
            run_verdict.word(),
            started.elapsed().as_secs()
        ),
    )?;
    let head_before_anchor = rec.head().to_owned();
    let anchor = worktree::anchor(&wt, &run_id, &head_before_anchor)?;
    rec.record(
        Source::Witnessed,
        None,
        format!("record anchored in commit {anchor}"),
    )?;

    live.end(run_verdict);
    live.save(&rec);
    dir.write_json(&dir.gates(), &records)?;
    let receipt = receipt::build(
        &wf,
        &run_id,
        &opts.task,
        &records,
        &rec,
        &anchor,
        &branch,
        where_ran,
        started.elapsed(),
    );
    dir.write_json(&dir.receipt(), &receipt)?;
    let verdict = receipt.verdict();
    let export = crate::otlp::Config::from_env().map(|cfg| export_run(&cfg, &dir, &run_id, &rec));
    Ok(Outcome {
        run_id,
        verdict,
        receipt,
        dir,
        worktree: wt,
        branch,
        export,
        deliver: wf.deliver.clone(),
    })
}

/// Sends a finished run to an OTLP collector, tagged with its receipt's hash.
fn export_run(
    cfg: &crate::otlp::Config,
    dir: &RunDir,
    run_id: &str,
    rec: &Recorder,
) -> Result<String, String> {
    crate::otlp::export(cfg, run_id, rec.events(), dir.receipt_sha256().as_deref())
}

/// Test names present before the stage runs, so a gate can count the ones it added. Read
/// with the stage's first check of test results, the way that check will read them after.
fn baseline_tests(
    stage: &Stage,
    wt: &Path,
    scratch: &Path,
    run_id: &str,
    timeout: Duration,
) -> BTreeSet<String> {
    let Some(cmd) = stage.all_gates().find_map(TestCommand::of) else {
        return BTreeSet::new();
    };
    // The baseline must list every test, and before a stage some are expected to fail:
    // `cargo test` stops at the first failing test binary unless told not to, which would
    // leave later binaries' tests out, and make them look new afterwards.
    let mut argv = cmd.argv.to_vec();
    if argv.get(1).map(String::as_str) == Some("test")
        && argv
            .first()
            .is_some_and(|p| p == "cargo" || p.ends_with("/cargo"))
        && !argv.iter().any(|a| a == "--no-fail-fast")
    {
        let at = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
        argv.insert(at, "--no-fail-fast".into());
    }
    let cmd = TestCommand { argv: &argv, ..cmd };
    let (_, report) = run_tests(cmd, wt, scratch, 0, run_id, timeout);
    report
        .map(|r| r.tests.into_iter().map(|t| t.name).collect())
        .unwrap_or_default()
}
