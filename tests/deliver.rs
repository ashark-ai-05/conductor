//! Delivery end to end: a passed run is pushed with its record and a draft pull request is
//! opened (against a fake GitHub API), `check-pr` accepts the branch in a fresh clone, and
//! rejects it once a commit lands after the receipt. No network.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Output};

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// Answers one request like GitHub's "create a pull request", and returns what it was sent.
fn fake_github() -> (String, std::thread::JoinHandle<(String, String, String)>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let (mut len, mut auth) = (0, String::new());
        loop {
            let mut h = String::new();
            r.read_line(&mut h).unwrap();
            if h.trim().is_empty() {
                break;
            }
            let lower = h.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                len = v.trim().parse().unwrap();
            }
            if lower.starts_with("authorization:") {
                auth = h.trim().to_owned();
            }
        }
        let mut body = vec![0; len];
        r.read_exact(&mut body).unwrap();
        let answer = r#"{"html_url":"https://github.com/o/r/pull/7","number":7}"#;
        write!(
            s,
            "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{answer}",
            answer.len()
        )
        .unwrap();
        (line, auth, String::from_utf8(body).unwrap())
    });
    (url, h)
}

fn repo_with_remote(agent: &str, deliver: bool) -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("work");
    let remote = d.path().join("remote.git");
    fs::create_dir_all(p.join(".conductor/workflows")).unwrap();
    Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&remote)
        .status()
        .unwrap();
    fs::write(p.join(".gitignore"), "/.conductor/runs/\n").unwrap();
    fs::write(
        p.join(".conductor/workflows/note.yaml"),
        format!(
            r#"id: note
version: 1
kind: build
stages:
  - id: write
    agent: {{ kind: script, command: ["sh", "-c", {agent:?}] }}
    scope: {{ write: ["notes/**"] }}
    outputs:
      - {{ id: out, path: notes/out.md }}
    gates:
      - {{ type: scope }}
      - {{ type: file_nonempty, ref: out }}
{}"#,
            if deliver {
                "deliver: { base: main }\n"
            } else {
                ""
            }
        ),
    )
    .unwrap();
    git(&p, &["init", "-q", "-b", "main"]);
    git(&p, &["config", "user.email", "t@example.com"]);
    git(&p, &["config", "user.name", "t"]);
    git(&p, &["add", "."]);
    git(&p, &["commit", "-q", "-m", "base"]);
    git(&p, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&p, &["push", "-q", "origin", "main"]);
    (d, p)
}

fn conductor(dir: &Path, args: &[&str], api: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args(args)
        .current_dir(dir)
        .env("CONDUCTOR_HOME", dir.join("../home"))
        .env("CONDUCTOR_GITHUB_API", api)
        .env("GITHUB_TOKEN", "test-token")
        .env_remove("GITHUB_EVENT_PATH")
        .env_remove("CONDUCTOR_OTLP_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap()
}

#[test]
fn a_passed_run_becomes_a_draft_pr_that_check_pr_accepts_until_it_changes() {
    let (d, p) = repo_with_remote("mkdir -p notes && echo hi > notes/out.md", true);
    let (api, got) = fake_github();
    let out = conductor(
        &p,
        &[
            "run",
            ".conductor/workflows/note.yaml",
            "-m",
            "Write a note",
            "--executor",
            "headless",
        ],
        &api,
    );
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(
        t.contains("draft pr   https://github.com/o/r/pull/7"),
        "{t}"
    );

    let (request, auth, body) = got.join().unwrap();
    assert!(request.starts_with("POST /repos/"), "{request}");
    assert!(request.contains("/pulls "), "{request}");
    assert_eq!(auth, "Authorization: Bearer test-token");
    let pr: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(pr["draft"], true);
    assert_eq!(pr["base"], "main");
    assert_eq!(pr["title"], "Write a note");
    let head = pr["head"].as_str().unwrap().to_owned();
    assert!(head.starts_with("conductor/"), "{head}");
    assert!(pr["body"].as_str().unwrap().starts_with("## ✓ PASSED"));

    // A reviewer's CI: a fresh clone of the pushed branch.
    let clone = d.path().join("clone");
    Command::new("git")
        .args(["clone", "-q", "--branch", &head])
        .arg(d.path().join("remote.git"))
        .arg(&clone)
        .status()
        .unwrap();
    let check = conductor(&clone, &["check-pr"], &api);
    let t = text(&check);
    assert!(check.status.success(), "{t}");
    for want in [
        "✓  the branch ends at run",
        "✓  the receipt commit adds only the run's record",
        "✓  the event chain is intact",
        "✓  the anchor's trailer matches the chain",
        "✓  all 1 stage commits are in this branch",
        "✓  the receipt's verdict, recomputed from its rows: passed",
    ] {
        assert!(t.contains(want), "missing {want:?}\n{t}");
    }

    // Someone pushes a change after the receipt: it no longer describes the branch.
    fs::write(clone.join("notes/out.md"), "changed").unwrap();
    git(
        &clone,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@e",
            "commit",
            "-qam",
            "tweak",
        ],
    );
    let check = conductor(&clone, &["check-pr"], &api);
    let t = text(&check);
    assert!(!check.status.success(), "{t}");
    assert!(t.contains("✗  1 commit(s) after run"), "{t}");
}

#[test]
fn a_failed_run_is_not_delivered() {
    let (_d, p) = repo_with_remote("echo nothing", false);
    let out = conductor(
        &p,
        &[
            "run",
            ".conductor/workflows/note.yaml",
            "-m",
            "note",
            "--executor",
            "headless",
        ],
        "http://127.0.0.1:1",
    );
    assert!(!out.status.success());
    let d = conductor(&p, &["deliver"], "http://127.0.0.1:1");
    let t = text(&d);
    assert!(!d.status.success(), "{t}");
    assert!(t.contains("only a passed run is delivered"), "{t}");
}

#[test]
fn check_pr_says_so_when_a_branch_has_no_receipt() {
    let (_d, p) = repo_with_remote("true", false);
    let out = conductor(&p, &["check-pr"], "http://127.0.0.1:1");
    assert!(!out.status.success());
    assert!(text(&out).contains("no conductor receipt on this branch"));
}
