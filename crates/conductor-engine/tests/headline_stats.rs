//! `conductor_engine::metrics::headline_stats`: the runs screen's three headline numbers,
//! computed from every run's metrics instead of from receipts.

use conductor_engine::metrics::headline_stats;
use conductor_engine::store::RunDir;
use conductor_model::{Event, Source};
use std::path::Path;

/// A minimal event. `seq` also drives a distinct, valid timestamp so events order correctly.
fn ev(seq: u64, source: Source, stage: Option<&str>, what: &str) -> Event {
    Event {
        seq,
        at: format!("2026-01-01T00:00:{seq:02}Z"),
        source,
        stage: stage.map(str::to_owned),
        what: what.to_owned(),
        prev: format!("sha256:{}", seq.wrapping_sub(1)),
        hash: format!("sha256:{seq}"),
    }
}

fn witnessed(seq: u64, stage: Option<&str>, what: &str) -> Event {
    ev(seq, Source::Witnessed, stage, what)
}

fn measured(seq: u64, stage: &str, what: &str) -> Event {
    ev(seq, Source::Measured, Some(stage), what)
}

fn write_run(repo: &Path, run_id: &str, events: &[Event]) {
    let dir = RunDir::create(repo, run_id).unwrap();
    let text: String = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap() + "\n")
        .collect();
    std::fs::write(dir.events(), text).unwrap();
    // `list_runs` only checks that a receipt exists; its content is never read here.
    std::fs::write(dir.receipt(), "{}").unwrap();
}

fn fixture_events() -> Vec<Event> {
    include_str!("../../../docs/examples/live-run-durations/events.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// A run with no stage activity at all: just started and ended.
fn bare_run(workflow: &str, verdict: &str) -> Vec<Event> {
    let started = format!("run started: {workflow} at abc123");
    let ended = format!("run {verdict} in 1s");
    let start = witnessed(0, None, &started);
    let end = witnessed(1, None, &ended);
    vec![start, end]
}

#[test]
fn no_repo_gives_none() {
    let repo = tempfile::tempdir().unwrap();
    assert_eq!(headline_stats(repo.path()), None);
}

#[test]
fn an_empty_runs_directory_gives_none() {
    let repo = tempfile::tempdir().unwrap();
    let runs_dir = repo.path().join(".conductor").join("runs");
    std::fs::create_dir_all(runs_dir).unwrap();
    assert_eq!(headline_stats(repo.path()), None);
}

#[test]
fn a_single_recorded_run_matches_the_worked_example() {
    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", &fixture_events());

    let stats = headline_stats(repo.path()).expect("one run is recorded");
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
fn two_runs_add_up_and_round_the_percentage_the_same_way() {
    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", &fixture_events());
    write_run(repo.path(), "R2", &fixture_events());

    let stats = headline_stats(repo.path()).expect("two runs are recorded");
    assert_eq!(stats[0].0, "2 of 2 passed");
    assert_eq!(stats[1].0, "33% caught");
    assert_eq!(stats[2].0, "$0.88");
    assert_eq!(stats[2].1, "1.3M tokens across all runs");
}

#[test]
fn no_finished_attempts_reads_as_an_em_dash_not_a_percent() {
    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", &bare_run("solo", "passed"));

    let stats = headline_stats(repo.path()).expect("one run is recorded");
    assert_eq!(stats[0].0, "1 of 1 passed");
    assert_eq!(stats[1].0, "— caught");
    assert_eq!(stats[2].0, "$0.00");
    assert_eq!(stats[2].1, "0 tokens across all runs");
}

#[test]
fn the_percentage_rounds_to_the_nearest_whole_number() {
    // Two attempts caught by a check, then a pass: 2 of 3 finished attempts is 66.67%, which
    // rounds up to 67%, not truncates to 66%.
    let events = vec![
        witnessed(0, None, "run started: rounding at abc123"),
        witnessed(1, Some("s"), "attempt 1: first try with a"),
        witnessed(2, Some("s"), "scope failed: nope"),
        witnessed(3, Some("s"), "attempt 2: retry with a"),
        witnessed(4, Some("s"), "scope failed: nope again"),
        witnessed(5, Some("s"), "attempt 3: retry with a"),
        witnessed(6, Some("s"), "stage passed on attempt 3"),
        witnessed(7, None, "run passed in 3s"),
    ];
    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", &events);

    let stats = headline_stats(repo.path()).expect("one run is recorded");
    assert_eq!(stats[1].0, "67% caught");
}

#[test]
fn attempts_where_the_agent_never_finished_do_not_count_as_finished() {
    // Attempt 1 never finishes (no catch, no pass): it must not inflate the denominator.
    // Attempt 2 is caught by a check, attempt 3 passes: 1 of 2 finished attempts is 50%.
    let events = vec![
        witnessed(0, None, "run started: solo at abc123"),
        witnessed(1, Some("s"), "attempt 1: first try with a"),
        witnessed(2, Some("s"), "agent did not finish: crashed"),
        witnessed(3, Some("s"), "attempt 2: retry with a"),
        witnessed(4, Some("s"), "scope failed: bad"),
        witnessed(5, Some("s"), "attempt 3: retry with a"),
        witnessed(6, Some("s"), "stage passed on attempt 3"),
        witnessed(7, None, "run passed in 2s"),
    ];
    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", &events);

    let stats = headline_stats(repo.path()).expect("one run is recorded");
    assert_eq!(stats[1].0, "50% caught");
}

#[test]
fn small_token_and_cost_totals_are_not_abbreviated() {
    let events = vec![
        witnessed(0, None, "run started: solo at abc123"),
        witnessed(1, Some("s"), "attempt 1: first try with a"),
        measured(2, "s", "500 tokens · $0.01"),
        witnessed(3, Some("s"), "stage passed on attempt 1"),
        witnessed(4, None, "run passed in 1s"),
    ];
    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", &events);

    let stats = headline_stats(repo.path()).expect("one run is recorded");
    assert_eq!(stats[1].0, "0% caught");
    assert_eq!(stats[2].0, "$0.01");
    assert_eq!(stats[2].1, "500 tokens across all runs");
}

#[test]
fn a_run_that_did_not_pass_still_counts_toward_the_denominator() {
    // One run finishes cleanly (2 of 3 finished attempts caught, like the rounding fixture
    // above), the other never gets past its first stage attempt and the run itself fails.
    let clean = vec![
        witnessed(0, None, "run started: rounding at abc123"),
        witnessed(1, Some("s"), "attempt 1: first try with a"),
        witnessed(2, Some("s"), "scope failed: nope"),
        witnessed(3, Some("s"), "attempt 2: retry with a"),
        witnessed(4, Some("s"), "scope failed: nope again"),
        witnessed(5, Some("s"), "attempt 3: retry with a"),
        witnessed(6, Some("s"), "stage passed on attempt 3"),
        witnessed(7, None, "run passed in 3s"),
    ];
    let failed = bare_run("solo", "failed");

    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", &clean);
    write_run(repo.path(), "R2", &failed);

    let stats = headline_stats(repo.path()).expect("two runs are recorded");
    assert_eq!(stats[0].0, "1 of 2 passed");
    // The failed run contributed no attempts and no cost, so the other numbers are unchanged.
    assert_eq!(stats[1].0, "67% caught");
    assert_eq!(stats[2].0, "$0.00");
    assert_eq!(stats[2].1, "0 tokens across all runs");
}
