//! A tool the agent asks to use, answered by a person.
//!
//! Claude Code in headless mode routes anything it would have prompted for to a tool named
//! by `--permission-prompt-tool`. `conductor ask` is that tool: an MCP server over stdio
//! that writes the question to the run's record and waits for the answer. The question is
//! one file, `asks/<n>.json`, and the answer another, `asks/<n>.answer.json`, the same
//! shape as a `human` stage's decision: any process with the run directory can answer, and
//! the answer survives the process that waited for it.
//!
//! While the agent waits, the stage clock is stopped; the time spent waiting is on the
//! record.

use crate::store::{RunDir, now};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The MCP server's name and tool, as Claude Code addresses them.
pub const SERVER: &str = "conductor";
pub const TOOL: &str = "ask";
pub const PROMPT_TOOL: &str = "mcp__conductor__ask";

/// How long an ask waits for a person before it is denied on their behalf.
pub const WAIT: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    pub id: u32,
    /// Claude's tool name: `Bash`, `Write`, …
    pub tool: String,
    /// The command, file or pattern the call was about, when there is one.
    pub target: Option<String>,
    pub input: serde_json::Value,
    pub since: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub allowed: bool,
    pub by: String,
    #[serde(default)]
    pub note: String,
    pub at: String,
}

impl Ask {
    /// The question a person sees.
    pub fn question(&self) -> String {
        match &self.target {
            Some(t) => format!("may the agent run `{t}`? ({})", self.tool),
            None => format!("may the agent use {}?", self.tool),
        }
    }

    /// Short form for the record: "Bash `mvn -v`".
    pub fn describe(&self) -> String {
        match &self.target {
            Some(t) => format!("{} `{t}`", self.tool),
            None => self.tool.clone(),
        }
    }
}

impl Answer {
    pub fn word(&self) -> &'static str {
        if self.allowed { "allowed" } else { "denied" }
    }

    /// What Claude tells the model when the answer is no.
    pub fn deny_message(&self) -> String {
        if self.note.trim().is_empty() {
            format!("denied by {}", self.by)
        } else {
            format!("denied by {}: {}", self.by, self.note.trim())
        }
    }
}

/// The file or command a tool call is about, from its input.
pub fn target_of(input: &serde_json::Value) -> Option<String> {
    ["file_path", "path", "command", "pattern", "url"]
        .iter()
        .find_map(|k| input.get(*k).and_then(|x| x.as_str()))
        .map(str::to_owned)
}

pub fn dir(run: &RunDir) -> PathBuf {
    run.root.join("asks")
}

fn ask_path(dir: &Path, id: u32) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn answer_path(dir: &Path, id: u32) -> PathBuf {
    dir.join(format!("{id}.answer.json"))
}

/// Records a new ask under the next free number.
pub fn write_ask(dir: &Path, tool: &str, input: serde_json::Value) -> io::Result<Ask> {
    std::fs::create_dir_all(dir)?;
    let mut id = 1;
    loop {
        let p = ask_path(dir, id);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(mut f) => {
                let ask = Ask {
                    id,
                    tool: tool.to_owned(),
                    target: target_of(&input),
                    input,
                    since: now(),
                };
                f.write_all(&serde_json::to_vec_pretty(&ask)?)?;
                return Ok(ask);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => id += 1,
            Err(e) => return Err(e),
        }
    }
}

/// Every ask so far, oldest first.
pub fn read_asks(dir: &Path) -> Vec<Ask> {
    let mut asks: Vec<Ask> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.ends_with(".json") && !n.ends_with(".answer.json"))
        })
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
        .collect();
    asks.sort_by_key(|a| a.id);
    asks
}

pub fn read_answer(dir: &Path, id: u32) -> Option<Answer> {
    let text = std::fs::read_to_string(answer_path(dir, id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Records the answer. A second answer to the same ask is refused: the first is what the
/// agent got.
pub fn write_answer(dir: &Path, id: u32, a: &Answer) -> io::Result<()> {
    if !ask_path(dir, id).is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("there is no ask {id}"),
        ));
    }
    let p = answer_path(dir, id);
    if p.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("ask {id} was already answered"),
        ));
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(a)?)?;
    std::fs::rename(tmp, p)
}

/// The oldest ask nobody has answered.
pub fn pending(dir: &Path) -> Option<Ask> {
    read_asks(dir)
        .into_iter()
        .find(|a| read_answer(dir, a.id).is_none())
}

/// What a watcher saw since it last looked.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    Asked(Ask),
    Answered {
        ask: Ask,
        answer: Answer,
        /// How long the agent waited, by the watcher's clock.
        waited: Duration,
    },
}

/// Polls the asks directory for the engine, so an ask and its answer are recorded the
/// moment they land.
pub struct Watch {
    dir: PathBuf,
    /// Asks seen and not yet answered, with when they were first seen.
    open: BTreeMap<u32, (Ask, Instant)>,
    highest: u32,
}

impl Watch {
    pub fn new(dir: PathBuf) -> Self {
        Watch {
            dir,
            open: BTreeMap::new(),
            highest: 0,
        }
    }

    /// True while an ask waits for its answer.
    pub fn waiting(&self) -> bool {
        !self.open.is_empty()
    }

    pub fn poll(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        if !self.dir.is_dir() {
            return seen;
        }
        for a in read_asks(&self.dir) {
            if a.id > self.highest {
                self.highest = a.id;
                self.open.insert(a.id, (a.clone(), Instant::now()));
                seen.push(Seen::Asked(a));
            }
        }
        let ids: Vec<u32> = self.open.keys().copied().collect();
        for id in ids {
            if let Some(answer) = read_answer(&self.dir, id)
                && let Some((ask, since)) = self.open.remove(&id)
            {
                seen.push(Seen::Answered {
                    ask,
                    answer,
                    waited: since.elapsed(),
                });
            }
        }
        seen
    }
}

/// The MCP server: reads JSON-RPC lines from `input`, answers on `output`. A `tools/call`
/// writes the ask and blocks until it is answered or `wait` runs out, then returns
/// Claude's permission verdict as the tool's text. Returns when `input` ends.
pub fn serve(
    dir: &Path,
    who: &str,
    wait: Duration,
    input: impl BufRead,
    mut output: impl Write,
) -> io::Result<()> {
    let mut send = |v: serde_json::Value| -> io::Result<()> {
        output.write_all(v.to_string().as_bytes())?;
        output.write_all(b"\n")?;
        output.flush()
    };
    for line in input.lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let Some(id) = id else {
            continue; // a notification
        };
        let result = match method {
            "initialize" => serde_json::json!({
                "protocolVersion": req.pointer("/params/protocolVersion").cloned()
                    .unwrap_or_else(|| "2025-06-18".into()),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": SERVER, "version": env!("CARGO_PKG_VERSION") }
            }),
            "tools/list" => serde_json::json!({ "tools": [{
                "name": TOOL,
                "description": "Asks the person running conductor whether the agent may use a tool.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tool_name": { "type": "string" },
                        "input": { "type": "object" },
                        "tool_use_id": { "type": "string" }
                    },
                    "required": ["tool_name", "input"]
                }
            }] }),
            "tools/call" => {
                let args = req
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_default();
                let tool = args
                    .get("tool_name")
                    .and_then(|t| t.as_str())
                    .unwrap_or("?")
                    .to_owned();
                let input = args.get("input").cloned().unwrap_or_default();
                let verdict = match write_ask(dir, &tool, input.clone()) {
                    Ok(ask) => {
                        let a = await_answer(dir, ask.id, who, wait);
                        if a.allowed {
                            serde_json::json!({ "behavior": "allow", "updatedInput": input })
                        } else {
                            serde_json::json!({ "behavior": "deny", "message": a.deny_message() })
                        }
                    }
                    Err(e) => serde_json::json!({
                        "behavior": "deny",
                        "message": format!("conductor could not record the ask: {e}")
                    }),
                };
                serde_json::json!({ "content": [{ "type": "text", "text": verdict.to_string() }] })
            }
            "ping" => serde_json::json!({}),
            _ => {
                send(serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32601, "message": format!("no method `{method}`") }
                }))?;
                continue;
            }
        };
        send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }))?;
    }
    Ok(())
}

/// Waits for ask `id` to be answered. Past `wait`, the answer is a deny in nobody's name,
/// written so the record shows it and nobody answers a question the agent has moved past.
fn await_answer(dir: &Path, id: u32, who: &str, wait: Duration) -> Answer {
    let started = Instant::now();
    loop {
        if let Some(a) = read_answer(dir, id) {
            return a;
        }
        if started.elapsed() > wait {
            let a = Answer {
                allowed: false,
                by: "nobody".into(),
                note: format!("{who} did not answer within {}s", wait.as_secs()),
                at: now(),
            };
            // Someone may have answered in the same instant; theirs stands.
            return match write_answer(dir, id, &a) {
                Ok(()) => a,
                Err(_) => read_answer(dir, id).unwrap_or(a),
            };
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_are_numbered_and_answered_once() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("asks");
        let a = write_ask(&dir, "Bash", serde_json::json!({"command": "mvn -v"})).unwrap();
        let b = write_ask(&dir, "Write", serde_json::json!({"file_path": "x.txt"})).unwrap();
        assert_eq!((a.id, b.id), (1, 2));
        assert_eq!(a.question(), "may the agent run `mvn -v`? (Bash)");
        assert_eq!(b.describe(), "Write `x.txt`");
        assert_eq!(pending(&dir).unwrap().id, 1);
        let yes = Answer {
            allowed: true,
            by: "krunal".into(),
            note: String::new(),
            at: now(),
        };
        write_answer(&dir, 1, &yes).unwrap();
        assert_eq!(
            write_answer(&dir, 1, &yes).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            write_answer(&dir, 9, &yes).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(pending(&dir).unwrap().id, 2);
        assert_eq!(read_answer(&dir, 1), Some(yes));
    }

    #[test]
    fn the_watcher_reports_each_ask_and_answer_once() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("asks");
        let mut w = Watch::new(dir.clone());
        assert!(w.poll().is_empty());
        let a = write_ask(&dir, "Bash", serde_json::json!({"command": "ls"})).unwrap();
        assert_eq!(w.poll(), vec![Seen::Asked(a.clone())]);
        assert!(w.waiting());
        assert!(w.poll().is_empty());
        let no = Answer {
            allowed: false,
            by: "krunal".into(),
            note: "not that".into(),
            at: now(),
        };
        write_answer(&dir, 1, &no).unwrap();
        let seen = w.poll();
        assert!(
            matches!(&seen[..], [Seen::Answered { ask, answer, .. }] if *ask == a && *answer == no),
            "{seen:?}"
        );
        assert!(!w.waiting());
        assert!(w.poll().is_empty());
    }

    fn rpc(lines: &[serde_json::Value]) -> String {
        lines.iter().map(|l| l.to_string() + "\n").collect()
    }

    fn results(out: &[u8]) -> Vec<serde_json::Value> {
        String::from_utf8_lossy(out)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn the_server_asks_waits_and_returns_the_answer() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("asks");
        let input = rpc(&[
            serde_json::json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}),
            serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ask","arguments":{"tool_name":"Bash","input":{"command":"mvn -v"},"tool_use_id":"t1"}}}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ask","arguments":{"tool_name":"Bash","input":{"command":"rm -rf /"},"tool_use_id":"t2"}}}),
        ]);
        // The person: allows the first, denies the second, each once it appears.
        let answerer = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                for (id, allowed) in [(1, true), (2, false)] {
                    while pending(&dir).map(|a| a.id) != Some(id) {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    write_answer(
                        &dir,
                        id,
                        &Answer {
                            allowed,
                            by: "krunal".into(),
                            note: if allowed { String::new() } else { "no".into() },
                            at: now(),
                        },
                    )
                    .unwrap();
                }
            })
        };
        let mut out = Vec::new();
        serve(
            &dir,
            "krunal",
            Duration::from_secs(10),
            input.as_bytes(),
            &mut out,
        )
        .unwrap();
        answerer.join().unwrap();
        let r = results(&out);
        assert_eq!(r.len(), 4, "{r:?}");
        assert_eq!(r[0]["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(r[1]["result"]["tools"][0]["name"], "ask");
        let verdict = |i: usize| -> serde_json::Value {
            serde_json::from_str(r[i]["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
        };
        assert_eq!(
            verdict(2),
            serde_json::json!({"behavior":"allow","updatedInput":{"command":"mvn -v"}})
        );
        assert_eq!(
            verdict(3),
            serde_json::json!({"behavior":"deny","message":"denied by krunal: no"})
        );
    }

    #[test]
    fn an_unanswered_ask_is_denied_in_nobodys_name() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("asks");
        let input = rpc(&[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ask","arguments":{"tool_name":"Bash","input":{"command":"ls"}}}}),
        ]);
        let mut out = Vec::new();
        serve(
            &dir,
            "PO",
            Duration::from_millis(300),
            input.as_bytes(),
            &mut out,
        )
        .unwrap();
        let r = results(&out);
        let text = r[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"deny\""), "{text}");
        assert!(text.contains("PO did not answer within 0s"), "{text}");
        let a = read_answer(&dir, 1).unwrap();
        assert_eq!(a.by, "nobody");
        assert!(!a.allowed);
    }
}
