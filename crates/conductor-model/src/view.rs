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
    /// A person's decision the run is waiting for.
    #[serde(default)]
    pub waiting: Option<Waiting>,
}

/// A `human` stage in progress: the run is paused until `who` decides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    pub stage: String,
    pub who: String,
    /// What they are asked to decide, from the stage's prompt.
    pub question: String,
    pub since: String,
    /// Set when the question is a tool the agent asked for (`asks/<n>.json`), answered
    /// with `conductor allow` or `deny`; unset for a `human` stage's decision.
    #[serde(default)]
    pub ask: Option<u32>,
}

/// What a person needs to decide a run, on one screen: the intent, the change, the evidence,
/// the cost, and then the decision. The engine writes it as `review.json` when a run pauses
/// for a person; the TUI and the page show it. Everything in it is derived from the record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub run_id: String,
    /// The stage waiting on the person, and who.
    pub stage: String,
    pub who: String,
    /// The work's title, from the ticket.
    pub title: String,
    /// The acceptance criteria, as the ticket lists them.
    pub criteria: Vec<String>,
    pub change: Change,
    pub evidence: Vec<EvidenceRow>,
    pub cost: Cost,
    /// What the stages declared as their outputs, with the first lines of each: for a
    /// question, the answer is the thing to read.
    #[serde(default)]
    pub produced: Vec<Produced>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Produced {
    pub path: String,
    pub lines: Vec<String>,
    /// How many lines the file has in all.
    pub total: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub files: Vec<String>,
    pub added: usize,
    pub removed: usize,
    pub branch: String,
}

/// One check, before the fix and after it, with the lines of its output that say so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRow {
    pub stage: String,
    pub check: String,
    pub claim: String,
    pub before: Option<Verdict>,
    pub after: Verdict,
    pub detail: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub tries: usize,
    pub tokens: u64,
    pub cost_usd: f64,
    /// Seconds spent waiting on people, with the clock stopped.
    pub waited_s: u64,
    /// Seconds from the run's start to the pause.
    pub wall_s: u64,
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
