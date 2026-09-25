//! Builds the receipt from what the engine recorded.
//!
//! Rows are written as plain claims a reviewer can read ("Tests pass after the change"),
//! and each one comes from a gate conductor ran itself. What conductor did not check is
//! listed, not implied.

use crate::executor::AgentRun;
use crate::store::Recorder;
use conductor_checks::gate::GateResult;
use conductor_model::workflow::{Gate, Stage, Workflow};
use conductor_model::{CheckRow, Integrity, Receipt, Source, Survivor, Verdict};
use serde::Serialize;
use std::time::Duration;

/// What happened in one stage: its attempts, what the agent reported, and the gate results of
/// the last attempt.
#[derive(Debug, Clone, Serialize)]
pub struct StageRecord {
    pub stage: String,
    pub agent: String,
    pub verdict: Verdict,
    pub attempts: usize,
    pub gates: Vec<GateResult>,
    pub tool_calls: usize,
    pub turns: Option<u64>,
    pub tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub model: Option<String>,
    pub notes: Vec<String>,
    /// Gate kinds declared for the stage, including ones never reached.
    pub declared: Vec<String>,
}

impl StageRecord {
    pub fn new(s: &Stage) -> Self {
        StageRecord {
            stage: s.id.clone(),
            agent: s.agent.kind.clone(),
            verdict: Verdict::Pending,
            attempts: 0,
            gates: vec![],
            tool_calls: 0,
            turns: None,
            tokens: None,
            cost_usd: None,
            model: None,
            notes: vec![],
            declared: s.all_gates().map(|g| g.name().to_owned()).collect(),
        }
    }

    pub fn halted(s: &Stage, why: &str) -> Self {
        let mut r = Self::new(s);
        r.verdict = Verdict::Unwitnessed;
        r.notes.push(why.to_owned());
        r
    }

    pub fn note(&mut self, n: &str) {
        self.notes.push(n.to_owned());
    }

    pub fn add_agent(&mut self, a: &AgentRun) {
        self.tool_calls += a.tool_calls.len();
        if let Some(t) = a.turns {
            *self.turns.get_or_insert(0) += t;
        }
        if let Some(u) = &a.usage {
            *self.tokens.get_or_insert(0) += u.total();
            if let Some(c) = u.cost_usd {
                *self.cost_usd.get_or_insert(0.0) += c;
            }
        }
        if a.model.is_some() {
            self.model = a.model.clone();
        }
    }

    pub fn finish(&mut self, gates: Vec<GateResult>, attempt: usize) {
        self.gates = gates;
        self.attempts = attempt;
    }
}

/// A claim a reviewer understands, for one gate of one stage.
fn claim(wf: &Workflow, stage: &str, g: &GateResult) -> String {
    if let Some(c) = &g.claim {
        return format!("{c} ({stage})");
    }
    let asserts: Vec<&str> = g.assertions.iter().map(|a| a.expr.as_str()).collect();
    match g.gate {
        "scope" => format!("Only allowed files changed ({stage})"),
        "file_nonempty" => format!("{stage} produced its output"),
        "mutation" => "Tests catch injected bugs".into(),
        "command_assert" if asserts.iter().any(|a| a.contains("failures.kind")) => {
            "Tests failed before the change, for the right reason".into()
        }
        "command_assert"
            if asserts
                .iter()
                .any(|a| a.replace(' ', "") == "tests_failed==0") =>
        {
            let reruns = wf.stages.iter().find(|s| s.id == stage).and_then(|s| {
                s.all_gates().find_map(|x| match x {
                    Gate::CommandAssert { reruns, .. } => Some(*reruns),
                    _ => None,
                })
            });
            if reruns.unwrap_or(0) > 0 {
                "Tests pass after the change, on every run".into()
            } else {
                "Tests pass after the change".into()
            }
        }
        other => format!("{stage}: {other} held"),
    }
}

/// Whether the tests a stage must not touch were written by a different agent kind.
fn independent_tests(wf: &Workflow) -> Option<(String, String)> {
    for s in &wf.stages {
        for f in &s.scope.frozen {
            for dep in &wf.stages {
                if dep.id != s.id
                    && dep
                        .outputs
                        .iter()
                        .any(|o| f.contains(&format!("stages.{}.outputs.{}", dep.id, o.id)))
                    && dep.agent.kind != s.agent.kind
                {
                    return Some((dep.agent.kind.clone(), s.agent.kind.clone()));
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub fn build(
    wf: &Workflow,
    run_id: &str,
    task: &str,
    stages: &[StageRecord],
    rec: &Recorder,
    anchor: &str,
    branch: &str,
    where_ran: &str,
    elapsed: Duration,
) -> Receipt {
    let mut checks = Vec::new();
    let mut survivors = Vec::new();
    let mut not_checked = vec!["the agent could read the whole machine: no sandbox".to_string()];

    for s in stages {
        // A stage halted before any check (its agent couldn't start, setup failed): the run
        // proved nothing, and must say so rather than read as still pending.
        if s.gates.is_empty() && s.verdict == Verdict::Unwitnessed {
            checks.push(CheckRow {
                claim: format!("The `{}` stage ran", s.stage),
                verdict: Verdict::Unwitnessed,
                detail: s.notes.first().cloned().unwrap_or_default(),
                source: Source::Witnessed,
            });
        }
        for g in &s.gates {
            checks.push(CheckRow {
                claim: claim(wf, &s.stage, g),
                verdict: g.verdict,
                detail: g.detail.clone(),
                source: Source::Witnessed,
            });
            if let Some(m) = &g.mutation {
                survivors.extend(m.survivors.iter().map(|x| Survivor {
                    at: format!("{}:{}", x.file, x.line),
                    change: x.what.clone(),
                }));
            }
            for f in g.scope.iter().flat_map(|r| &r.generated) {
                not_checked.push(format!(
                    "{f} ({}): written by tooling, allowed but not reviewed",
                    s.stage
                ));
            }
            if g.verdict == Verdict::Unwitnessed {
                not_checked.push(format!("{} ({}): {}", g.gate, s.stage, g.detail));
            }
        }
        let reached = s.gates.len();
        for skipped in s.declared.iter().skip(reached) {
            not_checked.push(format!(
                "{skipped} ({}): never reached, an earlier check stopped the stage",
                s.stage
            ));
        }
        // A person's stage has no checks to declare and no tokens: their decision is the check.
        let human = s.agent == "human";
        if s.declared.is_empty() && !human {
            not_checked.push(format!("{}: the stage declares no checks", s.stage));
        }
        if s.tokens.is_none() && !human {
            not_checked.push(format!(
                "{}: token usage unavailable for `{}`",
                s.stage, s.agent
            ));
        }
        for n in &s.notes {
            not_checked.push(format!("{}: {n}", s.stage));
        }
    }
    for s in wf
        .stages
        .iter()
        .filter(|w| !stages.iter().any(|r| r.stage == w.id))
    {
        not_checked.push(format!("{}: never ran", s.id));
    }

    let all_passed =
        stages.iter().all(|s| s.verdict == Verdict::Passed) && stages.len() == wf.stages.len();
    if let Some((author, implementer)) = independent_tests(wf)
        && all_passed
    {
        checks.insert(
            0,
            CheckRow {
                claim: "Tests were written by a different agent".into(),
                verdict: Verdict::Passed,
                detail: format!("{author} wrote them; {implementer} could not change them"),
                source: Source::Witnessed,
            },
        );
    }

    let tokens: Option<u64> = stages.iter().map(|s| s.tokens).sum();
    let cost: Option<f64> = stages.iter().map(|s| s.cost_usd).sum();
    let turns: u64 = stages.iter().filter_map(|s| s.turns).sum();
    let tool_calls: usize = stages.iter().map(|s| s.tool_calls).sum();
    let chain: Vec<String> = stages
        .iter()
        .map(|s| {
            format!(
                "{} · {}{}",
                s.stage,
                s.agent,
                if s.attempts > 1 {
                    format!(" ({} tries)", s.attempts)
                } else {
                    String::new()
                }
            )
        })
        .collect();
    let usage = match (tokens, cost) {
        (Some(t), Some(c)) => format!("{t} measured · ${c:.2}"),
        (Some(t), None) => format!("{t} measured"),
        _ => "unavailable for at least one stage".into(),
    };
    let how = vec![
        ("stages".to_string(), chain.join(" → ")),
        (
            "where".to_string(),
            format!("{where_ran} · worktree branch {branch}"),
        ),
        (
            "agent actions".to_string(),
            format!("{tool_calls} tool calls observed · {turns} turns"),
        ),
        ("tokens".to_string(), usage),
        (
            "wall clock".to_string(),
            format!("{}m{:02}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60),
        ),
        (
            "trace".to_string(),
            format!(
                "{} · `conductor trace {run_id}`",
                crate::trace::trace_id(run_id)
            ),
        ),
    ];

    let reproduces = conductor_model::Chain::verify(rec.events()).is_ok();
    let first_line = task
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .trim_start_matches('#')
        .trim();
    let work = if first_line.is_empty() {
        wf.id.clone()
    } else {
        first_line.chars().take(72).collect()
    };
    Receipt {
        run_id: run_id.to_owned(),
        work,
        kind: wf.kind.name().to_owned(),
        checks,
        survivors,
        not_checked,
        how,
        integrity: Integrity {
            chain_head: rec.head().to_owned(),
            anchored_in: Some(anchor[..anchor.len().min(12)].to_owned()),
            signed: false,
            reproduces,
            trace_id: None,
        },
        also_at: vec![
            format!(".conductor/runs/{run_id}/receipt.json"),
            format!("conductor receipt {run_id}"),
        ],
    }
}
