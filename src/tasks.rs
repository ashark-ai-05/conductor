//! Adapts existing build records and direct questions to the same workspace.
use super::RepoRuns;
use conductor_engine::{capabilities, test_results};
use conductor_engine::{question, store::RunDir};
use conductor_model::{
    Verdict,
    task::{Activity, State, TaskDetail, TaskSummary},
};
use conductor_tui::{app::RunSource, workspace::TaskSource};

fn state(v: Verdict) -> State {
    match v {
        Verdict::Running => State::Working,
        Verdict::Blocked => State::NeedsInput,
        Verdict::Passed | Verdict::Overridden => State::Finished,
        _ => State::Stopped,
    }
}

impl TaskSource for RepoRuns {
    fn host(&self) -> Option<Box<dyn conductor_tui::workspace::TaskHost>> {
        super::herdr_ui::host()
    }
    fn has_agent(&self, id: &str) -> bool {
        super::herdr_ui::has_agent(&self.0, id)
    }
    fn open_agent(&self, id: &str) -> Result<(), String> {
        super::herdr_ui::open_agent(&self.0, id).map_err(|e| e.to_string())
    }

    fn capabilities(&self) -> Result<Vec<conductor_model::capability::Available>, String> {
        Ok(capabilities::Registry::load(&self.0)?.available(&self.0))
    }
    fn invoke(
        &self,
        binding: &conductor_model::capability::Binding,
        parent: Option<&str>,
    ) -> Result<String, String> {
        capabilities::invoke(&self.0, binding, parent)
    }
    fn tasks(&self) -> Vec<TaskSummary> {
        let mut out: Vec<_> = question::list(&self.0)
            .into_iter()
            .map(|q| TaskSummary {
                id: q.id,
                title: q.title,
                kind: "Question".into(),
                state: q.turns.last().map(|t| t.state).unwrap_or(State::Stopped),
            })
            .collect();
        out.extend(
            capabilities::list(&self.0)
                .iter()
                .filter_map(|r| capabilities::detail(&self.0, r).ok().map(|d| d.summary)),
        );
        for id in conductor_engine::list_runs_any(&self.0) {
            let live = self.live(&id);
            let receipt = conductor_engine::read_receipt(&self.0, &id).ok();
            if let Some(l) = &live {
                out.push(TaskSummary {
                    id,
                    title: l.work.clone(),
                    kind: l.kind.clone(),
                    state: if l.waiting.is_some() {
                        State::NeedsInput
                    } else {
                        state(l.ended.unwrap_or(Verdict::Running))
                    },
                });
            } else if let Some(r) = receipt {
                let verdict = if r.checks.is_empty() {
                    Verdict::Failed
                } else {
                    r.verdict()
                };
                out.push(TaskSummary {
                    id,
                    title: r.work,
                    kind: r.kind,
                    state: state(verdict),
                });
            }
        }
        out.sort_by(|a, b| {
            let priority = |s: State| match s {
                State::NeedsInput => 0,
                State::Working => 1,
                _ => 2,
            };
            priority(a.state).cmp(&priority(b.state)).then_with(|| {
                b.id.trim_start_matches("q-")
                    .trim_start_matches("n-")
                    .cmp(a.id.trim_start_matches("q-").trim_start_matches("n-"))
            })
        });
        out
    }

    fn task(&self, id: &str) -> Option<TaskDetail> {
        if id.starts_with("n-") {
            return capabilities::read(&self.0, id)
                .and_then(|r| capabilities::detail(&self.0, &r))
                .ok();
        }
        if id.starts_with("q-") {
            let q = question::read(&self.0, id)?;
            return Some(TaskDetail {
                native: None,
                summary: TaskSummary {
                    id: id.into(),
                    title: q.title.clone(),
                    kind: "Question".into(),
                    state: q.turns.last().map(|t| t.state).unwrap_or(State::Stopped),
                },
                context: "Web question · No project files supplied".into(),
                body: String::new(),
                evidence: vec![],
                activity: vec![],
                waiting: None,
                question: Some(q),
                tests: None,
                changes: vec![],
            });
        }
        let live = self.live(id);
        let receipt = conductor_engine::read_receipt(&self.0, id).ok();
        let review = self.review(id);
        if live.is_none() && receipt.is_none() {
            return None;
        }
        let events = RunDir::for_run(&self.0, id)
            .read_events()
            .unwrap_or_default();
        let activity: Vec<_> = events
            .iter()
            .map(|e| Activity {
                at: e.at.clone(),
                actor: e.source.label().into(),
                title: e.what.clone(),
                detail: format!(
                    "{}\n\nStage: {}\nRecorded as: {}\nEvent: {}",
                    e.what,
                    e.stage.as_deref().unwrap_or("run"),
                    e.source,
                    e.seq
                ),
                failed: e.what.contains("failed") || e.what.contains("halted"),
            })
            .collect();
        let title = live
            .as_ref()
            .map(|l| l.work.clone())
            .or_else(|| receipt.as_ref().map(|r| r.work.clone()))?;
        let kind = live
            .as_ref()
            .map(|l| l.kind.clone())
            .or_else(|| receipt.as_ref().map(|r| r.kind.clone()))
            .unwrap_or_default();
        let waiting = live.as_ref().and_then(|l| l.waiting.clone());
        let status = if waiting.is_some() {
            State::NeedsInput
        } else if let Some(l) = &live {
            state(l.ended.unwrap_or(Verdict::Running))
        } else {
            state(
                receipt
                    .as_ref()
                    .map(|r| r.verdict())
                    .unwrap_or(Verdict::Failed),
            )
        };
        let context = match live.as_ref().and_then(|l| l.herdr_tab) {
            Some(tab) => format!("{kind} · Herdr tab {tab}"),
            None => kind.clone(),
        };
        let mut body = String::new();
        if status == State::Stopped {
            if let Some(c) = receipt
                .as_ref()
                .and_then(|r| r.checks.iter().find(|c| c.verdict != Verdict::Passed))
            {
                body.push_str(&format!("## Stopped\n{}\n{}\n\n", c.claim, c.detail));
            } else if let Some(a) = activity.iter().rev().find(|a| a.failed) {
                body.push_str(&format!("## Stopped\n{}\n\n", a.title));
            }
        }
        if let Some(l) = &live {
            body.push_str(
                &l.stages
                    .iter()
                    .map(|s| format!("{} {}", s.status.glyph(), s.name))
                    .collect::<Vec<_>>()
                    .join("  →  "),
            );
            body.push_str("\n\n");
            if let Some(note) = &l.note {
                body.push_str(note);
                body.push_str("\n\n");
            }
        }
        if let Some(r) = &review {
            for p in &r.produced {
                body.push_str(&format!("## {}\n{}\n\n", p.path, p.lines.join("\n")));
                if p.total > p.lines.len() {
                    body.push_str(&format!(
                        "Showing {} of {} lines from the saved review.\n\n",
                        p.lines.len(),
                        p.total
                    ));
                }
            }
            if !r.criteria.is_empty() {
                body.push_str("## Acceptance criteria\n");
                for c in &r.criteria {
                    body.push_str(&format!("- {c}\n"));
                }
                body.push('\n');
            }
            if !r.change.files.is_empty() {
                body.push_str(&format!(
                    "## Changes\n{} files · +{} −{}\n{}\n\n",
                    r.change.files.len(),
                    r.change.added,
                    r.change.removed,
                    r.change.files.join("\n")
                ));
            }
        }
        let mut evidence = vec![];
        if let Some(r) = &receipt {
            for c in &r.checks {
                evidence.push((
                    format!("{} {}", c.verdict.glyph(), c.claim),
                    format!("{} · {}\n{}", c.verdict.word(), c.source, c.detail),
                ));
            }
            if !r.not_checked.is_empty() {
                evidence.push(("Limitations".into(), r.not_checked.join("\n")));
            }
            evidence.push((
                "Run record".into(),
                format!(
                    "Run {id}\n{}\n\n{}",
                    r.how
                        .iter()
                        .map(|(k, v)| format!("{k}: {v}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    r.also_at.join("\n")
                ),
            ));
            body.push_str(&format!(
                "## Check results\n{} of {} checks passed.\n",
                r.passed_count(),
                r.checks.len()
            ));
            for c in r.checks.iter().filter(|c| c.verdict != Verdict::Passed) {
                body.push_str(&format!("{}: {}\n", c.claim, c.detail));
            }
            if r.checks.is_empty() {
                body.push_str("No checks completed. Open Activity for the stopping point.\n");
            }
        } else if let Some(r) = &review {
            for e in &r.evidence {
                evidence.push((
                    format!("{} {}", e.after.glyph(), e.claim),
                    format!(
                        "{}\n{}\n{}",
                        conductor_engine::review::before_after(e.before, e.after),
                        e.detail,
                        e.lines.join("\n")
                    ),
                ));
            }
        } else if let Some(l) = &live {
            for c in &l.checks {
                evidence.push((format!("{} {}", c.status.glyph(), c.name), c.detail.clone()));
            }
        }
        if status == State::Working
            && let Some(a) = activity.last()
        {
            body.push_str(&format!("\n{}\n", a.title));
        }
        let dir = RunDir::for_run(&self.0, id);
        let tests = test_results::read(&dir, &mut evidence);
        let changes = test_results::changes(&dir);
        if let Some(report) = &tests {
            body.push_str("\n## Recorded test cases\nLatest completed attempt per stage; latest parsed report per check.\n");
            body.push_str(&report.notices.join("\n"));
            for case in &report.cases {
                body.push_str(&format!(
                    "\n{} · {} · {} / {}",
                    case.outcome, case.name, case.stage, case.attempt
                ));
            }
        }
        Some(TaskDetail {
            native: None,
            summary: TaskSummary {
                id: id.into(),
                title,
                kind,
                state: status,
            },
            context,
            body,
            evidence,
            activity,
            waiting,
            question: None,
            tests,
            changes,
        })
    }

    fn question(&self, parent: Option<&str>, input: &str) -> Result<String, String> {
        question::launch(
            &self.0,
            &std::env::current_exe().map_err(|e| e.to_string())?,
            parent,
            input,
        )
    }
    fn respond(
        &self,
        response: &conductor_model::interaction::InputResponse,
    ) -> Result<String, String> {
        question::respond(
            &self.0,
            &std::env::current_exe().map_err(|e| e.to_string())?,
            response,
        )
    }
    fn cancel_question(&self, id: &str) -> Result<(), String> {
        question::cancel(&self.0, id)
    }
    fn open_source(&self, url: &str) -> Result<(), String> {
        if !conductor_model::task::web_url(url) {
            return Err("This source has no valid http or https link.".into());
        }
        let program = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let mut child = std::process::Command::new(program)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
    fn copy(&self, text: &str) -> Result<(), String> {
        use std::io::Write;
        if !cfg!(target_os = "macos") {
            return Err("Clipboard copy currently requires macOS.".into());
        }
        let mut child = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child
            .stdin
            .take()
            .ok_or("Clipboard input unavailable.")?
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string())?;
        if child.wait().map_err(|e| e.to_string())?.success() {
            Ok(())
        } else {
            Err("Could not copy the answer.".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_model::{
        demo,
        view::{Change, Cost, Produced, Review},
    };

    #[test]
    fn a_completed_build_keeps_its_output_and_its_own_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = RunDir::create(tmp.path(), "R1").unwrap();
        let mut receipt = demo::receipt();
        receipt.run_id = "R1".into();
        receipt.work = "A finished build".into();
        std::fs::write(dir.receipt(), serde_json::to_vec(&receipt).unwrap()).unwrap();
        std::fs::write(dir.events(), "").unwrap();
        let review = Review {
            run_id: "R1".into(),
            stage: "review".into(),
            who: "PO".into(),
            title: "A finished build".into(),
            criteria: vec!["Meets the request".into()],
            change: Change::default(),
            evidence: vec![],
            cost: Cost::default(),
            produced: vec![Produced {
                path: "result.md".into(),
                lines: vec!["Durable result content".into()],
                total: 1,
            }],
        };
        conductor_engine::review::write(&dir, &review).unwrap();
        let src = RepoRuns(tmp.path().to_owned());
        let task = src.task("R1").unwrap();
        assert!(task.body.contains("Durable result content"));
        assert!(task.body.contains("Meets the request"));
        assert!(task.waiting.is_none());
        assert_eq!(task.summary.id, "R1");
        assert!(task.evidence.iter().any(|(t, _)| t == "Run record"));
    }

    #[test]
    fn a_startup_failure_without_checks_is_still_a_task() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = RunDir::create(tmp.path(), "R2").unwrap();
        let mut receipt = demo::receipt();
        receipt.run_id = "R2".into();
        receipt.checks.clear();
        std::fs::write(dir.receipt(), serde_json::to_vec(&receipt).unwrap()).unwrap();
        std::fs::write(dir.events(), "").unwrap();
        let src = RepoRuns(tmp.path().to_owned());
        assert_eq!(src.tasks().len(), 1);
        assert_eq!(src.tasks()[0].state, State::Stopped);
        assert!(src.task("R2").unwrap().body.contains("No checks completed"));
    }
}
