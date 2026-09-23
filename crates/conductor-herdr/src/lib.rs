//! The one place conductor talks to herdr (SPEC §11.1–11.2).
//!
//! Everything goes through herdr's CLI, which herdr's own agent skill names as the authority
//! on syntax, and every id is read from herdr's JSON responses rather than predicted. The
//! adapter remembers which tabs and panes it created and refuses to close anything else:
//! conductor never touches a pane that belongs to a person or another run.
//!
//! herdr's view of an agent (`idle`, `working`, `blocked`, `done`, `unknown`) comes back as
//! [`AgentState`], and conductor treats it as `inferred`: good for deciding when to look,
//! never proof that work is finished.

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Protocol versions this adapter has been checked against. `doctor` refuses others.
pub const SUPPORTED_PROTOCOLS: std::ops::RangeInclusive<u64> = 22..=22;

#[derive(Debug, thiserror::Error)]
pub enum HerdrError {
    #[error("herdr is not installed or not on PATH ({0})")]
    Missing(std::io::Error),
    #[error("herdr rejected `{args}`: {message}")]
    Server { args: String, message: String },
    #[error("herdr did not understand `{args}`: {message}")]
    Usage { args: String, message: String },
    #[error(
        "herdr answered `{args}` with something that isn't the JSON conductor expected: {detail}"
    )]
    Shape { args: String, detail: String },
    #[error("herdr speaks protocol {found}; this conductor supports {supported}")]
    Protocol { found: u64, supported: String },
    #[error("conductor did not create {0}, so it will not close it")]
    NotOwned(String),
}

/// herdr's reading of an agent's lifecycle. Labelled `inferred` wherever conductor records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl AgentState {
    pub fn parse(s: &str) -> AgentState {
        match s {
            "idle" => AgentState::Idle,
            "working" => AgentState::Working,
            "blocked" => AgentState::Blocked,
            "done" => AgentState::Done,
            _ => AgentState::Unknown,
        }
    }

    /// Ready for the next prompt. `idle` and `done` differ only in whether a person has
    /// looked at the tab, so both count (SPEC §11.1).
    pub fn is_ready(self) -> bool {
        matches!(self, AgentState::Idle | AgentState::Done)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Right,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub tab_id: String,
    pub root_pane: String,
}

/// Which herdr server to talk to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The session this process runs in (inherited from the pane).
    Current,
    /// A named session, as `herdr --session <name>`.
    Session(String),
    /// A server socket, as `HERDR_SOCKET_PATH`.
    Socket(PathBuf),
}

/// A handle on one herdr server.
pub struct Herdr {
    bin: PathBuf,
    target: Target,
    owned: Mutex<BTreeSet<String>>,
}

impl Herdr {
    /// The herdr on PATH, talking to the session this process is in.
    pub fn new() -> Self {
        Self::with("herdr", Target::Current)
    }

    /// A specific binary and server, as tests and isolated sessions use.
    pub fn with(bin: impl Into<PathBuf>, target: Target) -> Self {
        Herdr {
            bin: bin.into(),
            target,
            owned: Mutex::new(BTreeSet::new()),
        }
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.bin);
        match &self.target {
            Target::Current => {}
            Target::Session(name) => {
                c.args(["--session", name]);
            }
            Target::Socket(path) => {
                c.env("HERDR_SOCKET_PATH", path);
            }
        }
        c
    }

    /// Runs one CLI call whose answer is plain text, such as `pane read`.
    pub fn call_text(&self, args: &[&str]) -> Result<String, HerdrError> {
        let out = self
            .command()
            .args(args)
            .output()
            .map_err(HerdrError::Missing)?;
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        match out.status.code() {
            Some(0) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
            Some(2) => Err(HerdrError::Usage {
                args: args.join(" "),
                message: stderr,
            }),
            _ => Err(HerdrError::Server {
                args: args.join(" "),
                message: serde_json::from_str::<Value>(&stderr)
                    .ok()
                    .and_then(|v| error_message(&v))
                    .unwrap_or(stderr),
            }),
        }
    }

    /// Runs one CLI call and returns its JSON. Server errors arrive as JSON on stderr with
    /// status 1; usage errors exit with status 2.
    pub fn call(&self, args: &[&str]) -> Result<Value, HerdrError> {
        let out = self
            .command()
            .args(args)
            .output()
            .map_err(HerdrError::Missing)?;
        let joined = args.join(" ");
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        match out.status.code() {
            Some(0) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let text = stdout.trim();
                if text.is_empty() {
                    return Ok(Value::Null);
                }
                serde_json::from_str(text).map_err(|e| HerdrError::Shape {
                    args: joined,
                    detail: format!("{e}: {}", truncate(text)),
                })
            }
            Some(2) => Err(HerdrError::Usage {
                args: joined,
                message: stderr,
            }),
            _ => {
                let message = serde_json::from_str::<Value>(&stderr)
                    .ok()
                    .and_then(|v| error_message(&v))
                    .unwrap_or(stderr);
                Err(HerdrError::Server {
                    args: joined,
                    message,
                })
            }
        }
    }

    /// herdr's protocol version, checked against [`SUPPORTED_PROTOCOLS`].
    pub fn check_protocol(&self) -> Result<u64, HerdrError> {
        let v = self.call(&["status", "--json"])?;
        let found = find_u64(&v, "protocol").ok_or_else(|| HerdrError::Shape {
            args: "status --json".into(),
            detail: "no protocol version".into(),
        })?;
        if SUPPORTED_PROTOCOLS.contains(&found) {
            Ok(found)
        } else {
            Err(HerdrError::Protocol {
                found,
                supported: format!(
                    "{}–{}",
                    SUPPORTED_PROTOCOLS.start(),
                    SUPPORTED_PROTOCOLS.end()
                ),
            })
        }
    }

    fn own(&self, id: &str) {
        self.owned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.to_owned());
    }

    pub fn owns(&self, id: &str) -> bool {
        self.owned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(id)
    }

    /// Opens a workspace, for when herdr has none yet (a fresh headless server). Returns
    /// the workspace id and its first tab.
    pub fn workspace_create(&self, label: &str, cwd: &Path) -> Result<(String, Tab), HerdrError> {
        let cwd = cwd.display().to_string();
        let args = [
            "workspace",
            "create",
            "--label",
            label,
            "--cwd",
            cwd.as_str(),
            "--no-focus",
        ];
        let v = self.call(&args)?;
        let r = result(&v);
        let ws = str_at(r, &["workspace", "workspace_id"])
            .ok_or_else(|| shape(&args, "no .result.workspace.workspace_id"))?;
        let tab_id =
            str_at(r, &["tab", "tab_id"]).ok_or_else(|| shape(&args, "no .result.tab.tab_id"))?;
        let root_pane = str_at(r, &["root_pane", "pane_id"])
            .ok_or_else(|| shape(&args, "no .result.root_pane.pane_id"))?;
        self.own(&ws);
        self.own(&tab_id);
        self.own(&root_pane);
        Ok((ws, Tab { tab_id, root_pane }))
    }

    /// The workspaces herdr has, as ids.
    pub fn workspaces(&self) -> Result<Vec<String>, HerdrError> {
        let v = self.call(&["workspace", "list"])?;
        Ok(result(&v)
            .get("workspaces")
            .and_then(|w| w.as_array())
            .into_iter()
            .flatten()
            .filter_map(|w| {
                w.get("workspace_id")
                    .and_then(|i| i.as_str())
                    .map(str::to_owned)
            })
            .collect())
    }

    /// Opens a tab for a run in `workspace`, without taking focus from whoever is working.
    pub fn tab_create(&self, workspace: &str, label: &str, cwd: &Path) -> Result<Tab, HerdrError> {
        let cwd = cwd.display().to_string();
        let args = [
            "tab",
            "create",
            "--workspace",
            workspace,
            "--label",
            label,
            "--cwd",
            cwd.as_str(),
            "--no-focus",
        ];
        let v = self.call(&args)?;
        let r = result(&v);
        let tab_id =
            str_at(r, &["tab", "tab_id"]).ok_or_else(|| shape(&args, "no .result.tab.tab_id"))?;
        let root_pane = str_at(r, &["root_pane", "pane_id"])
            .ok_or_else(|| shape(&args, "no .result.root_pane.pane_id"))?;
        self.own(&tab_id);
        self.own(&root_pane);
        Ok(Tab { tab_id, root_pane })
    }

    /// Splits a pane conductor owns, keeping focus where it is.
    pub fn pane_split(
        &self,
        pane: &str,
        direction: Direction,
        cwd: &Path,
    ) -> Result<String, HerdrError> {
        if !self.owns(pane) {
            return Err(HerdrError::NotOwned(pane.to_owned()));
        }
        let dir = match direction {
            Direction::Right => "right",
            Direction::Down => "down",
        };
        let cwd = cwd.display().to_string();
        let args = [
            "pane",
            "split",
            pane,
            "--direction",
            dir,
            "--cwd",
            cwd.as_str(),
            "--no-focus",
        ];
        let v = self.call(&args)?;
        let id = str_at(result(&v), &["pane", "pane_id"])
            .ok_or_else(|| shape(&args, "no .result.pane.pane_id"))?;
        self.own(&id);
        Ok(id)
    }

    /// Runs a shell command in a pane: the text and Enter, sent together.
    pub fn pane_run(&self, pane: &str, command: &str) -> Result<(), HerdrError> {
        self.call(&["pane", "run", pane, command]).map(|_| ())
    }

    /// Recent output with soft wraps joined. Corroborating evidence only: alternate-screen
    /// rows never reach scrollback (SPEC §11.1).
    pub fn pane_read(&self, pane: &str, lines: u32) -> Result<String, HerdrError> {
        let lines = lines.to_string();
        self.call_text(&[
            "pane",
            "read",
            pane,
            "--source",
            "recent-unwrapped",
            "--lines",
            lines.as_str(),
        ])
    }

    pub fn pane_close(&self, pane: &str) -> Result<(), HerdrError> {
        if !self.owns(pane) {
            return Err(HerdrError::NotOwned(pane.to_owned()));
        }
        self.call(&["pane", "close", pane]).map(|_| ())
    }

    pub fn tab_close(&self, tab: &str) -> Result<(), HerdrError> {
        if !self.owns(tab) {
            return Err(HerdrError::NotOwned(tab.to_owned()));
        }
        self.call(&["tab", "close", tab]).map(|_| ())
    }

    /// Starts an agent in a shell pane conductor owns. Arguments after `--` go to the agent.
    pub fn agent_start(
        &self,
        name: &str,
        kind: &str,
        pane: &str,
        agent_args: &[String],
    ) -> Result<(), HerdrError> {
        if !self.owns(pane) {
            return Err(HerdrError::NotOwned(pane.to_owned()));
        }
        let mut args: Vec<&str> = vec!["agent", "start", name, "--kind", kind, "--pane", pane];
        if !agent_args.is_empty() {
            args.push("--");
            args.extend(agent_args.iter().map(String::as_str));
        }
        self.call(&args).map(|_| ())
    }

    /// Sends a prompt and waits for the agent to settle. The returned state is herdr's
    /// inference; completion is confirmed by outputs and checks, not by this.
    pub fn agent_prompt(
        &self,
        name: &str,
        text: &str,
        timeout_ms: u64,
    ) -> Result<AgentState, HerdrError> {
        let t = timeout_ms.to_string();
        let v = self.call(&[
            "agent",
            "prompt",
            name,
            text,
            "--wait",
            "--timeout",
            t.as_str(),
        ])?;
        Ok(state_of(&v))
    }

    pub fn agent_state(&self, name: &str) -> Result<AgentState, HerdrError> {
        Ok(state_of(&self.call(&["agent", "get", name])?))
    }

    pub fn agent_read(&self, name: &str, lines: u32) -> Result<String, HerdrError> {
        let lines = lines.to_string();
        self.call_text(&[
            "agent",
            "read",
            name,
            "--source",
            "recent-unwrapped",
            "--lines",
            lines.as_str(),
        ])
    }
}

impl Default for Herdr {
    fn default() -> Self {
        Self::new()
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(200).collect()
}

fn shape(args: &[&str], detail: &str) -> HerdrError {
    HerdrError::Shape {
        args: args.join(" "),
        detail: detail.into(),
    }
}

fn result(v: &Value) -> &Value {
    v.get("result").unwrap_or(v)
}

fn str_at(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_str().map(str::to_owned)
}

fn error_message(v: &Value) -> Option<String> {
    let e = v.get("error").unwrap_or(v);
    let code = e.get("code").and_then(|c| c.as_str());
    let msg = e.get("message").and_then(|m| m.as_str());
    match (code, msg) {
        (Some(c), Some(m)) => Some(format!("{c}: {m}")),
        (Some(c), None) => Some(c.to_owned()),
        (None, Some(m)) => Some(m.to_owned()),
        _ => None,
    }
}

/// The first number found under `key` anywhere in the value.
fn find_u64(v: &Value, key: &str) -> Option<u64> {
    match v {
        Value::Object(m) => m
            .get(key)
            .and_then(|x| x.as_u64())
            .or_else(|| m.values().find_map(|x| find_u64(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find_u64(x, key)),
        _ => None,
    }
}

fn find_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    match v {
        Value::Object(m) => m
            .get(key)
            .and_then(|x| x.as_str())
            .or_else(|| m.values().find_map(|x| find_str(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find_str(x, key)),
        _ => None,
    }
}

fn state_of(v: &Value) -> AgentState {
    find_str(v, "agent_status")
        .or_else(|| find_str(v, "state"))
        .or_else(|| find_str(v, "status"))
        .map_or(AgentState::Unknown, AgentState::parse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn states_map_and_ready_means_idle_or_done() {
        assert!(AgentState::parse("idle").is_ready());
        assert!(AgentState::parse("done").is_ready());
        assert!(!AgentState::parse("working").is_ready());
        assert!(!AgentState::parse("blocked").is_ready());
        assert_eq!(AgentState::parse("something-new"), AgentState::Unknown);
        assert!(
            !AgentState::Unknown.is_ready(),
            "unknown never counts as finished"
        );
    }

    #[test]
    fn server_errors_are_read_from_json() {
        let v = json!({"error": {"code": "agent_blocked", "message": "the agent is waiting on a dialog"}});
        assert_eq!(
            error_message(&v).unwrap(),
            "agent_blocked: the agent is waiting on a dialog"
        );
    }

    #[test]
    fn ids_are_read_from_nested_results() {
        let v = json!({"result": {"tab": {"tab_id": "w1:t3"}, "root_pane": {"pane_id": "w1:p7"}}});
        assert_eq!(str_at(result(&v), &["tab", "tab_id"]).unwrap(), "w1:t3");
        assert_eq!(
            str_at(result(&v), &["root_pane", "pane_id"]).unwrap(),
            "w1:p7"
        );
    }

    #[test]
    fn nothing_conductor_did_not_create_can_be_closed() {
        let h = Herdr::with("/nonexistent/herdr", Target::Current);
        assert!(matches!(
            h.pane_close("w1:p1"),
            Err(HerdrError::NotOwned(_))
        ));
        assert!(matches!(h.tab_close("w1:t1"), Err(HerdrError::NotOwned(_))));
        assert!(matches!(
            h.pane_split("w1:p1", Direction::Right, Path::new(".")),
            Err(HerdrError::NotOwned(_))
        ));
    }

    #[test]
    fn a_missing_binary_says_so() {
        let h = Herdr::with("/nonexistent/herdr", Target::Current);
        assert!(matches!(
            h.call(&["status", "--json"]),
            Err(HerdrError::Missing(_))
        ));
    }
}
