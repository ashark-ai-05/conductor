//! Executors drive an agent through one attempt at a stage.
//!
//! Whatever an executor reports about the agent is labelled by where it came from: tool calls
//! read from the agent's own stream are `observed`, token counts are `measured`, and anything
//! it can't see is left out rather than guessed.

use crate::ask::{self, Answer, Ask};
use conductor_checks::runner::{self, Ended};
use conductor_model::workflow::{Agent, PermissionMode};
use serde::Serialize;
use std::cell::RefCell;
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
    /// Where a tool the agent asks for is put to a person, when there is one to ask.
    pub ask: Option<AskChannel>,
}

/// The run's asks directory and who answers, handed to the agent as its permission tool.
#[derive(Debug, Clone)]
pub struct AskChannel {
    pub dir: PathBuf,
    pub who: String,
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
    /// Tools the agent asked for and was refused with nobody to ask.
    pub refused: Vec<ToolCall>,
    /// Tools the agent asked a person for, and how many of those were denied.
    pub asked: usize,
    pub denied: usize,
    /// Time spent waiting for a person, with the stage clock stopped.
    pub waited_ms: u64,
}

/// Something seen while the agent works, handed over as it happens.
#[derive(Debug, Clone, PartialEq)]
pub enum Observed {
    /// A tool the agent used.
    Tool(ToolCall),
    /// A tool the agent asked for and was refused with nobody to ask.
    Refused { call: ToolCall, why: String },
    /// A tool the agent asked a person for; the agent waits.
    Asked(Ask),
    /// The person answered, after `waited`.
    Answered {
        ask: Ask,
        answer: Answer,
        waited: Duration,
    },
}

pub trait Executor: Send + Sync {
    fn name(&self) -> &'static str;
    /// Runs the agent to the end of its turn. `observe` is called with each thing seen on
    /// the way, from the calling thread, before `run` returns.
    fn run(&self, brief: &Brief, observe: &mut dyn FnMut(Observed)) -> AgentRun;
}

/// Picks the executor for an agent kind. Only headless executors exist so far; the herdr
/// executor will wrap the same agents in panes.
pub fn for_agent(kind: &str) -> Result<Box<dyn Executor>, String> {
    match kind {
        "script" => Ok(Box::new(Script)),
        "claude" => Ok(Box::new(ClaudeHeadless {
            program: std::env::var("CONDUCTOR_CLAUDE_BIN").ok(),
        })),
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

    fn run(&self, b: &Brief, _observe: &mut dyn FnMut(Observed)) -> AgentRun {
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
        if let Some(c) = &b.ask {
            a.extend([
                "--mcp-config".into(),
                mcp_config(c).to_string(),
                "--permission-prompt-tool".into(),
                ask::PROMPT_TOOL.into(),
            ]);
        }
        a
    }
}

/// The MCP server Claude asks: this same binary, serving the run's asks directory.
fn mcp_config(c: &AskChannel) -> serde_json::Value {
    let me = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "conductor".into());
    serde_json::json!({ "mcpServers": { ask::SERVER: {
        "command": me,
        "args": ["ask", "--dir", c.dir.display().to_string(), "--who", c.who]
    } } })
}

impl Executor for ClaudeHeadless {
    fn name(&self) -> &'static str {
        "headless"
    }

    fn run(&self, b: &Brief, observe: &mut dyn FnMut(Observed)) -> AgentRun {
        let argv = self.argv(b);
        let mut run = AgentRun::default();
        let mut stream = Stream::default();
        // Both the stream and the ask watcher report through `observe`.
        let observe = RefCell::new(observe);
        let mut watch = b.ask.as_ref().map(|c| ask::Watch::new(c.dir.clone()));
        let (mut asked, mut denied, mut waited) = (0usize, 0usize, Duration::ZERO);
        let mut look = |watch: &mut Option<ask::Watch>| {
            let Some(w) = watch else { return false };
            for seen in w.poll() {
                let o = match seen {
                    ask::Seen::Asked(a) => {
                        asked += 1;
                        Observed::Asked(a)
                    }
                    ask::Seen::Answered {
                        ask,
                        answer,
                        waited: t,
                    } => {
                        waited += t;
                        if !answer.allowed {
                            denied += 1;
                        }
                        Observed::Answered {
                            ask,
                            answer,
                            waited: t,
                        }
                    }
                };
                observe.borrow_mut()(o);
            }
            w.waiting()
        };
        let e = runner::run_pausable(
            &runner::Spec {
                argv: &argv,
                cwd: &b.cwd,
                timeout: b.timeout,
                pass_env: &[],
                run_id: &b.run_id,
                inherit_env: true,
                set_env: &b.env,
            },
            &mut |line| stream.feed(line, &mut run, &mut *observe.borrow_mut()),
            &mut || look(&mut watch),
        );
        // An answer that landed as the agent exited.
        look(&mut watch);
        stream.finish(&mut run);
        run.duration_ms = e.duration_ms;
        run.asked = asked;
        run.denied = denied;
        run.waited_ms = u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
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

/// Reads Claude Code's `stream-json` output, one JSON object per line, as it arrives.
#[derive(Default)]
struct Stream {
    saw_result: bool,
    /// Tool calls by id, so a refusal can be tied to what was asked.
    calls: std::collections::HashMap<String, ToolCall>,
}

impl Stream {
    fn feed(&mut self, line: &str, run: &mut AgentRun, observe: &mut dyn FnMut(Observed)) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
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
                        let target = c.get("input").and_then(ask::target_of);
                        let call = ToolCall { tool, target };
                        if let Some(id) = c.get("id").and_then(|i| i.as_str()) {
                            self.calls.insert(id.to_owned(), call.clone());
                        }
                        run.tool_calls.push(call.clone());
                        observe(Observed::Tool(call));
                    }
                }
            }
            // A refused tool comes back as an error result: Claude's permission system said
            // no, and in a headless run there is nobody to say yes. A person's "denied by …"
            // is an answer, already on the record from the asks directory.
            Some("user") => {
                for c in v
                    .pointer("/message/content")
                    .and_then(|c| c.as_array())
                    .into_iter()
                    .flatten()
                {
                    if c.get("type").and_then(|t| t.as_str()) != Some("tool_result")
                        || c.get("is_error").and_then(|b| b.as_bool()) != Some(true)
                    {
                        continue;
                    }
                    let text = c
                        .get("content")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim();
                    let refused = !text.starts_with("denied by ")
                        && (text.contains("requires approval")
                            || text.contains("permission")
                            || text.contains("not allowed"));
                    if !refused {
                        continue;
                    }
                    let call = c
                        .get("tool_use_id")
                        .and_then(|i| i.as_str())
                        .and_then(|i| self.calls.get(i).cloned())
                        .unwrap_or(ToolCall {
                            tool: "?".into(),
                            target: None,
                        });
                    run.refused.push(call.clone());
                    observe(Observed::Refused {
                        call,
                        why: text.lines().next().unwrap_or("").to_owned(),
                    });
                }
            }
            Some("result") => {
                self.saw_result = true;
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

    fn finish(&self, run: &mut AgentRun) {
        if !self.saw_result {
            run.finished = false;
            run.reason
                .get_or_insert_with(|| "claude produced no result".into());
        }
    }
}

/// Reads a whole `stream-json` transcript at once.
pub fn parse_claude_stream(text: &str) -> AgentRun {
    let mut run = AgentRun::default();
    let mut st = Stream::default();
    for line in text.lines() {
        st.feed(line, &mut run, &mut |_| {});
    }
    st.finish(&mut run);
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

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

    const REFUSED: &str = include_str!("../tests/fixtures/claude-stream-refused.jsonl");

    #[test]
    fn a_refused_tool_is_seen_and_tied_to_what_was_asked() {
        let r = parse_claude_stream(REFUSED);
        assert!(r.finished);
        assert_eq!(r.tool_calls.len(), 3);
        assert_eq!(
            r.refused,
            vec![ToolCall {
                tool: "Bash".into(),
                target: Some("mvn -v && ls /opt/homebrew/opt".into())
            }]
        );
    }

    #[test]
    fn observations_arrive_while_claude_runs() {
        // A stand-in claude: the transcript's first lines, a pause, then the rest.
        let d = tempfile::tempdir().unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude-stream-refused.jsonl");
        let fake = d.path().join("claude");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nhead -n 2 {f}\nsleep 1\ntail -n +3 {f}\n",
                f = fixture.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let exec = ClaudeHeadless {
            program: Some(fake.display().to_string()),
        };
        let brief = Brief {
            stage: "fix".into(),
            prompt: "fix it".into(),
            cwd: d.path().to_path_buf(),
            agent: Agent {
                kind: "claude".into(),
                command: vec![],
                model: None,
                permission_mode: PermissionMode::AcceptEdits,
                allowed_tools: vec![],
                who: None,
            },
            resume: None,
            timeout: Duration::from_secs(20),
            run_id: "r".into(),
            attempt: 1,
            env: vec![],
            ask: None,
        };
        let started = Instant::now();
        let mut seen: Vec<(Observed, Duration)> = Vec::new();
        let run = exec.run(&brief, &mut |o| seen.push((o, started.elapsed())));
        assert!(run.finished, "{:?}", run.reason);
        assert_eq!(seen.len(), 4, "{seen:?}");
        // The first tool call was seen before the pause ended.
        assert!(matches!(seen[0].0, Observed::Tool(_)));
        assert!(seen[0].1 < Duration::from_millis(900), "{:?}", seen[0].1);
        assert!(seen[3].1 >= Duration::from_millis(900), "{:?}", seen[3].1);
        assert!(
            matches!(&seen[2].0, Observed::Refused { why, .. } if why.contains("requires approval"))
        );
    }

    #[test]
    fn an_ask_pauses_the_clock_until_a_person_answers() {
        // A stand-in claude that asks the way the MCP server would: it writes the ask file
        // itself, waits for the answer, then reports the call and finishes.
        let d = tempfile::tempdir().unwrap();
        let asks = d.path().join("asks");
        let fake = d.path().join("claude");
        std::fs::write(
            &fake,
            format!(
                r#"#!/bin/sh
case "$*" in *"--permission-prompt-tool mcp__conductor__ask"*) ;; *) echo "no prompt tool: $*" >&2; exit 2;; esac
mkdir -p {a}
echo '{{"type":"system","subtype":"init","model":"claude-sonnet-5","session_id":"s1"}}'
printf '{{"id":1,"tool":"Bash","target":"mvn -v","input":{{"command":"mvn -v"}},"since":"2026-09-25T00:00:00Z"}}' > {a}/1.json
while [ ! -f {a}/1.answer.json ]; do sleep 0.1; done
echo '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{"command":"mvn -v"}}}}]}}}}'
echo '{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"denied by krunal: not that"}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false,"num_turns":2,"result":"ok","session_id":"s1"}}'
"#,
                a = asks.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let exec = ClaudeHeadless {
            program: Some(fake.display().to_string()),
        };
        let brief = Brief {
            stage: "fix".into(),
            prompt: "fix it".into(),
            cwd: d.path().to_path_buf(),
            agent: Agent {
                kind: "claude".into(),
                command: vec![],
                model: None,
                permission_mode: PermissionMode::AcceptEdits,
                allowed_tools: vec![],
                who: None,
            },
            resume: None,
            // Shorter than the wait below: only a paused clock lets this finish.
            timeout: Duration::from_millis(800),
            run_id: "r".into(),
            attempt: 1,
            env: vec![],
            ask: Some(AskChannel {
                dir: asks.clone(),
                who: "krunal".into(),
            }),
        };
        let answerer = {
            let asks = asks.clone();
            std::thread::spawn(move || {
                while ask::pending(&asks).is_none() {
                    std::thread::sleep(Duration::from_millis(20));
                }
                std::thread::sleep(Duration::from_millis(1200));
                ask::write_answer(
                    &asks,
                    1,
                    &Answer {
                        allowed: false,
                        by: "krunal".into(),
                        note: "not that".into(),
                        at: crate::store::now(),
                    },
                )
                .unwrap();
            })
        };
        let mut seen: Vec<Observed> = Vec::new();
        let run = exec.run(&brief, &mut |o| seen.push(o));
        answerer.join().unwrap();
        assert!(run.finished, "{:?}", run.reason);
        assert!(matches!(&seen[0], Observed::Asked(a) if a.target.as_deref() == Some("mvn -v")));
        // The answer and the tool call land in the same tick; their order is not a promise.
        assert!(
            seen.iter().any(|o| matches!(o, Observed::Answered { answer, waited, .. } if !answer.allowed && *waited >= Duration::from_secs(1))),
            "{seen:?}"
        );
        assert!(seen.iter().any(|o| matches!(o, Observed::Tool(_))));
        assert_eq!(seen.len(), 3, "{seen:?}");
        // A person's deny is not a headless refusal.
        assert!(run.refused.is_empty());
        assert_eq!((run.asked, run.denied), (1, 1));
        assert!(run.waited_ms >= 1000, "{}", run.waited_ms);
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
                who: None,
            },
            resume: Some("sess-1".into()),
            timeout: Duration::from_secs(1),
            run_id: "r".into(),
            attempt: 2,
            env: vec![],
            ask: None,
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
        assert!(!joined.contains("--permission-prompt-tool"));

        let b = Brief {
            ask: Some(AskChannel {
                dir: "/runs/R1/asks".into(),
                who: "PO".into(),
            }),
            ..b
        };
        let a = ClaudeHeadless::default().argv(&b);
        let i = a.iter().position(|x| x == "--mcp-config").unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&a[i + 1]).unwrap();
        let server = &cfg["mcpServers"]["conductor"];
        assert_eq!(
            server["args"],
            serde_json::json!(["ask", "--dir", "/runs/R1/asks", "--who", "PO"])
        );
        assert!(server["command"].as_str().unwrap().contains("conductor"));
        assert!(
            a.join(" ")
                .contains("--permission-prompt-tool mcp__conductor__ask")
        );
    }
}
