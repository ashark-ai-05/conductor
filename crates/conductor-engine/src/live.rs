//! The run as it stands, for the live screen: `.conductor/runs/<id>/live.json`.
//!
//! The engine rewrites it at every step (an attempt starts, a check lands, a stage ends), so
//! any process can show a run in progress: `conductor ui` in another terminal, or the status
//! pane in the run's herdr tab. It is a view, not evidence; the event chain is the record.

use crate::store::{Recorder, RunDir};
use conductor_model::Verdict;
use conductor_model::view::{CheckView, LiveRun, PaneView, StageView};
use conductor_model::workflow::{Stage, Workflow};
use std::path::{Path, PathBuf};

const FILE: &str = "live.json";

/// How many recent events the evidence feed shows.
const FEED: usize = 6;

pub struct Live {
    pub view: LiveRun,
    path: PathBuf,
}

/// The work's title: the task's first non-empty line, without Markdown heading marks.
pub fn title(task: &str) -> String {
    let line = task
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("untitled");
    let t = line.trim_start_matches('#').trim();
    if t.chars().count() > 72 {
        format!("{}…", t.chars().take(71).collect::<String>())
    } else {
        t.to_owned()
    }
}

impl Live {
    pub fn new(dir: &RunDir, run_id: &str, task: &str, wf: &Workflow, stages: &[&Stage]) -> Self {
        Live {
            view: LiveRun {
                run_id: run_id.into(),
                work: title(task),
                kind: wf.kind.name().into(),
                herdr_tab: None,
                stages: stages
                    .iter()
                    .map(|s| StageView {
                        name: s.id.clone(),
                        status: Verdict::Pending,
                        detail: s.agent.kind.clone(),
                    })
                    .collect(),
                checks: vec![],
                note: None,
                log: vec![],
                panes: vec![],
                ended: None,
                pid: Some(std::process::id()),
            },
            path: dir.root.join(FILE),
        }
    }

    /// herdr tab ids look like `w1:t3`; the screen shows the number.
    pub fn set_tab(&mut self, tab_id: &str) {
        self.view.herdr_tab = tab_id.rsplit_once(":t").and_then(|(_, n)| n.parse().ok());
    }

    pub fn stage(&mut self, id: &str, status: Verdict, detail: impl Into<String>) {
        if let Some(s) = self.view.stages.iter_mut().find(|s| s.name == id) {
            s.status = status;
            s.detail = detail.into();
        }
    }

    /// An attempt starts: its checks are pending again.
    pub fn attempt(&mut self, stage: &Stage, attempt: usize, how: &str) {
        let detail = if attempt == 1 {
            format!("{} · working", stage.agent.kind)
        } else {
            format!("{} · try {attempt}", stage.agent.kind)
        };
        self.stage(&stage.id, Verdict::Running, detail);
        self.view.checks = stage
            .all_gates()
            .map(|g| CheckView {
                name: g.name().into(),
                status: Verdict::Pending,
                detail: String::new(),
                progress: None,
            })
            .collect();
        if attempt > 1 {
            self.view.note = Some(format!(
                "`{}` is on try {attempt} ({how}): the last one didn't pass its checks",
                stage.id
            ));
        }
    }

    pub fn check(&mut self, index: usize, status: Verdict, detail: &str) {
        if let Some(c) = self.view.checks.get_mut(index) {
            c.status = status;
            c.detail = detail.to_owned();
        }
    }

    pub fn pane(&mut self, stage: &str, status: Verdict, open: bool, opened_by: &str) {
        match self
            .view
            .panes
            .iter_mut()
            .find(|p| p.stage == stage && p.opened_by == opened_by)
        {
            Some(p) => {
                p.status = status;
                p.open = open;
            }
            None => self.view.panes.push(PaneView {
                stage: stage.into(),
                status,
                open,
                opened_by: opened_by.into(),
            }),
        }
    }

    pub fn end(&mut self, verdict: Verdict) {
        self.view.ended = Some(verdict);
        self.view.note = None;
    }

    /// Refreshes the evidence feed from the record and rewrites the file. A failure to write
    /// the view never stops a run.
    pub fn save(&mut self, rec: &Recorder) {
        self.view.log.clear();
        let events = rec.events();
        for e in &events[events.len().saturating_sub(FEED)..] {
            let text = match &e.stage {
                Some(s) => format!("{s}: {}", e.what),
                None => e.what.clone(),
            };
            let at = e.at.get(11..19).unwrap_or(&e.at).to_owned();
            self.view.push_log(&at, e.source, &text);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&self.view) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

/// A run's live view, if it has one. A run whose conductor process is gone, and that never
/// finished, is shown as stopped rather than running forever.
pub fn read(repo: &Path, run_id: &str) -> Option<LiveRun> {
    let text = std::fs::read_to_string(RunDir::for_run(repo, run_id).root.join(FILE)).ok()?;
    let mut v: LiveRun = serde_json::from_str(&text).ok()?;
    if v.ended.is_none() && v.pid.is_some_and(|p| !alive(p)) {
        v.ended = Some(Verdict::Unwitnessed);
        v.note = Some("conductor stopped before the run finished; there is no receipt".into());
    }
    Some(v)
}

/// Runs with a live view and no receipt yet, newest first.
pub fn running(repo: &Path) -> Vec<LiveRun> {
    let mut ids: Vec<String> = std::fs::read_dir(repo.join(".conductor").join("runs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join(FILE).is_file() && !e.path().join("receipt.json").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids.reverse();
    ids.iter().filter_map(|id| read(repo, id)).collect()
}

fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // Signal 0 checks the process exists; EPERM means it does, owned by someone else.
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(e) => e == nix::errno::Errno::EPERM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_whose_process_is_gone_is_not_shown_as_running() {
        let d = tempfile::tempdir().unwrap();
        let dir = RunDir::create(d.path(), "R1").unwrap();
        let wf =
            conductor_model::Workflow::parse(include_str!("../../../examples/build.yaml")).unwrap();
        let mut live = Live::new(
            &dir,
            "R1",
            "# t",
            &wf,
            &wf.stages.iter().collect::<Vec<_>>(),
        );
        live.view.pid = Some(i32::MAX as u32);
        let rec = Recorder::open(&dir, None).unwrap();
        live.save(&rec);
        let got = running(d.path());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].ended, Some(Verdict::Unwitnessed));

        live.view.pid = Some(std::process::id());
        live.save(&rec);
        assert_eq!(running(d.path())[0].ended, None);
    }

    #[test]
    fn the_title_is_the_first_line_without_heading_marks() {
        assert_eq!(title("\n# Clamp values\n\nmore"), "Clamp values");
        assert_eq!(title(""), "untitled");
        assert!(title(&"x".repeat(200)).ends_with('…'));
    }
}
