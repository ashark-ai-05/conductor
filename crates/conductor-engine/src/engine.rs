//! The stage loop (SPEC §10): brief → dispatch → confirm → check → record, with the retry
//! ladder between failed attempts. Everything here is rule-based; the only model calls are
//! the agents themselves.

use crate::executor::{self, AgentRun, Brief};
use crate::herdr_exec::PaneOutcome;
use crate::receipt::{self, StageRecord};
use crate::store::{Recorder, RunDir, new_run_id};
use crate::worktree;
use conductor_checks::gate::{self, GateResult};
use conductor_model::workflow::{Gate, Policy, Rung, Stage, Workflow};
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
    pub watcher: Option<Sender<Event>>,
    /// Where worktrees go; defaults to [`worktree::conductor_home`].
    pub home: Option<PathBuf>,
    pub mode: Mode,
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
}

/// Paths an agent may never change, whatever the workflow says: conductor's own config.
const ALWAYS_PROTECTED: &[&str] = &[".conductor/*.yaml", ".conductor/workflows/**", ".github/**"];

fn policy_protected(repo: &Path, base: &str) -> Vec<String> {
    let mut out: Vec<String> = ALWAYS_PROTECTED.iter().map(|s| s.to_string()).collect();
    if let Ok(text) = worktree::show(repo, base, ".conductor/policy.yaml")
        && let Ok(p) = Policy::parse(&text)
    {
        out.extend(p.protected);
    }
    out
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

fn uses_new_tests(stage: &Stage) -> bool {
    stage.all_gates().any(|g| matches!(g, Gate::CommandAssert { assert, .. } if assert.iter().any(|a| a.contains("tests_new") || a.contains("new_tests"))))
}

struct StagePaths {
    write: Vec<String>,
    frozen: Vec<String>,
    outputs: BTreeMap<String, String>,
    inputs: BTreeMap<String, String>,
}

fn stage_paths(wf: &Workflow, s: &Stage, run_id: &str) -> StagePaths {
    StagePaths {
        write: s.scope.write.iter().map(|p| wf.render(p, run_id)).collect(),
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
            format!("`{}`{runs}: {}", command.join(" "), assert.join("; "))
        }
        Gate::Mutation { min_score, .. } => format!(
            "mutation testing of your change catches at least {:.0}% of injected bugs",
            min_score * 100.0
        ),
    }
}

fn brief_text(
    prompt: &str,
    task: &str,
    s: &Stage,
    p: &StagePaths,
    feedback: Option<&str>,
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
        t.push_str(&format!("- Check: {}\n", describe_gate(g)));
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
    if let Some(m) = &r.mutation {
        f.push_str("These injected bugs were not caught by any test:\n");
        for s in m.survivors.iter().take(15) {
            f.push_str(&format!("- {}:{} {}\n", s.file, s.line, s.what));
        }
    }
    f
}

fn record_agent(rec: &mut Recorder, stage: &str, a: &AgentRun) -> std::io::Result<()> {
    for n in &a.inferred {
        rec.record(Source::Inferred, Some(stage), n)?;
    }
    for c in &a.tool_calls {
        let what = match &c.target {
            Some(t) => format!("{} {t}", c.tool),
            None => c.tool.clone(),
        };
        rec.record(Source::Observed, Some(stage), what)?;
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
    let herdr_run = match std::mem::replace(&mut opts.mode, Mode::Headless) {
        Mode::Headless => None,
        Mode::Herdr(h) => {
            let hr = crate::herdr_exec::HerdrRun::new(h, &run_id);
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

    let protected = policy_protected(&opts.repo, &base);
    let stage_timeout = Duration::from_secs(wf.budget.max_stage_wall_clock_sec.unwrap_or(900));
    let total_budget = wf.budget.max_total_wall_clock_sec.map(Duration::from_secs);
    let mut records: Vec<StageRecord> = Vec::new();
    let mut run_verdict = Verdict::Passed;

    for stage in order(&wf.stages) {
        if let Some(limit) = total_budget
            && started.elapsed() > limit
        {
            rec.record(
                Source::Witnessed,
                Some(&stage.id),
                "halted: the run's wall-clock budget is spent",
            )?;
            run_verdict = Verdict::Failed;
            break;
        }
        let paths = stage_paths(&wf, stage, &run_id);
        let prompt = match &stage.prompt_file {
            Some(p) => worktree::show(&opts.repo, &base, p).unwrap_or_else(|_| {
                format!("You are the `{}` stage of a conductor workflow.", stage.id)
            }),
            None => format!("You are the `{}` stage of a conductor workflow.", stage.id),
        };
        let stage_base = worktree::head(&wt)?;
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
            Some(baseline_tests(stage, &wt, &run_id, stage_timeout))
        } else {
            None
        };

        let max_attempts = 1 + wf.defaults.retries.max as usize;
        let mut feedback_text: Option<String> = None;
        let mut session: Option<String> = None;
        let mut stage_rec = StageRecord::new(stage);
        let mut stage_verdict = Verdict::Failed;

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

            let brief = Brief {
                stage: stage.id.clone(),
                prompt: brief_text(&prompt, &opts.task, stage, &paths, feedback_text.as_deref()),
                cwd: wt.clone(),
                agent: stage.agent.clone(),
                resume,
                timeout: stage_timeout,
                run_id: run_id.clone(),
                attempt,
            };
            let agent = exec.run(&brief);
            record_agent(&mut rec, &stage.id, &agent)?;
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
            for (gi, g) in stage.all_gates().enumerate() {
                let scratch = dir.scratch(&stage.id, attempt).join(gi.to_string());
                let timeout = match g {
                    Gate::Mutation { .. } => {
                        Duration::from_secs(wf.budget.max_mutation_wall_clock_sec.unwrap_or(600))
                    }
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
                    outputs: &paths.outputs,
                    baseline_tests: baseline.as_ref(),
                    scratch: &scratch,
                };
                let r = gate::evaluate(g, &ctx);
                let hashes = r
                    .executions
                    .iter()
                    .map(|e| e.stdout_sha256.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let hashes = if hashes.is_empty() {
                    String::new()
                } else {
                    format!(" [stdout {hashes}]")
                };
                rec.record(
                    Source::Witnessed,
                    Some(&stage.id),
                    format!("{} {}: {}{hashes}", r.gate, r.verdict.word(), r.detail),
                )?;
                let bad = r.verdict != Verdict::Passed;
                results.push(r);
                if bad {
                    first_bad = Some(results.len() - 1);
                    break;
                }
            }

            match first_bad {
                None => {
                    stage_verdict = Verdict::Passed;
                    let sha =
                        worktree::commit_all(&wt, &format!("conductor: {} ({run_id})", stage.id))?;
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

        stage_rec.verdict = stage_verdict;
        records.push(stage_rec);
        if let Some(hr) = &herdr_run {
            let passed = stage_verdict == Verdict::Passed;
            let what = match hr.stage_done(&stage.id, passed, wf.herdr.close_passed_panes) {
                PaneOutcome::Closed => "closed the stage's pane",
                PaneOutcome::Kept => "kept the stage's pane for the next stage",
                PaneOutcome::LeftOpen => "left the stage's pane open for you",
                PaneOutcome::CloseFailed => "herdr would not close the stage's pane",
            };
            rec.record(Source::Witnessed, Some(&stage.id), what)?;
        }
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
    Ok(Outcome {
        run_id,
        verdict,
        receipt,
        dir,
        worktree: wt,
        branch,
    })
}

/// Test names present before the stage runs, so a gate can count the ones it added.
fn baseline_tests(stage: &Stage, wt: &Path, run_id: &str, timeout: Duration) -> BTreeSet<String> {
    let Some(Gate::CommandAssert { command, .. }) = stage
        .all_gates()
        .find(|g| matches!(g, Gate::CommandAssert { .. }))
    else {
        return BTreeSet::new();
    };
    let e = conductor_checks::runner::run(&conductor_checks::runner::Spec {
        argv: command,
        cwd: wt,
        timeout,
        pass_env: &[],
        run_id,
        inherit_env: false,
        set_env: &[],
    });
    conductor_checks::cargo_test::parse(&e.stdout_text())
        .tests
        .into_iter()
        .map(|t| t.name)
        .collect()
}
