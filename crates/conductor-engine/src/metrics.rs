//! Metrics (SPEC §9.9), derived from a run's trace and events, so they always agree with the
//! receipt. One run yields one delta: exporters send it over OTLP, `conductor stats` adds up
//! many runs locally.
//!
//! No run id is ever an attribute: every value is keyed by workflow, stage, agent, check and
//! outcome, so series stay few and add up across runs.

use crate::trace::{Span, Trace};
use conductor_model::Event;
use std::collections::BTreeMap;

pub type Attrs = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(u64),
    Double(f64),
    /// Raw observations, bucketed on export.
    Observations(Vec<f64>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub name: &'static str,
    pub unit: &'static str,
    pub description: &'static str,
    /// One value per distinct set of attributes.
    pub points: BTreeMap<Attrs, Value>,
}

/// Every metric conductor produces: (name, unit, description).
pub const CATALOG: &[(&str, &str, &str)] = &[
    ("conductor.runs", "{run}", "Runs, by workflow and verdict"),
    ("conductor.run.duration", "s", "Run wall clock"),
    (
        "conductor.attempts",
        "{attempt}",
        "Agent attempts, by stage, agent, retry rung and outcome",
    ),
    (
        "conductor.catches",
        "{attempt}",
        "Attempts where the agent finished but a check said no, by the check that caught it",
    ),
    (
        "conductor.checks",
        "{check}",
        "Checks run, by check and verdict",
    ),
    (
        "conductor.mutants",
        "{mutant}",
        "Bugs injected by mutation testing, caught or missed by the tests",
    ),
    (
        "conductor.tokens",
        "{token}",
        "Tokens measured, by stage and agent",
    ),
    (
        "conductor.cost",
        "USD",
        "Cost reported by the agent, by stage and agent",
    ),
    ("conductor.stage.duration", "s", "Stage wall clock"),
    (
        "conductor.events",
        "{event}",
        "Recorded events by evidence source: how much is witnessed, observed or only inferred",
    ),
];

#[derive(Default)]
struct Sink(BTreeMap<&'static str, BTreeMap<Attrs, Value>>);

fn attrs(pairs: &[(&str, &str)]) -> Attrs {
    let mut a: Attrs = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    a.sort();
    a
}

impl Sink {
    fn add(&mut self, name: &'static str, a: Attrs, v: Value) {
        let points = self.0.entry(name).or_default();
        match (points.get_mut(&a), v) {
            (Some(Value::Int(x)), Value::Int(y)) => *x += y,
            (Some(Value::Double(x)), Value::Double(y)) => *x += y,
            (Some(Value::Observations(x)), Value::Observations(y)) => x.extend(y),
            (_, v) => {
                points.insert(a, v);
            }
        }
    }

    fn count(&mut self, name: &'static str, a: Attrs) {
        self.add(name, a, Value::Int(1));
    }

    fn done(self) -> Vec<Metric> {
        CATALOG
            .iter()
            .filter_map(|(name, unit, description)| {
                let points = self.0.get(name)?.clone();
                Some(Metric {
                    name,
                    unit,
                    description,
                    points,
                })
            })
            .collect()
    }
}

fn attr<'a>(s: &'a Span, key: &str) -> Option<&'a str> {
    s.attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn secs(s: &Span) -> f64 {
    (s.end_ns - s.start_ns) as f64 / 1e9
}

fn verdict(ok: Option<bool>) -> &'static str {
    match ok {
        Some(true) => "passed",
        Some(false) => "failed",
        None => "unfinished",
    }
}

/// "caught 4 of 5 injected bugs …" → (4, 5).
fn caught_of(detail: &str) -> Option<(u64, u64)> {
    let rest = detail.strip_prefix("caught ")?;
    let (caught, rest) = rest.split_once(" of ")?;
    let total = rest.split_whitespace().next()?;
    Some((caught.parse().ok()?, total.parse().ok()?))
}

/// One run's metrics.
pub fn collect(t: &Trace, events: &[Event]) -> Vec<Metric> {
    let mut m = Sink::default();
    let Some(run) = t.spans.first() else {
        return vec![];
    };
    let workflow = attr(run, "conductor.workflow").unwrap_or("unknown");
    let children = |parent: &Span| -> Vec<&Span> {
        t.spans
            .iter()
            .filter(|s| s.parent.as_deref() == Some(parent.span_id.as_str()))
            .collect()
    };

    let run_verdict = attr(run, "conductor.verdict").unwrap_or("unfinished");
    m.count(
        "conductor.runs",
        attrs(&[("workflow", workflow), ("verdict", run_verdict)]),
    );
    m.add(
        "conductor.run.duration",
        attrs(&[("workflow", workflow), ("verdict", run_verdict)]),
        Value::Observations(vec![secs(run)]),
    );

    for stage in children(run) {
        let Some(stage_id) = attr(stage, "conductor.stage") else {
            continue;
        };
        let mut stage_agent = "unknown";
        for attempt in children(stage)
            .into_iter()
            .filter(|s| s.name.starts_with("attempt "))
        {
            let kids = children(attempt);
            let agent = kids.iter().find(|s| s.name.starts_with("agent "));
            let kind = agent
                .and_then(|a| attr(a, "gen_ai.agent.name"))
                .unwrap_or("unknown");
            stage_agent = kind;
            let rung = attr(attempt, "conductor.rung").unwrap_or("unknown");
            let checks: Vec<&&Span> = kids
                .iter()
                .filter(|s| s.name.starts_with("check "))
                .collect();
            let failed_check = checks.iter().find(|c| c.ok == Some(false));
            let outcome = if attempt.ok == Some(true) {
                "passed"
            } else if agent.is_some_and(|a| a.ok == Some(false)) {
                "did_not_finish"
            } else if failed_check.is_some() {
                "check_failed"
            } else {
                "incomplete"
            };
            let base = [("workflow", workflow), ("stage", stage_id), ("agent", kind)];
            let mut a = base.to_vec();
            a.extend([("rung", rung), ("outcome", outcome)]);
            m.count("conductor.attempts", attrs(&a));
            if let Some(c) = failed_check {
                let check = attr(c, "conductor.check").unwrap_or("unknown");
                let mut a = base.to_vec();
                a.push(("check", check));
                m.count("conductor.catches", attrs(&a));
            }
            if let Some(g) = agent {
                if let Some(tokens) = attr(g, "conductor.tokens").and_then(|v| v.parse().ok()) {
                    m.add("conductor.tokens", attrs(&base), Value::Int(tokens));
                }
                if let Some(cost) = attr(g, "conductor.cost_usd").and_then(|v| v.parse().ok()) {
                    m.add("conductor.cost", attrs(&base), Value::Double(cost));
                }
            }
            for c in &checks {
                let check = attr(c, "conductor.check").unwrap_or("unknown");
                let v = attr(c, "conductor.verdict").unwrap_or("unknown");
                m.count(
                    "conductor.checks",
                    attrs(&[
                        ("workflow", workflow),
                        ("stage", stage_id),
                        ("check", check),
                        ("verdict", v),
                    ]),
                );
                if check == "mutation"
                    && let Some((caught, total)) = attr(c, "conductor.detail").and_then(caught_of)
                {
                    let at = |outcome| {
                        attrs(&[
                            ("workflow", workflow),
                            ("stage", stage_id),
                            ("outcome", outcome),
                        ])
                    };
                    m.add("conductor.mutants", at("caught"), Value::Int(caught));
                    m.add(
                        "conductor.mutants",
                        at("missed"),
                        Value::Int(total.saturating_sub(caught)),
                    );
                }
            }
        }
        m.add(
            "conductor.stage.duration",
            attrs(&[
                ("workflow", workflow),
                ("stage", stage_id),
                ("agent", stage_agent),
                ("verdict", verdict(stage.ok)),
            ]),
            Value::Observations(vec![secs(stage)]),
        );
    }

    for e in events {
        m.count(
            "conductor.events",
            attrs(&[("workflow", workflow), ("source", e.source.label())]),
        );
    }
    m.done()
}

/// Adds another run's metrics into `into`.
pub fn merge(into: &mut Vec<Metric>, more: Vec<Metric>) {
    for metric in more {
        match into.iter_mut().find(|m| m.name == metric.name) {
            Some(existing) => {
                let mut s = Sink::default();
                for (a, v) in existing.points.clone().into_iter().chain(metric.points) {
                    s.add(metric.name, a, v);
                }
                existing.points = s.0.remove(metric.name).unwrap_or_default();
            }
            None => into.push(metric),
        }
    }
}

/// The sum of a counter's points whose attributes include every `(key, value)` in `filter`.
pub fn total(metrics: &[Metric], name: &str, filter: &[(&str, &str)]) -> f64 {
    metrics
        .iter()
        .filter(|m| m.name == name)
        .flat_map(|m| m.points.iter())
        .filter(|(a, _)| {
            filter
                .iter()
                .all(|(k, v)| a.iter().any(|(ak, av)| ak == k && av == v))
        })
        .map(|(_, v)| match v {
            Value::Int(n) => *n as f64,
            Value::Double(d) => *d,
            Value::Observations(o) => o.len() as f64,
        })
        .sum()
}

/// The distinct values one attribute takes in a metric.
pub fn values_of(metrics: &[Metric], name: &str, key: &str) -> Vec<String> {
    let mut out: Vec<String> = metrics
        .iter()
        .filter(|m| m.name == name)
        .flat_map(|m| m.points.keys())
        .filter_map(|a| a.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
        .collect();
    out.sort();
    out.dedup();
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

    fn metrics() -> Vec<Metric> {
        let e = fixture();
        collect(&crate::trace::build("R", &e), &e)
    }

    #[test]
    fn a_real_run_is_counted() {
        let m = metrics();
        assert_eq!(total(&m, "conductor.runs", &[("verdict", "passed")]), 1.0);
        // tests: one caught try, then a pass; implement: a pass on the first try.
        assert_eq!(total(&m, "conductor.attempts", &[]), 3.0);
        assert_eq!(
            total(&m, "conductor.attempts", &[("outcome", "check_failed")]),
            1.0
        );
        assert_eq!(
            total(
                &m,
                "conductor.attempts",
                &[("rung", "in-context retry"), ("outcome", "passed")]
            ),
            1.0
        );
        assert_eq!(
            total(
                &m,
                "conductor.catches",
                &[("check", "scope"), ("stage", "tests")]
            ),
            1.0
        );
        assert_eq!(
            total(
                &m,
                "conductor.checks",
                &[("check", "scope"), ("verdict", "failed")]
            ),
            1.0
        );
        assert_eq!(
            total(&m, "conductor.mutants", &[("outcome", "caught")]),
            20.0
        );
        assert_eq!(
            total(&m, "conductor.mutants", &[("outcome", "missed")]),
            0.0
        );
        assert_eq!(
            total(&m, "conductor.tokens", &[("stage", "tests")]),
            (243_496 + 267_964) as f64
        );
        let cost = total(&m, "conductor.cost", &[]);
        assert!((cost - 0.44).abs() < 1e-9, "{cost}");
        assert_eq!(total(&m, "conductor.stage.duration", &[]), 2.0);
        assert_eq!(total(&m, "conductor.events", &[]), fixture().len() as f64);
    }

    #[test]
    fn no_attribute_names_the_run() {
        for metric in metrics() {
            for a in metric.points.keys() {
                assert!(
                    a.iter().all(|(k, v)| k != "run_id" && v != "R"),
                    "{}: {a:?}",
                    metric.name
                );
            }
        }
    }

    #[test]
    fn two_runs_add_up() {
        let mut all = metrics();
        merge(&mut all, metrics());
        assert_eq!(total(&all, "conductor.runs", &[]), 2.0);
        assert_eq!(
            total(&all, "conductor.mutants", &[("outcome", "caught")]),
            40.0
        );
        let durations = all
            .iter()
            .find(|m| m.name == "conductor.run.duration")
            .unwrap();
        let Value::Observations(o) = durations.points.values().next().unwrap() else {
            panic!()
        };
        assert_eq!(o.len(), 2);
    }
}

fn pct(part: f64, whole: f64) -> String {
    if whole == 0.0 {
        "—".into()
    } else {
        format!("{:.0}%", 100.0 * part / whole)
    }
}

fn human(n: f64) -> String {
    match n {
        n if n >= 1e6 => format!("{:.1}M", n / 1e6),
        n if n >= 1e3 => format!("{:.0}k", n / 1e3),
        n => format!("{n:.0}"),
    }
}

fn median(metrics: &[Metric], name: &str, key: &str, value: &str) -> Option<f64> {
    let mut all: Vec<f64> = metrics
        .iter()
        .filter(|m| m.name == name)
        .flat_map(|m| m.points.iter())
        .filter(|(a, _)| a.iter().any(|(k, v)| k == key && v == value))
        .flat_map(|(_, v)| match v {
            Value::Observations(o) => o.clone(),
            _ => vec![],
        })
        .collect();
    if all.is_empty() {
        return None;
    }
    all.sort_by(f64::total_cmp);
    let mid = all.len() / 2;
    Some(if all.len().is_multiple_of(2) {
        (all[mid - 1] + all[mid]) / 2.0
    } else {
        all[mid]
    })
}

/// Many runs' metrics, as `conductor stats` prints them.
pub fn summary(m: &[Metric]) -> String {
    let t = |name, f: &[(&str, &str)]| total(m, name, f);
    let runs = t("conductor.runs", &[]);
    let passed = t("conductor.runs", &[("verdict", "passed")]);
    let finished = t("conductor.attempts", &[("outcome", "passed")])
        + t("conductor.attempts", &[("outcome", "check_failed")]);
    let caught = t("conductor.catches", &[]);
    let mut lines = vec![
        format!(
            "runs       {passed:.0} passed · {:.0} did not · {}",
            runs - passed,
            pct(passed, runs)
        ),
        format!(
            "caught     {caught:.0} of {finished:.0} finished attempts: the agent said done, a check said no ({})",
            pct(caught, finished)
        ),
    ];
    let by_check: Vec<String> = values_of(m, "conductor.catches", "check")
        .iter()
        .map(|c| format!("{c} {:.0}", t("conductor.catches", &[("check", c)])))
        .collect();
    if !by_check.is_empty() {
        lines.push(format!("           by {}", by_check.join(" · ")));
    }
    let retries: Vec<String> = values_of(m, "conductor.attempts", "rung")
        .iter()
        .filter(|r| *r != "first try")
        .map(|r| format!("{r} {:.0}", t("conductor.attempts", &[("rung", r)])))
        .collect();
    let retry_total: f64 =
        t("conductor.attempts", &[]) - t("conductor.attempts", &[("rung", "first try")]);
    lines.push(if retries.is_empty() {
        "retries    none".into()
    } else {
        format!("retries    {retry_total:.0} · {}", retries.join(" · "))
    });
    let checks: Vec<String> = values_of(m, "conductor.checks", "check")
        .iter()
        .map(|c| {
            format!(
                "{c} {:.0}/{:.0}",
                t("conductor.checks", &[("check", c), ("verdict", "passed")]),
                t("conductor.checks", &[("check", c)])
            )
        })
        .collect();
    lines.push(format!("checks     {} passed", checks.join(" · ")));
    let (killed, missed) = (
        t("conductor.mutants", &[("outcome", "caught")]),
        t("conductor.mutants", &[("outcome", "missed")]),
    );
    if killed + missed > 0.0 {
        lines.push(format!(
            "bugs       {killed:.0} of {:.0} injected bugs caught by the tests ({})",
            killed + missed,
            pct(killed, killed + missed)
        ));
    }
    let cost = t("conductor.cost", &[]);
    lines.push(format!(
        "spend      {} tokens{}",
        human(t("conductor.tokens", &[])),
        if cost > 0.0 {
            format!(" · ${cost:.2}")
        } else {
            String::new()
        }
    ));
    let stages: Vec<String> = values_of(m, "conductor.stage.duration", "stage")
        .iter()
        .filter_map(|s| {
            median(m, "conductor.stage.duration", "stage", s).map(|d| format!("{s} {d:.0}s"))
        })
        .collect();
    if !stages.is_empty() {
        lines.push(format!("median     {}", stages.join(" · ")));
    }
    let events = t("conductor.events", &[]);
    let sources: Vec<String> = [
        "witnessed",
        "observed",
        "measured",
        "inferred",
        "human",
        "unmanaged",
    ]
    .iter()
    .filter_map(|s| {
        let n = t("conductor.events", &[("source", s)]);
        (n > 0.0).then(|| format!("{s} {}", pct(n, events)))
    })
    .collect();
    lines.push(format!("evidence   {}", sources.join(" · ")));
    lines.iter().map(|l| format!("  {l}\n")).collect()
}

#[cfg(test)]
mod summary_tests {
    use super::*;

    #[test]
    fn the_summary_reads_as_sentences() {
        let e: Vec<Event> = include_str!("../../../docs/examples/live-run-durations/events.jsonl")
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let s = summary(&collect(&crate::trace::build("R", &e), &e));
        assert!(s.contains("runs       1 passed · 0 did not · 100%"), "{s}");
        assert!(s.contains("caught     1 of 3 finished attempts"), "{s}");
        assert!(s.contains("by scope 1"), "{s}");
        assert!(s.contains("retries    1 · in-context retry 1"), "{s}");
        assert!(s.contains("mutation 1/1"), "{s}");
        assert!(s.contains("20 of 20 injected bugs caught"), "{s}");
        assert!(s.contains("$0.44"), "{s}");
        assert!(s.contains("witnessed "), "{s}");
    }
}
