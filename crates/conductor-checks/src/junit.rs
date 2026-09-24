//! Reads JUnit XML, the test report nearly every test runner can write: pytest
//! (`--junitxml`), jest and vitest (a junit reporter), go (gotestsum), Maven and Gradle, .NET,
//! PHPUnit, and cargo-nextest. One format makes conductor's test gates language-agnostic.
//!
//! The format has no field for why a test failed, so the kind is read from the failure's
//! message, the same judgement [`cargo_test`](crate::cargo_test) makes from a panic. And it
//! has no notion of a build: a suite that could not be collected or compiled shows up as an
//! `<error>` on a pseudo-test, or on the suite itself, and is counted as a compile error
//! rather than a test.

use crate::cargo_test::{FailureKind, Outcome, TestCase, TestReport};
use roxmltree::{Document, Node};

/// Messages that say the suite never got as far as running tests: an import, syntax or
/// build error. Matched case-insensitively against a failure's message, type and text.
const NOT_BUILT: &[&str] = &[
    "collection failure",
    "error collecting",
    "importerror",
    "modulenotfounderror",
    "syntaxerror",
    "test suite failed to run",
    "failed to load",
    "[build failed]",
    "compilation failed",
    "compilation error",
    "cannot find module",
];

/// Messages a stub gives when it is called: the "fails because the work isn't done yet"
/// that a new test should produce.
const UNIMPLEMENTED: &[&str] = &[
    "not implemented",
    "not yet implemented",
    "notimplemented",
    "unimplemented",
    "unsupportedoperationexception",
    "todo!",
];

/// Failures that hold whatever the code does, when they are the whole message. (A failure
/// that says nothing at all, like Go's `t.Fail()` or JUnit's `fail()`, is trivial too.)
/// pytest's `assert False` is only trivial alone: `assert is_valid(x)` failing reads
/// "assert False" followed by "where False = is_valid(x)".
const TRIVIAL: &[&str] = &[
    "assert false",
    "assert 0",
    "assertion failed: false",
    "explicit panic",
];

/// Parses one report. Several `<testsuite>`s, nested or under `<testsuites>`, are fine.
pub fn parse(xml: &str) -> Result<TestReport, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if !matches!(root.tag_name().name(), "testsuites" | "testsuite") {
        return Err(format!(
            "the root element is <{}>, not <testsuites> or <testsuite>",
            root.tag_name().name()
        ));
    }
    let mut report = TestReport::default();
    for node in root.descendants().filter(Node::is_element) {
        match node.tag_name().name() {
            "testcase" => read_case(node, &mut report),
            // A suite-level error: the suite itself could not run.
            "error" | "failure"
                if node
                    .parent_element()
                    .is_some_and(|p| p.tag_name().name() == "testsuite") =>
            {
                let suite = node.parent_element().and_then(|p| p.attribute("name"));
                not_built(&mut report, suite.unwrap_or("suite"), &Failed::read(node));
            }
            _ => {}
        }
    }
    report.compiled = Some(report.compile_errors == 0);
    Ok(report)
}

/// Adds `other`'s tests and errors to `into`, for a runner that writes one file per suite.
pub fn merge(into: &mut TestReport, other: TestReport) {
    into.compile_errors += other.compile_errors;
    for m in other.compile_messages {
        if into.compile_messages.len() < 5 {
            into.compile_messages.push(m);
        }
    }
    into.tests.extend(other.tests);
    into.compiled = Some(into.compile_errors == 0);
}

fn read_case(node: Node, report: &mut TestReport) {
    let name = node.attribute("name").unwrap_or("").trim();
    let class = node.attribute("classname").unwrap_or("").trim();
    // jest-junit repeats the title as the class; pytest leaves the class empty for a module.
    let id = if class.is_empty() || class == name {
        name.to_owned()
    } else if name.is_empty() {
        class.to_owned()
    } else {
        format!("{class}::{name}")
    };
    let child = |tag: &str| {
        node.children()
            .find(|c| c.is_element() && c.tag_name().name() == tag)
    };
    let (outcome, failure, message) = if let Some(f) = child("failure").or(child("error")) {
        let failed = Failed::read(f);
        // vitest reports a test file that failed to load as a test named after the file.
        if failed.says(NOT_BUILT) || (class == name && is_file(name)) {
            not_built(report, &id, &failed);
            return;
        }
        let kind = failed.kind(f.tag_name().name() == "error");
        (Outcome::Failed, Some(kind), Some(failed.headline))
    } else if child("skipped").is_some() {
        (Outcome::Ignored, None, None)
    } else {
        (Outcome::Passed, None, None)
    };
    report.tests.push(TestCase {
        name: id,
        outcome,
        failure,
        message: message.filter(|m| !m.is_empty()),
    });
}

fn is_file(name: &str) -> bool {
    name.contains('/')
        || [".js", ".mjs", ".cjs", ".jsx", ".ts", ".mts", ".tsx", ".py"]
            .iter()
            .any(|e| name.ends_with(e))
}

fn not_built(report: &mut TestReport, what: &str, f: &Failed) {
    report.compile_errors += 1;
    if report.compile_messages.len() < 5 {
        // pytest puts the real error on its `E` lines, under a generic "collection failure".
        let line = f
            .body
            .iter()
            .rev()
            .find_map(|l| l.strip_prefix("E ").map(str::trim))
            .or_else(|| f.headline.lines().next())
            .unwrap_or("");
        report.compile_messages.push(format!("{what}: {line}"));
    }
}

/// One `<failure>` or `<error>`, read the way its runner meant it.
struct Failed {
    /// What the failure says: its message, or when that is missing or generic ("Failed", as
    /// gotestsum writes), the first lines of its text that aren't the runner announcing the
    /// test (`=== RUN`, `--- FAIL`) or a stack frame.
    headline: String,
    /// The exception class, when the runner names one.
    kind: String,
    body: Vec<String>,
}

impl Failed {
    fn read(f: Node) -> Self {
        let message = f.attribute("message").unwrap_or("").trim();
        let kind = f.attribute("type").unwrap_or("").trim().to_owned();
        let body: Vec<String> = f
            .text()
            .unwrap_or("")
            .lines()
            .map(str::trim)
            .filter(|l| {
                !l.is_empty()
                    && !l.starts_with("=== ")
                    && !l.starts_with("--- FAIL")
                    && !l.starts_with("--- PASS")
                    && !l.starts_with("at ")
                    && *l != kind
            })
            .map(str::to_owned)
            .collect();
        let generic = matches!(
            message.to_ascii_lowercase().as_str(),
            "" | "failed" | "failure" | "error"
        );
        let headline = if generic {
            body.iter().take(3).cloned().collect::<Vec<_>>().join("\n")
        } else {
            message.to_owned()
        };
        Failed {
            headline,
            kind,
            body,
        }
    }

    fn says(&self, needles: &[&str]) -> bool {
        let all = format!("{}\n{}\n{}", self.headline, self.kind, self.body.join("\n"));
        says_any(&all, needles)
    }

    /// Why the test failed. `<failure>` is an assertion that didn't hold; `<error>` is an
    /// exception the test didn't expect.
    fn kind(&self, error: bool) -> FailureKind {
        let headline = self.headline.trim().to_ascii_lowercase();
        let headline = headline.trim_end_matches(['.', ':']);
        if says_any(&format!("{headline}\n{}", self.kind), UNIMPLEMENTED) {
            FailureKind::Unimplemented
        } else if headline.is_empty() || TRIVIAL.contains(&headline) {
            FailureKind::Trivial
        } else if error {
            FailureKind::Panic
        } else {
            FailureKind::Assertion
        }
    }
}

fn says_any(text: &str, needles: &[&str]) -> bool {
    let t = text.to_ascii_lowercase();
    needles.iter().any(|n| t.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> TestReport {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        parse(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Each test's outcome and failure kind, by name.
    fn kinds(r: &TestReport) -> Vec<(String, &'static str)> {
        let mut v: Vec<_> = r
            .tests
            .iter()
            .map(|t| {
                let k = match (t.outcome, t.failure) {
                    (Outcome::Passed, _) => "passed",
                    (Outcome::Ignored, _) => "ignored",
                    (Outcome::Failed, Some(k)) => k.name(),
                    (Outcome::Failed, None) => "failed",
                };
                (t.name.clone(), k)
            })
            .collect();
        v.sort();
        v
    }

    fn expect(pairs: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
        let mut v: Vec<_> = pairs.iter().map(|(n, k)| ((*n).to_owned(), *k)).collect();
        v.sort();
        v
    }

    #[test]
    fn pytest() {
        let r = fixture("pytest-junit.xml");
        assert_eq!(r.compiled, Some(true));
        assert_eq!(
            kinds(&r),
            expect(&[
                ("tests.test_clamp::test_in_range", "passed"),
                ("tests.test_clamp::test_caps_high", "assertion"),
                ("tests.test_clamp::test_new_api", "unimplemented"),
                ("tests.test_clamp::test_fake", "trivial"),
                ("tests.test_clamp::test_later", "ignored"),
            ])
        );
        assert_eq!(r.tests_run(), 4);
    }

    #[test]
    fn pytest_collection_error_is_a_build_failure_not_a_test() {
        let r = fixture("pytest-junit-collection.xml");
        assert_eq!((r.compiled, r.compile_errors), (Some(false), 1));
        assert!(r.tests.is_empty());
        assert!(
            r.compile_messages[0].contains("ImportError: cannot import name 'missing'"),
            "{:?}",
            r.compile_messages
        );
    }

    #[test]
    fn gotestsum_puts_the_reason_in_the_body() {
        let r = fixture("gotestsum-junit.xml");
        assert_eq!(
            kinds(&r),
            expect(&[
                ("example.com/clamp::TestInRange", "passed"),
                ("example.com/clamp::TestCapsHigh", "assertion"),
                ("example.com/clamp::TestBare", "trivial"),
            ])
        );
        let caps = r
            .tests
            .iter()
            .find(|t| t.name.ends_with("CapsHigh"))
            .unwrap();
        assert_eq!(
            caps.message.as_deref(),
            Some("clamp_test.go:13: got 50, want 10")
        );
        let r = fixture("gotestsum-panic.xml");
        let new = r
            .tests
            .iter()
            .find(|t| t.name.ends_with("TestNew"))
            .unwrap();
        assert_eq!(new.failure, Some(FailureKind::Unimplemented));
        let r = fixture("gotestsum-build-failed.xml");
        assert_eq!((r.compiled, r.tests.len()), (Some(false), 0));
    }

    #[test]
    fn vitest_and_jest() {
        let r = fixture("vitest-junit.xml");
        // `expect(true).toBe(false)` reads exactly like a real boolean check failing, so it
        // can't be told apart from one: it counts as an assertion.
        assert_eq!(
            kinds(&r),
            expect(&[
                ("v.test.mjs::in range", "passed"),
                ("v.test.mjs::caps high", "assertion"),
                ("v.test.mjs::new api", "unimplemented"),
                ("v.test.mjs::fake", "assertion"),
                ("v.test.mjs::later", "ignored"),
            ])
        );
        let r = fixture("vitest-load-error.xml");
        assert_eq!((r.compiled, r.tests.len()), (Some(false), 0));
        assert!(r.compile_messages[0].contains("Cannot find module"));

        let r = fixture("jest-junit.xml");
        assert_eq!(
            kinds(&r),
            expect(&[
                ("in range", "passed"),
                ("caps high", "assertion"),
                ("new api", "unimplemented"),
                ("fake", "assertion"),
                ("later", "ignored"),
            ])
        );
        let caps = r.tests.iter().find(|t| t.name == "caps high").unwrap();
        assert!(caps.message.as_deref().unwrap().contains("Received: 50"));
    }

    #[test]
    fn surefire_errors_and_merging() {
        let a = parse(
            r#"<testsuite name="ClampTest">
              <testcase classname="ClampTest" name="capsHigh">
                <failure message="expected: &lt;10&gt; but was: &lt;50&gt;" type="org.opentest4j.AssertionFailedError"/>
              </testcase>
              <testcase classname="ClampTest" name="newApi">
                <error type="java.lang.UnsupportedOperationException">java.lang.UnsupportedOperationException
	at Clamp.newApi(Clamp.java:5)</error>
              </testcase>
              <testcase classname="ClampTest" name="npe">
                <error message="boom" type="java.lang.NullPointerException"/>
              </testcase>
              <testcase classname="ClampTest" name="bare">
                <failure type="org.opentest4j.AssertionFailedError">org.opentest4j.AssertionFailedError
	at ClampTest.bare(ClampTest.java:9)</failure>
              </testcase>
            </testsuite>"#,
        )
        .unwrap();
        let b =
            parse(r#"<testsuite name="Other"><testcase classname="Other" name="ok"/></testsuite>"#)
                .unwrap();
        let mut all = a;
        merge(&mut all, b);
        assert_eq!(
            kinds(&all),
            expect(&[
                ("ClampTest::capsHigh", "assertion"),
                ("ClampTest::newApi", "unimplemented"),
                ("ClampTest::npe", "panic"),
                ("ClampTest::bare", "trivial"),
                ("Other::ok", "passed"),
            ])
        );
        assert_eq!(all.compiled, Some(true));
    }

    #[test]
    fn a_suite_level_error_means_the_suite_did_not_run() {
        let r = parse(r#"<testsuite name="s"><error message="boom"/></testsuite>"#).unwrap();
        assert_eq!((r.compiled, r.compile_errors), (Some(false), 1));
    }

    #[test]
    fn not_junit_is_an_error_not_an_empty_report() {
        assert!(parse("<html/>").is_err());
        assert!(parse("not xml").is_err());
    }
}
