//! Receipts and run summaries: what a finished (or running) run can claim.
//!
//! The rule a receipt enforces is SPEC §5: it may only say "passed" about a check that could
//! have failed, and that check must have been witnessed by conductor itself.
//! [`Receipt::verdict`] is derived from the rows, never stored alongside them, so the two
//! cannot disagree.

use crate::evidence::Source;
use serde::{Deserialize, Serialize};

/// The state of a check, a stage or a whole run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Passed,
    Failed,
    /// Mixed results across reruns. Never counted as a pass (SPEC §9.4).
    Flaky,
    Running,
    /// Waiting on a person.
    Blocked,
    /// A failed check a person accepted with a recorded reason.
    Overridden,
    Pending,
}

impl Verdict {
    /// The status glyph. Shares herdr's vocabulary where herdr has one.
    pub fn glyph(self) -> &'static str {
        match self {
            Verdict::Passed => "✓",
            Verdict::Failed => "✗",
            Verdict::Flaky => "≈",
            Verdict::Running => "◐",
            Verdict::Blocked => "◆",
            Verdict::Overridden => "▲",
            Verdict::Pending => "○",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Verdict::Passed => "passed",
            Verdict::Failed => "failed",
            Verdict::Flaky => "flaky",
            Verdict::Running => "running",
            Verdict::Blocked => "needs you",
            Verdict::Overridden => "overridden",
            Verdict::Pending => "not started",
        }
    }
}

/// One line of "what was proven".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRow {
    /// Written as a plain claim: "Tests pass after the change".
    pub claim: String,
    pub verdict: Verdict,
    pub detail: String,
    pub source: Source,
}

/// A mutant the tests failed to catch: where the reviewer should look first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Survivor {
    pub at: String,
    pub change: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Integrity {
    pub chain_head: String,
    pub anchored_in: Option<String>,
    pub signed: bool,
    /// `verify` re-derived the same verdict from stored evidence.
    pub reproduces: bool,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub run_id: String,
    pub work: String,
    pub kind: String,
    pub checks: Vec<CheckRow>,
    pub survivors: Vec<Survivor>,
    pub not_checked: Vec<String>,
    /// Label and value pairs for "how it ran".
    pub how: Vec<(String, String)>,
    pub integrity: Integrity,
    /// Other places the same receipt can be read.
    pub also_at: Vec<String>,
}

impl Receipt {
    /// The run's verdict, derived from its rows.
    ///
    /// A row that claims to have passed without conductor witnessing it fails the receipt
    /// outright: that is the one mistake a receipt must never make quietly.
    pub fn verdict(&self) -> Verdict {
        if self.checks.is_empty() {
            return Verdict::Pending;
        }
        let mut overridden = false;
        for c in &self.checks {
            match c.verdict {
                Verdict::Passed if !c.source.is_proof() => return Verdict::Failed,
                Verdict::Failed => return Verdict::Failed,
                Verdict::Blocked => return Verdict::Blocked,
                Verdict::Flaky => return Verdict::Flaky,
                Verdict::Running | Verdict::Pending => return Verdict::Running,
                Verdict::Overridden => overridden = true,
                Verdict::Passed => {}
            }
        }
        if overridden {
            Verdict::Overridden
        } else {
            Verdict::Passed
        }
    }

    pub fn passed_count(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.verdict == Verdict::Passed)
            .count()
    }
}

/// Where a run is, from the operator's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    HerdrTab(u16),
    TabClosed,
    Headless,
}

impl Place {
    pub fn describe(self) -> String {
        match self {
            Place::HerdrTab(n) => format!("herdr tab {n}"),
            Place::TabClosed => "tab closed".into(),
            Place::Headless => "headless · CI".into(),
        }
    }
}

/// One row of the runs list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub work: String,
    pub kind: String,
    pub status: Verdict,
    /// One entry per check, in workflow order, for the small bar in the list.
    pub checks: Vec<Verdict>,
    pub place: Place,
}

impl RunSummary {
    pub fn is_finished(&self) -> bool {
        matches!(
            self.status,
            Verdict::Passed | Verdict::Failed | Verdict::Flaky | Verdict::Overridden
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(verdict: Verdict, source: Source) -> CheckRow {
        CheckRow {
            claim: "c".into(),
            verdict,
            detail: String::new(),
            source,
        }
    }

    fn receipt(rows: Vec<CheckRow>) -> Receipt {
        Receipt {
            run_id: "r".into(),
            work: "w".into(),
            kind: "build".into(),
            checks: rows,
            survivors: vec![],
            not_checked: vec![],
            how: vec![],
            integrity: Integrity {
                chain_head: String::new(),
                anchored_in: None,
                signed: false,
                reproduces: false,
                trace_id: None,
            },
            also_at: vec![],
        }
    }

    #[test]
    fn all_witnessed_passes_make_a_pass() {
        let r = receipt(vec![
            row(Verdict::Passed, Source::Witnessed),
            row(Verdict::Passed, Source::Witnessed),
        ]);
        assert_eq!(r.verdict(), Verdict::Passed);
    }

    #[test]
    fn a_pass_nobody_witnessed_fails_the_receipt() {
        let r = receipt(vec![
            row(Verdict::Passed, Source::Witnessed),
            row(Verdict::Passed, Source::Inferred),
        ]);
        assert_eq!(r.verdict(), Verdict::Failed);
    }

    #[test]
    fn flaky_is_never_a_pass() {
        let r = receipt(vec![
            row(Verdict::Passed, Source::Witnessed),
            row(Verdict::Flaky, Source::Witnessed),
        ]);
        assert_eq!(r.verdict(), Verdict::Flaky);
    }

    #[test]
    fn an_override_is_visible_in_the_verdict() {
        let r = receipt(vec![
            row(Verdict::Passed, Source::Witnessed),
            row(Verdict::Overridden, Source::Human),
        ]);
        assert_eq!(r.verdict(), Verdict::Overridden);
    }

    #[test]
    fn a_failure_outranks_everything_after_it() {
        let r = receipt(vec![
            row(Verdict::Failed, Source::Witnessed),
            row(Verdict::Blocked, Source::Human),
        ]);
        assert_eq!(r.verdict(), Verdict::Failed);
    }

    #[test]
    fn no_checks_is_not_a_pass() {
        assert_eq!(receipt(vec![]).verdict(), Verdict::Pending);
    }
}
