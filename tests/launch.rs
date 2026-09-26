//! The launch tab's way of starting a run: `conductor run` in the background, found by the
//! run directory that appears. A ticket path becomes `--spec`; anything else is the task.

use std::path::Path;
use std::process::Command;

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
fn a_run_started_from_the_launch_tab_is_found_by_its_id_and_takes_a_ticket_or_text() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path();
    std::fs::create_dir_all(repo.join(".conductor/workflows")).unwrap();
    std::fs::create_dir_all(repo.join("tickets")).unwrap();
    std::fs::write(repo.join(".gitignore"), "/.conductor/runs/\n/.home/\n").unwrap();
    std::fs::write(repo.join("tickets/T-3.md"), "# T-3: say hi\n\n- AC1: hi\n").unwrap();
    std::fs::write(
        repo.join(".conductor/workflows/quick.yaml"),
        r#"id: quick
version: 1
kind: build
defaults:
  retries: { max: 0, ladder: [] }
stages:
  - id: say
    agent: { kind: script, command: ["sh", "-c", "mkdir -p out && echo hi > out/hi.txt"] }
    scope: { write: ["out/**"] }
    gates:
      - { type: command_assert, command: ["grep", "-q", "hi", "out/hi.txt"], parser: exit }
"#,
    )
    .unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.email", "t@example.com"]);
    git(repo, &["config", "user.name", "t"]);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "base"]);

    assert_eq!(
        conductor_engine::launch::workflows(repo),
        vec![".conductor/workflows/quick.yaml"]
    );
    let bin = Path::new(env!("CARGO_BIN_EXE_conductor"));
    let id = conductor_engine::launch::start(
        repo,
        bin,
        ".conductor/workflows/quick.yaml",
        "tickets/T-3.md",
        false,
        &[("CONDUCTOR_HOME", &repo.join(".home").display().to_string())],
    )
    .unwrap();
    assert_eq!(id.len(), 11, "{id}");
    // The run goes on without us and passes.
    let started = std::time::Instant::now();
    let receipt = loop {
        let p = conductor_engine::store::RunDir::for_run(repo, &id).receipt();
        if p.is_file() {
            break std::fs::read_to_string(p).unwrap();
        }
        assert!(started.elapsed().as_secs() < 60, "no receipt for {id}");
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert!(receipt.contains("T-3: say hi"), "{receipt}");

    let id2 = conductor_engine::launch::start(
        repo,
        bin,
        ".conductor/workflows/quick.yaml",
        "just say hi to everyone",
        false,
        &[("CONDUCTOR_HOME", &repo.join(".home").display().to_string())],
    )
    .unwrap();
    assert_ne!(id2, id);
    let log = std::fs::read_to_string(repo.join(".conductor/runs/launch.log")).unwrap();
    assert!(log.contains("run started: quick"), "{log}");
    assert!(
        !repo.join(".conductor/launch.log").exists(),
        "the log must not dirty the repository"
    );
}
