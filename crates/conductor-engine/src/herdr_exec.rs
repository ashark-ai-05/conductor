//! Runs stages in herdr panes, where a person can watch them and step in (SPEC §8, §11).
//!
//! One tab per run, one pane per active stage. The pane of a stage that passed is closed;
//! a failed or blocked one stays open for a person to look at. herdr's agent state decides
//! only *when* to look; what the agent did is read from its own session log, and whether the
//! work is done is decided by the gates.

use crate::agent_panes;
use crate::executor::{AgentRun, Brief, Executor};
use crate::transcript;
use conductor_herdr::{AgentState, Direction, Herdr, HerdrError, Tab};
use conductor_model::workflow::PermissionMode;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// herdr state shared by every stage of one run.
pub struct HerdrRun {
    pub herdr: Herdr,
    run_id: String,
    state: Mutex<RunPanes>,
    /// The run's agent action log, when agents may control panes in the run's tab.
    actions: Option<PathBuf>,
}

#[derive(Default)]
struct RunPanes {
    workspace: Option<String>,
    tab: Option<Tab>,
    /// An open pane no stage is using: the tab's root at first, then a passed stage's pane
    /// that was kept because closing a tab's last pane would close the tab.
    free: Option<String>,
    last_pane: Option<String>,
    /// stage → (pane, agent name)
    stages: BTreeMap<String, (String, Option<String>)>,
}

/// What happened to a stage's pane when the stage ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneOutcome {
    Closed,
    /// The pane was the tab's last; it is reused by the next stage.
    Kept,
    /// Left open for a person: the stage did not pass, or the workflow keeps panes.
    LeftOpen,
    /// herdr refused to close it; it is left as it is.
    CloseFailed,
}

/// A herdr agent name: `[a-z][a-z0-9_-]{0,31}`, unique per run and stage.
pub fn agent_name(stage: &str, run_id: &str) -> String {
    let raw = format!("{stage}-{}", run_id.to_lowercase());
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if !s.starts_with(|c: char| c.is_ascii_lowercase()) {
        s.insert(0, 's');
    }
    s.truncate(32);
    s
}

impl HerdrRun {
    pub fn new(herdr: Herdr, run_id: &str) -> Self {
        HerdrRun {
            herdr,
            run_id: run_id.to_owned(),
            state: Mutex::new(RunPanes::default()),
            actions: None,
        }
    }

    /// Lets agents open and close panes in the run's tab, logging to `actions`.
    pub fn with_agent_control(mut self, actions: PathBuf) -> Self {
        self.actions = Some(actions);
        self
    }

    pub fn agent_control(&self) -> bool {
        self.actions.is_some()
    }

    /// The environment granting a stage's agent pane control; empty when not allowed.
    pub fn agent_env(&self, stage: &str) -> Vec<(String, String)> {
        match (&self.actions, self.tab_id()) {
            (Some(a), Some(tab)) => agent_panes::env(&self.herdr, &tab, a, stage),
            _ => Vec::new(),
        }
    }

    /// Everything agents did through `conductor pane`, in order.
    pub fn agent_actions(&self) -> Vec<agent_panes::Action> {
        self.actions
            .as_deref()
            .map(agent_panes::read_actions)
            .unwrap_or_default()
    }

    /// Closes the panes a stage's agent opened and left open. Returns each with whether
    /// herdr closed it.
    pub fn close_agent_panes(&self, stage: &str) -> Vec<(String, bool)> {
        agent_panes::open_panes(&self.agent_actions(), Some(stage))
            .into_iter()
            .map(|p| {
                self.herdr.adopt(&p);
                let ok = self.herdr.pane_close(&p).is_ok();
                (p, ok)
            })
            .collect()
    }

    /// Panes in the run's tab that neither conductor nor an agent opened through it: a
    /// person, or an agent calling herdr directly.
    pub fn unmanaged(&self) -> Vec<String> {
        let Some(tab) = self.tab_id() else {
            return Vec::new();
        };
        let Ok(panes) = self.herdr.panes() else {
            return Vec::new();
        };
        let mut known = agent_panes::open_panes(&self.agent_actions(), None);
        {
            let st = self.lock();
            known.extend(st.stages.values().map(|e| e.0.clone()));
            known.extend(st.free.clone());
        }
        panes
            .into_iter()
            .filter(|p| p.tab_id == tab && !known.contains(&p.pane_id))
            .map(|p| p.pane_id)
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RunPanes> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Opens the run's tab in the caller's workspace (or the first one, or a new one).
    pub fn open_tab(&self, cwd: &Path) -> Result<Tab, HerdrError> {
        let mut st = self.lock();
        if let Some(t) = &st.tab {
            return Ok(t.clone());
        }
        let ws = match std::env::var("HERDR_WORKSPACE_ID")
            .ok()
            .filter(|w| !w.is_empty())
        {
            Some(w) => w,
            None => match self.herdr.workspaces()?.into_iter().next() {
                Some(w) => w,
                None => self.herdr.workspace_create("conductor", cwd)?.0,
            },
        };
        let label = format!("run {}", self.run_id);
        let tab = self.herdr.tab_create(&ws, &label, cwd)?;
        st.workspace = Some(ws);
        st.tab = Some(tab.clone());
        st.free = Some(tab.root_pane.clone());
        Ok(tab)
    }

    /// A pane for a stage: the tab's root for the first, a split after that.
    fn pane_for(&self, stage: &str, cwd: &Path, fresh: bool) -> Result<String, HerdrError> {
        let tab = self.open_tab(cwd)?;
        let mut st = self.lock();
        if let Some((pane, _)) = st.stages.get(stage).cloned() {
            if !fresh {
                return Ok(pane);
            }
            // A fresh attempt gets a clean pane: reuse this one when it is the tab's last.
            st.stages.remove(stage);
            if st.stages.is_empty() {
                st.free = Some(pane.clone());
            } else if self.herdr.pane_close(&pane).is_ok() {
                if st.last_pane.as_deref() == Some(pane.as_str()) {
                    st.last_pane = None;
                }
            } else {
                st.free = Some(pane.clone());
            }
        }
        let pane = if let Some(p) = st.free.take() {
            p
        } else {
            // Split from the most recent open pane; any open stage pane will do otherwise.
            let from = st
                .last_pane
                .clone()
                .or_else(|| st.stages.values().next().map(|e| e.0.clone()))
                .unwrap_or(tab.root_pane.clone());
            self.herdr.pane_split(&from, Direction::Right, cwd)?
        };
        st.last_pane = Some(pane.clone());
        st.stages.insert(stage.to_owned(), (pane.clone(), None));
        Ok(pane)
    }

    fn set_agent(&self, stage: &str, name: &str) {
        if let Some(e) = self.lock().stages.get_mut(stage) {
            e.1 = Some(name.to_owned());
        }
    }

    fn agent_of(&self, stage: &str) -> Option<String> {
        self.lock().stages.get(stage).and_then(|e| e.1.clone())
    }

    /// Closes a stage's pane once it passed; a failed stage's pane stays for a person.
    /// The tab's last pane is kept for the next stage, since closing it closes the tab.
    pub fn stage_done(&self, stage: &str, passed: bool, close_passed: bool) -> PaneOutcome {
        if !(passed && close_passed) {
            return PaneOutcome::LeftOpen;
        }
        let mut st = self.lock();
        let Some((pane, _)) = st.stages.remove(stage) else {
            return PaneOutcome::LeftOpen;
        };
        if st.stages.is_empty() && st.free.is_none() {
            st.free = Some(pane);
            return PaneOutcome::Kept;
        }
        if self.herdr.pane_close(&pane).is_err() {
            return PaneOutcome::CloseFailed;
        }
        if st.last_pane.as_deref() == Some(pane.as_str()) {
            st.last_pane = None;
        }
        PaneOutcome::Closed
    }

    pub fn tab_id(&self) -> Option<String> {
        self.lock().tab.as_ref().map(|t| t.tab_id.clone())
    }
}

pub struct HerdrExecutor<'a> {
    pub run: &'a HerdrRun,
    pub kind: String,
}

impl Executor for HerdrExecutor<'_> {
    fn name(&self) -> &'static str {
        "herdr"
    }

    fn run(&self, b: &Brief) -> AgentRun {
        let started = Instant::now();
        let since = SystemTime::now();
        let fresh = b.resume.is_none() && b.attempt > 1;
        let pane = match self.run.pane_for(&b.stage, &b.cwd, fresh) {
            Ok(p) => p,
            Err(e) => return failed(format!("herdr could not open a pane: {e}"), started),
        };
        let mut notes = vec![format!(
            "stage pane {pane} in tab {}",
            self.run.tab_id().unwrap_or_default()
        )];
        let mut run = match self.kind.as_str() {
            "script" => run_script(&self.run.herdr, &pane, b, &self.run.agent_env(&b.stage)),
            "claude" => run_claude(self.run, &pane, b, &mut notes),
            other => failed(
                format!("the herdr executor can't run `{other}` agents yet"),
                started,
            ),
        };
        if self.kind == "claude"
            && let Some(t) = transcript::latest(&transcript::claude_home(), &b.cwd, since)
        {
            run.session_id = t.session_id;
            run.model = t.model;
            run.tool_calls = t.tool_calls;
            run.usage = t.usage;
            run.turns = Some(t.turns);
        }
        run.inferred.extend(notes);
        run.duration_ms = started.elapsed().as_millis() as u64;
        run
    }
}

fn failed(reason: String, started: Instant) -> AgentRun {
    AgentRun {
        finished: false,
        reason: Some(reason),
        duration_ms: started.elapsed().as_millis() as u64,
        ..AgentRun::default()
    }
}

/// Runs a scripted agent in the pane and waits for a sentinel only the command's *output*
/// can contain: the arithmetic is evaluated by the shell, so the echoed command line never
/// matches (SPEC §11.1).
fn exports(env: &[(String, String)]) -> String {
    env.iter()
        .map(|(k, v)| format!("{k}={}", quote(v)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn run_script(h: &Herdr, pane: &str, b: &Brief, env: &[(String, String)]) -> AgentRun {
    let started = Instant::now();
    let nonce = format!("{}{}", b.attempt, std::process::id());
    let cmd = b
        .agent
        .command
        .iter()
        .map(|a| quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    let line = format!(
        "export {} CONDUCTOR_RUN_ID={} CONDUCTOR_STAGE={} CONDUCTOR_ATTEMPT={} CONDUCTOR_PROMPT={}; {cmd}; echo \"__cd_$((40+2))_{nonce}_rc=$?\"",
        exports(env),
        quote(&b.run_id),
        quote(&b.stage),
        b.attempt,
        quote(&b.prompt)
    );
    if let Err(e) = h.pane_run(pane, &line) {
        return failed(format!("herdr could not run the script: {e}"), started);
    }
    let marker = format!("__cd_42_{nonce}_rc=");
    while started.elapsed() < b.timeout {
        if let Ok(text) = h.pane_read(pane, 400)
            && let Some(rest) = text.lines().rev().find_map(|l| l.split(&marker).nth(1))
        {
            let rc: i32 = rest
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(-1);
            return AgentRun {
                finished: rc == 0,
                reason: (rc != 0).then(|| format!("the script exited with {rc}")),
                ..AgentRun::default()
            };
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    failed(format!("timed out after {}s", b.timeout.as_secs()), started)
}

fn claude_args(b: &Brief) -> Vec<String> {
    let mut a = vec![
        "--permission-mode".to_string(),
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
    a
}

fn run_claude(run: &HerdrRun, pane: &str, b: &Brief, notes: &mut Vec<String>) -> AgentRun {
    let started = Instant::now();
    let h = &run.herdr;
    let name = match run.agent_of(&b.stage) {
        Some(n) => n,
        None => {
            let n = agent_name(&b.stage, &b.run_id);
            let env = run.agent_env(&b.stage);
            if !env.is_empty() {
                // The agent inherits the pane shell's environment; give the shell the grant
                // first, then let it settle at its prompt before herdr starts the agent.
                if let Err(e) = h.pane_run(pane, &format!("export {}", exports(&env))) {
                    return failed(format!("herdr could not prepare the pane: {e}"), started);
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            if let Err(e) = h.agent_start(&n, "claude", pane, &claude_args(b)) {
                return failed(format!("herdr could not start claude: {e}"), started);
            }
            run.set_agent(&b.stage, &n);
            n
        }
    };
    let timeout_ms = b.timeout.as_millis().min(u128::from(u64::MAX)) as u64;
    let mut state = match h.agent_prompt(&name, &b.prompt, timeout_ms) {
        Ok(s) => s,
        Err(e) => return failed(format!("herdr could not prompt the agent: {e}"), started),
    };
    notes.push(format!("herdr says the agent is {state:?}").to_lowercase());
    // Blocked is normal control flow: a person answers in the pane. Wait for them, within
    // the stage's budget, and never treat `unknown` as finished.
    let mut waited = false;
    while !state.is_ready() && started.elapsed() < b.timeout {
        if state == AgentState::Blocked && !waited {
            notes.push("the agent is waiting for you in its pane".into());
            waited = true;
        }
        std::thread::sleep(Duration::from_secs(2));
        state = h.agent_state(&name).unwrap_or(AgentState::Unknown);
    }
    if state.is_ready() {
        AgentRun {
            finished: true,
            final_text: h.agent_read(&name, 60).unwrap_or_default(),
            ..AgentRun::default()
        }
    } else {
        failed(
            format!(
                "the agent never settled (herdr says {state:?}) within {}s",
                b.timeout.as_secs()
            ),
            started,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_names_fit_herdrs_rules() {
        let n = agent_name("implement", "0MUE8IKOXFM");
        assert_eq!(n, "implement-0mue8ikoxfm");
        let long = agent_name("a-very-long-stage-name-that-goes-on", "0MUE8IKOXFM");
        assert!(long.len() <= 32);
        assert_eq!(agent_name("9lives", "R"), "s9lives-r");
        assert!(
            agent_name("with space", "R")
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        );
    }
}
