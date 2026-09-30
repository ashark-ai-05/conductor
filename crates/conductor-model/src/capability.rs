//! Versioned native operations. Operation names are data, not a closed business-task enum.
use crate::task::{Answer, Fact, Presentation, ResultRow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const VERSION: u32 = 1;
pub const CAPTURE_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub version: u32,
    pub id: String,
    pub label: String,
    pub adapter: String,
    pub input: Value,
}

impl Descriptor {
    pub fn binding(&self) -> Binding {
        Binding {
            id: self.id.clone(),
            fingerprint: hash(&serde_json::to_vec(self).expect("descriptor is serializable")),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.version != VERSION
            || !valid_id(&self.id)
            || self.label.trim().is_empty()
            || self.label.len() > 120
            || self.label.chars().any(char::is_control)
        {
            return Err("Unsupported capability version, identifier or label.".into());
        }
        Ok(())
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && id != "."
        && id != ".."
}
pub fn hash(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub id: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct Available {
    pub binding: Binding,
    pub label: String,
    pub detail: String,
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capture {
    pub source: String,
    pub media_type: String,
    pub body: String,
    pub hash: String,
}
impl Capture {
    pub fn new(source: String, media_type: String, body: String) -> Result<Self, String> {
        if body.len() > CAPTURE_LIMIT {
            return Err(
                "Output exceeds the 64 KiB capture limit; no partial result was accepted.".into(),
            );
        }
        let hash = hash(body.as_bytes());
        Ok(Self {
            source,
            media_type,
            body,
            hash,
        })
    }

    /// Only data shapes choose presentation. A payload's `kind`, `command`, or `actions`
    /// fields are ordinary data and can never acquire runtime authority.
    pub fn answer(&self, label: &str) -> Answer {
        let mut answer = Answer {
            text: self.body.clone(),
            ..Answer::default()
        };
        if self.media_type != "application/json" {
            return answer;
        }
        let Ok(value) = serde_json::from_str::<Value>(&self.body) else {
            return answer;
        };
        let summary =
            "Captured data · Enter inspects the saved source · v shows the full payload".to_owned();
        let cell = |v: &Value| match v {
            Value::String(s) => s.clone(),
            _ => v.to_string(),
        };
        let presentation = match value {
            Value::Object(object) if !object.is_empty() && object.len() <= 30 => {
                Some(Presentation::Facts {
                    title: label.into(),
                    summary,
                    facts: object
                        .into_iter()
                        .map(|(label, v)| Fact {
                            label,
                            value: cell(&v),
                            sources: vec![],
                        })
                        .collect(),
                })
            }
            Value::Array(rows)
                if !rows.is_empty() && rows.len() <= 200 && rows.iter().all(Value::is_object) =>
            {
                let columns: Vec<_> = rows
                    .iter()
                    .flat_map(|r| r.as_object().unwrap().keys().cloned())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                if (2..=8).contains(&columns.len()) {
                    let rows = rows
                        .iter()
                        .map(|r| ResultRow {
                            cells: columns
                                .iter()
                                .map(|k| r.get(k).map(&cell).unwrap_or_else(|| "—".into()))
                                .collect(),
                            sources: vec![],
                        })
                        .collect();
                    Some(Presentation::Table {
                        title: label.into(),
                        summary,
                        columns,
                        rows,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(p) = presentation {
            answer = answer.with_presentation(p);
            if answer.presentation().is_err() {
                answer.presentation = None;
            }
        }
        answer
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: usize,
    pub at: String,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    Started { attempt: usize },
    Captured { attempt: usize, output: Capture },
    Failed { attempt: usize, error: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub version: u32,
    pub id: String,
    pub descriptor: Descriptor,
    pub binding: Binding,
    pub events: Vec<Event>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Projection {
    pub attempts: usize,
    pub pending: bool,
    pub output: Option<Capture>,
    pub output_attempt: usize,
    pub error: Option<String>,
}
impl Record {
    /// Replay has no I/O and never dispatches work. Unknown versions/invalid transitions fail closed.
    pub fn project(&self) -> Result<Projection, String> {
        self.descriptor.validate()?;
        if self.version != VERSION
            || !valid_id(&self.id)
            || !self.id.starts_with("n-")
            || self.binding != self.descriptor.binding()
        {
            return Err("Unsupported or inconsistent native task record.".into());
        }
        let mut p = Projection::default();
        for (seq, e) in self.events.iter().enumerate() {
            if e.seq != seq {
                return Err("Native task event sequence is invalid.".into());
            }
            match &e.kind {
                EventKind::Started { attempt } if !p.pending && *attempt == p.attempts + 1 => {
                    p.attempts = *attempt;
                    p.pending = true;
                    p.error = None;
                }
                EventKind::Captured { attempt, output } if p.pending && *attempt == p.attempts => {
                    if output.body.len() > CAPTURE_LIMIT
                        || output.hash != hash(output.body.as_bytes())
                    {
                        return Err(
                            "Captured output is incomplete or its hash does not match.".into()
                        );
                    }
                    p.output = Some(output.clone());
                    p.output_attempt = *attempt;
                    p.pending = false;
                }
                EventKind::Failed { attempt, error } if p.pending && *attempt == p.attempts => {
                    p.error = Some(error.clone());
                    p.pending = false;
                }
                _ => return Err("Native task has an invalid or repeated event transition.".into()),
            }
        }
        Ok(p)
    }
}

/// Projection into the existing workspace; native results are not agent answers or test verdicts.
#[derive(Debug, Clone)]
pub struct NativeView {
    pub media_type: String,
    pub answer: Option<Answer>,
    pub refresh: Option<Binding>,
    pub notice: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn unfamiliar_data_is_readable_and_never_executable() {
        let capture = Capture::new(
            "fixture".into(),
            "application/json".into(),
            json!({"kind":"check_report","actions":["delete"],"passed":true}).to_string(),
        )
        .unwrap();
        let a = capture.answer("External report");
        assert!(matches!(
            a.presentation().unwrap(),
            Some(Presentation::Facts { .. })
        ));
        assert!(a.text.contains("delete"));
        let large = Capture::new(
            "fixture".into(),
            "application/json".into(),
            json!([{"x":"", "y":1}]).to_string(),
        )
        .unwrap();
        assert!(large.answer("Empty field").presentation.is_none());
    }
    #[test]
    fn replay_keeps_success_after_failure_and_rejects_duplicate_results() {
        let descriptor = Descriptor {
            version: 1,
            id: "report".into(),
            label: "Report".into(),
            adapter: "file.read.v1".into(),
            input: json!({"path":"a.json"}),
        };
        let mut r = Record {
            version: 1,
            id: "n-test".into(),
            binding: descriptor.binding(),
            descriptor,
            events: vec![],
        };
        let output = Capture::new("a.json".into(), "application/json".into(), "[]".into()).unwrap();
        for kind in [
            EventKind::Started { attempt: 1 },
            EventKind::Captured {
                attempt: 1,
                output: output.clone(),
            },
            EventKind::Started { attempt: 2 },
            EventKind::Failed {
                attempt: 2,
                error: "File removed".into(),
            },
        ] {
            r.events.push(Event {
                seq: r.events.len(),
                at: "now".into(),
                kind,
            });
        }
        let p = r.project().unwrap();
        assert_eq!(p, r.project().unwrap());
        assert_eq!(p.output, Some(output.clone()));
        assert_eq!(p.output_attempt, 1);
        r.events.push(Event {
            seq: 4,
            at: "now".into(),
            kind: EventKind::Captured { attempt: 1, output },
        });
        assert!(r.project().is_err());
    }
}
