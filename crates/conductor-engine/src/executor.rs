//! Executors drive an agent through one attempt at a stage.
//!
//! Whatever an executor reports about the agent is labelled by where it came from: tool calls
//! read from the agent's own stream are `observed`, token counts are `measured`, and anything
//! it can't see is left out rather than guessed.

use conductor_checks::runner::{self, Ended};
use conductor_model::workflow::{Agent, PermissionMode};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Brief {
    pub stage: String,
    pub prompt: String,
    pub cwd: PathBuf,
    pub agent: Agent,
    /// Continue this agent session instead of starting a new one (the in-context rung).
    pub resume: Option<String>,
    pub timeout: Duration,
    pub run_id: String,
    pub attempt: usize,
    /// Extra environment for the agent, such as the pane-control grant in a headless run.
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolCall {
    pub tool: String,
    /// The file or command the call was about, when there is one.
    pub target: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: Option<f64>,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentRun {
    /// The agent finished its turn without an error.
    pub finished: bool,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub turns: Option<u64>,
    pub tool_calls: Vec<ToolCall>,
    /// `None` when the agent exposes no usage: the receipt says `unavailable`.
    pub usage: Option<Usage>,
    pub final_text: String,
    pub reason: Option<String>,
    pub duration_ms: u64,
    /// herdr's readings (pane opened, agent idle, waiting for you): recorded as `inferred`.
    pub inferred: Vec<String>,
}

pub trait Executor: Send + Sync {
    fn name(&self) -> &'static str;
    fn run(&self, brief: &Brief) -> AgentRun;
}

/// Picks the executor for an agent kind. Only headless executors exist so far; the herdr
/// executor will wrap the same agents in panes.
pub fn for_agent(kind: &str) -> Result<Box<dyn Executor>, String> {
    match kind {
        "script" => Ok(Box::new(Script)),
        "claude" => Ok(Box::new(ClaudeHeadless::default())),
        other => Err(format!(
            "no executor for agent kind `{other}` yet; available: claude, script"
        )),
    }
}

/// A command that stands in for an agent. Deterministic, so the engine can be tested end to
/// end without a model. The prompt is in `CONDUCTOR_PROMPT`.
pub struct Script;

impl Executor for Script {
    fn name(&self) -> &'static str {
        "script"
    }

    fn run(&self, b: &Brief) -> AgentRun {
        let mut env = vec![
            ("CONDUCTOR_PROMPT".to_string(), b.prompt.clone()),
            ("CONDUCTOR_STAGE".to_string(), b.stage.clone()),
            ("CONDUCTOR_ATTEMPT".to_string(), b.attempt.to_string()),
        ];
        env.extend(b.env.iter().cloned());
        let e = runner::run(&runner::Spec {
            argv: &b.agent.command,
            cwd: &b.cwd,
            timeout: b.timeout,
            pass_env: &[],
            run_id: &b.run_id,
            inherit_env: true,
            set_env: &env,
        });
        AgentRun {
            finished: e.ended == Ended::Exited && e.exit_code == Some(0),
            final_text: e.stdout_text().trim().to_owned(),
            reason: e.reason.clone().or_else(|| {
                (e.exit_code != Some(0)).then(|| {
                    format!(
                        "the script exited with {:?}: {}",
                        e.exit_code, e.stderr_tail
                    )
                })
            }),
            duration_ms: e.duration_ms,
            ..AgentRun::default()
        }
    }
}

/// Claude Code in headless mode: `claude -p … --output-format stream-json`.
#[derive(Default)]
pub struct ClaudeHeadless {
    /// Override the binary, for tests.
    pub program: Option<String>,
}

impl ClaudeHeadless {
    pub fn argv(&self, b: &Brief) -> Vec<String> {
        let mut a = vec![
            self.program.clone().unwrap_or_else(|| "claude".into()),
            "-p".into(),
            b.prompt.clone(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--permission-mode".into(),
            match b.agent.permission_mode {
                PermissionMode::AcceptEdits => "acceptEdits".into(),
                PermissionMode::Bypass => "bypassPermissions".into(),
            },
        ];
        if let Some(m) = &b.agent.model {
            a.extend(["--model".into(), m.clone()]);
        }
        if !b.agent.allowed_tools.is_empty() {
            a.push("--allowedTools".into());
            a.extend(b.agent.allowed_tools.iter().cloned());
        }
        if let Some(s) = &b.resume {
            a.extend(["--resume".into(), s.clone()]);
        }
        a
    }
}

impl Executor for ClaudeHeadless {
    fn name(&self) -> &'static str {
        "headless"
    }

    fn run(&self, b: &Brief) -> AgentRun {
        let argv = self.argv(b);
        let e = runner::run(&runner::Spec {
            argv: &argv,
            cwd: &b.cwd,
            timeout: b.timeout,
            pass_env: &[],
            run_id: &b.run_id,
            inherit_env: true,
            set_env: &b.env,
        });
        let mut run = parse_claude_stream(&e.stdout_text());
        run.duration_ms = e.duration_ms;
        if e.ended != Ended::Exited {
            run.finished = false;
            run.reason = e.reason.clone();
        } else if e.exit_code != Some(0) && run.reason.is_none() {
            run.finished = false;
            run.reason = Some(format!(
                "claude exited with {:?}: {}",
                e.exit_code, e.stderr_tail
            ));
        }
        run
    }
}

/// Reads Claude Code's `stream-json` output: one JSON object per line.
pub fn parse_claude_stream(text: &str) -> AgentRun {
    let mut run = AgentRun::default();
    let mut saw_result = false;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("system") if v.get("subtype").and_then(|s| s.as_str()) == Some("init") => {
                run.model = v.get("model").and_then(|m| m.as_str()).map(str::to_owned);
                run.session_id = v
                    .get("session_id")
                    .and_then(|s| s.as_str())
                    .map(str::to_owned);
            }
            Some("assistant") => {
                for c in v
                    .pointer("/message/content")
                    .and_then(|c| c.as_array())
                    .into_iter()
                    .flatten()
                {
                    if c.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        let tool = c
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("?")
                            .to_owned();
                        let input = c.get("input");
                        let target = ["file_path", "path", "command", "pattern", "url"]
                            .iter()
                            .find_map(|k| input.and_then(|i| i.get(*k)).and_then(|x| x.as_str()))
                            .map(str::to_owned);
                        run.tool_calls.push(ToolCall { tool, target });
                    }
                }
            }
            Some("result") => {
                saw_result = true;
                let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(true);
                let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
                run.finished = !is_error && subtype == "success";
                if !run.finished {
                    run.reason = Some(format!("claude ended with `{subtype}`"));
                }
                run.turns = v.get("num_turns").and_then(|n| n.as_u64());
                run.final_text = v
                    .get("result")
                    .and_then(|r| r.as_str())
                    .unwrap_or_default()
                    .to_owned();
                if let Some(s) = v.get("session_id").and_then(|s| s.as_str()) {
                    run.session_id = Some(s.to_owned());
                }
                if let Some(u) = v.get("usage") {
                    let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                    run.usage = Some(Usage {
                        input_tokens: n("input_tokens"),
                        output_tokens: n("output_tokens"),
                        cache_read_tokens: n("cache_read_input_tokens"),
                        cache_write_tokens: n("cache_creation_input_tokens"),
                        cost_usd: v.get("total_cost_usd").and_then(|c| c.as_f64()),
                    });
                }
            }
            _ => {}
        }
    }
    if !saw_result {
        run.finished = false;
        run.reason
            .get_or_insert_with(|| "claude produced no result".into());
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = include_str!("../tests/fixtures/claude-stream.jsonl");

    #[test]
    fn a_real_claude_stream_is_read() {
        let r = parse_claude_stream(STREAM);
        assert!(r.finished);
        assert_eq!(r.turns, Some(2));
        assert_eq!(r.final_text, "Done.");
        assert!(r.model.as_deref().unwrap_or("").starts_with("claude-"));
        assert_eq!(
            r.tool_calls,
            vec![ToolCall {
                tool: "Write".into(),
                target: Some("/tmp/cprobe/hello.txt".into())
            }]
        );
        let u = r.usage.unwrap();
        assert_eq!(u.output_tokens, 89);
        assert!(u.total() > 70_000);
        assert!(u.cost_usd.unwrap() > 0.0);
    }

    #[test]
    fn a_stream_without_a_result_did_not_finish() {
        let r = parse_claude_stream(
            STREAM
                .lines()
                .take(2)
                .collect::<Vec<_>>()
                .join("\n")
                .as_str(),
        );
        assert!(!r.finished);
        assert!(r.reason.unwrap().contains("no result"));
    }

    #[test]
    fn resume_and_options_reach_the_command_line() {
        let b = Brief {
            stage: "s".into(),
            prompt: "do it".into(),
            cwd: ".".into(),
            agent: Agent {
                kind: "claude".into(),
                command: vec![],
                model: Some("claude-sonnet-5".into()),
                permission_mode: PermissionMode::AcceptEdits,
                allowed_tools: vec!["Bash(cargo test:*)".into()],
            },
            resume: Some("sess-1".into()),
            timeout: Duration::from_secs(1),
            run_id: "r".into(),
            attempt: 2,
            env: vec![],
        };
        let a = ClaudeHeadless::default().argv(&b);
        let joined = a.join(" ");
        for want in [
            "-p do it",
            "--permission-mode acceptEdits",
            "--model claude-sonnet-5",
            "--allowedTools Bash(cargo test:*)",
            "--resume sess-1",
        ] {
            assert!(joined.contains(want), "{joined}");
        }
    }
}
