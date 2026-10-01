//! Copilot and Amp question adapters. Presentation is data; these sessions expose no tools.
use super::question_agent::program;
use conductor_model::agent::{AgentKind, AgentSelection};
use serde_json::{Value, json};
use std::{collections::HashSet, io, path::Path};

pub fn argv(selection: &AgentSelection, prompt: String, scratch: &Path) -> io::Result<Vec<String>> {
    let mut args = vec![program(selection.kind)];
    match selection.kind {
        AgentKind::Copilot => {
            args.extend(
                [
                    "--output-format",
                    "json",
                    "--available-tools",
                    "--no-custom-instructions",
                    "--disable-builtin-mcps",
                    "--no-ask-user",
                    "--no-auto-update",
                    "--no-remote-export",
                    "--log-level",
                    "none",
                ]
                .map(str::to_owned),
            );
            if let Some(model) = &selection.model {
                args.extend(["--model".into(), model.clone()]);
            }
            args.extend(["-p".into(), prompt]);
        }
        AgentKind::Amp => {
            // Only this unique scratch workspace is configured. Keep the user's authentication,
            // endpoint and corporate proxy configuration in place; never rewrite user settings.
            std::fs::create_dir(scratch.join(".amp"))?;
            std::fs::write(
                scratch.join(".amp/settings.json"),
                json!({
                    "amp.tools.disable": ["*"],
                    "amp.mcpPermissions": [
                        {"matches":{"command":"*"},"action":"reject"},
                        {"matches":{"url":"*"},"action":"reject"}
                    ],
                    "amp.updates.mode":"disabled"
                })
                .to_string(),
            )?;
            args.extend(
                [
                    "--stream-json",
                    "--no-ide",
                    "--no-notifications",
                    "--no-color",
                    "--visibility",
                    "private",
                ]
                .map(str::to_owned),
            );
            if let Some(mode) = &selection.mode {
                args.extend(["--mode".into(), mode.clone()]);
            }
            args.extend(["--execute".into(), prompt]);
        }
        _ => return Err(io::Error::other("Unsupported question CLI")),
    }
    Ok(args)
}

pub fn environment(kind: AgentKind) -> Vec<(String, String)> {
    match kind {
        AgentKind::Copilot => vec![
            ("COPILOT_ALLOW_ALL".into(), "false".into()),
            ("COPILOT_AUTO_UPDATE".into(), "false".into()),
            (
                "GITHUB_COPILOT_PROMPT_MODE_EXTENSIONS".into(),
                "false".into(),
            ),
        ],
        AgentKind::Amp => vec![("AMP_SKIP_UPDATE_CHECK".into(), "1".into())],
        _ => vec![],
    }
}

pub struct Update {
    pub title: &'static str,
    pub detail: String,
    pub failed: bool,
}

#[derive(Default)]
pub struct Stream {
    pub text: String,
    pub error: Option<String>,
    pub tokens: Option<u64>,
    pub finished: bool,
    pub model: Option<String>,
    pub violation: bool,
    seen: HashSet<String>,
}
impl Stream {
    pub fn feed(&mut self, kind: AgentKind, line: &str) -> Option<Update> {
        let event: Value = serde_json::from_str(line).ok()?;
        if let Some(id) = event["id"].as_str()
            && !self.seen.insert(id.into())
        {
            return None;
        }
        // Only main-agent output belongs in the answer; nested output is not a substitute.
        if event["agentId"].as_str().is_some_and(|id| id != "main")
            || !event["parent_tool_use_id"].is_null()
            || !event["data"]["parentToolCallId"].is_null()
        {
            return None;
        }
        match kind {
            AgentKind::Copilot => self.copilot(&event),
            AgentKind::Amp => self.amp(&event),
            _ => None,
        }
    }
    fn update(&self, title: &'static str, detail: String) -> Option<Update> {
        Some(Update {
            title,
            detail,
            failed: self.error.is_some(),
        })
    }
    fn fail(&mut self, text: String) -> Option<Update> {
        self.error = Some(text.clone());
        self.finished = false;
        self.update("Agent stopped", text)
    }
    fn unexpected_tools(&mut self) -> Option<Update> {
        self.violation = true;
        self.fail("The CLI exposed or requested tools in a no-tools question session. Conductor stopped the process.".into())
    }
    fn received(&self) -> Option<Update> {
        self.update(
            "Response received",
            format!(
                "Runtime reported: {}\nNo live sources captured.",
                self.model.as_deref().unwrap_or("Unavailable")
            ),
        )
    }
    fn copilot(&mut self, e: &Value) -> Option<Update> {
        let d = &e["data"];
        match e["type"].as_str()? {
            "assistant.turn_start" => {
                self.finished = false;
                self.text.clear();
                None
            }
            "assistant.message" => {
                if d["toolRequests"].as_array().is_some_and(|a| !a.is_empty()) {
                    return self.unexpected_tools();
                }
                if matches!(
                    d["phase"].as_str(),
                    Some("analysis" | "reasoning" | "thinking")
                ) {
                    return None;
                }
                let content = d["content"].as_str()?;
                // Authoritative message chunks, never deltas or reasoning fields.
                if d["chunkIndex"].as_u64().is_some_and(|i| i > 0) {
                    self.text.push_str(content);
                } else {
                    self.text = content.into();
                }
                if let Some(model) = d["model"].as_str() {
                    self.model = Some(model.into());
                }
                self.received()
            }
            "assistant.usage" => {
                self.model = d["model"].as_str().map(str::to_owned).or(self.model.take());
                if d["availableToolCount"].as_u64().is_some_and(|n| n > 0) {
                    return self.unexpected_tools();
                }
                if let (Some(input), Some(output)) =
                    (d["inputTokens"].as_u64(), d["outputTokens"].as_u64())
                {
                    self.tokens = Some(
                        self.tokens
                            .unwrap_or(0)
                            .saturating_add(input)
                            .saturating_add(output),
                    );
                }
                // `cost` is a billing multiplier, NOT dollars. Do not populate cost_usd.
                if d["contentFilterTriggered"] == true
                    || d["finishReason"].as_str().is_some_and(|r| r != "stop")
                {
                    return self.fail(format!(
                        "Copilot did not finish normally: {}",
                        d["finishReason"].as_str().unwrap_or("content filter")
                    ));
                }
                None
            }
            "assistant.turn_end" => {
                self.finished = !self.text.is_empty() && self.error.is_none();
                None
            }
            "session.idle" if d["aborted"] == true => {
                self.fail("Copilot aborted the response.".into())
            }
            "session.error" => self.fail(
                d["message"]
                    .as_str()
                    .unwrap_or("Copilot returned an error.")
                    .into(),
            ),
            "session.shutdown" if d["shutdownType"] == "error" => self.fail(
                d["errorReason"]
                    .as_str()
                    .unwrap_or("Copilot stopped with an error.")
                    .into(),
            ),
            "abort" => self.fail("Copilot aborted the response.".into()),
            "tool.execution_start" => self.unexpected_tools(),
            "session.warning" => self.update("CLI warning", d["message"].as_str()?.into()),
            _ => None,
        }
    }
    fn amp(&mut self, e: &Value) -> Option<Update> {
        match e["type"].as_str()? {
            "system" if e["subtype"] == "init" => {
                if e["tools"].as_array().is_some_and(|a| !a.is_empty()) {
                    return self.unexpected_tools();
                }
                self.model = e["agent_mode"]
                    .as_str()
                    .map(|mode| format!("Amp mode {mode}"));
                self.update(
                    "Agent connected",
                    "Amp question session started; no tools enabled.".into(),
                )
            }
            "system"
                if e["subtype"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("error")) =>
            {
                self.fail(
                    e["error"]
                        .as_str()
                        .unwrap_or("Amp returned an error.")
                        .into(),
                )
            }
            "assistant" => {
                let m = &e["message"];
                let content = m["content"].as_array()?;
                if content.iter().any(|b| b["type"] == "tool_use") {
                    return self.unexpected_tools();
                }
                self.text = content
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Some(n) = amp_tokens(&m["usage"]) {
                    self.tokens = Some(self.tokens.unwrap_or(0).saturating_add(n));
                }
                if let Some(reason) = m["stop_reason"].as_str()
                    && !matches!(reason, "end_turn" | "stop_sequence")
                {
                    return self.fail(format!("Amp did not finish normally: {reason}"));
                }
                self.received()
            }
            "result" => {
                if let Some(n) = amp_tokens(&e["usage"]) {
                    self.tokens = Some(n);
                }
                if e["is_error"] != false || e["subtype"] != "success" {
                    return self.fail(
                        e["error"]
                            .as_str()
                            .or(e["result"].as_str())
                            .unwrap_or("Amp did not complete the response.")
                            .into(),
                    );
                }
                if let Some(result) = e["result"].as_str() {
                    self.text = result.into();
                }
                self.finished = self.error.is_none() && !self.text.is_empty();
                self.received()
            }
            _ => None,
        }
    }
}
fn amp_tokens(usage: &Value) -> Option<u64> {
    let input = usage["input_tokens"].as_u64()?;
    let output = usage["output_tokens"].as_u64()?;
    Some(
        input
            .saturating_add(output)
            .saturating_add(usage["cache_creation_input_tokens"].as_u64().unwrap_or(0))
            .saturating_add(usage["cache_read_input_tokens"].as_u64().unwrap_or(0)),
    )
}
