//! A run as a trace (SPEC §9.9): run → stage → attempt → agent and checks.
//!
//! Spans are derived from the event chain after the fact, so `conductor trace` and the OTLP
//! export always agree with the receipt, and need no collector while the run happens. Ids
//! are derived from the run id: the receipt can name its trace before anything is exported.
//!
//! Timing is only as good as the record. Tool calls are recorded when the agent returns, so
//! they are events on the agent's span, not spans of their own; an agent's span runs from
//! its attempt starting to its first recorded evidence.

use conductor_model::{Event, Source};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub span_id: String,
    pub parent: Option<String>,
    pub name: String,
    pub start_ns: u128,
    pub end_ns: u128,
    pub attrs: Vec<(String, String)>,
    /// Point-in-time events on the span: (time, name, event).
    pub events: Vec<(u128, String, usize)>,
    /// `None` while unknown, `Some(true)` passed, `Some(false)` failed with a reason.
    pub ok: Option<bool>,
    pub message: String,
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub trace_id: String,
    pub spans: Vec<Span>,
    /// For each chain event, the span it belongs to.
    pub event_span: Vec<String>,
}

fn hex_id(parts: &[&str], bytes: usize) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    hex::encode(&h.finalize()[..bytes])
}

/// A run's trace id: 16 bytes, from its run id.
pub fn trace_id(run_id: &str) -> String {
    hex_id(&["conductor-trace", run_id], 16)
}

fn span_id(run_id: &str, path: &str) -> String {
    hex_id(&["conductor-span", run_id, path], 8)
}

/// Nanoseconds since the epoch, from an RFC 3339 time.
pub fn unix_ns(at: &str) -> u128 {
    time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
        .map(|t| t.unix_timestamp_nanos().max(0) as u128)
        .unwrap_or(0)
}

const GATES: &[&str] = &[
    "scope",
    "file_nonempty",
    "schema",
    "command_assert",
    "mutation",
];
const VERDICTS: &[&str] = &["passed", "failed", "flaky", "not witnessed"];

/// `(gate, verdict, detail)` when the event reports a check.
fn check_event(what: &str) -> Option<(&str, &str, &str)> {
    let (gate, rest) = what.split_once(' ')?;
    if !GATES.contains(&gate) {
        return None;
    }
    let verdict = VERDICTS
        .iter()
        .find(|v| rest.starts_with(&format!("{v}:")))?;
    let detail = rest[verdict.len() + 1..].trim();
    let detail = detail.split(" [stdout ").next().unwrap_or(detail);
    Some((gate, verdict, detail))
}

struct Builder<'a> {
    run_id: &'a str,
    spans: Vec<Span>,
}

impl Builder<'_> {
    fn open(&mut self, path: &str, parent: Option<usize>, name: String, at: u128) -> usize {
        let depth = parent.map(|p| self.spans[p].depth + 1).unwrap_or(0);
        self.spans.push(Span {
            span_id: span_id(self.run_id, path),
            parent: parent.map(|p| self.spans[p].span_id.clone()),
            name,
            start_ns: at,
            end_ns: at,
            attrs: vec![],
            events: vec![],
            ok: None,
            message: String::new(),
            depth,
        });
        self.spans.len() - 1
    }

    fn extend(&mut self, i: usize, at: u128) {
        let s = &mut self.spans[i];
        s.end_ns = s.end_ns.max(at);
        if let Some(p) = s.parent.clone()
            && let Some(pi) = self.spans.iter().position(|x| x.span_id == p)
        {
            self.extend(pi, at);
        }
    }
}

/// Builds a run's trace from its events.
pub fn build(run_id: &str, events: &[Event]) -> Trace {
    let mut b = Builder {
        run_id,
        spans: vec![],
    };
    let first = events.first().map(|e| unix_ns(&e.at)).unwrap_or(0);
    let run = b.open("run", None, format!("run {run_id}"), first);
    b.spans[run]
        .attrs
        .push(("conductor.run_id".into(), run_id.into()));
    let mut event_span = Vec::with_capacity(events.len());
    let mut stage: Option<(String, usize)> = None;
    let mut attempt: Option<usize> = None;
    let mut agent: Option<usize> = None;
    let mut prev_at = first;

    for (i, e) in events.iter().enumerate() {
        let at = unix_ns(&e.at);
        let what = e.what.as_str();
        let mut home = run;

        match e.stage.as_deref() {
            None => {
                if let Some(rest) = what.strip_prefix("run started: ") {
                    let wf = rest.split(" at ").next().unwrap_or(rest);
                    b.spans[run]
                        .attrs
                        .push(("conductor.workflow".into(), wf.into()));
                    b.spans[run].name = format!("run {run_id} · {wf}");
                }
                if let Some(rest) = what.strip_prefix("run ")
                    && let Some(v) = VERDICTS
                        .iter()
                        .find(|v| rest.starts_with(&format!("{v} in ")))
                {
                    b.spans[run].ok = Some(*v == "passed");
                    b.spans[run].message = if *v == "passed" {
                        String::new()
                    } else {
                        format!("the run {v}")
                    };
                    b.spans[run]
                        .attrs
                        .push(("conductor.verdict".into(), (*v).into()));
                }
                b.spans[run].events.push((at, what.to_owned(), i));
            }
            Some(s) => {
                let st = match &stage {
                    Some((name, idx)) if name == s => *idx,
                    _ => {
                        let idx =
                            b.open(&format!("stage/{s}"), Some(run), format!("stage {s}"), at);
                        b.spans[idx]
                            .attrs
                            .push(("conductor.stage".into(), s.into()));
                        stage = Some((s.to_owned(), idx));
                        attempt = None;
                        agent = None;
                        idx
                    }
                };
                home = st;
                if let Some(rest) = what.strip_prefix("attempt ")
                    && let Some((n, how)) = rest.split_once(": ")
                {
                    let a = b.open(
                        &format!("stage/{s}/attempt/{n}"),
                        Some(st),
                        format!("attempt {n}"),
                        at,
                    );
                    let kind = how.rsplit(" with ").next().unwrap_or("");
                    b.spans[a].attrs.push((
                        "conductor.rung".into(),
                        how.split(" with ").next().unwrap_or(how).into(),
                    ));
                    let g = b.open(
                        &format!("stage/{s}/attempt/{n}/agent"),
                        Some(a),
                        format!("agent {kind}"),
                        at,
                    );
                    b.spans[g]
                        .attrs
                        .push(("gen_ai.agent.name".into(), kind.into()));
                    attempt = Some(a);
                    agent = Some(g);
                    home = g;
                } else if let Some((gate, verdict, detail)) = check_event(what) {
                    let parent = attempt.unwrap_or(st);
                    let c = b.open(
                        &format!("stage/{s}/check/{i}"),
                        Some(parent),
                        format!("check {gate}"),
                        prev_at,
                    );
                    b.spans[c].ok = Some(verdict == "passed");
                    if verdict != "passed" {
                        b.spans[c].message = format!("{verdict}: {detail}");
                    }
                    b.spans[c]
                        .attrs
                        .push(("conductor.check".into(), gate.into()));
                    b.spans[c]
                        .attrs
                        .push(("conductor.verdict".into(), verdict.into()));
                    b.spans[c]
                        .attrs
                        .push(("conductor.detail".into(), detail.into()));
                    b.spans[c].events.push((at, what.to_owned(), i));
                    b.extend(c, at);
                    if verdict != "passed"
                        && let Some(a) = attempt
                    {
                        b.spans[a].ok = Some(false);
                        b.spans[a].message = format!("check {gate} {verdict}");
                    }
                    // The agent finished its turn: its work went to the checks.
                    if let Some(g) = agent
                        && b.spans[g].ok.is_none()
                    {
                        b.spans[g].ok = Some(true);
                    }
                    agent = None;
                    home = c;
                } else if what.starts_with("stage passed on attempt") {
                    b.spans[st].ok = Some(true);
                    if let Some(a) = attempt {
                        b.spans[a].ok = Some(true);
                    }
                } else if let Some(v) = what.strip_prefix("run halted: stage ") {
                    b.spans[st].ok = Some(false);
                    b.spans[st].message = format!("the stage {v}");
                } else if let Some(g) = agent {
                    // Evidence of the agent's turn: its tool calls, tokens and herdr notes.
                    if let Some(t) = what
                        .strip_suffix(" tokens")
                        .or_else(|| what.split_once(" tokens · ").map(|(t, _)| t))
                        && e.source == Source::Measured
                    {
                        b.spans[g].attrs.push(("conductor.tokens".into(), t.into()));
                        if let Some((_, cost)) = what.split_once(" · $") {
                            b.spans[g]
                                .attrs
                                .push(("conductor.cost_usd".into(), cost.trim().into()));
                        }
                    }
                    if what.starts_with("agent did not finish") {
                        b.spans[g].ok = Some(false);
                        b.spans[g].message = what.to_owned();
                    }
                    b.spans[g].events.push((at, what.to_owned(), i));
                    b.extend(g, at);
                    home = g;
                } else if let Some(a) = attempt {
                    home = a;
                }
                if home == st {
                    b.spans[st].events.push((at, what.to_owned(), i));
                }
                b.extend(home, at);
            }
        }
        event_span.push(b.spans[home].span_id.clone());
        prev_at = at;
    }
    let spans = b.spans;
    Trace {
        trace_id: trace_id(run_id),
        spans,
        event_span,
    }
}

fn dur(ns: u128) -> String {
    let ms = ns / 1_000_000;
    match ms {
        0..=999 => format!("{ms}ms"),
        1000..=59_999 => format!("{:.1}s", ms as f64 / 1000.0),
        _ => format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1000),
    }
}

/// The trace as an indented tree with a bar per span, for `conductor trace`.
pub fn render(t: &Trace) -> String {
    let mut out = String::new();
    let Some(root) = t.spans.first() else {
        return out;
    };
    let (t0, total) = (root.start_ns, (root.end_ns - root.start_ns).max(1));
    const WIDTH: u128 = 32;
    let _ = writeln!(out, "trace {}\n", t.trace_id);
    for s in &t.spans {
        let glyph = match s.ok {
            Some(true) => "✓",
            Some(false) => "✗",
            None => "·",
        };
        let label = format!("{}{glyph} {}", "  ".repeat(s.depth), s.name);
        let from = ((s.start_ns - t0) * WIDTH / total) as usize;
        let len = (((s.end_ns - s.start_ns) * WIDTH).div_ceil(total) as usize).max(1);
        let bar = format!(
            "{}{}",
            " ".repeat(from),
            "━".repeat(len.min(WIDTH as usize - from.min(WIDTH as usize - 1)))
        );
        let _ = write!(
            out,
            "{label:<34} {bar:<32} {:>8}",
            dur(s.end_ns - s.start_ns)
        );
        if !s.message.is_empty() {
            let _ = write!(out, "  {}", s.message);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Event> {
        include_str!("../../../docs/examples/live-run-durations/events.jsonl")
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn a_real_run_becomes_stages_attempts_agents_and_checks() {
        let t = build("0MUE8IKOXFM", &fixture());
        let names: Vec<&str> = t.spans.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names[0], "run 0MUE8IKOXFM · duration-parser");
        assert!(names.contains(&"stage tests"));
        assert!(names.contains(&"stage implement"));
        assert_eq!(names.iter().filter(|n| **n == "attempt 2").count(), 1);
        assert_eq!(names.iter().filter(|n| **n == "check mutation").count(), 1);
        assert_eq!(t.event_span.len(), fixture().len());

        let root = &t.spans[0];
        assert_eq!(root.ok, Some(true));
        let secs = (root.end_ns - root.start_ns) / 1_000_000_000;
        assert!((103..=105).contains(&secs), "{secs}");

        // The first try at `tests` failed its scope check; the second passed.
        let tries: Vec<&Span> = t
            .spans
            .iter()
            .filter(|s| s.name.starts_with("attempt"))
            .collect();
        assert_eq!(tries[0].ok, Some(false));
        assert_eq!(tries[0].message, "check scope failed");
        assert_eq!(tries[1].ok, Some(true));

        // The agent's span covers its turn and carries its evidence.
        let agent = t.spans.iter().find(|s| s.name == "agent claude").unwrap();
        assert!((agent.end_ns - agent.start_ns) / 1_000_000_000 >= 40);
        assert!(
            agent
                .attrs
                .iter()
                .any(|(k, v)| k == "conductor.tokens" && v == "243496")
        );
        assert!(
            agent
                .attrs
                .iter()
                .any(|(k, v)| k == "conductor.cost_usd" && v == "0.14")
        );
        assert!(!agent.events.is_empty());

        // Every span sits inside its parent.
        for s in &t.spans {
            if let Some(p) = &s.parent {
                let p = t.spans.iter().find(|x| &x.span_id == p).unwrap();
                assert!(
                    p.start_ns <= s.start_ns && s.end_ns <= p.end_ns,
                    "{} in {}",
                    s.name,
                    p.name
                );
            }
        }
    }

    #[test]
    fn ids_are_stable_and_the_right_size() {
        assert_eq!(trace_id("R1"), trace_id("R1"));
        assert_ne!(trace_id("R1"), trace_id("R2"));
        assert_eq!(trace_id("R1").len(), 32);
        let t = build("R1", &fixture());
        assert!(t.spans.iter().all(|s| s.span_id.len() == 16));
        let ids: std::collections::BTreeSet<_> = t.spans.iter().map(|s| &s.span_id).collect();
        assert_eq!(ids.len(), t.spans.len(), "span ids are unique");
    }

    #[test]
    fn the_tree_renders_with_durations_and_failures() {
        let out = render(&build("0MUE8IKOXFM", &fixture()));
        assert!(out.contains("✓ run 0MUE8IKOXFM"), "{out}");
        assert!(out.contains("✗ attempt 1"), "{out}");
        assert!(out.contains("check scope failed"), "{out}");
        assert!(out.contains("1m4"), "{out}");
    }
}
