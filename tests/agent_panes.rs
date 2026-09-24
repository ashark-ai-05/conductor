//! An agent inside a run opens a pane beside its own through `conductor pane`, is refused
//! panes it doesn't own, and conductor records it all and cleans up. Needs a real herdr, so
//! runs only when `CONDUCTOR_HERDR_BIN` is set.

use std::fs;
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

/// The agent: open a pane, run something in it, read it back, try (and fail) to take over
/// its own stage pane, open one more by calling herdr directly (bypassing conductor), then
/// leave the side pane open for conductor to close.
const AGENT: &str = r#"set -e
P=$("$CONDUCTOR_BIN" pane split)
"$CONDUCTOR_BIN" pane run "$P" 'echo side-$((40+2))'
for i in 1 2 3 4 5 6 7 8 9 10; do
  "$CONDUCTOR_BIN" pane read "$P" | grep -q side-42 && break
  sleep 0.5
done
"$CONDUCTOR_BIN" pane read "$P" | grep -q side-42
if "$CONDUCTOR_BIN" pane close "$HERDR_PANE_ID" 2>/dev/null; then exit 3; fi
if "$CONDUCTOR_BIN" pane run "$HERDR_PANE_ID" true 2>/dev/null; then exit 4; fi
"$CONDUCTOR_HERDR_BIN" --session "$CONDUCTOR_HERDR_SESSION" pane split "$HERDR_PANE_ID" --direction down --no-focus >/dev/null
mkdir -p notes && echo "side pane $P" > notes/out.md
"#;

struct Server {
    bin: String,
    session: String,
    child: std::process::Child,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new(&self.bin)
            .args(["--session", &self.session, "server", "stop"])
            .output();
        let _ = self.child.wait();
    }
}

#[test]
fn an_agent_opens_a_pane_in_its_own_tab_and_nowhere_else() {
    let Ok(bin) = std::env::var("CONDUCTOR_HERDR_BIN") else {
        eprintln!("CONDUCTOR_HERDR_BIN not set; skipping");
        return;
    };
    let session = format!("conductor-panes-{}", std::process::id());
    let child = Command::new(&bin)
        .args(["--session", &session, "server"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _server = Server {
        bin: bin.clone(),
        session: session.clone(),
        child,
    };
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

    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    fs::create_dir_all(p.join(".conductor/workflows")).unwrap();
    fs::write(p.join(".gitignore"), "/.conductor/runs/\n").unwrap();
    fs::write(
        p.join(".conductor/workflows/side.yaml"),
        format!(
            r#"id: side-pane
version: 1
kind: build
stages:
  - id: work
    agent: {{ kind: script, command: ["sh", "-c", {AGENT:?}] }}
    scope: {{ write: ["notes/**"] }}
    outputs:
      - {{ id: out, path: notes/out.md }}
    gates:
      - {{ type: scope }}
      - {{ type: file_nonempty, ref: out }}
"#
        ),
    )
    .unwrap();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "base"]);

    let out = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args([
            "run",
            ".conductor/workflows/side.yaml",
            "-m",
            "open a side pane",
            "--executor",
            "herdr",
        ])
        .current_dir(p)
        .env("CONDUCTOR_HERDR_BIN", &bin)
        .env("CONDUCTOR_HERDR_SESSION", &session)
        .env("CONDUCTOR_HOME", p.join(".home"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let runs = p.join(".conductor/runs");
    let run = fs::read_dir(&runs).unwrap().next().unwrap().unwrap().path();
    let events: Vec<serde_json::Value> = fs::read_to_string(run.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let said = |source: &str, prefix: &str| {
        events
            .iter()
            .any(|e| e["source"] == source && e["what"].as_str().unwrap_or("").starts_with(prefix))
    };
    // The run's tab shows its live view beside the stage, so the stage's own pane can close.
    assert!(said("witnessed", "status pane "), "{stdout}");
    assert!(said("witnessed", "closed the stage's pane"), "{stdout}");
    let status = events
        .iter()
        .filter_map(|e| e["what"].as_str()?.strip_prefix("status pane "))
        .find_map(|rest| rest.split_whitespace().next())
        .unwrap()
        .to_owned();
    let mut screen = String::new();
    for _ in 0..40 {
        let o = Command::new(&bin)
            .args([
                "--session",
                &session,
                "pane",
                "read",
                &status,
                "--source",
                "visible",
            ])
            .output()
            .unwrap();
        screen = String::from_utf8_lossy(&o.stdout).into_owned();
        if screen.contains("the run passed") || screen.contains("every check passed") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    assert!(
        screen.contains("every check passed"),
        "the status pane shows the finished run:\n{screen}"
    );
    assert!(said("observed", "agent opened pane "), "{stdout}");
    assert!(
        said("observed", "agent ran `echo side-$((40+2))` in pane "),
        "{stdout}"
    );
    assert!(said("witnessed", "closed pane "), "{stdout}");
    // The refused attempts changed nothing, and are on the record.
    assert!(!said("observed", "agent closed pane"), "{stdout}");
    assert!(
        said("observed", "agent was refused close on pane "),
        "{stdout}"
    );
    assert!(
        said("observed", "agent was refused run on pane "),
        "{stdout}"
    );
    // The pane opened behind conductor's back is seen and put on the receipt.
    assert!(said("inferred", "pane w1:"), "{stdout}");
    let receipt: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(run.join("receipt.json")).unwrap()).unwrap();
    assert!(
        receipt["not_checked"].as_array().unwrap().iter().any(|n| n
            .as_str()
            .unwrap()
            .contains("appeared in the run's tab outside conductor")),
        "{receipt:#}"
    );
}

/// Headless, the same commands start background processes: the agent reads their output,
/// and conductor stops whatever is left running when the stage ends. No herdr needed.
const HEADLESS_AGENT: &str = r#"set -e
P=$("$CONDUCTOR_BIN" pane split)
"$CONDUCTOR_BIN" pane run "$P" 'echo side-$((40+2)); sleep 60'
for i in 1 2 3 4 5 6 7 8 9 10; do
  "$CONDUCTOR_BIN" pane read "$P" | grep -q side-42 && break
  sleep 0.3
done
"$CONDUCTOR_BIN" pane read "$P" | grep -q side-42
if "$CONDUCTOR_BIN" pane close local-9 2>/dev/null; then exit 3; fi
mkdir -p notes && echo "background pane $P" > notes/out.md
"#;

#[test]
fn headless_panes_are_background_processes_stopped_at_stage_end() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    fs::create_dir_all(p.join(".conductor/workflows")).unwrap();
    fs::write(p.join(".gitignore"), "/.conductor/runs/\n").unwrap();
    fs::write(
        p.join(".conductor/workflows/side.yaml"),
        format!(
            r#"id: side-pane
version: 1
kind: build
stages:
  - id: work
    agent: {{ kind: script, command: ["sh", "-c", {HEADLESS_AGENT:?}] }}
    scope: {{ write: ["notes/**"] }}
    outputs:
      - {{ id: out, path: notes/out.md }}
    gates:
      - {{ type: scope }}
      - {{ type: file_nonempty, ref: out }}
"#
        ),
    )
    .unwrap();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "base"]);

    let started = std::time::Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args([
            "run",
            ".conductor/workflows/side.yaml",
            "-m",
            "open a side pane",
            "--executor",
            "headless",
        ])
        .current_dir(p)
        .env("CONDUCTOR_HOME", p.join(".home"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the background `sleep 60` must not hold the run open"
    );

    let run = fs::read_dir(p.join(".conductor/runs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let events = fs::read_to_string(run.join("events.jsonl")).unwrap();
    for want in [
        "agent opened pane local-1",
        "agent ran `echo side-$((40+2)); sleep 60` in pane local-1",
        "agent was refused close on pane local-9",
        "stopped background pane local-1 the agent left running",
    ] {
        assert!(events.contains(want), "missing {want:?}\n{stdout}");
    }
    let pgid = fs::read_to_string(run.join("panes/local-1.pids")).unwrap();
    // Any member of the group that is not a zombie still running?
    let ps = Command::new("ps")
        .args(["-eo", "pgid=,stat="])
        .output()
        .unwrap();
    let alive = String::from_utf8_lossy(&ps.stdout).lines().any(|l| {
        let mut f = l.split_whitespace();
        f.next() == Some(pgid.trim()) && !f.next().unwrap_or("Z").starts_with('Z')
    });
    assert!(!alive, "the background process group was stopped");
}
