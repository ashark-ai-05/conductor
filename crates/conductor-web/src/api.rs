//! What the browser asks for, assembled from a repository's run records. Everything here is
//! read from `.conductor/runs/<id>/` and git; nothing is written.

use crate::timeline::{self, Timeline};
use conductor_engine::store::RunDir;
use conductor_engine::{live, metrics, trace};
use conductor_model::view::LiveRun;
use conductor_model::{Event, Receipt, Verdict};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct StageSummary {
    pub name: String,
    pub status: Verdict,
    pub attempts: usize,
}

/// One row of the runs list.
#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub work: String,
    pub kind: String,
    pub verdict: Verdict,
    /// Still being run by a live conductor process.
    pub running: bool,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<u64>,
    pub stages: Vec<StageSummary>,
    pub cost_usd: f64,
    pub tokens: u64,
    pub catches: usize,
    pub checks_passed: usize,
    pub checks: usize,
    pub survivors: usize,
}

/// Everything the run page shows.
#[derive(Debug, Clone, Serialize)]
pub struct RunDetail {
    pub summary: RunSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Receipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<LiveRun>,
    pub timeline: Timeline,
    /// `(stage, attempt)` pairs whose diff is on disk.
    pub diffs: Vec<(String, usize)>,
}

/// What a page polls for while a run is in progress.
#[derive(Debug, Clone, Serialize)]
pub struct Tail {
    pub summary: RunSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<LiveRun>,
    /// Entries after `since`.
    pub entries: Vec<timeline::Entry>,
    pub attempts: Vec<timeline::Attempt>,
    pub cost_usd: f64,
    pub tokens: u64,
    pub catches: usize,
    pub diffs: Vec<(String, usize)>,
    pub ended: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricPoint {
    pub attrs: BTreeMap<String, String>,
    pub value: f64,
    /// For a histogram, the raw observations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observations: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricOut {
    pub name: String,
    pub unit: String,
    pub description: String,
    pub points: Vec<MetricPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub runs: Vec<RunSummary>,
    pub metrics: Vec<MetricOut>,
}

fn events_of(repo: &Path, id: &str) -> Vec<Event> {
    RunDir::for_run(repo, id).read_events().unwrap_or_default()
}

fn receipt_of(repo: &Path, id: &str) -> Option<Receipt> {
    let text = std::fs::read_to_string(RunDir::for_run(repo, id).receipt()).ok()?;
    serde_json::from_str(&text).ok()
}

fn diffs_of(repo: &Path, id: &str) -> Vec<(String, usize)> {
    let dir = RunDir::for_run(repo, id).root.join("attempts");
    let mut out: Vec<(String, usize)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".patch")?;
            let (stage, n) = stem.rsplit_once('-')?;
            Some((stage.to_owned(), n.parse().ok()?))
        })
        .collect();
    out.sort();
    out
}

fn summarize(
    id: &str,
    events: &[Event],
    receipt: Option<&Receipt>,
    live: Option<&LiveRun>,
    t: &Timeline,
) -> RunSummary {
    let tr = trace::build(id, events);
    let run_span = tr.spans.first();
    let started_at = events.first().map(|e| e.at.clone()).unwrap_or_default();
    let ended = events.iter().rev().any(|e| {
        e.stage.is_none() && e.what.starts_with("run ") && !e.what.starts_with("run started")
    });
    let duration_s = run_span
        .filter(|_| ended)
        .map(|s| u64::try_from((s.end_ns.saturating_sub(s.start_ns)) / 1_000_000_000).unwrap_or(0));
    let running = receipt.is_none() && live.is_some_and(|l| l.ended.is_none());
    let verdict = match (receipt, live) {
        (Some(r), _) => r.verdict(),
        (None, Some(l)) => l.ended.unwrap_or(Verdict::Running),
        (None, None) => Verdict::Unwitnessed,
    };
    // Stages: from the live view when there is one (it knows every declared stage), else
    // from what the timeline saw.
    let stages: Vec<StageSummary> = match live {
        Some(l) => l
            .stages
            .iter()
            .map(|s| StageSummary {
                name: s.name.clone(),
                status: s.status,
                attempts: t.attempts.iter().filter(|a| a.stage == s.name).count(),
            })
            .collect(),
        None => {
            let mut seen: Vec<StageSummary> = vec![];
            for a in &t.attempts {
                match seen.iter_mut().find(|s| s.name == a.stage) {
                    Some(s) => {
                        s.attempts += 1;
                        s.status = outcome_verdict(&a.outcome);
                    }
                    None => seen.push(StageSummary {
                        name: a.stage.clone(),
                        status: outcome_verdict(&a.outcome),
                        attempts: 1,
                    }),
                }
            }
            seen
        }
    };
    let work = receipt
        .map(|r| r.work.clone())
        .or_else(|| live.map(|l| l.work.clone()))
        .unwrap_or_else(|| "untitled".into());
    let kind = receipt
        .map(|r| r.kind.clone())
        .or_else(|| live.map(|l| l.kind.clone()))
        .unwrap_or_default();
    RunSummary {
        run_id: id.to_owned(),
        work,
        kind,
        verdict,
        running,
        started_at,
        duration_s,
        stages,
        cost_usd: t.cost_usd,
        tokens: t.tokens,
        catches: t.catches,
        checks_passed: receipt.map(Receipt::passed_count).unwrap_or(0),
        checks: receipt.map(|r| r.checks.len()).unwrap_or(0),
        survivors: receipt.map(|r| r.survivors.len()).unwrap_or(0),
    }
}

fn outcome_verdict(outcome: &str) -> Verdict {
    match outcome {
        "passed" => Verdict::Passed,
        "check_failed" => Verdict::Failed,
        "running" => Verdict::Running,
        _ => Verdict::Unwitnessed,
    }
}

pub fn runs(repo: &Path) -> Vec<RunSummary> {
    let mut out: Vec<RunSummary> = conductor_engine::list_runs_any(repo)
        .iter()
        .map(|id| {
            let events = events_of(repo, id);
            let receipt = receipt_of(repo, id);
            let live = live::read(repo, id);
            let t = timeline::build(id, &events);
            summarize(id, &events, receipt.as_ref(), live.as_ref(), &t)
        })
        .collect();
    out.sort_by_key(|r| !r.running);
    out
}

pub fn run(repo: &Path, id: &str) -> Option<RunDetail> {
    let events = events_of(repo, id);
    if events.is_empty() {
        return None;
    }
    let receipt = receipt_of(repo, id);
    let live = live::read(repo, id);
    let t = timeline::build(id, &events);
    let summary = summarize(id, &events, receipt.as_ref(), live.as_ref(), &t);
    Some(RunDetail {
        summary,
        receipt,
        live,
        timeline: t,
        diffs: diffs_of(repo, id),
    })
}

pub fn tail(repo: &Path, id: &str, since: u64) -> Option<Tail> {
    let events = events_of(repo, id);
    if events.is_empty() {
        return None;
    }
    let receipt = receipt_of(repo, id);
    let live = live::read(repo, id);
    let t = timeline::build(id, &events);
    let summary = summarize(id, &events, receipt.as_ref(), live.as_ref(), &t);
    let ended = !summary.running;
    Some(Tail {
        summary,
        live,
        entries: t.entries.into_iter().filter(|e| e.seq > since).collect(),
        attempts: t.attempts,
        cost_usd: t.cost_usd,
        tokens: t.tokens,
        catches: t.catches,
        diffs: diffs_of(repo, id),
        ended,
    })
}

pub fn diff(repo: &Path, id: &str, stage: &str, attempt: usize) -> Option<String> {
    if stage.contains(['/', '\\', '.']) {
        return None;
    }
    std::fs::read_to_string(RunDir::for_run(repo, id).attempt_diff(stage, attempt)).ok()
}

pub fn stats(repo: &Path) -> Stats {
    let runs = runs(repo);
    let mut all = Vec::new();
    for r in &runs {
        let events = events_of(repo, &r.run_id);
        let t = trace::build(&r.run_id, &events);
        metrics::merge(&mut all, metrics::collect(&t, &events));
    }
    let metrics = all
        .into_iter()
        .map(|m| MetricOut {
            name: m.name.to_owned(),
            unit: m.unit.to_owned(),
            description: m.description.to_owned(),
            points: m
                .points
                .into_iter()
                .map(|(attrs, v)| {
                    let (value, observations) = match v {
                        metrics::Value::Int(i) => (i as f64, None),
                        metrics::Value::Double(d) => (d, None),
                        metrics::Value::Observations(o) => (o.iter().sum::<f64>(), Some(o)),
                    };
                    MetricPoint {
                        attrs: attrs.into_iter().collect(),
                        value,
                        observations,
                    }
                })
                .collect(),
        })
        .collect();
    Stats { runs, metrics }
}
