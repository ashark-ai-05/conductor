//! Reads `cargo test --message-format json` output.
//!
//! That flag makes cargo's own messages JSON (one object per line), while the test harness
//! still prints its familiar text. Stable Rust has no JSON test output, so both are read here:
//! the JSON lines say whether the build succeeded, and the text says what each test did and,
//! for a failure, why it panicked.

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    Ignored,
}

/// Why a test failed. This is what "red for the right reason" is decided from (SPEC §9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// An assertion about the code's behaviour did not hold.
    Assertion,
    /// The code under test is a `todo!()` or `unimplemented!()` stub.
    Unimplemented,
    /// A test that fails whatever the code does: `assert!(false)`, a bare `panic!()`.
    Trivial,
    /// Any other panic.
    Panic,
}

impl FailureKind {
    pub fn name(self) -> &'static str {
        match self {
            FailureKind::Assertion => "assertion",
            FailureKind::Unimplemented => "unimplemented",
            FailureKind::Trivial => "trivial",
            FailureKind::Panic => "panic",
        }
    }

    fn classify(message: &str) -> Self {
        let m = message.trim();
        if m.is_empty() || m == "assertion failed: false" || m == "explicit panic" {
            FailureKind::Trivial
        } else if m.starts_with("not yet implemented") || m.starts_with("not implemented") {
            FailureKind::Unimplemented
        } else if m.starts_with("assertion") {
            FailureKind::Assertion
        } else {
            FailureKind::Panic
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestCase {
    pub name: String,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TestReport {
    /// `None` when cargo never said whether the build finished.
    pub compiled: Option<bool>,
    pub compile_errors: usize,
    pub tests: Vec<TestCase>,
}

impl TestReport {
    pub fn count(&self, outcome: Outcome) -> usize {
        self.tests.iter().filter(|t| t.outcome == outcome).count()
    }

    pub fn tests_run(&self) -> usize {
        self.count(Outcome::Passed) + self.count(Outcome::Failed)
    }
}

pub fn parse(stdout: &str) -> TestReport {
    let mut report = TestReport::default();
    let mut order: Vec<String> = Vec::new();
    let mut outcomes: BTreeMap<String, Outcome> = BTreeMap::new();
    let mut messages: BTreeMap<String, String> = BTreeMap::new();

    let lines: Vec<&str> = stdout.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                match v.get("reason").and_then(|r| r.as_str()) {
                    Some("build-finished") => {
                        report.compiled = v.get("success").and_then(|s| s.as_bool())
                    }
                    Some("compiler-message")
                        if v.pointer("/message/level").and_then(|l| l.as_str())
                            == Some("error") =>
                    {
                        report.compile_errors += 1;
                    }
                    _ => {}
                }
            }
        } else if let Some(rest) = line.strip_prefix("test ")
            && let Some((name, result)) = rest.rsplit_once(" ... ")
        {
            let outcome = match result.trim() {
                "ok" => Some(Outcome::Passed),
                "FAILED" => Some(Outcome::Failed),
                r if r.starts_with("ignored") => Some(Outcome::Ignored),
                _ => None,
            };
            if let Some(o) = outcome {
                let name = name.trim().to_owned();
                if outcomes.insert(name.clone(), o).is_none() {
                    order.push(name);
                }
            }
        } else if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|l| l.strip_suffix(" stdout ----"))
            && let Some(msg) = panic_message(&lines[i + 1..])
        {
            messages.insert(name.to_owned(), msg);
        }
        i += 1;
    }

    report.tests = order
        .into_iter()
        .map(|name| {
            let outcome = outcomes[&name];
            let message = messages.get(&name).cloned();
            let failure = (outcome == Outcome::Failed)
                .then(|| FailureKind::classify(message.as_deref().unwrap_or("")));
            TestCase {
                name,
                outcome,
                failure,
                message,
            }
        })
        .collect();
    report
}

/// The panic message in one failed test's captured output.
///
/// Recent Rust prints `thread 'name' (id) panicked at file:line:col:` and the message on the
/// following lines; older Rust printed `panicked at 'message', file`. Both are read.
fn panic_message(section: &[&str]) -> Option<String> {
    for (j, line) in section.iter().enumerate() {
        if line.starts_with("---- ") || *line == "failures:" {
            return None;
        }
        let Some(at) = line.find("panicked at ") else {
            continue;
        };
        let after = &line[at + "panicked at ".len()..];
        if let Some(quoted) = after.strip_prefix('\'') {
            return quoted.rsplit_once("',").map(|(m, _)| m.to_owned());
        }
        let mut msg = Vec::new();
        for next in &section[j + 1..] {
            if next.starts_with("stack backtrace:")
                || next.starts_with("note: ")
                || next.starts_with("---- ")
                || next.is_empty()
            {
                break;
            }
            msg.push(*next);
        }
        return Some(msg.join("\n"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: &str = include_str!("../tests/fixtures/cargo-test-red.out");
    const BROKEN: &str = include_str!("../tests/fixtures/cargo-test-compile-error.out");

    fn case<'a>(r: &'a TestReport, name: &str) -> &'a TestCase {
        r.tests
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test {name}"))
    }

    #[test]
    fn a_real_red_run_is_read_test_by_test() {
        let r = parse(RED);
        assert_eq!(r.compiled, Some(true));
        assert_eq!(r.compile_errors, 0);
        assert_eq!(r.count(Outcome::Passed), 2);
        assert_eq!(r.count(Outcome::Failed), 4);
        assert_eq!(r.count(Outcome::Ignored), 1);
        assert_eq!(r.tests_run(), 6);
    }

    #[test]
    fn each_failure_is_classified_by_why_it_panicked() {
        let r = parse(RED);
        assert_eq!(
            case(&r, "tests::eq_fails").failure,
            Some(FailureKind::Assertion)
        );
        assert_eq!(
            case(&r, "tests::unimpl").failure,
            Some(FailureKind::Unimplemented)
        );
        assert_eq!(
            case(&r, "tests::trivial").failure,
            Some(FailureKind::Trivial)
        );
        assert_eq!(case(&r, "tests::boom").failure, Some(FailureKind::Panic));
        assert_eq!(case(&r, "tests::passes").failure, None);
    }

    #[test]
    fn the_message_stops_before_the_backtrace() {
        let r = parse(RED);
        let msg = case(&r, "tests::eq_fails").message.as_deref().unwrap();
        assert!(msg.starts_with("assertion `left == right` failed"), "{msg}");
        assert!(!msg.contains("stack backtrace"), "{msg}");
    }

    #[test]
    fn a_build_that_failed_reports_no_tests_and_its_errors() {
        let r = parse(BROKEN);
        assert_eq!(r.compiled, Some(false));
        assert!(r.compile_errors >= 1);
        assert!(r.tests.is_empty());
    }

    #[test]
    fn the_old_panic_format_is_read_too() {
        let out = "test t ... FAILED\n\nfailures:\n\n---- t stdout ----\nthread 't' panicked at 'assertion failed: x > 1', src/lib.rs:3:5\n";
        let r = parse(out);
        assert_eq!(r.tests[0].failure, Some(FailureKind::Assertion));
    }

    #[test]
    fn output_with_no_build_messages_leaves_compiled_unknown() {
        assert_eq!(parse("test a ... ok\n").compiled, None);
    }
}
