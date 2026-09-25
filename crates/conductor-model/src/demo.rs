//! Sample data for `conductor ui --demo`, matching the design mockup (docs/mockup.html).
//!
//! Everything here is plainly fictional and is only reachable through `--demo`.

use crate::evidence::Source;
use crate::receipt::{CheckRow, Integrity, Place, Receipt, RunSummary, Survivor, Verdict};
use crate::view::{CheckView, LiveRun, PaneView, StageView};

use Verdict::{Failed as F, Passed as P, Pending as O, Running as R};

fn summary(
    run_id: &str,
    work: &str,
    status: Verdict,
    checks: &[Verdict],
    place: Place,
) -> RunSummary {
    RunSummary {
        run_id: run_id.into(),
        work: work.into(),
        kind: "build".into(),
        status,
        checks: checks.to_vec(),
        place,
    }
}

pub fn runs() -> Vec<RunSummary> {
    vec![
        summary(
            "01JBXQ7",
            "PTT-1234 add --json to status",
            R,
            &[P, P, R, O, O],
            Place::HerdrTab(2),
        ),
        summary(
            "01JBXN9",
            "PTT-1229 retry backoff",
            Verdict::Blocked,
            &[P, P, R, O, O],
            Place::HerdrTab(3),
        ),
        summary(
            "01JBXR1",
            "PTT-1238 holiday calendar",
            R,
            &[P, R, O, O, O],
            Place::HerdrTab(4),
        ),
        summary(
            "01JBXP2",
            "PTT-1231 ledger rounding",
            P,
            &[P, P, P, P, P],
            Place::TabClosed,
        ),
        summary(
            "01JBXK1",
            "nightly: bump serde",
            P,
            &[P, P, P, P, P],
            Place::Headless,
        ),
        summary(
            "01JBXJ4",
            "PTT-1210 fx cutoff",
            F,
            &[P, P, P, F, P],
            Place::Headless,
        ),
    ]
}

/// The one run that is waiting on a person, with what it is asking.
pub fn needs_you() -> Option<(&'static str, &'static str, u16)> {
    Some((
        "PTT-1229",
        "claude wants to edit Cargo.lock, which is protected",
        3,
    ))
}

fn stage(name: &str, status: Verdict, detail: &str) -> StageView {
    StageView {
        name: name.into(),
        status,
        detail: detail.into(),
    }
}

fn check(name: &str, status: Verdict, detail: &str, progress: Option<u8>) -> CheckView {
    CheckView {
        name: name.into(),
        status,
        detail: detail.into(),
        progress,
    }
}

fn pane(stage: &str, status: Verdict, open: bool) -> PaneView {
    PaneView {
        stage: stage.into(),
        status,
        open,
        opened_by: "conductor".into(),
    }
}

pub fn live() -> LiveRun {
    let mut run = LiveRun {
        run_id: "01JBXQ7".into(),
        work: "PTT-1234 add --json to status".into(),
        kind: "build".into(),
        herdr_tab: Some(2),
        stages: vec![
            stage("spec", P, "claude · 1m12s"),
            stage("tests", P, "codex · 7 failing tests"),
            stage("implement", R, "claude · 2nd try"),
            stage("deliver", O, "PR + receipt"),
        ],
        checks: vec![
            check("scope", P, "only src/ changed", None),
            check("tests", R, "5 of 7 passing", Some(71)),
            check("mutation", O, "next · needs 0.70", None),
        ],
        note: Some("1st try failed: tests missed 11 injected bugs. The 2nd try starts fresh and gets that list.".into()),
        log: vec![],
        panes: vec![pane("spec", P, false), pane("tests", P, false), pane("implement", R, true)],
        ended: None,
        pid: None,
        waiting: None,
    };
    run.push_log("12:04:13", Source::Observed, "edited src/status.rs");
    run.push_log("12:04:20", Source::Witnessed, "scope passed");
    run.push_log("12:04:23", Source::Measured, "+18k tokens");
    run.push_log("12:04:25", Source::Inferred, "herdr: claude idle");
    run
}

/// Advances the demo run by one step. Returns false once there is nothing left to do.
pub fn step(run: &mut LiveRun, n: usize) -> bool {
    let set =
        |run: &mut LiveRun, name: &str, status: Verdict, detail: &str, progress: Option<u8>| {
            if let Some(c) = run.checks.iter_mut().find(|c| c.name == name) {
                c.status = status;
                c.detail = detail.into();
                c.progress = progress;
            }
        };
    match n {
        0 => set(run, "tests", R, "6 of 7 passing", Some(86)),
        1 => {
            set(run, "tests", R, "7 of 7 passing · second run", Some(100));
            run.push_log("12:04:31", Source::Witnessed, "tests: 7 of 7 passed");
        }
        2 => {
            set(run, "tests", P, "7 of 7, twice, no flakes", None);
            run.push_log("12:04:34", Source::Witnessed, "second test run matched");
        }
        3 => {
            set(
                run,
                "mutation",
                R,
                "injecting bugs · 9 of 23 tried",
                Some(39),
            );
            run.push_log("12:04:37", Source::Witnessed, "mutation testing started");
        }
        4 => set(
            run,
            "mutation",
            R,
            "injecting bugs · 17 of 23 tried",
            Some(74),
        ),
        5 => {
            set(run, "mutation", P, "caught 19 of 23 · 0.83", None);
            run.push_log("12:04:43", Source::Witnessed, "mutation 0.83, needs 0.70");
            run.push_log("12:04:43", Source::Measured, "212k tokens in total");
        }
        6 => {
            for s in &mut run.stages {
                s.status = P;
            }
            run.stages[2].detail = "claude · passed on 2nd try".into();
            run.stages[3].detail = "PR #412 · receipt".into();
            for p in &mut run.panes {
                p.status = P;
                p.open = false;
            }
            run.push_log("12:04:46", Source::Witnessed, "record anchored in 4e1c0d2");
        }
        _ => return false,
    }
    true
}

pub const STEPS: usize = 7;

fn row(claim: &str, detail: &str) -> CheckRow {
    CheckRow {
        claim: claim.into(),
        verdict: P,
        detail: detail.into(),
        source: Source::Witnessed,
    }
}

pub fn receipt() -> Receipt {
    Receipt {
        run_id: "01JBXQ7".into(),
        work: "PTT-1234 add --json to status".into(),
        kind: "build".into(),
        checks: vec![
            row(
                "Tests were written by a different agent",
                "codex, then locked",
            ),
            row("Tests failed before the change", "7 of 7, on assertions"),
            row("Tests pass after it", "7 of 7, twice, no flakes"),
            row(
                "Tests catch injected bugs",
                "19 of 23 · score 0.83, needs 0.70",
            ),
            row(
                "Only allowed files changed",
                "src/ only; protected paths untouched",
            ),
        ],
        survivors: vec![
            Survivor {
                at: "status.rs:88".into(),
                change: "`>` became `>=` unnoticed".into(),
            },
            Survivor {
                at: "status.rs:102".into(),
                change: "removing `flush()` went unnoticed".into(),
            },
            Survivor {
                at: "status.rs:131".into(),
                change: "returning `Ok(())` early went unnoticed".into(),
            },
            Survivor {
                at: "status.rs:140".into(),
                change: "`&&` became `||` in the --json guard".into(),
            },
        ],
        not_checked: vec![
            "non-UTF-8 paths: no test covers them".into(),
            "the agent could read the whole machine".into(),
        ],
        how: vec![
            (
                "stages".into(),
                "spec · claude → tests · codex → implement · claude (2 tries)".into(),
            ),
            ("where".into(), "herdr tab 2 · no human steps needed".into()),
            ("turns".into(), "11 observed · 0 inferred".into()),
            ("tokens".into(), "212k measured · 11m05s".into()),
        ],
        integrity: Integrity {
            chain_head: "sha256:9f3a…c21e".into(),
            anchored_in: Some("4e1c0d2".into()),
            signed: true,
            reproduces: true,
            trace_id: Some("4f1c9a0b".into()),
        },
        also_at: vec![
            "PR #412".into(),
            ".conductor/runs/01JBXQ7/receipt.json".into(),
            "conductor receipt 01JBXQ7".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_receipt_passes_on_its_own_rows() {
        assert_eq!(receipt().verdict(), P);
    }

    #[test]
    fn the_demo_run_finishes_after_its_last_step() {
        let mut run = live();
        assert!(!run.is_done());
        for n in 0..STEPS {
            assert!(step(&mut run, n));
        }
        assert!(run.is_done());
        assert!(!step(&mut run, STEPS));
        assert!(run.panes.iter().all(|p| !p.open), "passed panes are closed");
    }

    #[test]
    fn the_evidence_feed_keeps_only_its_latest_lines() {
        let mut run = live();
        for n in 0..STEPS {
            step(&mut run, n);
        }
        assert!(run.log.len() <= 6);
        assert_eq!(run.log.last().unwrap().text, "record anchored in 4e1c0d2");
    }
}
