//! A requested runtime and model. Credentials and model discovery belong to the runtime.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    Claude,
    Pi,
    Copilot,
    Amp,
    Codex,
    Hermes,
}

impl AgentKind {
    pub const ALL: [Self; 6] = [
        Self::Claude,
        Self::Pi,
        Self::Copilot,
        Self::Amp,
        Self::Codex,
        Self::Hermes,
    ];
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "claude" => Ok(Self::Claude),
            "pi" => Ok(Self::Pi),
            "copilot" => Ok(Self::Copilot),
            "amp" => Ok(Self::Amp),
            "codex" => Ok(Self::Codex),
            "hermes" => Ok(Self::Hermes),
            _ => Err("Supported question agents: claude, pi, copilot, amp, codex, hermes"),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Pi => "Pi",
            Self::Copilot => "GitHub Copilot",
            Self::Amp => "Amp",
            Self::Codex => "Codex",
            Self::Hermes => "Hermes Agent",
        }
    }
    pub fn scope(self) -> &'static str {
        match self {
            Self::Claude => "Web tools · No project files supplied",
            Self::Codex => {
                "Read-only local sandbox · Runtime MCP rules · No project files supplied"
            }
            Self::Hermes => "Hermes tools and permission rules · No project files supplied",
            Self::Pi | Self::Copilot | Self::Amp => {
                "No tools · No live sources · No project files supplied"
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSelection {
    #[serde(default)]
    pub kind: AgentKind,
    /// A runtime model ID/pattern, never a shell fragment or a credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Amp mode; Amp controls the model routing behind it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Whether the runtime's own tools are switched on for this question;
    /// a saved question without this field ran with no tools.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tools: bool,
}

impl AgentSelection {
    /// Defaults reflect the capabilities of each CLI adapter.
    pub fn starting(kind: AgentKind) -> Self {
        Self {
            kind,
            tools: matches!(kind, AgentKind::Amp | AgentKind::Codex | AgentKind::Hermes),
            ..Default::default()
        }
    }
    /// What the agent can reach in a question, in the words shown beside the request.
    pub fn scope(&self) -> String {
        if self.kind == AgentKind::Amp && self.tools {
            "Amp's own tools · Amp's permission rules · No project files supplied".into()
        } else {
            self.kind.scope().into()
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.tools
            && !matches!(
                self.kind,
                AgentKind::Amp | AgentKind::Codex | AgentKind::Hermes
            )
        {
            return Err("Runtime tools are supported for Amp, Codex and Hermes.");
        }
        if !self.tools && matches!(self.kind, AgentKind::Codex | AgentKind::Hermes) {
            return Err(
                "Codex and Hermes use runtime tools. This adapter does not offer a no-tools mode.",
            );
        }
        if self.kind == AgentKind::Amp && self.model.is_some() {
            return Err(
                "Amp chooses its models. Use --agent-mode or the Mode field instead of --model.",
            );
        }
        if self.kind != AgentKind::Amp && self.mode.is_some() {
            return Err("Agent mode is supported only for Amp. Use a model ID for this runtime.");
        }
        if let Some(mode) = &self.mode
            && (mode.is_empty()
                || mode.len() > 80
                || mode.starts_with('-')
                || !mode
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-:/".contains(c)))
        {
            return Err(
                "Use an Amp mode name (up to 80 ASCII characters), or leave it blank for the runtime default.",
            );
        }
        if let Some(model) = &self.model
            && (model.is_empty()
                || model.len() > 200
                || model.starts_with('-')
                || model.chars().any(|c| c.is_control() || c.is_whitespace()))
        {
            return Err(
                "Use a model ID without spaces (up to 200 bytes), or leave it blank for the runtime default.",
            );
        }
        Ok(())
    }
    pub fn label(&self) -> String {
        if self.kind == AgentKind::Amp {
            return format!(
                "Amp · {}",
                self.mode
                    .as_ref()
                    .map(|m| format!("{m} mode"))
                    .unwrap_or_else(|| "runtime default mode".into())
            );
        }
        format!(
            "{} · {}",
            self.kind.label(),
            self.model.as_deref().unwrap_or("runtime default")
        )
    }
}

#[derive(Debug, Clone)]
pub struct AgentAvailability {
    pub kind: AgentKind,
    /// Finding a binary does not establish authentication or model availability.
    pub unavailable: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_selection_is_bounded_and_legacy_selection_defaults_to_claude() {
        assert_eq!(
            serde_json::from_str::<AgentSelection>("{}").unwrap(),
            AgentSelection::default()
        );
        for name in ["", "--help", "a\nb", "model name"] {
            assert!(
                AgentSelection {
                    model: Some(name.into()),
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            AgentSelection {
                kind: AgentKind::Pi,
                model: Some("openrouter/provider/model:high".into()),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }
    #[test]
    fn amp_modes_are_distinct_from_model_ids() {
        let valid = AgentSelection {
            kind: AgentKind::Amp,
            mode: Some("example-mode".into()),
            ..Default::default()
        };
        assert!(valid.validate().is_ok());
        let mut wrong = valid.clone();
        wrong.model = Some("model-id".into());
        assert!(wrong.validate().is_err());
        wrong = valid.clone();
        wrong.kind = AgentKind::Copilot;
        assert!(wrong.validate().is_err());
        for mode in ["", "--help", "mode with spaces", "a\nb"] {
            let mut bad = valid.clone();
            bad.mode = Some(mode.into());
            assert!(bad.validate().is_err());
        }
        for name in ["claude", "pi", "copilot", "amp", "codex", "hermes"] {
            assert!(AgentKind::parse(name).is_ok());
        }
    }
}
