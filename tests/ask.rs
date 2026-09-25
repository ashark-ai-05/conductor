//! A tool the agent asks for is answered by a person: a stand-in claude speaks MCP to the
//! real `conductor ask` server, the question shows on the live view the moment it is
//! asked, the stage clock stops, `conductor allow` answers it, and every step is on the
//! record.

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

/// A claude that asks once, the way the real one does: it starts the MCP server named in
/// `--mcp-config`, calls its tool, and reports the outcome in its stream.
const FAKE_CLAUDE: &str = r#"#!/usr/bin/env python3
import json, subprocess, sys
argv = sys.argv
assert "--permission-prompt-tool" in argv and argv[argv.index("--permission-prompt-tool") + 1] == "mcp__conductor__ask", argv
cfg = json.loads(argv[argv.index("--mcp-config") + 1])["mcpServers"]["conductor"]
srv = subprocess.Popen([cfg["command"], *cfg["args"]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
def call(o):
    srv.stdin.write(json.dumps(o) + "\n"); srv.stdin.flush()
    return json.loads(srv.stdout.readline())
call({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-11-25"}})
print(json.dumps({"type": "system", "subtype": "init", "model": "claude-sonnet-5", "session_id": "s1"}), flush=True)
print(json.dumps({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "mvn -v"}}]}}), flush=True)
r = call({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "ask", "arguments": {"tool_name": "Bash", "input": {"command": "mvn -v"}, "tool_use_id": "t1"}}})
verdict = json.loads(r["result"]["content"][0]["text"])
if verdict["behavior"] == "allow":
    result = {"type": "tool_result", "tool_use_id": "t1", "content": "Apache Maven 3.9.9"}
else:
    result = {"type": "tool_result", "tool_use_id": "t1", "is_error": True, "content": verdict["message"]}
print(json.dumps({"type": "user", "message": {"content": [result]}}), flush=True)
print(json.dumps({"type": "result", "subtype": "success", "is_error": False, "num_turns": 2, "result": "done", "session_id": "s1"}), flush=True)
srv.stdin.close(); srv.wait()
"#;

#[test]
fn a_tool_the_agent_asks_for_is_answered_from_outside_the_run() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path();
    let fake = repo.join("fake-claude");
    std::fs::write(&fake, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(repo.join(".conductor/workflows")).unwrap();
    std::fs::write(
        repo.join(".gitignore"),
        "/.conductor/runs/\n/.home/\n/fake-claude\n",
    )
    .unwrap();
    // A two-second stage: only a stopped clock lets it wait three seconds for an answer.
    std::fs::write(
        repo.join(".conductor/workflows/build.yaml"),
        r#"id: asking
version: 1
kind: build
defaults:
  retries: { max: 0, ladder: [] }
budget:
  max_stage_wall_clock_sec: 2
stages:
  - id: fix
    agent: { kind: claude, allowed_tools: ["Bash(cargo test:*)"] }
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
        .env("USER", "krunal")
        .env_remove("CONDUCTOR_OTLP_ENDPOINT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // The question reaches the live view while the agent waits.
    let started = Instant::now();
    let (id, waiting) = loop {
        if let Some(id) = conductor_engine::list_runs_any(repo).first()
            && let Some(l) = conductor_engine::live::read(repo, id)
            && let Some(w) = l.waiting
        {
            break (id.clone(), w);
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the run never waited on the ask"
        );
        if let Some(status) = run.try_wait().unwrap() {
            let out = run.wait_with_output().unwrap();
            panic!(
                "the run ended ({status}) before asking:\n{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(waiting.ask, Some(1));
    assert_eq!(waiting.who, "krunal");
    assert_eq!(waiting.question, "may the agent run `mvn -v`? (Bash)");
    let dir = conductor_engine::store::RunDir::for_run(repo, &id);
    let asked_at = dir
        .read_events()
        .unwrap()
        .iter()
        .position(|e| e.what == "asked: Bash `mvn -v`")
        .expect("the ask is on the record");

    // Longer than the stage's clock allows: the clock is stopped.
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        run.try_wait().unwrap().is_none(),
        "the run ended while waiting on the person"
    );
    let allow = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args(["allow", &id, "-m", "fine on this checkout"])
        .current_dir(repo)
        .env("USER", "krunal")
        .output()
        .unwrap();
    assert!(
        allow.status.success(),
        "{}",
        String::from_utf8_lossy(&allow.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&allow.stdout).trim(),
        "Bash `mvn -v`: allowed by krunal"
    );
    // A second answer is refused.
    let again = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args(["deny", &id])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(!again.status.success());

    let out = run.wait_with_output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    let events = dir.read_events().unwrap();
    let whats: Vec<(conductor_model::Source, &str)> = events[asked_at..]
        .iter()
        .map(|e| (e.source, e.what.as_str()))
        .collect();
    use conductor_model::Source::*;
    assert_eq!(whats[0], (Observed, "asked: Bash `mvn -v`"));
    assert_eq!(
        whats[1],
        (Human, "krunal allowed Bash `mvn -v`: fine on this checkout")
    );
    assert!(
        matches!(whats[2], (Witnessed, w) if w.starts_with("the agent waited 3s for krunal; the stage clock was stopped")),
        "{whats:?}"
    );
    // The call itself was seen before the ask, as Claude reports it.
    assert!(
        events[..asked_at]
            .iter()
            .any(|e| e.source == Observed && e.what == "Bash mvn -v"),
        "{events:?}"
    );
    assert!(
        !whats.iter().any(|(_, w)| w.starts_with("refused")),
        "{whats:?}"
    );
    assert!(
        text.contains("asked for 1 tool(s), 0 denied, and waited 3s"),
        "{text}"
    );
    assert!(
        conductor_engine::live::read(repo, &id)
            .unwrap()
            .waiting
            .is_none()
    );
}
