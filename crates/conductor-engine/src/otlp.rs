//! OpenTelemetry export over OTLP/HTTP with JSON bodies (SPEC §9.9): the run's trace and
//! every chain event as a log record. Plain OTLP, no vendor SDK; off unless an endpoint is set.
//!
//! What agents did is summarised, not copied: an `observed` event (a tool call, a command an
//! agent ran) is exported as its first word unless content capture is on, since commands and
//! paths can carry secrets. Checks, verdicts and conductor's own events are exported whole.

use crate::trace::{Trace, unix_ns};
use conductor_model::{Event, Source};
use serde_json::{Value, json};

/// Where to send, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The collector's base URL, such as `http://localhost:4318`.
    pub endpoint: String,
    /// Extra headers, such as an API key: `OTEL_EXPORTER_OTLP_HEADERS` (`k=v,k=v`).
    pub headers: Vec<(String, String)>,
    /// Export what agents did in full (`CONDUCTOR_OTLP_CONTENT=1`).
    pub content: bool,
}

impl Config {
    /// From `CONDUCTOR_OTLP_ENDPOINT`, falling back to `OTEL_EXPORTER_OTLP_ENDPOINT`. `None`
    /// when neither is set: conductor never exports by default.
    pub fn from_env() -> Option<Config> {
        Config::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Option<Config> {
        let endpoint = get("CONDUCTOR_OTLP_ENDPOINT")
            .or_else(|| get("OTEL_EXPORTER_OTLP_ENDPOINT"))
            .filter(|e| !e.trim().is_empty())?;
        let headers = get("OTEL_EXPORTER_OTLP_HEADERS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .filter(|(k, _)| !k.is_empty())
            .collect();
        Some(Config {
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            headers,
            content: get("CONDUCTOR_OTLP_CONTENT").is_some_and(|v| v == "1" || v == "true"),
        })
    }
}

fn attr(k: &str, v: &str) -> Value {
    json!({"key": k, "value": {"stringValue": v}})
}

fn resource(run_id: &str, receipt_sha256: Option<&str>) -> Value {
    let mut attrs = vec![
        attr("service.name", "conductor"),
        attr("service.version", env!("CARGO_PKG_VERSION")),
        attr("conductor.run_id", run_id),
    ];
    if let Some(h) = receipt_sha256 {
        attrs.push(attr("conductor.receipt.sha256", h));
    }
    json!({ "attributes": attrs })
}

fn scope() -> Value {
    json!({"name": "conductor", "version": env!("CARGO_PKG_VERSION")})
}

/// An event's text as exported: `observed` events are cut to their first word unless
/// content capture is on.
pub fn exported_text(e: &Event, content: bool) -> String {
    if content || e.source != Source::Observed {
        return e.what.clone();
    }
    let first = e.what.split_whitespace().next().unwrap_or("");
    let first = if first == "agent" {
        // Pane actions read "agent ran `…` in pane …": keep who and the verb.
        e.what
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        first.to_owned()
    };
    format!("{first} …")
}

/// `ExportTraceServiceRequest` as JSON.
pub fn traces_body(
    run_id: &str,
    t: &Trace,
    events: &[Event],
    receipt_sha256: Option<&str>,
    content: bool,
) -> Value {
    let spans: Vec<Value> = t
        .spans
        .iter()
        .map(|s| {
            let status = match s.ok {
                Some(true) => json!({"code": 1}),
                Some(false) => json!({"code": 2, "message": s.message}),
                None => json!({"code": 0}),
            };
            json!({
                "traceId": t.trace_id,
                "spanId": s.span_id,
                "parentSpanId": s.parent.clone().unwrap_or_default(),
                "name": s.name,
                "kind": 1,
                "startTimeUnixNano": s.start_ns.to_string(),
                "endTimeUnixNano": s.end_ns.to_string(),
                "attributes": s.attrs.iter().map(|(k, v)| attr(k, v)).collect::<Vec<_>>(),
                "events": s.events.iter().map(|(at, _, i)| {
                    let e = &events[*i];
                    json!({
                        "timeUnixNano": at.to_string(),
                        "name": exported_text(e, content),
                        "attributes": [attr("conductor.source", e.source.label())],
                    })
                }).collect::<Vec<_>>(),
                "status": status,
            })
        })
        .collect();
    json!({"resourceSpans": [{
        "resource": resource(run_id, receipt_sha256),
        "scopeSpans": [{"scope": scope(), "spans": spans}],
    }]})
}

/// `ExportLogsServiceRequest` as JSON: one record per chain event, linked to its span.
pub fn logs_body(
    run_id: &str,
    t: &Trace,
    events: &[Event],
    receipt_sha256: Option<&str>,
    content: bool,
) -> Value {
    let records: Vec<Value> = events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut attrs = vec![
                attr("conductor.run_id", run_id),
                attr("conductor.source", e.source.label()),
                attr("conductor.event.hash", &e.hash),
                attr("conductor.event.seq", &e.seq.to_string()),
            ];
            if let Some(s) = &e.stage {
                attrs.push(attr("conductor.stage", s));
            }
            let at = unix_ns(&e.at).to_string();
            json!({
                "timeUnixNano": at,
                "observedTimeUnixNano": at,
                "severityNumber": 9,
                "severityText": "INFO",
                "body": {"stringValue": exported_text(e, content)},
                "attributes": attrs,
                "traceId": t.trace_id,
                "spanId": t.event_span.get(i).cloned().unwrap_or_default(),
            })
        })
        .collect();
    json!({"resourceLogs": [{
        "resource": resource(run_id, receipt_sha256),
        "scopeLogs": [{"scope": scope(), "logRecords": records}],
    }]})
}

fn post(cfg: &Config, path: &str, body: &Value) -> Result<(), String> {
    let url = format!("{}{path}", cfg.endpoint);
    let mut req = crate::http::agent_for(&url, std::time::Duration::from_secs(10))
        .post(&url)
        .set("Content-Type", "application/json");
    for (k, v) in &cfg.headers {
        req = req.set(k, v);
    }
    req.send_string(&body.to_string())
        .map(|_| ())
        .map_err(|e| format!("{url}: {e}"))
}

/// Histogram bucket bounds for durations, in seconds.
pub const DURATION_BOUNDS: &[f64] = &[
    1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1200.0, 1800.0, 3600.0,
];

/// `ExportMetricsServiceRequest` as JSON: one run's values as deltas over the run.
///
/// The resource names only the service, never the run: a collector turning deltas into
/// cumulative series (for Prometheus, the `deltatocumulative` processor) keys series by
/// resource, and per-run resources would never add up.
pub fn metrics_body(metrics: &[crate::metrics::Metric], start_ns: u128, end_ns: u128) -> Value {
    use crate::metrics::Value as V;
    let out: Vec<Value> = metrics
        .iter()
        .map(|m| {
            let attrs =
                |a: &crate::metrics::Attrs| a.iter().map(|(k, v)| attr(k, v)).collect::<Vec<_>>();
            let histogram = m.points.values().any(|v| matches!(v, V::Observations(_)));
            let points: Vec<Value> = m
                .points
                .iter()
                .map(|(a, v)| {
                    let mut p = json!({
                        "attributes": attrs(a),
                        "startTimeUnixNano": start_ns.to_string(),
                        "timeUnixNano": end_ns.to_string(),
                    });
                    match v {
                        V::Int(n) => p["asInt"] = json!(n.to_string()),
                        V::Double(d) => p["asDouble"] = json!(d),
                        V::Observations(o) => {
                            let mut buckets = vec![0u64; DURATION_BOUNDS.len() + 1];
                            for x in o {
                                let i = DURATION_BOUNDS
                                    .iter()
                                    .position(|b| x <= b)
                                    .unwrap_or(DURATION_BOUNDS.len());
                                buckets[i] += 1;
                            }
                            p["count"] = json!(o.len().to_string());
                            p["sum"] = json!(o.iter().sum::<f64>());
                            p["min"] = json!(o.iter().copied().fold(f64::INFINITY, f64::min));
                            p["max"] = json!(o.iter().copied().fold(0.0, f64::max));
                            p["explicitBounds"] = json!(DURATION_BOUNDS);
                            p["bucketCounts"] =
                                json!(buckets.iter().map(u64::to_string).collect::<Vec<_>>());
                        }
                    }
                    p
                })
                .collect();
            let mut metric = json!({"name": m.name, "unit": m.unit, "description": m.description});
            if histogram {
                metric["histogram"] = json!({"aggregationTemporality": 1, "dataPoints": points});
            } else {
                metric["sum"] = json!({
                    "aggregationTemporality": 1,
                    "isMonotonic": true,
                    "dataPoints": points,
                });
            }
            metric
        })
        .collect();
    json!({"resourceMetrics": [{
        "resource": {"attributes": [
            attr("service.name", "conductor"),
            attr("service.version", env!("CARGO_PKG_VERSION")),
        ]},
        "scopeMetrics": [{"scope": scope(), "metrics": out}],
    }]})
}

/// Sends the run's trace, logs and metrics. Returns the trace id.
pub fn export(
    cfg: &Config,
    run_id: &str,
    events: &[Event],
    receipt_sha256: Option<&str>,
) -> Result<String, String> {
    let t = crate::trace::build(run_id, events);
    post(
        cfg,
        "/v1/traces",
        &traces_body(run_id, &t, events, receipt_sha256, cfg.content),
    )?;
    post(
        cfg,
        "/v1/logs",
        &logs_body(run_id, &t, events, receipt_sha256, cfg.content),
    )?;
    if let Some(run) = t.spans.first() {
        let metrics = crate::metrics::collect(&t, events);
        post(
            cfg,
            "/v1/metrics",
            &metrics_body(&metrics, run.start_ns, run.end_ns),
        )?;
    }
    Ok(t.trace_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    fn fixture() -> Vec<Event> {
        include_str!("../../../docs/examples/live-run-durations/events.jsonl")
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn metrics_are_deltas_with_bucketed_durations() {
        let events = fixture();
        let t = crate::trace::build("R", &events);
        let m = crate::metrics::collect(&t, &events);
        let body = metrics_body(&m, 1, 2);
        let metrics = body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap();
        let find = |n: &str| metrics.iter().find(|m| m["name"] == n).unwrap();
        let runs = &find("conductor.runs")["sum"];
        assert_eq!(runs["aggregationTemporality"], 1);
        assert_eq!(runs["dataPoints"][0]["asInt"], "1");
        let d = &find("conductor.run.duration")["histogram"]["dataPoints"][0];
        assert_eq!(d["count"], "1");
        // 103 s lands in the (60, 120] bucket.
        assert_eq!(d["bucketCounts"][5], "1");
        assert!(find("conductor.cost")["sum"]["dataPoints"][0]["asDouble"].is_number());
    }

    #[test]
    fn nothing_is_exported_without_an_endpoint() {
        assert_eq!(Config::from_lookup(|_| None), None);
        let c = Config::from_lookup(|k| match k {
            "OTEL_EXPORTER_OTLP_ENDPOINT" => Some("http://c:4318/".into()),
            "OTEL_EXPORTER_OTLP_HEADERS" => Some("x-api-key=abc, x-team = t".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(c.endpoint, "http://c:4318");
        assert_eq!(c.headers[1], ("x-team".into(), "t".into()));
        assert!(!c.content);
    }

    #[test]
    fn what_agents_did_is_summarised_unless_content_capture_is_on() {
        let events = fixture();
        let bash = events
            .iter()
            .find(|e| e.source == Source::Observed && e.what.starts_with("Bash "))
            .unwrap();
        assert_eq!(exported_text(bash, false), "Bash …");
        assert_eq!(exported_text(bash, true), bash.what);
        let t = crate::trace::build("R", &events);
        let body = logs_body("R", &t, &events, None, false).to_string();
        assert!(
            !body.contains("cargo test 2>&1"),
            "commands stay out of the export"
        );
        assert!(
            body.contains("scope failed: Cargo.lock is outside"),
            "checks are exported whole"
        );
    }

    #[test]
    fn the_bodies_have_the_otlp_shape() {
        let events = fixture();
        let t = crate::trace::build("R", &events);
        let tr = traces_body("R", &t, &events, Some("sha256:abc"), false);
        let spans = &tr["resourceSpans"][0]["scopeSpans"][0]["spans"];
        assert_eq!(spans.as_array().unwrap().len(), t.spans.len());
        assert_eq!(spans[0]["traceId"].as_str().unwrap().len(), 32);
        assert_eq!(spans[0]["parentSpanId"], "");
        assert!(spans[1]["parentSpanId"].as_str().unwrap().len() == 16);
        let logs = logs_body("R", &t, &events, None, false);
        let recs = &logs["resourceLogs"][0]["scopeLogs"][0]["logRecords"];
        assert_eq!(recs.as_array().unwrap().len(), events.len());
        assert!(tr.to_string().contains("conductor.receipt.sha256"));
    }

    /// A one-shot HTTP collector on localhost that records each request's path and body.
    fn collector(requests: usize) -> (String, std::thread::JoinHandle<Vec<(String, String)>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let mut got = vec![];
            for _ in 0..requests {
                let (mut s, _) = listener.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap().to_owned();
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                    .unwrap();
                got.push((path, String::from_utf8(body).unwrap()));
            }
            got
        });
        (url, h)
    }

    #[test]
    fn a_run_is_sent_as_traces_then_logs() {
        let (url, h) = collector(3);
        let cfg = Config {
            endpoint: url,
            headers: vec![],
            content: false,
        };
        let id = export(&cfg, "R", &fixture(), None).unwrap();
        let got = h.join().unwrap();
        assert_eq!(got[0].0, "/v1/traces");
        assert_eq!(got[1].0, "/v1/logs");
        assert_eq!(got[2].0, "/v1/metrics");
        assert!(got[0].1.contains(&id));
        assert!(
            !got[2].1.contains("conductor.run_id"),
            "metrics never name the run"
        );
    }

    #[test]
    fn an_unreachable_collector_is_an_error_not_a_hang() {
        let cfg = Config {
            endpoint: "http://127.0.0.1:1".into(),
            headers: vec![],
            content: false,
        };
        assert!(export(&cfg, "R", &fixture(), None).is_err());
    }
}
