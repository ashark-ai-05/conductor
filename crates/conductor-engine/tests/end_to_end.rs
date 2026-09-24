//! Whole runs against a throwaway Rust repository, with scripted agents standing in for
//! models so every outcome is deterministic.

use conductor_engine::{Options, run, verify};
use conductor_model::{Source, Verdict};
use std::fs;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

const TESTS: &str = "use demo::clamp;\n#[test] fn caps_high() { assert_eq!(clamp(50), 10); }\n#[test] fn keeps_low() { assert_eq!(clamp(5), 5); }\n";

/// A repository with a stubbed function and a build workflow whose agents are shell scripts.
/// `implement` is the implementer's script body.
fn repo(implement: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    fs::create_dir_all(p.join("src")).unwrap();
    fs::create_dir_all(p.join(".conductor/workflows")).unwrap();
    fs::write(
        p.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    fs::write(
        p.join("src/lib.rs"),
        "pub fn clamp(_x: i32) -> i32 { todo!() }\n",
    )
    .unwrap();
    fs::write(
        p.join(".gitignore"),
        "/target\n/.conductor/runs/\n/.test-conductor-home/\n",
    )
    .unwrap();
    let write_tests = format!("mkdir -p tests && cat > tests/clamp.rs <<'EOF'\n{TESTS}EOF\n");
    let workflow = format!(
        r#"id: scripted-build
version: 1
kind: build
defaults:
  retries: {{ max: 2, ladder: [fresh, fresh] }}
budget:
  max_stage_wall_clock_sec: 300
stages:
  - id: spec
    agent: {{ kind: script, command: ["sh", "-c", "mkdir -p .conductor/$CONDUCTOR_RUN_ID && echo '# clamp: cap at 10' > .conductor/$CONDUCTOR_RUN_ID/spec.md"] }}
    scope: {{ write: ["{{{{artifact_root}}}}/**"] }}
    outputs:
      - {{ id: spec, path: "{{{{artifact_root}}}}/spec.md" }}
    gate: {{ type: file_nonempty, ref: spec }}

  - id: tests
    depends_on: [spec]
    agent: {{ kind: script, command: ["sh", "-c", {write_tests:?}] }}
    scope: {{ write: ["tests/**"] }}
    outputs:
      - {{ id: tests, path: tests/clamp.rs }}
    gates:
      - {{ type: scope }}
      - type: command_assert
        command: ["cargo", "test", "--no-fail-fast", "--message-format", "json"]
        parser: cargo_json
        assert: ["compiled == true", "tests_failed > 0", "failures.kind all != 'trivial'"]

  - id: implement
    depends_on: [tests]
    agent: {{ kind: script, command: ["sh", "-c", {implement:?}] }}
    scope:
      write: ["src/**"]
      frozen: ["{{{{stages.tests.outputs.tests.path}}}}"]
    gates:
      - {{ type: scope }}
      - type: command_assert
        command: ["cargo", "test", "--message-format", "json"]
        parser: cargo_json
        reruns: 1
        assert: ["tests_failed == 0", "tests_run > 0"]
"#
    );
    fs::write(p.join(".conductor/workflows/build.yaml"), workflow).unwrap();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "base"]);
    d
}

fn options(dir: &Path) -> Options {
    Options {
        repo: dir.to_path_buf(),
        workflow: ".conductor/workflows/build.yaml".into(),
        base: "HEAD".into(),
        task: "# Clamp values above 10\n\nclamp(x) returns 10 for anything above 10.".into(),
        watcher: None,
        home: Some(dir.join(".test-conductor-home")),
        mode: conductor_engine::Mode::Headless,
        status_ui: None,
    }
}

const WRONG_THEN_RIGHT: &str = r#"if [ "$CONDUCTOR_ATTEMPT" = 1 ]; then echo 'pub fn clamp(x: i32) -> i32 { x }' > src/lib.rs; else echo 'pub fn clamp(x: i32) -> i32 { if x > 10 { 10 } else { x } }' > src/lib.rs; fi"#;

#[test]
fn a_run_retries_fresh_after_failing_tests_and_passes_with_a_verifiable_receipt() {
    let d = repo(WRONG_THEN_RIGHT);
    let out = run(options(d.path())).expect("run completes");
    assert_eq!(out.verdict, Verdict::Passed, "{:#?}", out.receipt);

    let claims: Vec<&str> = out
        .receipt
        .checks
        .iter()
        .map(|c| c.claim.as_str())
        .collect();
    assert!(
        claims.contains(&"Tests failed before the change, for the right reason"),
        "{claims:?}"
    );
    assert!(
        claims.contains(&"Tests pass after the change, on every run"),
        "{claims:?}"
    );
    assert!(
        out.receipt
            .checks
            .iter()
            .all(|c| c.source == Source::Witnessed)
    );
    assert!(
        out.receipt
            .not_checked
            .iter()
            .any(|n| n.contains("token usage unavailable"))
    );
    assert!(
        out.receipt
            .how
            .iter()
            .any(|(k, v)| k == "stages" && v.contains("implement · script (2 tries)")),
        "{:?}",
        out.receipt.how
    );

    let events = out.dir.read_events().unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.what.starts_with("attempt 2: fresh retry"))
    );
    assert!(
        events
            .iter()
            .any(|e| e.what.contains("command_assert failed"))
    );

    let v = verify(d.path(), &out.run_id);
    assert!(v.ok(), "{v:?}");
    assert_eq!(v.verdict, Some(Verdict::Passed));

    // The user's checkout is untouched: the work is on the run's branch.
    let lib = fs::read_to_string(d.path().join("src/lib.rs")).unwrap();
    assert!(lib.contains("todo!()"));
    let on_branch = Command::new("git")
        .arg("-C")
        .arg(d.path())
        .args(["show", &format!("{}:src/lib.rs", out.branch)])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&on_branch.stdout).contains("if x > 10"));
}

#[test]
fn an_implementer_that_edits_the_locked_tests_never_passes() {
    // Guts the tests so they pass, and writes a wrong implementation.
    let cheat = "echo 'pub fn clamp(x: i32) -> i32 { x }' > src/lib.rs; echo '#[test] fn ok() {}' > tests/clamp.rs";
    let d = repo(cheat);
    let out = run(options(d.path())).expect("run completes");
    assert_eq!(out.verdict, Verdict::Failed);
    let scope = out
        .receipt
        .checks
        .iter()
        .find(|c| {
            c.claim
                .starts_with("Only allowed files changed (implement)")
        })
        .expect("scope row");
    assert_eq!(scope.verdict, Verdict::Failed);
    assert!(
        scope
            .detail
            .contains("tests/clamp.rs is locked by an earlier stage"),
        "{}",
        scope.detail
    );
    assert!(
        out.receipt
            .not_checked
            .iter()
            .any(|n| n.contains("never reached"))
    );
    assert!(verify(d.path(), &out.run_id).ok());
}

#[test]
fn tampering_with_the_stored_record_is_caught() {
    let d = repo(WRONG_THEN_RIGHT);
    let out = run(options(d.path())).expect("run completes");
    let path = out.dir.events();
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("command_assert failed", "command_assert passed");
    fs::write(&path, text).unwrap();
    let v = verify(d.path(), &out.run_id);
    assert!(!v.ok());
    assert!(!v.chain_intact, "{v:?}");
}

#[test]
fn the_live_view_ends_with_the_runs_verdict_and_every_stage() {
    let d = repo(WRONG_THEN_RIGHT);
    let out = run(options(d.path())).expect("run completes");
    let live = conductor_engine::live::read(d.path(), &out.receipt.run_id).expect("live.json");
    assert_eq!(live.ended, Some(Verdict::Passed));
    assert_eq!(
        live.stages
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["spec", "tests", "implement"]
    );
    assert!(live.stages.iter().all(|s| s.status == Verdict::Passed));
    assert!(
        live.stages[2].detail.contains("after 2 tries"),
        "{:?}",
        live.stages[2]
    );
    assert!(!live.log.is_empty());
    // A finished run is not listed as running.
    assert!(conductor_engine::live::running(d.path()).is_empty());
}

#[test]
fn a_workflow_is_read_from_the_base_not_the_working_tree() {
    let d = repo(WRONG_THEN_RIGHT);
    // An uncommitted edit that would loosen the rules must have no effect.
    let wf = d.path().join(".conductor/workflows/build.yaml");
    fs::write(&wf, "not: a workflow").unwrap();
    let out = run(options(d.path())).expect("the committed workflow is used");
    assert_eq!(out.verdict, Verdict::Passed);
}

/// The same workflow in real herdr panes. Runs only when `CONDUCTOR_HERDR_BIN` is set.
#[test]
fn a_run_in_herdr_gets_a_tab_and_closes_each_passed_stages_pane() {
    let Ok(bin) = std::env::var("CONDUCTOR_HERDR_BIN") else {
        eprintln!("CONDUCTOR_HERDR_BIN not set; skipping");
        return;
    };
    let session = format!("conductor-e2e-{}", std::process::id());
    let mut server = Command::new(&bin)
        .args(["--session", &session, "server"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline
        && !Command::new(&bin)
            .args(["--session", &session, "status", "--json"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    let d = repo(WRONG_THEN_RIGHT);
    let mut opts = options(d.path());
    opts.mode = conductor_engine::Mode::Herdr(conductor_herdr::Herdr::with(
        &bin,
        conductor_herdr::Target::Session(session.clone()),
    ));
    let out = run(opts);

    let _ = Command::new(&bin)
        .args(["--session", &session, "server", "stop"])
        .output();
    let _ = server.wait();

    let out = out.expect("run completes");
    assert_eq!(out.verdict, Verdict::Passed, "{:#?}", out.receipt);
    let events = out.dir.read_events().unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.what.starts_with("herdr protocol 22; run tab")),
        "{events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.source == Source::Inferred && e.what.starts_with("stage pane"))
    );
    // Every passed stage's pane is accounted for; the tab's last pane is kept, never
    // closed, because closing it would close the run's tab.
    let panes: Vec<&str> = events
        .iter()
        .filter(|e| e.what.contains("the stage's pane"))
        .map(|e| e.what.as_str())
        .collect();
    assert_eq!(panes.len(), 3, "{events:#?}");
    assert!(panes.iter().all(
        |w| *w == "kept the stage's pane for the next stage" || *w == "closed the stage's pane"
    ));
    assert!(
        out.receipt
            .how
            .iter()
            .any(|(k, v)| k == "where" && v.starts_with("herdr"))
    );
}

#[test]
fn a_lockfile_rewritten_by_tooling_is_allowed_and_named_on_the_receipt() {
    let d = repo(
        "echo 'pub fn clamp(x: i32) -> i32 { if x > 10 { 10 } else { x } }' > src/lib.rs; echo '# touched' >> Cargo.lock",
    );
    let out = run(options(d.path())).expect("run completes");
    assert_eq!(out.verdict, Verdict::Passed, "{:#?}", out.receipt);
    assert!(
        out.receipt
            .not_checked
            .iter()
            .any(|n| n.starts_with("Cargo.lock (implement): written by tooling")),
        "{:#?}",
        out.receipt.not_checked
    );
}

#[test]
fn the_policy_can_turn_lockfile_allowance_off() {
    let d = repo(
        "echo 'pub fn clamp(x: i32) -> i32 { if x > 10 { 10 } else { x } }' > src/lib.rs; echo '# touched' >> Cargo.lock",
    );
    fs::write(d.path().join(".conductor/policy.yaml"), "generated: []\n").unwrap();
    git(d.path(), &["add", "-A"]);
    git(d.path(), &["commit", "-qm", "policy"]);
    let out = run(options(d.path())).expect("run completes");
    assert_eq!(out.verdict, Verdict::Failed);
    assert!(
        out.receipt.checks.iter().any(|c| c
            .detail
            .starts_with("Cargo.lock is outside what this stage may change")),
        "{:#?}",
        out.receipt.checks
    );
}
