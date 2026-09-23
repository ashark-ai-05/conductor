//! `conductor pane …`: an agent's hands on its run's herdr tab (SPEC §11.2).
//!
//! An agent may open panes beside its own (a dev server, a watcher, a second shell), run
//! commands in them, read them and close them. It may touch only panes in its run's tab, run
//! commands and close only in panes it opened, and every action lands in the run's record.
//!
//! The engine hands the grant to the agent through its environment. Each action is appended
//! to the run's action log, which the engine reads into the event chain after every attempt.
//! The log sits outside the worktree but is writable by the agent, so its lines are recorded
//! as `observed`, never `witnessed`.

use conductor_herdr::{Direction, Herdr, PaneInfo, Target};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const ENV_TAB: &str = "CONDUCTOR_HERDR_TAB";
pub const ENV_ACTIONS: &str = "CONDUCTOR_ACTIONS";
pub const ENV_BIN: &str = "CONDUCTOR_BIN";

/// One thing an agent did through `conductor pane`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub at: String,
    pub stage: String,
    /// `split`, `run`, `close`, or `refused` (with the attempted action as detail).
    pub action: String,
    pub pane: String,
    #[serde(default)]
    pub detail: String,
}

impl Action {
    pub fn describe(&self) -> String {
        match self.action.as_str() {
            "split" => format!("agent opened pane {} {}", self.pane, self.detail)
                .trim_end()
                .to_owned(),
            "run" => format!("agent ran `{}` in pane {}", self.detail, self.pane),
            "close" => format!("agent closed pane {}", self.pane),
            "refused" => format!("agent was refused {} on pane {}", self.detail, self.pane),
            other => format!("agent did `{other}` to pane {}", self.pane),
        }
    }
}

/// The environment that grants an agent pane control in `tab`.
pub fn env(herdr: &Herdr, tab: &str, actions: &Path, stage: &str) -> Vec<(String, String)> {
    let mut e = vec![
        (ENV_TAB.to_string(), tab.to_string()),
        (ENV_ACTIONS.to_string(), actions.display().to_string()),
        ("CONDUCTOR_STAGE".to_string(), stage.to_string()),
        (
            "CONDUCTOR_HERDR_BIN".to_string(),
            herdr.bin().display().to_string(),
        ),
    ];
    if let Ok(me) = std::env::current_exe() {
        e.push((ENV_BIN.to_string(), me.display().to_string()));
    }
    match herdr.target() {
        Target::Current => {}
        Target::Session(s) => e.push(("CONDUCTOR_HERDR_SESSION".into(), s.clone())),
        Target::Socket(p) => e.push(("HERDR_SOCKET_PATH".into(), p.display().to_string())),
    }
    e
}

/// Every action in the log; lines that don't parse are skipped.
pub fn read_actions(path: &Path) -> Vec<Action> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Panes the agent of `stage` (any stage when `None`) opened and hasn't closed.
pub fn open_panes(actions: &[Action], stage: Option<&str>) -> BTreeSet<String> {
    let mut open = BTreeSet::new();
    for a in actions {
        if stage.is_some_and(|s| s != a.stage) {
            continue;
        }
        match a.action.as_str() {
            "split" => {
                open.insert(a.pane.clone());
            }
            "close" => {
                open.remove(&a.pane);
            }
            _ => {}
        }
    }
    open
}

/// An agent's grant, read from its environment.
pub struct Grant {
    pub herdr: Herdr,
    pub tab: String,
    pub actions: PathBuf,
    pub stage: String,
}

impl Grant {
    pub fn from_env(herdr: Herdr) -> Result<Self, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let (Some(tab), Some(actions)) = (var(ENV_TAB), var(ENV_ACTIONS)) else {
            return Err(
                "pane control is only for agents in a conductor run in herdr, \
                        and only when the workflow allows it (herdr.agent_control: own_tab)"
                    .into(),
            );
        };
        Ok(Grant {
            herdr,
            tab,
            actions: PathBuf::from(actions),
            stage: var("CONDUCTOR_STAGE").unwrap_or_default(),
        })
    }

    pub fn list(&self) -> Result<Vec<PaneInfo>, String> {
        Ok(self
            .herdr
            .panes()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|p| p.tab_id == self.tab)
            .collect())
    }

    fn in_tab(&self, pane: &str) -> Result<(), String> {
        if self.list()?.iter().any(|p| p.pane_id == pane) {
            self.herdr.adopt(pane);
            Ok(())
        } else {
            Err(format!("pane {pane} is not in this run's tab"))
        }
    }

    /// Checks the agent opened `pane`; a refusal is recorded, since trying is evidence too.
    fn opened_by_agent(&self, pane: &str, attempted: &str) -> Result<(), String> {
        let why = if open_panes(&read_actions(&self.actions), Some(&self.stage)).contains(pane) {
            match self.in_tab(pane) {
                Ok(()) => return Ok(()),
                Err(e) => e,
            }
        } else {
            format!(
                "pane {pane} was not opened with `conductor pane split`; agents may run \
                 commands in and close only the panes they opened"
            )
        };
        self.log("refused", pane, attempted)?;
        Err(why)
    }

    fn log(&self, action: &str, pane: &str, detail: &str) -> Result<(), String> {
        let a = Action {
            at: crate::store::now(),
            stage: self.stage.clone(),
            action: action.into(),
            pane: pane.into(),
            detail: detail.into(),
        };
        let line = serde_json::to_string(&a).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.actions)
            .map_err(|e| format!("recording the action: {e}"))?;
        writeln!(f, "{line}").map_err(|e| format!("recording the action: {e}"))
    }

    /// Opens a pane beside `from` (the caller's pane by default) and returns its id.
    pub fn split(&self, from: Option<&str>, down: bool, cwd: &Path) -> Result<String, String> {
        let from = match from {
            Some(p) => p.to_owned(),
            None => std::env::var("HERDR_PANE_ID")
                .ok()
                .filter(|p| !p.is_empty())
                .ok_or("say which pane to split beside with --from <pane>")?,
        };
        if let Err(e) = self.in_tab(&from) {
            self.log("refused", &from, "split")?;
            return Err(e);
        }
        let dir = if down {
            Direction::Down
        } else {
            Direction::Right
        };
        let pane = self
            .herdr
            .pane_split(&from, dir, cwd)
            .map_err(|e| e.to_string())?;
        self.log("split", &pane, &format!("beside {from}"))?;
        Ok(pane)
    }

    pub fn run(&self, pane: &str, command: &str) -> Result<(), String> {
        self.opened_by_agent(pane, "run")?;
        self.herdr
            .pane_run(pane, command)
            .map_err(|e| e.to_string())?;
        self.log("run", pane, command)
    }

    /// Any pane in the run's tab may be read.
    pub fn read(&self, pane: &str, lines: u32) -> Result<String, String> {
        self.in_tab(pane)?;
        self.herdr.pane_read(pane, lines).map_err(|e| e.to_string())
    }

    pub fn close(&self, pane: &str) -> Result<(), String> {
        self.opened_by_agent(pane, "close")?;
        self.herdr.pane_close(pane).map_err(|e| e.to_string())?;
        self.log("close", pane, "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(stage: &str, action: &str, pane: &str) -> Action {
        Action {
            at: String::new(),
            stage: stage.into(),
            action: action.into(),
            pane: pane.into(),
            detail: String::new(),
        }
    }

    #[test]
    fn open_panes_are_splits_not_yet_closed() {
        let log = [
            a("implement", "split", "w1:p3"),
            a("implement", "split", "w1:p4"),
            a("implement", "run", "w1:p3"),
            a("implement", "close", "w1:p3"),
            a("tests", "split", "w1:p5"),
        ];
        assert_eq!(
            open_panes(&log, None),
            BTreeSet::from(["w1:p4".to_string(), "w1:p5".to_string()])
        );
        assert_eq!(
            open_panes(&log, Some("implement")),
            BTreeSet::from(["w1:p4".to_string()])
        );
    }

    #[test]
    fn actions_read_as_plain_sentences() {
        let mut run = a("s", "run", "w1:p3");
        run.detail = "cargo watch".into();
        assert_eq!(run.describe(), "agent ran `cargo watch` in pane w1:p3");
        assert_eq!(
            a("s", "close", "w1:p3").describe(),
            "agent closed pane w1:p3"
        );
    }

    #[test]
    fn a_garbled_log_line_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.jsonl");
        let good = serde_json::to_string(&a("s", "split", "w1:p2")).unwrap();
        std::fs::write(&p, format!("not json\n{good}\n")).unwrap();
        assert_eq!(read_actions(&p).len(), 1);
    }

    #[test]
    fn without_the_grant_pane_control_is_refused() {
        // The test process is not a conductor agent, so neither variable is set.
        if std::env::var_os(ENV_TAB).is_none() {
            assert!(Grant::from_env(Herdr::new()).is_err());
        }
    }
}
