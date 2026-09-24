//! The `init` workflow for other languages, run for real with scripted agents: the same
//! red-then-green rules as for Rust, read from each test runner's JUnit XML.
//!
//! Each test needs its language's toolchain. Without it the test says so and returns, unless
//! `CONDUCTOR_LANG_TESTS=1` (as in CI), where a missing tool is a failure.

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

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// Whether `program args` runs; if not, whether the test should be skipped.
fn have(program: &str, args: &[&str]) -> bool {
    let ok = Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        assert!(
            std::env::var("CONDUCTOR_LANG_TESTS").as_deref() != Ok("1"),
            "`{program}` is needed for this test"
        );
        eprintln!("skipped: `{program} {}` is not available", args.join(" "));
    }
    ok
}

/// A repository with `files`, set up by `conductor init`, with scripted agents in place of
/// Claude; then a headless run of it.
fn run(files: &[(&str, &str)], tests: &str, implement: &str) -> (tempfile::TempDir, Output) {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "t"]);
    for (path, body) in files {
        let p = dir.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    let init = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .arg("init")
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(init.status.success(), "{}", text(&init));
    let wf = dir.join(".conductor/workflows/build.yaml");
    let mut yaml = fs::read_to_string(&wf).unwrap();
    for (stage, script) in [("tests", tests), ("implement", implement)] {
        let prompt = format!("    prompt_file: .conductor/prompts/{stage}.md\n");
        let at = yaml.find(&prompt).unwrap();
        let from = yaml[..at].rfind("    agent:\n").unwrap();
        yaml.replace_range(
            from..at + prompt.len(),
            &format!("    agent: {{ kind: script, command: [\"sh\", \"-c\", {script:?}] }}\n"),
        );
    }
    fs::write(&wf, yaml).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "init"]);
    let out = Command::new(env!("CARGO_BIN_EXE_conductor"))
        .args([
            "run",
            ".conductor/workflows/build.yaml",
            "--spec",
            ".conductor/task.md",
            "--executor",
            "headless",
        ])
        .current_dir(dir)
        .env("CONDUCTOR_HOME", dir.join(".home"))
        .env_remove("CONDUCTOR_OTLP_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap();
    (d, out)
}

fn passes(out: &Output) {
    let t = text(out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("PASSED"), "{t}");
}

const PY: &[(&str, &str)] = &[
    (
        "pyproject.toml",
        "[project]\nname = \"calc\"\nversion = \"0.1.0\"\n",
    ),
    ("calc/__init__.py", ""),
];
const PY_TESTS: &str = "mkdir -p tests && printf 'from calc import double\\n\\ndef test_twice():\\n    assert double(2) == 4\\n\\ndef test_negative():\\n    assert double(-3) == -6\\n' > tests/test_double.py";

#[test]
fn python_an_honest_run_passes() {
    if !have("python3", &["-m", "pytest", "--version"]) {
        return;
    }
    let stub = format!(
        "{PY_TESTS} && printf 'def double(x):\\n    raise NotImplementedError\\n' > calc/__init__.py"
    );
    let implement = "printf 'def double(x):\\n    return x * 2\\n' > calc/__init__.py";
    let (_d, out) = run(PY, &stub, implement);
    passes(&out);
}

#[test]
fn python_a_tests_agent_that_implements_the_work_is_stopped() {
    if !have("python3", &["-m", "pytest", "--version"]) {
        return;
    }
    let cheat =
        format!("{PY_TESTS} && printf 'def double(x):\\n    return x * 2\\n' > calc/__init__.py");
    let (_d, out) = run(PY, &cheat, "true");
    let t = text(&out);
    assert!(!out.status.success(), "{t}");
    assert!(t.contains("did not hold: tests_failed == tests_new"), "{t}");
}

#[test]
fn python_tests_that_cannot_import_are_sent_back_as_a_build_failure() {
    if !have("python3", &["-m", "pytest", "--version"]) {
        return;
    }
    // No stub: the import fails, so no test ran, whatever the counts say.
    let (_d, out) = run(PY, PY_TESTS, "true");
    let t = text(&out);
    assert!(!out.status.success(), "{t}");
    assert!(t.contains("compiled == true"), "{t}");
}

#[test]
fn go_an_honest_run_passes() {
    if !have("go", &["version"]) || !have("gotestsum", &["--version"]) {
        return;
    }
    let files = &[
        ("go.mod", "module example.com/calc\n\ngo 1.21\n"),
        ("calc.go", "package calc\n"),
    ];
    let tests = "printf 'package calc\\n\\nimport \"testing\"\\n\\nfunc TestTwice(t *testing.T) {\\n\\tif Double(2) != 4 {\\n\\t\\tt.Errorf(\"got %%d\", Double(2))\\n\\t}\\n}\\n\\nfunc TestNegative(t *testing.T) {\\n\\tif Double(-3) != -6 {\\n\\t\\tt.Errorf(\"got %%d\", Double(-3))\\n\\t}\\n}\\n' > calc_test.go && printf 'package calc\\n\\nfunc Double(x int) int { return 0 }\\n' > calc.go";
    let implement =
        "printf 'package calc\\n\\nfunc Double(x int) int { return x * 2 }\\n' > calc.go";
    let (_d, out) = run(files, tests, implement);
    passes(&out);
}

#[test]
fn javascript_an_honest_run_passes() {
    if !have("npm", &["--version"]) {
        return;
    }
    let files = &[
        (
            "package.json",
            "{\"name\":\"calc\",\"type\":\"module\",\"devDependencies\":{\"vitest\":\"^3\"}}\n",
        ),
        (".gitignore", "node_modules/\n"),
        ("src/index.js", ""),
    ];
    let tests = "printf \"import { test, expect } from 'vitest';\\nimport { double } from './src/double.js';\\ntest('twice', () => { expect(double(2)).toBe(4); });\\ntest('negative', () => { expect(double(-3)).toBe(-6); });\\n\" > double.test.js && printf 'export function double(x) { throw new Error(\"not implemented\"); }\\n' > src/double.js";
    let implement = "printf 'export function double(x) { return x * 2; }\\n' > src/double.js";
    let (_d, out) = run(files, tests, implement);
    passes(&out);
}
