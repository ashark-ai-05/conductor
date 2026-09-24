//! A run's trace: rendered by `conductor trace`, exported over OTLP when an endpoint is set,
//! and named on the receipt. Uses a fake collector on localhost; no network, no herdr.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
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

/// Accepts `n` HTTP requests and returns each one's path and body.
fn collector(n: usize) -> (String, std::thread::JoinHandle<Vec<(String, String)>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let mut got = vec![];
        for _ in 0..n {
            let (mut s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap().to_owned();
            let mut len = 0;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                .unwrap();
            got.push((path, String::from_utf8(body).unwrap()));
        }
        got
    });
    (url, h)
}

#[test]
fn a_run_is_exported_and_its_trace_is_on_the_receipt() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    fs::create_dir_all(p.join(".conductor/workflows")).unwrap();
    fs::write(p.join(".gitignore"), "/.conductor/runs/\n").unwrap();
    fs::write(
        p.join(".conductor/workflows/note.yaml"),
        r#"id: note
version: 1
kind: build
stages:
  - id: write
    agent: { kind: script, command: ["sh", "-c", "mkdir -p notes && echo hi > notes/out.md"] }
    scope: { write: ["notes/**"] }
    outputs:
      - { id: out, path: notes/out.md }
    gates:
      - { type: scope }
      - { type: file_nonempty, ref: out }
"#,
    )
    .unwrap();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "base"]);

    let (url, got) = collector(3);
    let out = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args([
            "run",
            ".conductor/workflows/note.yaml",
            "-m",
            "write a note",
            "--executor",
            "headless",
        ])
        .current_dir(p)
        .env("CONDUCTOR_HOME", p.join(".home"))
        .env("CONDUCTOR_OTLP_ENDPOINT", &url)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let got = got.join().unwrap();
    assert_eq!(got[0].0, "/v1/traces");
    assert_eq!(got[1].0, "/v1/logs");
    assert_eq!(got[2].0, "/v1/metrics");
    assert!(got[2].1.contains("conductor.runs"));

    // The id printed, the id on the receipt and the id exported are the same.
    let exported = stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("exported  trace "))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or_else(|| panic!("{stdout}"))
        .to_owned();
    assert!(got[0].1.contains(&exported));
    assert!(
        stdout.contains(&format!("{exported} · `conductor trace")),
        "{stdout}"
    );
    assert!(got[0].1.contains("conductor.receipt.sha256"));
    assert!(got[0].1.contains("check file_nonempty"));

    // `conductor trace` renders the same run without any collector.
    let t = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .arg("trace")
        .current_dir(p)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&t.stdout);
    assert!(t.status.success(), "{text}");
    assert!(text.contains(&format!("trace {exported}")), "{text}");
    assert!(text.contains("✓ stage write"), "{text}");
    assert!(text.contains("✓ check scope"), "{text}");

    // `conductor stats` adds the repository's runs up, also without a collector.
    let st = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .arg("stats")
        .current_dir(p)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&st.stdout);
    assert!(st.status.success(), "{text}");
    assert!(text.contains("conductor stats · 1 run\n"), "{text}");
    assert!(
        text.contains("runs       1 passed · 0 did not · 100%"),
        "{text}"
    );
    assert!(text.contains("file_nonempty 1/1"), "{text}");
}

#[test]
fn a_dead_collector_warns_but_does_not_fail_the_run() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    fs::create_dir_all(p.join(".conductor/workflows")).unwrap();
    fs::write(p.join(".gitignore"), "/.conductor/runs/\n").unwrap();
    fs::write(
        p.join(".conductor/workflows/note.yaml"),
        r#"id: note
version: 1
kind: build
stages:
  - id: write
    agent: { kind: script, command: ["sh", "-c", "mkdir -p notes && echo hi > notes/out.md"] }
    scope: { write: ["notes/**"] }
    outputs:
      - { id: out, path: notes/out.md }
    gates:
      - { type: file_nonempty, ref: out }
"#,
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
            ".conductor/workflows/note.yaml",
            "-m",
            "note",
            "--executor",
            "headless",
        ])
        .current_dir(p)
        .env("CONDUCTOR_HOME", p.join(".home"))
        .env("CONDUCTOR_OTLP_ENDPOINT", "http://127.0.0.1:1")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("the OTLP export failed"));
}
