//! The JSON API over a run record written the way the engine writes one.

use conductor_engine::store::{Recorder, RunDir};
use conductor_model::Source;
use conductor_web::respond;
use std::io::Read;
use std::path::Path;
use tiny_http::Method;

fn body(repo: &Path, url: &str) -> (u16, String) {
    let r = respond(repo, &Method::Get, url, "");
    let status = r.status_code().0;
    let mut s = String::new();
    r.into_reader().read_to_string(&mut s).unwrap();
    (status, s)
}

fn record(repo: &Path, id: &str, finished: bool) {
    let dir = RunDir::create(repo, id).unwrap();
    let mut rec = Recorder::open(&dir, None).unwrap();
    let w = |rec: &mut Recorder, st: Option<&str>, what: &str| {
        rec.record(Source::Witnessed, st, what).unwrap();
    };
    w(&mut rec, None, "run started: build at abc123");
    w(&mut rec, Some("tests"), "attempt 1: first try with script");
    rec.record(Source::Observed, Some("tests"), "Write tests/a.rs")
        .unwrap();
    rec.record(Source::Measured, Some("tests"), "100 tokens · $0.10")
        .unwrap();
    w(
        &mut rec,
        Some("tests"),
        "scope failed: 1 file(s) outside scope",
    );
    w(
        &mut rec,
        Some("tests"),
        "attempt 2: fresh retry with script",
    );
    w(
        &mut rec,
        Some("tests"),
        "scope passed: 1 file(s) changed, all within scope",
    );
    if finished {
        w(
            &mut rec,
            Some("tests"),
            "stage passed on attempt 2; committed 1234567",
        );
        w(&mut rec, None, "run passed in 3s");
        let receipt = serde_json::json!({
            "run_id": id, "work": "Add clamp", "kind": "build",
            "checks": [{"claim": "Only allowed files changed (tests)", "verdict": "passed", "detail": "ok", "source": "witnessed"}],
            "survivors": [], "not_checked": [], "how": [["stages", "tests · script (2 tries)"]],
            "integrity": {"chain_head": rec.head(), "anchored_in": "deadbeef", "signed": false, "reproduces": true, "trace_id": null},
            "also_at": []
        });
        dir.write_json(&dir.receipt(), &receipt).unwrap();
    }
    let patch = dir.attempt_diff("tests", 1);
    std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
    std::fs::write(patch, "diff --git a/tests/a.rs b/tests/a.rs\n+fn t() {}\n").unwrap();
}

#[test]
fn runs_are_listed_with_what_the_record_says() {
    let d = tempfile::tempdir().unwrap();
    record(d.path(), "0MUAAAAAAAAA1", true);
    record(d.path(), "0MUAAAAAAAAA2", false);
    let (status, s) = body(d.path(), "/api/runs");
    assert_eq!(status, 200);
    let runs: serde_json::Value = serde_json::from_str(&s).unwrap();
    let runs = runs.as_array().unwrap();
    assert_eq!(runs.len(), 2);
    let done = runs
        .iter()
        .find(|r| r["run_id"] == "0MUAAAAAAAAA1")
        .unwrap();
    assert_eq!(done["verdict"], "passed");
    assert_eq!(done["catches"], 1);
    assert_eq!(done["cost_usd"], 0.1);
    assert_eq!(done["stages"][0]["attempts"], 2);
    assert_eq!(done["duration_s"], 0);
    // Never finished, and no conductor process claims it: not running, not passed.
    let dead = runs
        .iter()
        .find(|r| r["run_id"] == "0MUAAAAAAAAA2")
        .unwrap();
    assert_eq!(dead["verdict"], "unwitnessed");
    assert_eq!(dead["running"], false);
}

#[test]
fn a_run_has_two_lanes_a_receipt_and_its_diffs() {
    let d = tempfile::tempdir().unwrap();
    record(d.path(), "0MUAAAAAAAAA1", true);
    let (status, s) = body(d.path(), "/api/runs/0MUAAAAAAAAA1");
    assert_eq!(status, 200);
    let run: serde_json::Value = serde_json::from_str(&s).unwrap();
    let entries = run["timeline"]["entries"].as_array().unwrap();
    assert_eq!(entries[2]["lane"], "agent");
    assert_eq!(entries[2]["tool"], "Write");
    assert_eq!(entries[4]["lane"], "conductor");
    assert_eq!(entries[4]["verdict"], "failed");
    assert_eq!(run["timeline"]["attempts"][0]["caught_by"], "scope");
    assert_eq!(run["receipt"]["work"], "Add clamp");
    assert_eq!(run["diffs"], serde_json::json!([["tests", 1]]));

    let (status, patch) = body(d.path(), "/api/runs/0MUAAAAAAAAA1/diff/tests/1");
    assert_eq!(status, 200);
    assert!(patch.contains("+fn t() {}"));
    assert_eq!(
        body(d.path(), "/api/runs/0MUAAAAAAAAA1/diff/tests/2").0,
        404
    );

    let (_, tail) = body(d.path(), "/api/runs/0MUAAAAAAAAA1/tail?since=5");
    let tail: serde_json::Value = serde_json::from_str(&tail).unwrap();
    assert_eq!(tail["entries"].as_array().unwrap().len(), 3);
    assert_eq!(tail["ended"], true);
}

#[test]
fn only_conductors_own_paths_are_served() {
    let d = tempfile::tempdir().unwrap();
    record(d.path(), "0MUAAAAAAAAA1", true);
    assert_eq!(body(d.path(), "/api/runs/../../etc/passwd").0, 404);
    assert_eq!(body(d.path(), "/api/runs/0MUAAAAAAAAA1/diff/../1").0, 404);
    assert_eq!(body(d.path(), "/api/runs/NOPE").0, 404);
    assert_eq!(body(d.path(), "/app.css").0, 200);
    assert!(body(d.path(), "/run").1.contains("<title>conductor"));
    assert_eq!(
        respond(d.path(), &Method::Post, "/api/runs", "")
            .status_code()
            .0,
        405
    );
    // The one write: a decision, once.
    let decide = |body: &str| {
        respond(
            d.path(),
            &Method::Post,
            "/api/runs/0MUAAAAAAAAA1/decide",
            body,
        )
        .status_code()
        .0
    };
    assert_eq!(
        decide(r#"{"stage":"review","approved":true,"by":"PO"}"#),
        200
    );
    assert_eq!(decide(r#"{"stage":"review","approved":false}"#), 409);
    assert_eq!(decide(r#"{"stage":"../x","approved":true}"#), 400);
    let d1 =
        conductor_engine::decision::read(&RunDir::for_run(d.path(), "0MUAAAAAAAAA1"), "review")
            .unwrap();
    assert_eq!((d1.approved, d1.by.as_str()), (true, "PO"));
    // The one write: a decision, once.
    let (_, stats) = body(d.path(), "/api/stats");
    assert!(stats.contains("conductor.catches"));
}
