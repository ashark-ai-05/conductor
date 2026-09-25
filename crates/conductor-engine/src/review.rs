//! What a person needs to decide a run, on one screen (`.conductor/runs/<id>/review.json`).
//!
//! Written when a run pauses for a person, from the record and nothing else: the ticket's
//! title and acceptance criteria, what changed, each check before the fix and after it,
//! and what it cost. The TUI and the page show it; the decision is made from it.

use crate::receipt::StageRecord;
use crate::store::RunDir;
use conductor_checks::gate::GateResult;
use conductor_model::Verdict;
use conductor_model::view::{Change, Cost, EvidenceRow, Review};
use std::path::Path;
use std::time::Duration;

const FILE: &str = "review.json";

/// How many result lines of a check's output the review keeps.
const LINES: usize = 8;

/// The work's title: the first heading or non-empty line, whole.
pub fn title(task: &str) -> String {
    task.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("untitled")
        .trim_start_matches('#')
        .trim()
        .to_owned()
}

/// The acceptance criteria: the bullets under a heading that says "acceptance", else the
/// first bullets in the text, at most twelve.
pub fn criteria(task: &str) -> Vec<String> {
    let mut lines = task.lines().peekable();
    let mut under_heading = false;
    let mut found: Vec<String> = Vec::new();
    let mut any: Vec<String> = Vec::new();
    for l in &mut lines {
        let t = l.trim();
        if t.starts_with('#') {
            if under_heading && !found.is_empty() {
                break;
            }
            under_heading = t.to_lowercase().contains("acceptance");
            continue;
        }
        let bullet = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .map(str::trim)
            .filter(|b| !b.is_empty());
        if let Some(b) = bullet {
            if under_heading {
                found.push(b.to_owned());
            }
            any.push(b.to_owned());
        } else if under_heading && !t.is_empty() && !found.is_empty() {
            // A wrapped bullet continues the last one.
            if let Some(last) = found.last_mut() {
                last.push(' ');
                last.push_str(t);
            }
        }
    }
    let mut out = if found.is_empty() { any } else { found };
    out.truncate(12);
    out
}

/// The change, from a patch: files touched and lines added and removed.
pub fn change(patch: &str, branch: &str) -> Change {
    Change {
        files: patch
            .lines()
            .filter_map(|l| l.strip_prefix("diff --git a/"))
            .map(|l| l.split(" b/").next().unwrap_or(l).to_owned())
            .collect(),
        added: patch
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .count(),
        removed: patch
            .lines()
            .filter(|l| l.starts_with('-') && !l.starts_with("---"))
            .count(),
        branch: branch.to_owned(),
    }
}

/// The lines of a check's output that state a result (`PASS …`, `FAIL …`, `✓`, `✗`), else
/// its last two lines.
fn result_lines(r: &GateResult) -> Vec<String> {
    let text: String = r
        .executions
        .iter()
        .map(|e| e.stdout_text())
        .collect::<Vec<_>>()
        .join("\n");
    let said: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| {
            l.starts_with("PASS")
                || l.starts_with("FAIL")
                || l.starts_with('✓')
                || l.starts_with('✗')
        })
        .map(str::to_owned)
        .take(LINES)
        .collect();
    if !said.is_empty() {
        return said;
    }
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .rev()
        .take(2)
        .map(str::to_owned)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// Each check of the stages before the pause, before the fix and after it.
pub fn evidence(records: &[StageRecord]) -> Vec<EvidenceRow> {
    let mut rows = Vec::new();
    for s in records {
        // Before-checks are the stage's command checks in order; match them up that way.
        let mut before = s.before.iter().filter(|b| b.gate == "command_assert");
        for g in &s.gates {
            let b = if g.gate == "command_assert" {
                before.next().map(|b| b.verdict)
            } else {
                None
            };
            rows.push(EvidenceRow {
                stage: s.stage.clone(),
                check: g.gate.to_owned(),
                claim: g.claim.clone().unwrap_or_default(),
                before: b,
                after: g.verdict,
                detail: g.detail.clone(),
                lines: result_lines(g),
            });
        }
    }
    rows
}

pub fn cost(records: &[StageRecord], wall: Duration) -> Cost {
    Cost {
        tries: records.iter().map(|s| s.attempts).sum(),
        tokens: records.iter().filter_map(|s| s.tokens).sum(),
        // A fold from +0.0: an empty sum of floats is -0.0, which prints as "$-0.00".
        cost_usd: records
            .iter()
            .filter_map(|s| s.cost_usd)
            .fold(0.0, |a, b| a + b),
        waited_s: records.iter().map(|s| s.waited_ms).sum::<u64>() / 1000,
        wall_s: wall.as_secs(),
    }
}

/// Assembles the review for a pause at `stage`, from the stages recorded so far.
#[allow(clippy::too_many_arguments)]
pub fn build(
    dir: &RunDir,
    run_id: &str,
    stage: &str,
    who: &str,
    task: &str,
    records: &[StageRecord],
    branch: &str,
    wall: Duration,
) -> Review {
    // The change: the last attempt's patch of each stage that ran an agent.
    let mut patch = String::new();
    for s in records.iter().filter(|s| s.attempts > 0) {
        if let Ok(p) = std::fs::read_to_string(dir.attempt_diff(&s.stage, s.attempts)) {
            patch.push_str(&p);
        }
    }
    Review {
        run_id: run_id.to_owned(),
        stage: stage.to_owned(),
        who: who.to_owned(),
        title: title(task),
        criteria: criteria(task),
        change: change(&patch, branch),
        evidence: evidence(records),
        cost: cost(records, wall),
    }
}

pub fn write(dir: &RunDir, r: &Review) -> std::io::Result<()> {
    let p = dir.root.join(FILE);
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(r)?)?;
    std::fs::rename(tmp, p)
}

pub fn read(repo: &Path, run_id: &str) -> Option<Review> {
    let text = std::fs::read_to_string(RunDir::for_run(repo, run_id).root.join(FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Verdict words for a before/after pair, for whoever renders it.
pub fn before_after(before: Option<Verdict>, after: Verdict) -> String {
    match before {
        Some(b) => format!("{} before → {} after", b.word(), after.word()),
        None => after.word().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICKET: &str = "# BUG-7: a title\n\n**App:** x\n\n## Description\n\n- not a criterion\n\n## Acceptance criteria\n\n- AC1: `GET /a` returns 200 with `\"x\"`.\n- AC2: `GET /b` still returns 404.\n- The unit tests pass, and the log shows\n  the balance.\n\n## Root cause\n\n_(later)_\n";

    #[test]
    fn the_criteria_are_the_bullets_under_the_acceptance_heading() {
        assert_eq!(title(TICKET), "BUG-7: a title");
        let c = criteria(TICKET);
        assert_eq!(c.len(), 3, "{c:?}");
        assert!(c[0].starts_with("AC1:"));
        assert_eq!(c[2], "The unit tests pass, and the log shows the balance.");
        // Without the heading, the first bullets stand in.
        assert_eq!(criteria("# t\n\n- one\n- two\n"), vec!["one", "two"]);
        assert!(criteria("# t\n\nno bullets\n").is_empty());
    }

    #[test]
    fn the_change_counts_files_and_lines() {
        let patch = "diff --git a/src/A.java b/src/A.java\n--- a/src/A.java\n+++ b/src/A.java\n@@\n-old\n+new\n+more\ndiff --git a/src/B.java b/src/B.java\n+++ b/src/B.java\n+x\n";
        let c = change(patch, "conductor/R1");
        assert_eq!(c.files, vec!["src/A.java", "src/B.java"]);
        assert_eq!((c.added, c.removed), (3, 1));
    }

    #[test]
    fn before_and_after_are_paired_in_order() {
        let mut s = StageRecord {
            stage: "fix".into(),
            agent: "claude".into(),
            verdict: Verdict::Passed,
            attempts: 2,
            gates: vec![],
            tool_calls: 0,
            turns: None,
            tokens: Some(10),
            cost_usd: Some(1.5),
            model: None,
            notes: vec![],
            declared: vec![],
            before: vec![],
            waited_ms: 4200,
        };
        let g = |gate: &'static str, v: Verdict| GateResult {
            gate,
            verdict: v,
            detail: "d".into(),
            executions: vec![],
            assertions: vec![],
            scope: None,
            mutation: None,
            tests: None,
            claim: None,
        };
        s.gates = vec![
            g("scope", Verdict::Passed),
            g("command_assert", Verdict::Passed),
            g("command_assert", Verdict::Passed),
        ];
        s.before = vec![
            g("command_assert", Verdict::Passed),
            g("command_assert", Verdict::Failed),
        ];
        let rows = evidence(std::slice::from_ref(&s));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].before, None);
        assert_eq!(rows[1].before, Some(Verdict::Passed));
        assert_eq!(rows[2].before, Some(Verdict::Failed));
        assert_eq!(
            before_after(rows[2].before, rows[2].after),
            "failed before → passed after"
        );
        let c = cost(std::slice::from_ref(&s), Duration::from_secs(90));
        assert_eq!((c.tries, c.tokens, c.waited_s, c.wall_s), (2, 10, 4, 90));
        assert!((c.cost_usd - 1.5).abs() < 1e-9);
        assert!(!cost(&[], Duration::ZERO).cost_usd.is_sign_negative());
    }
}
