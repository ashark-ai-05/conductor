//! Durable questions and the common task workspace. A returned answer is not a check verdict.
use crate::view::Waiting;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Working,
    Answered,
    NeedsInput,
    Finished,
    Stopped,
    Cancelled,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Answered => "Answered",
            Self::NeedsInput => "Needs you",
            Self::Finished => "Finished",
            Self::Stopped => "Stopped",
            Self::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub supports: String,
    /// Reported by the answering agent, never substituted with retrieval time.
    #[serde(default)]
    pub observed_at: Option<String>,
    /// Filled only by matching a recorded successful fetch of this URL.
    #[serde(default)]
    pub retrieved_at: Option<String>,
    #[serde(default)]
    pub captured: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub text: String,
    #[serde(default)]
    pub sources: Vec<Citation>,
    #[serde(default)]
    pub needs_input: bool,
    /// Optional typed clarification; invalid shapes fall back to the original prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_request: Option<serde_json::Value>,
    /// Optional, untrusted display data. Invalid/unknown shapes never prevent reading text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<serde_json::Value>,
    /// A bounded, flat composition. Invalid sections leave the original answer readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerSection {
    pub id: String,
    pub title: String,
    /// Complete Markdown fallback for this section, including its caveats.
    pub text: String,
    pub sources: Vec<usize>,
    pub presentation: Option<Presentation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Presentation {
    Series {
        title: String,
        summary: String,
        unit: String,
        points: Vec<SeriesPoint>,
    },
    Facts {
        title: String,
        summary: String,
        facts: Vec<Fact>,
    },
    Table {
        title: String,
        summary: String,
        columns: Vec<String>,
        rows: Vec<ResultRow>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeriesPoint {
    pub label: String,
    pub value: f64,
    pub detail: String,
    pub sources: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub label: String,
    pub value: String,
    /// One-based references into this answer's citations, never another turn's.
    pub sources: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultRow {
    pub cells: Vec<String>,
    pub sources: Vec<usize>,
}

impl Answer {
    pub fn with_sections(mut self, sections: Vec<AnswerSection>) -> Self {
        self.sections = serde_json::to_value(sections).ok();
        self
    }
    pub fn sections(&self) -> Result<Vec<AnswerSection>, &'static str> {
        let Some(value) = self.sections.as_ref().filter(|v| !v.is_null()) else {
            return Ok(vec![]);
        };
        let error = "Invalid answer sections. Showing the full answer.";
        let sections: Vec<AnswerSection> =
            serde_json::from_value(value.clone()).map_err(|_| error)?;
        if sections.len() > 6 {
            return Err(error);
        }
        let mut ids = std::collections::HashSet::new();
        let mut length = 0;
        for section in &sections {
            length += section.text.len();
            if section.id.is_empty()
                || section.id.len() > 64
                || !section
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
                || !ids.insert(&section.id)
                || section.title.trim().is_empty()
                || section.title.chars().count() > 160
                || section.title.chars().any(char::is_control)
                || section.text.trim().is_empty()
                || length > 64_000
                || section.sources.len() > 20
                || section
                    .sources
                    .iter()
                    .any(|n| *n == 0 || *n > self.sources.len())
            {
                return Err(error);
            }
            if let Some(view) = &section.presentation {
                // Reuse the same bounds and citation checks as a standalone widget.
                let answer = Self {
                    sources: self.sources.clone(),
                    ..Self::default()
                }
                .with_presentation(view.clone());
                answer.presentation().map_err(|_| error)?;
            }
        }
        Ok(sections)
    }
    pub fn with_presentation(mut self, view: Presentation) -> Self {
        self.presentation = serde_json::to_value(view).ok();
        self
    }
    pub fn presentation(&self) -> Result<Option<Presentation>, &'static str> {
        let Some(value) = self.presentation.as_ref().filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let view: Presentation = serde_json::from_value(value.clone())
            .map_err(|_| "Unsupported result layout. Showing the full answer.")?;
        let text = |s: &str, max: usize| {
            !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(|c| c.is_control())
        };
        let refs =
            |r: &[usize]| r.len() <= 20 && r.iter().all(|n| *n > 0 && *n <= self.sources.len());
        let valid = match &view {
            Presentation::Series {
                title,
                summary,
                unit,
                points,
            } => {
                text(title, 160)
                    && text(summary, 1200)
                    && text(unit, 40)
                    && (2..=200).contains(&points.len())
                    && points.iter().all(|p| {
                        text(&p.label, 80)
                            && text(&p.detail, 1200)
                            && refs(&p.sources)
                            && p.value.is_finite()
                            && p.value.abs() <= 1e12
                    })
            }
            Presentation::Facts {
                title,
                summary,
                facts,
            } => {
                text(title, 160)
                    && text(summary, 1200)
                    && !facts.is_empty()
                    && facts.len() <= 30
                    && facts
                        .iter()
                        .all(|f| text(&f.label, 160) && text(&f.value, 2000) && refs(&f.sources))
            }
            Presentation::Table {
                title,
                summary,
                columns,
                rows,
            } => {
                text(title, 160)
                    && text(summary, 1200)
                    && (2..=8).contains(&columns.len())
                    && columns.iter().all(|c| text(c, 80))
                    && !rows.is_empty()
                    && rows.len() <= 200
                    && rows.iter().all(|r| {
                        r.cells.len() == columns.len()
                            && r.cells.iter().all(|c| text(c, 2000))
                            && refs(&r.sources)
                    })
            }
        };
        if valid {
            Ok(Some(view))
        } else {
            Err("Invalid result data or source links. Showing the full answer.")
        }
    }
}

/// A view of engine-parsed test cases, never accepted from a question's agent output.
#[derive(Debug, Clone, Default)]
pub struct TestResults {
    pub cases: Vec<TestResult>,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TestResult {
    pub id: String,
    pub name: String,
    pub outcome: String,
    pub stage: String,
    pub attempt: usize,
    pub evidence: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub at: String,
    pub actor: String,
    pub title: String,
    pub detail: String,
    pub failed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_response: Option<crate::interaction::InputResponse>,
    pub question: String,
    pub started_at: String,
    pub state: State,
    pub answer: Option<Answer>,
    pub activity: Vec<Activity>,
    pub error: Option<String>,
    pub tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// Fixed for this task; follow-ups cannot silently switch runtimes.
    #[serde(default)]
    pub agent: crate::agent::AgentSelection,
    pub id: String,
    pub title: String,
    pub turns: Vec<Turn>,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub state: State,
}

/// A saved attempt's patch, never a diff of whatever happens to be in the checkout now.
#[derive(Debug, Clone)]
pub struct TaskChange {
    pub stage: String,
    pub attempt: usize,
    pub path: String,
    pub patch: String,
}

#[derive(Debug, Clone)]
pub struct TaskDetail {
    pub receipt: Option<crate::Receipt>,
    pub native: Option<crate::capability::NativeView>,
    pub summary: TaskSummary,
    pub context: String,
    pub body: String,
    pub evidence: Vec<(String, String)>,
    pub activity: Vec<Activity>,
    pub waiting: Option<Waiting>,
    pub question: Option<Question>,
    pub tests: Option<TestResults>,
    pub changes: Vec<TaskChange>,
}

/// Only web links are actionable. Terminal escape sequences and custom URI schemes are not.
pub fn web_url(url: &str) -> bool {
    let host = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    host.is_some_and(|s| {
        !s.is_empty() && !s.starts_with('/') && !s.starts_with('?') && !s.starts_with('#')
    }) && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_and_unknown_views_keep_the_complete_answer() {
        for presentation in [
            None,
            Some(json!({"kind":"execute","command":"rm"})),
            Some(json!("bad")),
        ] {
            let mut data =
                json!({"text":"The answer and its caveats.","sources":[],"needs_input":false});
            if let Some(value) = presentation {
                data["presentation"] = value;
            }
            let answer: Answer = serde_json::from_value(data).unwrap();
            assert_eq!(answer.text, "The answer and its caveats.");
            assert!(answer.presentation().ok().flatten().is_none());
            assert_eq!(
                serde_json::from_str::<Answer>(&serde_json::to_string(&answer).unwrap()).unwrap(),
                answer
            );
        }
    }

    #[test]
    fn tables_require_matching_cells_and_existing_citations() {
        let mut answer = Answer {
            text: "Complete text".into(),
            sources: vec![Citation::default()],
            ..Answer::default()
        };
        let good = json!({"kind":"table","title":"Cities","summary":"Observation and caveats","columns":["City","Temperature"],"rows":[{"cells":["LA","24 C"],"sources":[1]}]});
        answer.presentation = Some(good.clone());
        assert!(matches!(
            answer.presentation().unwrap(),
            Some(Presentation::Table { .. })
        ));
        for (field, bad) in [
            ("cells", json!(["LA"])),
            ("sources", json!([0])),
            ("sources", json!([2])),
            ("sources", json!([-1])),
        ] {
            let mut value = good.clone();
            value["rows"][0][field] = bad;
            answer.presentation = Some(value);
            assert!(answer.presentation().is_err());
        }
        answer.presentation = Some(
            json!({"kind":"facts","title":"Weather","summary":"Demo","facts":[{"label":"Temperature","value":"\u{1b}[31m24","sources":[]}]}),
        );
        assert!(answer.presentation().is_err());
    }
}

#[cfg(test)]
mod series_tests {
    use super::*;
    #[test]
    fn series_cannot_smuggle_invalid_values_references_or_actions() {
        let valid = serde_json::json!({"kind":"series","title":"Load","summary":"Captured samples","unit":"ms","points":[{"label":"one","value":1.5,"detail":"first","sources":[]},{"label":"two","value":2.0,"detail":"second","sources":[]}]});
        let mut answer = Answer {
            text: "Readable fallback".into(),
            presentation: Some(valid.clone()),
            ..Default::default()
        };
        assert!(answer.presentation().unwrap().is_some());
        for change in ["source", "range", "action", "empty"] {
            let mut value = valid.clone();
            match change {
                "source" => value["points"][0]["sources"] = serde_json::json!([1]),
                "range" => value["points"][0]["value"] = serde_json::json!(1e20),
                "action" => value["command"] = serde_json::json!("run something"),
                _ => value["points"] = serde_json::json!([]),
            }
            answer.presentation = Some(value);
            assert!(answer.presentation().is_err());
            assert_eq!(answer.text, "Readable fallback");
        }
    }
}

#[cfg(test)]
mod section_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn composition_is_flat_bounded_and_uses_this_answers_sources() {
        let section = json!({"id":"summary","title":"Summary","text":"Readable [1]","sources":[1],"presentation":null});
        let mut answer = Answer {
            text: "Original".into(),
            sources: vec![Citation::default()],
            sections: Some(json!([section.clone()])),
            ..Default::default()
        };
        assert_eq!(answer.sections().unwrap().len(), 1);
        for bad in [
            json!([section.clone(), section.clone()]),
            json!([{"id":"bad","title":"Bad","text":"Text","sources":[2],"presentation":null}]),
            json!([{"id":"bad","title":"Bad","text":"Text","sources":[],"presentation":{"kind":"execute","command":"ignored"}}]),
            json!([{"id":"bad","title":"Bad","text":"Text","sources":[],"presentation":null,"action":"execute"}]),
            json!([{"id":"bad","title":"Bad","text":"x".repeat(64_001),"sources":[],"presentation":null}]),
        ] {
            answer.sections = Some(bad);
            assert!(answer.sections().is_err());
            assert_eq!(answer.text, "Original");
        }
        let legacy: Answer = serde_json::from_value(json!({"text":"Old answer"})).unwrap();
        assert!(legacy.sections().unwrap().is_empty());
    }
}
