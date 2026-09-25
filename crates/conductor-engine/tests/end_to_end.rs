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

    // Each attempt's diff is kept, so a person can see try 1 next to try 2.
    let try1 = fs::read_to_string(out.dir.attempt_diff("implement", 1)).unwrap();
    let try2 = fs::read_to_string(out.dir.attempt_diff("implement", 2)).unwrap();
    assert!(
        try1.contains("+pub fn clamp(x: i32) -> i32 { x }"),
        "{try1}"
    );
    assert!(try2.contains("if x > 10"), "{try2}");

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

/// An exit-code check (a formatter, a linter) fails the first try; the retry must be told
/// what the command printed, or it could not know what to fix.
#[test]
fn an_exit_check_sends_its_output_back_to_the_agent() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    fs::create_dir_all(p.join(".conductor/workflows")).unwrap();
    fs::write(
        p.join(".gitignore"),
        "/.conductor/runs/\n/.test-conductor-home/\n",
    )
    .unwrap();
    let agent = r#"mkdir -p notes; if echo "$CONDUCTOR_PROMPT" | grep -q 'expected ok, found no'; then echo ok > notes/out.md; else echo no > notes/out.md; fi"#;
    let check =
        r#"grep -qx ok notes/out.md || { echo "expected ok, found $(cat notes/out.md)"; exit 1; }"#;
    fs::write(
        p.join(".conductor/workflows/build.yaml"),
        format!(
            r#"id: fmt
version: 1
kind: build
defaults:
  retries: {{ max: 1, ladder: [in_context] }}
stages:
  - id: write
    agent: {{ kind: script, command: ["sh", "-c", {agent:?}] }}
    scope: {{ write: ["notes/**"] }}
    gates:
      - {{ type: command_assert, command: ["sh", "-c", {check:?}], parser: exit }}
"#
        ),
    )
    .unwrap();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "base"]);
    let out = run(options(p)).expect("run completes");
    assert_eq!(out.verdict, Verdict::Passed, "{:#?}", out.receipt);
    assert!(
        out.receipt.checks[0].claim.contains("succeeds (write)"),
        "{:?}",
        out.receipt.checks
    );
    let events = out.dir.read_events().unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.what == "attempt 2: in-context retry with script")
    );
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

/// A one-stage workflow whose agent writes `notes/out.md`, with `setup` and `gates` as given.
fn with_setup(setup: &str, gates: &str) -> (tempfile::TempDir, conductor_engine::Outcome) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    fs::create_dir_all(p.join(".conductor/workflows")).unwrap();
    fs::write(
        p.join(".gitignore"),
        "/.conductor/runs/\n/.test-conductor-home/\n/deps/\n/reports/\n",
    )
    .unwrap();
    fs::write(
        p.join(".conductor/workflows/build.yaml"),
        format!(
            r#"id: setup
version: 1
kind: build
setup: {setup}
defaults:
  retries: {{ max: 0, ladder: [] }}
stages:
  - id: write
    agent: {{ kind: script, command: ["sh", "-c", "mkdir -p notes && echo hi > notes/out.md"] }}
    scope: {{ write: ["notes/**"] }}
    gates:
{gates}
"#
        ),
    )
    .unwrap();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "base"]);
    let out = run(options(p)).expect("run completes");
    (d, out)
}

fn said(out: &conductor_engine::Outcome, what: &str) -> bool {
    out.dir
        .read_events()
        .unwrap()
        .iter()
        .any(|e| e.what.contains(what))
}

#[test]
fn setup_prepares_the_checkout_once_for_the_checks() {
    let (_d, out) = with_setup(
        r#"[["sh", "-c", "mkdir -p deps && echo 1 > deps/installed"]]"#,
        r#"      - { type: command_assert, command: ["test", "-f", "deps/installed"], parser: exit }"#,
    );
    assert_eq!(out.verdict, Verdict::Passed, "{:#?}", out.receipt);
    assert!(said(&out, "setup `sh -c mkdir -p deps"));
}

#[test]
fn a_failed_setup_halts_the_run_before_any_agent() {
    let (_d, out) = with_setup(
        r#"[["sh", "-c", "echo registry unreachable >&2; exit 3"]]"#,
        "      - { type: scope }",
    );
    assert_eq!(out.verdict, Verdict::Unwitnessed);
    assert!(said(&out, "halted: setup `sh -c"));
    assert!(said(&out, "exited 3: registry unreachable"));
    assert!(!said(&out, "attempt 1"));
}

#[test]
fn setup_output_git_would_see_halts_the_run() {
    let (_d, out) = with_setup(r#"[["touch", "stray.txt"]]"#, "      - { type: scope }");
    assert_eq!(out.verdict, Verdict::Unwitnessed);
    assert!(said(
        &out,
        "setup left files git doesn't ignore (stray.txt)"
    ));
}

/// A report left in place from before (here by setup) is never read as the check's: the
/// check's command wrote nothing, so no tests ran.
#[test]
fn a_junit_report_is_always_fresh() {
    let stale = r#"<testsuite name="s"><testcase classname="s" name="ok"/></testsuite>"#;
    let (_d, out) = with_setup(
        &format!(
            "[[\"sh\", \"-c\", {:?}]]",
            format!("mkdir -p reports && echo '{stale}' > reports/TEST-s.xml")
        ),
        r#"      - type: command_assert
        command: ["sh", "-c", "exit 1"]
        parser: junit_xml
        report: "reports/*.xml"
        assert: ["tests_run > 0"]"#,
    );
    assert_eq!(out.verdict, Verdict::Failed, "{:#?}", out.receipt);
    assert!(said(&out, "the tests did not load or build (1 errors)"));

    let fresh = r#"<testsuite name='s'><testcase classname='s' name='ok'/></testsuite>"#;
    let (_d, out) = with_setup(
        "[]",
        &format!(
            r#"      - type: command_assert
        command: ["sh", "-c", {:?}]
        parser: junit_xml
        report: "reports/*.xml"
        assert: ["tests_run == 1", "tests_failed == 0"]"#,
            format!("mkdir -p reports && echo \"{fresh}\" > reports/TEST-s.xml")
        ),
    );
    assert_eq!(out.verdict, Verdict::Passed, "{:#?}", out.receipt);
    assert!(said(&out, "[report reports/TEST-s.xml sha256:"));
}
