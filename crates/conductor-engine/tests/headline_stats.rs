//! The runs screen's three headline numbers, built from every run's metrics rather than from
//! receipts, so they show how often checks caught an agent.

use std::fs;
use std::path::Path;

use conductor_engine::metrics::headline_stats;

const FIXTURE: &str = include_str!("../../../docs/examples/live-run-durations/events.jsonl");
const EMPTY_RUN: &str = r#"{"seq":0,"at":"2026-01-01T00:00:00Z","source":"witnessed","what":"run started: demo at abcdef","prev":"sha256:0","hash":"sha256:1"}"#;

fn write_run(repo: &Path, run_id: &str, events_jsonl: &str) {
    let dir = repo.join(".conductor").join("runs").join(run_id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("events.jsonl"), events_jsonl).unwrap();
    fs::write(dir.join("receipt.json"), "{}").unwrap();
}

#[test]
fn no_conductor_directory_at_all_is_none() {
    let repo = tempfile::tempdir().unwrap();
    assert_eq!(headline_stats(repo.path()), None);
}

#[test]
fn an_empty_runs_directory_is_none() {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join(".conductor").join("runs")).unwrap();
    assert_eq!(headline_stats(repo.path()), None);
}

#[test]
fn a_run_without_a_receipt_is_not_recorded() {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path().join(".conductor").join("runs").join("R1");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("events.jsonl"), FIXTURE).unwrap();
    // No receipt.json: list_runs should not count this as a recorded run.
    assert_eq!(headline_stats(repo.path()), None);
}

#[test]
fn one_recorded_run_matches_the_documented_example() {
    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", FIXTURE);
    let stats = headline_stats(repo.path()).expect("a repository with a run has stats");
    assert_eq!(
        stats[0],
        (
            "1 of 1 passed".to_string(),
            "runs recorded in this repository".to_string()
        )
    );
    assert_eq!(
        stats[1],
        (
            "33% caught".to_string(),
            "the agent said done, a check said no".to_string()
        )
    );
    assert_eq!(
        stats[2],
        (
            "$0.44".to_string(),
            "672k tokens across all runs".to_string()
        )
    );
}

#[test]
fn two_recorded_runs_are_merged_before_the_stats_are_derived() {
    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", FIXTURE);
    write_run(repo.path(), "R2", FIXTURE);
    let stats = headline_stats(repo.path()).unwrap();
    assert_eq!(stats[0].0, "2 of 2 passed");
    // The catch rate is a ratio, so it does not double even though the totals do.
    assert_eq!(stats[1].0, "33% caught");
    assert_eq!(
        stats[2],
        (
            "$0.88".to_string(),
            "1.3M tokens across all runs".to_string()
        )
    );
}

#[test]
fn a_run_with_no_finished_attempts_shows_a_dash_for_the_catch_rate() {
    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", EMPTY_RUN);
    let stats = headline_stats(repo.path()).unwrap();
    assert_eq!(
        stats[1],
        (
            "— caught".to_string(),
            "the agent said done, a check said no".to_string()
        )
    );
}

#[test]
fn labels_are_the_documented_ones() {
    let repo = tempfile::tempdir().unwrap();
    write_run(repo.path(), "R1", FIXTURE);
    let stats = headline_stats(repo.path()).unwrap();
    assert_eq!(stats[0].1, "runs recorded in this repository");
    assert_eq!(stats[1].1, "the agent said done, a check said no");
    assert_eq!(stats[2].1, "672k tokens across all runs");
}
