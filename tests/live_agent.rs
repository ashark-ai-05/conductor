//! What a Claude agent does is on the record while it works, not after: a stand-in claude
//! prints half its transcript, pauses, then the rest, and the events appear in between.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn an_agents_actions_and_refusals_are_recorded_as_they_happen() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/conductor-engine/tests/fixtures/claude-stream-refused.jsonl");
    let fake = repo.join("fake-claude");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nhead -n 2 {f}\nsleep 3\ntail -n +3 {f}\n",
            f = fixture.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(repo.join(".conductor/workflows")).unwrap();
    std::fs::write(
        repo.join(".gitignore"),
        "/.conductor/runs/\n/.home/\n/fake-claude\n",
    )
    .unwrap();
    std::fs::write(
        repo.join(".conductor/workflows/build.yaml"),
        r#"id: watched
version: 1
kind: build
defaults:
  retries: { max: 0, ladder: [] }
stages:
  - id: fix
    agent: { kind: claude, allowed_tools: ["Bash(mvn:*)"] }
    scope: { write: ["src/**"] }
    gates:
      - { type: command_assert, command: ["true"], parser: exit }
"#,
    )
    .unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.email", "t@example.com"]);
    git(repo, &["config", "user.name", "t"]);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "base"]);

    let mut run = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args([
            "run",
            ".conductor/workflows/build.yaml",
            "-m",
            "fix it",
            "--executor",
            "headless",
        ])
        .current_dir(repo)
        .env("CONDUCTOR_HOME", repo.join(".home"))
        .env("CONDUCTOR_CLAUDE_BIN", &fake)
        .env_remove("CONDUCTOR_OTLP_ENDPOINT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // During the pause, the first tool call is already on the record and the run is
    // still going.
    let started = Instant::now();
    let seen_early = loop {
        let ids = conductor_engine::list_runs_any(repo);
        if let Some(id) = ids.first() {
            let dir = conductor_engine::store::RunDir::for_run(repo, id);
            let events = dir.read_events().unwrap_or_default();
            if events.iter().any(|e| e.what.starts_with("Read ")) {
                break events.len();
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "nothing observed live"
        );
        assert!(
            run.try_wait().unwrap().is_none(),
            "the run ended before anything was observed"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        run.try_wait().unwrap().is_none(),
        "the run had already ended"
    );

    let out = run.wait_with_output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    let id = conductor_engine::list_runs_any(repo).remove(0);
    let events = conductor_engine::store::RunDir::for_run(repo, &id)
        .read_events()
        .unwrap();
    assert!(events.len() > seen_early);
    let observed: Vec<&str> = events
        .iter()
        .filter(|e| e.source == conductor_model::Source::Observed)
        .map(|e| e.what.as_str())
        .collect();
    // Three tool calls and one refusal, each once: nothing recorded twice at the end.
    assert_eq!(observed.len(), 4, "{observed:?}");
    assert!(
        observed
            .iter()
            .any(|w| w.starts_with("refused: Bash mvn -v && ls /opt/homebrew/opt (")),
        "{observed:?}"
    );
    assert!(text.contains("1 tool call(s) refused"), "{text}");
}
