//! Input requests are data. They can continue a question, never approve a tool or operation.
use crate::task::{Answer, Question, State};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    SingleChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputRequest {
    pub kind: InputKind,
    pub question: String,
    pub explanation: String,
    pub options: Vec<Choice>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputBinding {
    pub task: String,
    /// One-based turn containing the request.
    pub turn: usize,
    pub fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputResponse {
    pub binding: InputBinding,
    pub option: Option<String>,
    pub text: String,
    pub context: String,
}

impl Answer {
    pub fn with_input_request(mut self, request: InputRequest) -> Self {
        self.needs_input = true;
        self.input_request = serde_json::to_value(request).ok();
        self
    }
    /// Unknown requests retain the complete prose and the ordinary reply composer.
    pub fn input_request(&self) -> Result<Option<InputRequest>, &'static str> {
        let Some(value) = self.input_request.as_ref().filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let request: InputRequest = serde_json::from_value(value.clone())
            .map_err(|_| "Unsupported clarification. Press f to write a reply.")?;
        let text =
            |s: &str, max: usize| s.chars().count() <= max && !s.chars().any(char::is_control);
        let id = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        };
        let mut ids = std::collections::BTreeSet::new();
        let mut labels = std::collections::BTreeSet::new();
        if request.question.trim().is_empty()
            || !text(&request.question, 240)
            || !text(&request.explanation, 600)
            || !(2..=8).contains(&request.options.len())
            || !request.options.iter().all(|c| {
                id(&c.id)
                    && ids.insert(&c.id)
                    && !c.label.trim().is_empty()
                    && text(&c.label, 120)
                    && labels.insert(c.label.trim().to_lowercase())
                    && text(&c.description, 300)
            })
        {
            return Err("Invalid clarification options. Press f to write a reply.");
        }
        Ok(Some(request))
    }
}
impl Question {
    pub fn input_at(&self, index: usize) -> Option<(InputBinding, InputRequest)> {
        let request = self
            .turns
            .get(index)?
            .answer
            .as_ref()?
            .input_request()
            .ok()??;
        let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&request).ok()?));
        Some((
            InputBinding {
                task: self.id.clone(),
                turn: index + 1,
                fingerprint,
            },
            request,
        ))
    }
    pub fn pending_input(&self) -> Option<(InputBinding, InputRequest)> {
        if self.turns.last()?.state != State::NeedsInput {
            return None;
        }
        self.input_at(self.turns.len().checked_sub(1)?)
    }
    /// Validate against the latest saved state while the caller holds the submission lock.
    pub fn resolve_input(&self, response: &InputResponse) -> Result<String, &'static str> {
        let (binding, request) = self.pending_input().ok_or(
            "This clarification is no longer waiting for an answer. Open the latest turn.",
        )?;
        if response.binding != binding {
            return Err(
                "This clarification has changed or was already answered. Open the latest turn.",
            );
        }
        let text = response.text.trim();
        let context = response.context.trim();
        if [text, context]
            .iter()
            .any(|s| s.len() > 4000 || s.chars().any(|c| c.is_control() && c != '\n' && c != '\t'))
        {
            return Err("Keep each response field under 4,000 bytes of plain text.");
        }
        let value = match response.option.as_deref() {
            Some(id) if text.is_empty() => request
                .options
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.label.as_str())
                .ok_or("That option is not part of this clarification.")?,
            None if !text.is_empty() => text,
            _ => return Err("Choose one option or write your own answer."),
        };
        let mut out = format!("Clarification: {}\nAnswer: {value}", request.question);
        if !context.is_empty() {
            out.push_str(&format!("\nAdditional context: {context}"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn answer() -> Answer {
        Answer {
            text: "Choose a subject".into(),
            needs_input: true,
            input_request: Some(
                json!({"kind":"single_choice","question":"Subject?","explanation":"","options":[{"id":"a","label":"Engineering","description":""},{"id":"b","label":"Security","description":""}]}),
            ),
            ..Answer::default()
        }
    }
    #[test]
    fn invalid_or_unknown_requests_never_turn_data_into_controls() {
        assert!(answer().input_request().unwrap().is_some());
        for value in [
            json!({"kind":"approval"}),
            json!("question"),
            json!({"kind":"single_choice","question":"?","explanation":"","options":[]}),
        ] {
            let mut a = answer();
            a.input_request = Some(value);
            assert!(a.input_request().is_err());
            assert_eq!(a.text, "Choose a subject");
        }
        for (field, value) in [
            ("id", "a"),
            ("label", "engineering"),
            ("description", "escape\u{1b}"),
        ] {
            let mut a = answer();
            a.input_request.as_mut().unwrap()["options"][1][field] = json!(value);
            assert!(a.input_request().is_err());
        }
        let mut a = answer();
        a.input_request.as_mut().unwrap()["command"] = json!("execute");
        assert!(a.input_request().is_err());
        let old: Answer =
            serde_json::from_value(json!({"text":"old answer","needs_input":true,"sources":[]}))
                .unwrap();
        assert!(old.input_request().unwrap().is_none());
    }
    #[test]
    fn replies_are_bound_to_the_exact_request_and_accept_only_one_answer() {
        let turn = crate::task::Turn {
            input_response: None,
            question: "study".into(),
            started_at: "now".into(),
            state: State::NeedsInput,
            answer: Some(answer()),
            activity: vec![],
            error: None,
            tokens: None,
            cost_usd: None,
        };
        let mut q = Question {
            agent: Default::default(),
            id: "q-test".into(),
            title: "Study".into(),
            turns: vec![turn],
            pid: None,
        };
        let mut reply = InputResponse {
            binding: q.pending_input().unwrap().0,
            option: Some("a".into()),
            text: String::new(),
            context: "Interviews".into(),
        };
        assert!(q.resolve_input(&reply).unwrap().contains("Engineering"));
        reply.text = "another answer".into();
        assert!(q.resolve_input(&reply).is_err());
        reply.option = None;
        assert!(q.resolve_input(&reply).unwrap().contains("another answer"));
        reply.text.clear();
        assert!(q.resolve_input(&reply).is_err());
        reply.option = Some("a".into());
        let original = q.clone();
        q.id = "q-other".into();
        assert!(q.resolve_input(&reply).is_err());
        q = original.clone();
        q.turns[0]
            .answer
            .as_mut()
            .unwrap()
            .input_request
            .as_mut()
            .unwrap()["options"][0]["label"] = json!("Changed meaning");
        assert!(q.resolve_input(&reply).is_err());
        q = original.clone();
        q.turns.push(q.turns[0].clone());
        assert!(q.resolve_input(&reply).is_err());
        q = original;
        q.turns[0].state = State::Answered;
        assert!(q.resolve_input(&reply).is_err());
    }
}
