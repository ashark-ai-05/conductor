//! What the live-run screen shows. The engine fills it from the event stream; the demo fills
//! it from a script. The UI only ever reads it.

use crate::evidence::Source;
use crate::receipt::Verdict;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageView {
    pub name: String,
    pub status: Verdict,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckView {
    pub name: String,
    pub status: Verdict,
    pub detail: String,
    /// 0–100 when the check reports progress.
    pub progress: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub at: String,
    pub source: Source,
    pub text: String,
}

/// A herdr pane conductor manages for this run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneView {
    pub stage: String,
    pub status: Verdict,
    pub open: bool,
    /// Who opened it: `conductor`, an agent name, or `human`.
    pub opened_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveRun {
    pub run_id: String,
    pub work: String,
    pub kind: String,
    pub herdr_tab: Option<u16>,
    pub stages: Vec<StageView>,
    /// Checks for the stage that is running now.
    pub checks: Vec<CheckView>,
    /// One plain sentence about retries so far, if any.
    pub note: Option<String>,
    pub log: Vec<LogLine>,
    pub panes: Vec<PaneView>,
    /// The run's verdict once it has ended; `None` while it runs.
    #[serde(default)]
    pub ended: Option<Verdict>,
    /// The conductor process running it, so a run whose process died isn't shown as running.
    #[serde(default)]
    pub pid: Option<u32>,
}

impl LiveRun {
    /// Every stage has passed.
    pub fn is_done(&self) -> bool {
        self.ended.is_some()
            || (!self.stages.is_empty() && self.stages.iter().all(|s| s.status == Verdict::Passed))
    }

    /// Keeps the evidence feed to its latest lines.
    pub fn push_log(&mut self, at: &str, source: Source, text: &str) {
        self.log.push(LogLine {
            at: at.into(),
            source,
            text: text.into(),
        });
        const KEEP: usize = 6;
        if self.log.len() > KEEP {
            let extra = self.log.len() - KEEP;
            self.log.drain(..extra);
        }
    }
}
