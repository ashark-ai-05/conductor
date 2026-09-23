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
