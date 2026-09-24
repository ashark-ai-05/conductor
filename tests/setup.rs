//! `conductor init` and `conductor doctor` on throwaway repositories.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

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

fn conductor(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args(args)
        .current_dir(dir)
        .env_remove("CONDUCTOR_OTLP_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn repo(rust: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q", "-b", "main"]);
    git(d.path(), &["config", "user.email", "t@example.com"]);
    git(d.path(), &["config", "user.name", "t"]);
    if rust {
        fs::write(
            d.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
    }
    d
}

#[test]
fn init_sets_up_a_rust_repository_once_and_keeps_what_is_there() {
    let d = repo(true);
    fs::write(d.path().join(".gitignore"), "/target").unwrap();
    let o = conductor(d.path(), &["init"]);
    assert!(o.status.success(), "{}", text(&o));
    for f in [
        ".conductor/workflows/build.yaml",
        ".conductor/prompts/tests.md",
        ".conductor/prompts/implement.md",
        ".conductor/policy.yaml",
        ".conductor/task.md",
    ] {
        assert!(d.path().join(f).is_file(), "{f}");
    }
    assert_eq!(
        fs::read_to_string(d.path().join(".gitignore")).unwrap(),
        "/target\n/.conductor/runs/\n"
    );
    let v = conductor(d.path(), &["validate", ".conductor/workflows/build.yaml"]);
    assert!(v.status.success(), "{}", text(&v));

    // A second init changes nothing.
    fs::write(d.path().join(".conductor/task.md"), "mine").unwrap();
    let again = conductor(d.path(), &["init"]);
    assert!(text(&again).contains("kept     .conductor/task.md"));
    assert_eq!(
        fs::read_to_string(d.path().join(".conductor/task.md")).unwrap(),
        "mine"
    );
    assert_eq!(
        fs::read_to_string(d.path().join(".gitignore"))
            .unwrap()
            .matches(".conductor/runs")
            .count(),
        1
    );
}

#[test]
fn init_says_plainly_when_a_project_is_not_rust() {
    let d = repo(false);
    let o = conductor(d.path(), &["init"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("no Cargo.toml"), "{}", text(&o));
    assert!(!d.path().join(".conductor").exists());
}

#[test]
fn doctor_insists_workflows_are_committed() {
    let d = repo(true);
    fs::write(d.path().join("README"), "x").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "base"]);
    let o = conductor(d.path(), &["init"]);
    assert!(o.status.success());

    let before = conductor(d.path(), &["doctor"]);
    assert!(!before.status.success());
    assert!(
        text(&before).contains("none committed"),
        "{}",
        text(&before)
    );

    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "conductor"]);
    let after = text(&conductor(d.path(), &["doctor"]));
    assert!(
        after.contains(
            "✓  workflow               .conductor/workflows/build.yaml: `build`, 2 stages"
        ),
        "{after}"
    );
    assert!(after.contains("✓  run records"), "{after}");
    assert!(after.contains("!  telemetry"), "{after}");
}

/// The `init` workflow with scripted agents in place of Claude, and without the mutation
/// check (which needs cargo-mutants): the red-first rules themselves, run for real.
fn scripted_init(tests_script: &str) -> (tempfile::TempDir, Output) {
    let d = repo(true);
    fs::create_dir_all(d.path().join("src")).unwrap();
    fs::write(d.path().join("src/lib.rs"), "").unwrap();
    assert!(conductor(d.path(), &["init"]).status.success());
    let wf = d.path().join(".conductor/workflows/build.yaml");
    let text = fs::read_to_string(&wf).unwrap();
    let agent = |prompt: &str, script: &str| {
        (
            format!(
                "    agent:\n      kind: claude\n      allowed_tools: [\"Bash(cargo test:*)\", \"Bash(cargo build:*)\"]\n    prompt_file: .conductor/prompts/{prompt}.md\n"
            ),
            format!("    agent: {{ kind: script, command: [\"sh\", \"-c\", {script:?}] }}\n"),
        )
    };
    let implement = "echo 'pub fn double(x: i32) -> i32 { x * 2 }' > src/lib.rs";
    let mut text = text;
    for (from, to) in [agent("tests", tests_script), agent("implement", implement)] {
        assert!(text.contains(&from));
        text = text.replace(&from, &to);
    }
    let text: String = text
        .lines()
        .filter(|l| !l.contains("type: mutation"))
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(&wf, text).unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "init"]);
    let out = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args([
            "run",
            ".conductor/workflows/build.yaml",
            "--spec",
            ".conductor/task.md",
            "--executor",
            "headless",
        ])
        .current_dir(d.path())
        .env("CONDUCTOR_HOME", d.path().join(".home"))
        .output()
        .unwrap();
    (d, out)
}

const TESTS: &str = "mkdir -p tests && printf 'use demo::double;\\n#[test] fn twice() { assert_eq!(double(2), 4); }\\n#[test] fn negative() { assert_eq!(double(-3), -6); }\\n' > tests/double.rs";

#[test]
fn the_init_workflow_lets_tests_stub_new_code_and_passes_an_honest_run() {
    let stub = format!("{TESTS}; echo 'pub fn double(_x: i32) -> i32 {{ todo!() }}' > src/lib.rs");
    let (_d, out) = scripted_init(&stub);
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("PASSED"), "{t}");
}

#[test]
fn the_init_workflow_stops_a_tests_agent_that_implements_the_work() {
    let cheat = format!("{TESTS}; echo 'pub fn double(x: i32) -> i32 {{ x * 2 }}' > src/lib.rs");
    let (_d, out) = scripted_init(&cheat);
    let t = text(&out);
    assert!(!out.status.success(), "{t}");
    assert!(t.contains("did not hold: tests_failed == tests_new"), "{t}");
}
