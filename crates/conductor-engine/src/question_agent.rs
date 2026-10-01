//! The Pi question adapter. Uses one-shot JSON events and the existing bounded runner.
//! Tools and discovered resources are disabled; this is not a coding/permission adapter.
use conductor_model::agent::{AgentAvailability, AgentKind, AgentSelection};
use serde_json::Value;
use std::path::Path;

pub fn program(kind: AgentKind) -> String {
    let (key, fallback) = match kind {
        AgentKind::Claude => ("CONDUCTOR_CLAUDE_BIN", "claude"),
        AgentKind::Pi => ("CONDUCTOR_PI_BIN", "pi"),
        AgentKind::Copilot => ("CONDUCTOR_COPILOT_BIN", "copilot"),
        AgentKind::Amp => ("CONDUCTOR_AMP_BIN", "amp"),
    };
    std::env::var(key).unwrap_or_else(|_| fallback.into())
}

pub fn available() -> Vec<AgentAvailability> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |path: &Path| {
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    AgentKind::ALL
        .into_iter()
        .map(|kind| {
            let bin = program(kind);
            let found = if bin.contains('/') {
                executable(Path::new(&bin))
            } else {
                std::env::var_os("PATH").is_some_and(|p| {
                    std::env::split_paths(&p).any(|dir| executable(&dir.join(&bin)))
                })
            };
            AgentAvailability {
                kind,
                unavailable: (!found).then(|| {
                    format!(
                        "{} CLI not found. Install and authenticate it first.",
                        kind.label()
                    )
                }),
            }
        })
        .collect()
}

pub fn pi_argv(selection: &AgentSelection, prompt: String) -> Vec<String> {
    let mut argv: Vec<String> = [
        program(AgentKind::Pi),
        "--mode".into(),
        "json".into(),
        "--print".into(),
        "--no-session".into(),
        "--no-tools".into(),
        "--no-extensions".into(),
        "--no-skills".into(),
        "--no-prompt-templates".into(),
        "--no-themes".into(),
        "--no-context-files".into(),
    ]
    .into();
    if let Some(model) = &selection.model {
        argv.extend(["--model".into(), model.clone()]);
    }
    argv.extend(["--".into(), prompt]);
    argv
}

#[derive(Default)]
pub(crate) struct PiStream {
    pub text: String,
    pub error: Option<String>,
    pub tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub finished: bool,
    pub model: Option<String>,
    tool_violation: bool,
}

impl PiStream {
    /// Accept only authoritative assistant messages. Never store thinking deltas.
    pub fn feed(&mut self, line: &str) -> bool {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return false;
        };
        match event["type"].as_str() {
            Some("message_end") if event["message"]["role"] == "assistant" => {
                let m = &event["message"];
                self.text = m["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| c["type"] == "text")
                    .filter_map(|c| c["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.error = if self.tool_violation {
                    Some("Pi emitted a tool call in a no-tools question session.".into())
                } else if matches!(m["stopReason"].as_str(), Some("error" | "aborted")) {
                    Some(
                        m["errorMessage"]
                            .as_str()
                            .unwrap_or("Pi stopped before returning an answer.")
                            .into(),
                    )
                } else if m["stopReason"] != "stop" {
                    Some(format!(
                        "Pi response did not finish normally: {}",
                        m["stopReason"].as_str().unwrap_or("missing stop reason")
                    ))
                } else {
                    None
                };
                if let Some(tokens) = m["usage"]["totalTokens"].as_u64() {
                    self.tokens = Some(self.tokens.unwrap_or(0).saturating_add(tokens));
                }
                if let Some(cost) = m["usage"]["cost"]["total"]
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= 0.0)
                {
                    self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost);
                }
                self.model = m["model"]
                    .as_str()
                    .map(|model| match m["provider"].as_str() {
                        Some(provider) => format!("{provider}/{model}"),
                        None => model.into(),
                    });
                true
            }
            // JSON mode also exits after automatic work; the caller requires a clean exit.
            Some("agent_end") if !event["willRetry"].as_bool().unwrap_or(false) => {
                self.finished = true;
                false
            }
            Some("agent_start") => {
                self.finished = false;
                false
            }
            Some("tool_execution_start") => {
                self.tool_violation = true;
                self.error = Some("Pi emitted a tool call in a no-tools question session.".into());
                false
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn pi_stream_requires_completion_and_ignores_thinking_and_tool_results() {
        let mut s = PiStream::default();
        s.feed(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"private"}}).to_string());
        s.feed(&json!({"type":"message_end","message":{"role":"toolResult","content":[{"type":"text","text":"fake answer"}]}}).to_string());
        assert!(s.text.is_empty());
        assert!(!s.finished);
        s.feed(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"private"},{"type":"text","text":"Answer"}],"stopReason":"stop","usage":{"totalTokens":42}}}).to_string());
        assert_eq!(s.text, "Answer");
        assert_eq!(s.tokens, Some(42));
        s.feed(r#"{"type":"agent_end","willRetry":true}"#);
        assert!(!s.finished);
        s.feed(r#"{"type":"agent_end","willRetry":false}"#);
        assert!(s.finished);
    }
}
