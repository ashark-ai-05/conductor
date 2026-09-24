//! `conductor_engine::metrics::headline_stats`: the runs screen's three headline numbers,
//! computed from every run's metrics instead of from receipts alone.

use conductor_engine::metrics::{self, headline_stats};
use conductor_model::{Event, Source};
use std::fs;
use std::path::Path;

fn fixture() -> Vec<Event> {
    include_str!("../../../docs/examples/live-run-durations/events.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Records a run in `repo`: its events, and a receipt file so `list_runs` picks it up.
/// The receipt's own contents are never read by `headline_stats`.
fn write_run(repo: &Path, id: &str, events: &[Event]) {
    let dir = repo.join(".conductor").join("runs").join(id);
    fs::create_dir_all(&dir).unwrap();
    let text: String = events
        .iter()
        .map(|e| format!("{}\n", serde_json::to_string(e).unwrap()))
        .collect();
    fs::write(dir.join("events.jsonl"), text).unwrap();
    fs::write(dir.join("receipt.json"), "{}").unwrap();
}

/// A run with no stages at all: it starts and then halts before any attempt, so it has zero
/// finished attempts and zero catches.
fn no_attempts_run() -> Vec<Event> {
    vec![
        Event {
            seq: 0,
            at: "2024-01-01T00:00:00Z".into(),
            source: Source::Witnessed,
            stage: None,
            what: "run started: demo-workflow at abc123".into(),
            prev: "sha256:genesis".into(),
            hash: "sha256:one".into(),
        },
        Event {
            seq: 1,
            at: "2024-01-01T00:00:05Z".into(),
            source: Source::Witnessed,
            stage: None,
            what: "run failed in 5s".into(),
            prev: "sha256:one".into(),
            hash: "sha256:two".into(),
        },
    ]
}

/// The `human()` formatting `conductor stats` uses: "672k", "2.6M", or the plain number.
fn human(n: f64) -> String {
    match n {
        n if n >= 1e6 => format!("{:.1}M", n / 1e6),
        n if n >= 1e3 => format!("{:.0}k", n / 1e3),
        n => format!("{n:.0}"),
    }
}

#[test]
fn a_repository_with_no_recorded_runs_has_no_headline_stats() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join(".conductor").join("runs")).unwrap();
    assert_eq!(headline_stats(d.path()), None);
}

#[test]
fn a_repository_that_was_never_touched_has_no_headline_stats() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(headline_stats(d.path()), None);
}

#[test]
fn one_real_run_matches_the_documented_example() {
    let d = tempfile::tempdir().unwrap();
    write_run(d.path(), "0MUE8IKOXFM", &fixture());
    let stats = headline_stats(d.path()).expect("a run is recorded");
    assert_eq!(
        stats,
        [
            (
                "1 of 1 passed".to_string(),
                "runs recorded in this repository".to_string()
            ),
            (
                "33% caught".to_string(),
                "the agent said done, a check said no".to_string()
            ),
            (
                "$0.44".to_string(),
                "672k tokens across all runs".to_string()
            ),
        ]
    );
}

#[test]
fn two_runs_add_up_across_the_repository() {
    let d = tempfile::tempdir().unwrap();
    write_run(d.path(), "RUN1", &fixture());
    write_run(d.path(), "RUN2", &fixture());
    let stats = headline_stats(d.path()).expect("two runs are recorded");

    // Cross-check against the already-tested collect/merge/total path, rather than
    // hard-coding numbers that would need to be kept in sync by hand.
    let e = fixture();
    let mut all = metrics::collect(&conductor_engine::trace::build("RUN1", &e), &e);
    metrics::merge(&mut all, metrics::collect(&conductor_engine::trace::build("RUN2", &e), &e));
    let runs = metrics::total(&all, "conductor.runs", &[]);
    let passed = metrics::total(&all, "conductor.runs", &[("verdict", "passed")]);
    let finished = metrics::total(&all, "conductor.attempts", &[("outcome", "passed")])
        + metrics::total(&all, "conductor.attempts", &[("outcome", "check_failed")]);
    let caught = metrics::total(&all, "conductor.catches", &[]);
    let cost = metrics::total(&all, "conductor.cost", &[]);
    let tokens = metrics::total(&all, "conductor.tokens", &[]);

    assert_eq!(stats[0].0, format!("{passed:.0} of {runs:.0} passed"));
    assert_eq!(stats[0].1, "runs recorded in this repository");
    assert_eq!(
        stats[1].0,
        format!("{:.0}% caught", 100.0 * caught / finished)
    );
    assert_eq!(stats[1].1, "the agent said done, a check said no");
    assert_eq!(stats[2].0, format!("${cost:.2}"));
    assert_eq!(stats[2].1, format!("{} tokens across all runs", human(tokens)));
}

#[test]
fn a_run_with_no_finished_attempts_shows_a_dash_for_caught() {
    let d = tempfile::tempdir().unwrap();
    write_run(d.path(), "R1", &no_attempts_run());
    let stats = headline_stats(d.path()).expect("a run is recorded");
    assert_eq!(stats[1].0, "— caught");
    assert_eq!(stats[1].1, "the agent said done, a check said no");
    assert_eq!(stats[0].0, "0 of 1 passed");
    assert_eq!(stats[2].0, "$0.00");
    assert_eq!(stats[2].1, "0 tokens across all runs");
}
