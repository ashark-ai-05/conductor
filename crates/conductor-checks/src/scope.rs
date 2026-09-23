//! The scope check: did a stage change only what it was allowed to?
//!
//! This is the check that keeps an implementer away from the tests it has to pass. It needs
//! no sandbox and no cooperation from the agent: it compares the files that actually changed
//! against the stage's declared patterns, after the stage has finished.

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Breach {
    /// Changed a file the stage's `write` patterns don't cover.
    OutsideScope,
    /// Changed a file an earlier stage produced and this stage must leave alone.
    Frozen,
    /// Changed a file the repository's policy protects from every agent.
    Protected,
}

impl Breach {
    pub fn describe(self) -> &'static str {
        match self {
            Breach::OutsideScope => "outside what this stage may change",
            Breach::Frozen => "locked by an earlier stage",
            Breach::Protected => "protected by the repository's policy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    pub path: String,
    pub breach: Breach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeReport {
    pub changed: Vec<String>,
    pub violations: Vec<Violation>,
}

impl ScopeReport {
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("`{pattern}` is not a valid path pattern: {source}")]
pub struct PatternError {
    pattern: String,
    source: globset::Error,
}

fn set(patterns: &[String]) -> Result<GlobSet, PatternError> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p).map_err(|source| PatternError {
            pattern: p.clone(),
            source,
        })?;
        b.add(g);
    }
    b.build().map_err(|source| PatternError {
        pattern: patterns.join(", "),
        source,
    })
}

/// Compares changed paths (relative to the repository root, as git reports them) against a
/// stage's patterns. Frozen and protected paths win over `write`: a stage allowed to write
/// `tests/**` still may not touch the one test file an earlier stage locked.
pub fn check(
    changed: &[String],
    write: &[String],
    frozen: &[String],
    protected: &[String],
) -> Result<ScopeReport, PatternError> {
    let (write, frozen, protected) = (set(write)?, set(frozen)?, set(protected)?);
    let mut changed: Vec<String> = changed.to_vec();
    changed.sort();
    changed.dedup();
    let violations = changed
        .iter()
        .filter_map(|p| {
            let breach = if protected.is_match(p) {
                Breach::Protected
            } else if frozen.is_match(p) {
                Breach::Frozen
            } else if !write.is_match(p) {
                Breach::OutsideScope
            } else {
                return None;
            };
            Some(Violation {
                path: p.clone(),
                breach,
            })
        })
        .collect();
    Ok(ScopeReport {
        changed,
        violations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn run(changed: &[&str]) -> ScopeReport {
        check(
            &v(changed),
            &v(&["src/**", "tests/**"]),
            &v(&["tests/generated_tests.rs"]),
            &v(&[".github/**", "Cargo.lock"]),
        )
        .unwrap()
    }

    #[test]
    fn changes_inside_scope_pass() {
        let r = run(&["src/status.rs", "src/cli/mod.rs", "tests/other.rs"]);
        assert!(r.passed(), "{r:?}");
    }

    #[test]
    fn touching_the_locked_tests_fails_even_though_tests_are_writable() {
        let r = run(&["src/status.rs", "tests/generated_tests.rs"]);
        assert_eq!(
            r.violations,
            vec![Violation {
                path: "tests/generated_tests.rs".into(),
                breach: Breach::Frozen
            }]
        );
    }

    #[test]
    fn protected_paths_are_named_as_such() {
        let r = run(&["Cargo.lock", ".github/workflows/ci.yml"]);
        assert!(r.violations.iter().all(|v| v.breach == Breach::Protected));
        assert_eq!(r.violations.len(), 2);
    }

    #[test]
    fn anything_else_is_outside_scope() {
        let r = run(&["README.md"]);
        assert_eq!(r.violations[0].breach, Breach::OutsideScope);
    }

    #[test]
    fn no_changes_is_a_pass_with_nothing_changed() {
        let r = run(&[]);
        assert!(r.passed());
        assert!(r.changed.is_empty());
    }

    #[test]
    fn a_bad_pattern_is_an_error() {
        assert!(check(&v(&["a"]), &v(&["src/[**"]), &[], &[]).is_err());
    }
}
