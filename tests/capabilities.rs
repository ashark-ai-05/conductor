use std::{
    fs,
    process::{Command, Output},
};
fn run(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}
#[test]
fn registered_report_is_discoverable_refreshable_and_retained_after_removal() {
    let repo = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        repo.path().join("report.json"),
        r#"[{"test":"account","status":"failed"}]"#,
    )
    .unwrap();
    let add = run(
        repo.path(),
        &[
            "capability",
            "add",
            "custom.report",
            "report.json",
            "--label",
            "My report",
        ],
    );
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let listed = run(repo.path(), &["capability", "list"]);
    assert!(String::from_utf8_lossy(&listed.stdout).contains("My report"));
    let started = run(repo.path(), &["capability", "run", "custom.report"]);
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let id = String::from_utf8(started.stdout)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned();
    assert!(id.starts_with("n-"));
    assert!(
        run(repo.path(), &["capability", "refresh", &id])
            .status
            .success()
    );
    assert!(
        run(repo.path(), &["capability", "remove", "custom.report"])
            .status
            .success()
    );
    let saved = run(repo.path(), &["capability", "show", &id]);
    assert!(saved.status.success());
    let record: serde_json::Value = serde_json::from_slice(&saved.stdout).unwrap();
    assert_eq!(record["events"].as_array().unwrap().len(), 4);
    assert!(
        !run(repo.path(), &["capability", "refresh", &id])
            .status
            .success()
    );
    assert_eq!(
        saved.stdout,
        run(repo.path(), &["capability", "show", &id]).stdout
    );
    assert!(!repo.path().join(".conductor/questions").exists());
    assert!(!repo.path().join(".conductor/runs").exists());
}
