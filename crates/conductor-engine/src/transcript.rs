//! Reads Claude Code's own session log, the evidence source for agents running
//! interactively in herdr panes (SPEC §8): tool calls are `observed`, tokens `measured`.
//!
//! Claude Code writes each session to `~/.claude/projects/<cwd with / and . as ->/<id>.jsonl`.

use crate::executor::{ToolCall, Usage};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
    pub turns: u64,
}

pub fn project_dir(claude_home: &Path, cwd: &Path) -> PathBuf {
    let slug: String = cwd
        .display()
        .to_string()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect();
    claude_home.join("projects").join(slug)
}

/// `~/.claude`, or `$CLAUDE_CONFIG_DIR` when set.
pub fn claude_home() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".claude")
        })
}

/// The session log for `cwd` written to most recently since `since`, read in full.
pub fn latest(claude_home: &Path, cwd: &Path, since: SystemTime) -> Option<Transcript> {
    let dir = project_dir(claude_home, cwd);
    let newest = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(m, _)| *m >= since)
        .max_by_key(|(m, _)| *m)?;
    Some(parse(&std::fs::read_to_string(newest.1).ok()?))
}

/// Reads a session log. Streaming writes the same message more than once, so usage is
/// taken once per message id, from its last record.
pub fn parse(text: &str) -> Transcript {
    let mut t = Transcript::default();
    let mut usage_by_msg: BTreeMap<String, Usage> = BTreeMap::new();
    let mut tools_by_msg: BTreeMap<String, Vec<ToolCall>> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if t.session_id.is_none() {
            t.session_id = v
                .get("sessionId")
                .and_then(|s| s.as_str())
                .map(str::to_owned);
        }
        if v.get("type").and_then(|x| x.as_str()) != Some("assistant") {
            continue;
        }
        let Some(m) = v.get("message") else { continue };
        let id = m
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or("")
            .to_owned();
        if !usage_by_msg.contains_key(&id) && !tools_by_msg.contains_key(&id) {
            order.push(id.clone());
        }
        if let Some(model) = m.get("model").and_then(|x| x.as_str()) {
            t.model = Some(model.to_owned());
        }
        if let Some(u) = m.get("usage") {
            let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            usage_by_msg.insert(
                id.clone(),
                Usage {
                    input_tokens: n("input_tokens"),
                    output_tokens: n("output_tokens"),
                    cache_read_tokens: n("cache_read_input_tokens"),
                    cache_write_tokens: n("cache_creation_input_tokens"),
                    cost_usd: None,
                },
            );
        }
        for c in m
            .get("content")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            if c.get("type").and_then(|x| x.as_str()) == Some("tool_use") {
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
                let calls = tools_by_msg.entry(id.clone()).or_default();
                let call = ToolCall { tool, target };
                if !calls.contains(&call) {
                    calls.push(call);
                }
            }
        }
    }
    t.turns = order.len() as u64;
    for id in &order {
        t.tool_calls
            .extend(tools_by_msg.remove(id).unwrap_or_default());
    }
    if !usage_by_msg.is_empty() {
        let mut total = Usage::default();
        for u in usage_by_msg.values() {
            total.input_tokens += u.input_tokens;
            total.output_tokens += u.output_tokens;
            total.cache_read_tokens += u.cache_read_tokens;
            total.cache_write_tokens += u.cache_write_tokens;
        }
        t.usage = Some(total);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = r#"{"type":"user","sessionId":"s-1","message":{"role":"user","content":"hi"}}
{"type":"assistant","sessionId":"s-1","message":{"id":"m1","model":"claude-sonnet-5","content":[{"type":"tool_use","name":"Write","input":{"file_path":"tests/a.rs"}}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}
{"type":"assistant","sessionId":"s-1","message":{"id":"m1","model":"claude-sonnet-5","content":[{"type":"tool_use","name":"Write","input":{"file_path":"tests/a.rs"}}],"usage":{"input_tokens":10,"output_tokens":7,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}
{"type":"assistant","sessionId":"s-1","message":{"id":"m2","model":"claude-sonnet-5","content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}],"usage":{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":50,"cache_creation_input_tokens":10}}}
"#;

    #[test]
    fn usage_is_counted_once_per_message_and_tools_are_observed() {
        let t = parse(LOG);
        assert_eq!(t.session_id.as_deref(), Some("s-1"));
        assert_eq!(t.turns, 2);
        assert_eq!(t.tool_calls.len(), 2);
        assert_eq!(t.tool_calls[1].target.as_deref(), Some("cargo test"));
        let u = t.usage.unwrap();
        assert_eq!(
            u.output_tokens,
            7 + 2,
            "the last record of m1 wins, not the sum of both"
        );
        assert_eq!(u.total(), (10 + 7 + 100) + (3 + 2 + 50 + 10));
    }

    #[test]
    fn the_project_directory_is_the_cwd_with_separators_replaced() {
        let d = project_dir(
            Path::new("/h/.claude"),
            Path::new("/root/.local/share/conductor/wt/R1"),
        );
        assert_eq!(
            d,
            PathBuf::from("/h/.claude/projects/-root--local-share-conductor-wt-R1")
        );
    }

    #[test]
    fn the_newest_log_since_the_attempt_started_is_read() {
        let home = tempfile::tempdir().unwrap();
        let cwd = Path::new("/work/repo");
        let dir = project_dir(home.path(), cwd);
        std::fs::create_dir_all(&dir).unwrap();
        let before = SystemTime::now() - std::time::Duration::from_secs(5);
        std::fs::write(dir.join("s-1.jsonl"), LOG).unwrap();
        let t = latest(home.path(), cwd, before).unwrap();
        assert_eq!(t.turns, 2);
        assert!(
            latest(
                home.path(),
                cwd,
                SystemTime::now() + std::time::Duration::from_secs(60)
            )
            .is_none()
        );
    }
}
