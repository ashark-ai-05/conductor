//! The run as two lanes: what the agent did, and what conductor witnessed between.
//!
//! The chain records both with the same shape; the lane is the evidence source. Witnessed
//! events are conductor's own (a check, a halt, a commit); observed, measured and inferred
//! ones describe the agent. Showing them side by side is the whole point: the agent never
//! grades its own work, and here you can see who said what.

use conductor_model::{Event, Source, Verdict};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Agent,
    Conductor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The run began, its worktree, its anchor.
    Run,
    Setup,
    /// An attempt began.
    Attempt,
    /// A tool the agent used: the text is `<tool> <target>`.
    Action,
    /// Tokens and cost.
    Usage,
    /// A check's verdict.
    Check,
    /// A stage passed and was committed.
    Commit,
    Halt,
    /// Something conductor could only infer about the agent.
    Inferred,
    /// A person decided.
    Decision,
    /// The agent asked for a tool and was refused, with nobody to ask.
    Refused,
    /// The agent asked a person for a tool and waited.
    Asked,
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub seq: u64,
    pub at: String,
    /// Milliseconds since the run's first event.
    pub t_ms: u64,
    pub lane: Lane,
    pub source: Source,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<usize>,
    /// For a check: which one, and how it went.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    /// For an action: the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub text: String,
}

/// One attempt of one stage: a band on the timeline.
#[derive(Debug, Clone, Serialize)]
pub struct Attempt {
    pub stage: String,
    pub attempt: usize,
    pub agent: String,
    /// "first try", "in-context retry", "fresh retry".
    pub how: String,
    pub start_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    /// `passed`, `check_failed`, `did_not_finish`, `halted`, or `running`.
    pub outcome: String,
    pub actions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// The check that stopped it, if one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caught_by: Option<String>,
    pub first_seq: u64,
    pub last_seq: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Timeline {
    pub entries: Vec<Entry>,
    pub attempts: Vec<Attempt>,
    /// Total spend so far, from every measured usage event.
    pub cost_usd: f64,
    pub tokens: u64,
    /// Attempts where the agent said done and a check said no.
    pub catches: usize,
}

const CHECKS: &[&str] = &[
    "scope",
    "file_nonempty",
    "schema",
    "command_assert",
    "mutation",
];

/// "<gate> <verdict>: <detail>" → (gate, verdict, detail).
fn check_of(what: &str) -> Option<(&str, Verdict, &str)> {
    let (head, detail) = what.split_once(": ")?;
    let (gate, word) = head.split_once(' ')?;
    if !CHECKS.contains(&gate) {
        return None;
    }
    let verdict = match word {
        "passed" => Verdict::Passed,
        "failed" => Verdict::Failed,
        "flaky" => Verdict::Flaky,
        "unwitnessed" => Verdict::Unwitnessed,
        "blocked" => Verdict::Blocked,
        _ => return None,
    };
    // The evidence hashes belong to the record, not to a person reading the lane.
    let detail = detail.split(" [stdout ").next().unwrap_or(detail);
    Some((gate, verdict, detail))
}

/// "7797886 tokens · $2.53" → (tokens, cost).
fn usage_of(what: &str) -> Option<(u64, f64)> {
    let (t, rest) = what.split_once(" tokens")?;
    let tokens = t.trim().parse().ok()?;
    let cost = rest
        .split('$')
        .nth(1)
        .and_then(|c| c.trim().parse().ok())
        .unwrap_or(0.0);
    Some((tokens, cost))
}

/// An agent action's target, with the worktree prefix (`…/<run_id>/`) taken off.
fn shorten(text: &str, run_id: &str) -> String {
    let marker = format!("/{run_id}/");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(&marker) {
        // Back up to the start of the absolute path this marker ends.
        let path_start = rest[..i].rfind(' ').map_or(0, |s| s + 1);
        let before = &rest[..path_start];
        if rest[path_start..i].starts_with('/') {
            out.push_str(before);
        } else {
            out.push_str(&rest[..i + marker.len()]);
            rest = &rest[i + marker.len()..];
            continue;
        }
        rest = &rest[i + marker.len()..];
    }
    out.push_str(rest);
    out
}

pub fn build(run_id: &str, events: &[Event]) -> Timeline {
    let mut t = Timeline::default();
    let Some(first) = events.first() else {
        return t;
    };
    let t0 = conductor_engine::trace::unix_ns(&first.at);
    let ms = |at: &str| {
        let ns = conductor_engine::trace::unix_ns(at);
        u64::try_from(ns.saturating_sub(t0) / 1_000_000).unwrap_or(u64::MAX)
    };
    let mut current: Option<usize> = None;

    for e in events {
        let t_ms = ms(&e.at);
        let what = e.what.as_str();
        let mut entry = Entry {
            seq: e.seq,
            at: e.at.clone(),
            t_ms,
            // A person's action belongs with conductor's: it is not the agent grading itself.
            lane: if matches!(e.source, Source::Witnessed | Source::Human) {
                Lane::Conductor
            } else {
                Lane::Agent
            },
            source: e.source,
            kind: Kind::Other,
            stage: e.stage.clone(),
            attempt: current.map(|i| t.attempts[i].attempt),
            check: None,
            verdict: None,
            tool: None,
            text: what.to_owned(),
        };

        match e.source {
            Source::Witnessed => {
                if let Some(rest) = what.strip_prefix("attempt ")
                    && let Some((n, how)) = rest.split_once(": ")
                    && let Ok(n) = n.parse::<usize>()
                    && let Some(stage) = &e.stage
                {
                    let (how, agent) = how.rsplit_once(" with ").unwrap_or((how, "?"));
                    t.attempts.push(Attempt {
                        stage: stage.clone(),
                        attempt: n,
                        agent: agent.to_owned(),
                        how: how.to_owned(),
                        start_ms: t_ms,
                        end_ms: None,
                        outcome: "running".into(),
                        actions: 0,
                        tokens: None,
                        cost_usd: None,
                        caught_by: None,
                        first_seq: e.seq,
                        last_seq: e.seq,
                    });
                    current = Some(t.attempts.len() - 1);
                    entry.attempt = Some(n);
                    entry.kind = Kind::Attempt;
                    entry.text = format!("{how} with {agent}");
                } else if let Some((gate, verdict, detail)) = check_of(what) {
                    entry.kind = Kind::Check;
                    entry.check = Some(gate.to_owned());
                    entry.verdict = Some(verdict);
                    entry.text = detail.to_owned();
                    if verdict != Verdict::Passed
                        && let Some(i) = current
                        && t.attempts[i].outcome == "running"
                    {
                        let a = &mut t.attempts[i];
                        a.outcome = "check_failed".into();
                        a.caught_by = Some(gate.to_owned());
                        a.end_ms = Some(t_ms);
                        if verdict == Verdict::Failed {
                            t.catches += 1;
                        }
                    }
                } else if what.starts_with("stage passed") {
                    entry.kind = Kind::Commit;
                    if let Some(i) = current {
                        let a = &mut t.attempts[i];
                        a.outcome = "passed".into();
                        a.end_ms = Some(t_ms);
                    }
                } else if let Some(why) = what.strip_prefix("halted: ") {
                    entry.kind = Kind::Halt;
                    entry.text = why.to_owned();
                    if let Some(i) = current
                        && t.attempts[i].outcome == "running"
                    {
                        let a = &mut t.attempts[i];
                        a.outcome = "halted".into();
                        a.end_ms = Some(t_ms);
                    }
                } else if what.starts_with("setup ") {
                    entry.kind = Kind::Setup;
                } else if what.starts_with("the agent did not finish")
                    || what.starts_with("agent did not finish")
                {
                    entry.kind = Kind::Halt;
                    if let Some(i) = current
                        && t.attempts[i].outcome == "running"
                    {
                        let a = &mut t.attempts[i];
                        a.outcome = "did_not_finish".into();
                        a.end_ms = Some(t_ms);
                    }
                } else if e.stage.is_none() {
                    entry.kind = Kind::Run;
                }
            }
            Source::Observed => {
                entry.kind = if what.starts_with("refused: ") {
                    Kind::Refused
                } else if what.starts_with("asked: ") {
                    Kind::Asked
                } else {
                    Kind::Action
                };
                let text = shorten(what, run_id);
                let (tool, _) = text.split_once(' ').unwrap_or((&text, ""));
                entry.tool = Some(tool.to_owned());
                entry.text = text;
                if let Some(i) = current {
                    t.attempts[i].actions += 1;
                }
            }
            Source::Measured => {
                if let Some((tokens, cost)) = usage_of(what) {
                    entry.kind = Kind::Usage;
                    t.tokens += tokens;
                    t.cost_usd += cost;
                    if let Some(i) = current {
                        let a = &mut t.attempts[i];
                        a.tokens = Some(tokens);
                        a.cost_usd = Some(cost);
                    }
                }
            }
            Source::Inferred => entry.kind = Kind::Inferred,
            Source::Human => entry.kind = Kind::Decision,
            Source::Unmanaged => {}
        }
        if let Some(i) = current
            && e.stage.is_some()
        {
            t.attempts[i].last_seq = e.seq;
        }
        t.entries.push(entry);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_model::Chain;

    fn events(script: &[(Source, Option<&str>, &str)]) -> Vec<Event> {
        let mut c = Chain::new();
        for (i, (src, stage, what)) in script.iter().enumerate() {
            let at = format!("2026-01-01T00:00:{:02}Z", i);
            c.append(&at, *src, *stage, what);
        }
        c.events().to_vec()
    }

    #[test]
    fn a_caught_attempt_and_a_passed_one_are_told_apart() {
        let ev = events(&[
            (Source::Witnessed, None, "run started: build at abc"),
            (
                Source::Witnessed,
                Some("tests"),
                "attempt 1: first try with claude",
            ),
            (
                Source::Observed,
                Some("tests"),
                "Write /home/x/worktrees/R1/tests/a.rs",
            ),
            (Source::Measured, Some("tests"), "1200 tokens · $0.40"),
            (
                Source::Witnessed,
                Some("tests"),
                "scope passed: 1 file(s) changed, all within scope",
            ),
            (
                Source::Witnessed,
                Some("tests"),
                "command_assert failed: does not compile (1 errors); did not hold: compiled == true [stdout sha256:ab]",
            ),
            (
                Source::Witnessed,
                Some("tests"),
                "attempt 2: in-context retry with claude",
            ),
            (Source::Measured, Some("tests"), "800 tokens · $0.20"),
            (
                Source::Witnessed,
                Some("tests"),
                "command_assert passed: 3 passed, 0 failed [stdout sha256:cd]",
            ),
            (
                Source::Witnessed,
                Some("tests"),
                "stage passed on attempt 2; committed 1234567",
            ),
            (Source::Witnessed, None, "run passed in 9s"),
        ]);
        let t = build("R1", &ev);
        assert_eq!(t.attempts.len(), 2);
        let a1 = &t.attempts[0];
        assert_eq!(
            (a1.outcome.as_str(), a1.caught_by.as_deref(), a1.actions),
            ("check_failed", Some("command_assert"), 1)
        );
        assert_eq!((a1.start_ms, a1.end_ms), (1000, Some(5000)));
        assert_eq!(a1.cost_usd, Some(0.40));
        let a2 = &t.attempts[1];
        assert_eq!((a2.outcome.as_str(), a2.end_ms), ("passed", Some(9000)));
        assert_eq!(t.catches, 1);
        assert!((t.cost_usd - 0.60).abs() < 1e-9);
        assert_eq!(t.tokens, 2000);

        let action = &t.entries[2];
        assert_eq!((action.lane, action.kind), (Lane::Agent, Kind::Action));
        assert_eq!(action.tool.as_deref(), Some("Write"));
        assert_eq!(action.text, "Write tests/a.rs");
        let check = &t.entries[5];
        assert_eq!((check.lane, check.kind), (Lane::Conductor, Kind::Check));
        assert_eq!(check.verdict, Some(Verdict::Failed));
        assert!(!check.text.contains("sha256"), "{}", check.text);
        assert_eq!(t.entries[9].kind, Kind::Commit);
        assert_eq!(t.entries[10].kind, Kind::Run);
    }

    #[test]
    fn a_halt_ends_the_attempt_without_counting_as_a_catch() {
        let ev = events(&[
            (
                Source::Witnessed,
                Some("s"),
                "attempt 1: first try with script",
            ),
            (
                Source::Witnessed,
                Some("s"),
                "halted: the run's wall-clock budget is spent",
            ),
        ]);
        let t = build("R", &ev);
        assert_eq!(t.attempts[0].outcome, "halted");
        assert_eq!(t.catches, 0);
        assert_eq!(t.entries[1].kind, Kind::Halt);
        assert_eq!(t.entries[1].text, "the run's wall-clock budget is spent");
    }

    #[test]
    fn worktree_paths_are_shortened_only_when_absolute() {
        assert_eq!(
            shorten("Read /a/b/R1/src/x.rs and /a/b/R1/src/y.rs", "R1"),
            "Read src/x.rs and src/y.rs"
        );
        assert_eq!(shorten("Bash cargo test", "R1"), "Bash cargo test");
    }
}
