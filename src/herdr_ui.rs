//! Herdr owns navigation and attention; Conductor owns task results and check verdicts.
use anyhow::{Context, Result, bail};
use conductor_engine::herdr_exec::Binding;
use conductor_herdr::{Herdr, Target};
use conductor_model::task::State;
use conductor_tui::workspace::TaskHost;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    sync::mpsc,
    thread,
};

const SOURCE: &str = "custom:conductor";
const PLUGIN: &str = "conductor.tasks";

fn inside() -> bool {
    std::env::var("HERDR_ENV").as_deref() == Ok("1")
}

fn pane(h: &Herdr, id: &str) -> Result<Value> {
    let v = h.call(&["pane", "get", id])?;
    v.pointer("/result/pane")
        .cloned()
        .context("Herdr did not return a pane")
}

fn lifecycle(state: State) -> &'static str {
    match state {
        State::Working => "working",
        State::NeedsInput => "blocked",
        _ => "idle",
    }
}

struct Host {
    tx: Option<mpsc::Sender<(String, String, State)>>,
    worker: Option<thread::JoinHandle<()>>,
    last: Option<(String, String, State)>,
}

impl TaskHost for Host {
    fn publish(&mut self, id: &str, title: &str, state: State) {
        let next = (id.to_owned(), title.to_owned(), state);
        if self.last.as_ref() != Some(&next) {
            if let Some(tx) = &self.tx {
                let _ = tx.send(next.clone());
            }
            self.last = Some(next);
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn host() -> Option<Box<dyn TaskHost>> {
    if !inside() {
        return None;
    }
    let own = std::env::var("HERDR_PANE_ID").ok()?;
    let h = super::herdr_handle();
    let socket = h.socket_path().ok()?;
    let h = Herdr::with(h.bin(), Target::Socket(socket));
    let (tx, rx) = mpsc::channel::<(String, String, State)>();
    let worker = thread::spawn(move || {
        // One ordered publisher, separate from the UI loop. Only our pane is reported.
        let mut reported = false;
        for (id, title, state) in rx {
            let title: String = title.chars().filter(|c| !c.is_control()).take(72).collect();
            if id.is_empty() {
                let _ = h.call(&[
                    "pane",
                    "report-metadata",
                    &own,
                    "--source",
                    SOURCE,
                    "--clear-title",
                    "--clear-token",
                    "conductor_task",
                ]);
            } else {
                let token = format!("conductor_task={id}");
                let _ = h.call(&[
                    "pane",
                    "report-metadata",
                    &own,
                    "--source",
                    SOURCE,
                    "--title",
                    &title,
                    "--token",
                    &token,
                ]);
            }
            reported |= h
                .call(&[
                    "pane",
                    "report-agent",
                    &own,
                    "--source",
                    SOURCE,
                    "--agent",
                    "conductor",
                    "--state",
                    lifecycle(state),
                    "--message",
                    state.label(),
                ])
                .is_ok();
        }
        if reported {
            let _ = h.call(&[
                "pane",
                "release-agent",
                &own,
                "--source",
                SOURCE,
                "--agent",
                "conductor",
            ]);
        }
        let _ = h.call(&[
            "pane",
            "report-metadata",
            &own,
            "--source",
            SOURCE,
            "--clear-title",
            "--clear-token",
            "conductor_task",
        ]);
    });
    Some(Box::new(Host {
        tx: Some(tx),
        worker: Some(worker),
        last: None,
    }))
}

pub fn has_agent(repo: &Path, id: &str) -> bool {
    inside() && Binding::read(repo, id).is_some_and(|b| !b.stages.is_empty())
}

pub fn open_agent(repo: &Path, id: &str) -> Result<()> {
    if !inside() {
        bail!("Open this task inside Herdr to visit its agent.");
    }
    let own = std::env::var("HERDR_PANE_ID").context("No current Herdr pane")?;
    let h = super::herdr_handle();
    let binding = Binding::read(repo, id).context("This task has no saved Herdr panes")?;
    if h.socket_path()? != binding.socket {
        bail!("This task belongs to another Herdr session.");
    }
    // Check both endpoints immediately before navigation; a completed pane may have closed.
    pane(&h, &own)?;
    let candidates = binding.current.iter().chain(binding.stages.values());
    for target in candidates {
        if pane(&h, target).is_ok_and(|p| {
            p["tab_id"].as_str() == Some(&binding.tab)
                && p.pointer("/tokens/conductor_run").and_then(Value::as_str) == Some(id)
        }) {
            h.call(&[
                "pane",
                "report-metadata",
                target,
                "--source",
                SOURCE,
                "--token",
                &format!("conductor_return={own}"),
                "--token",
                &format!("conductor_return_task={id}"),
            ])?;
            h.focus_pane(target)?;
            return Ok(());
        }
    }
    bail!("The agent pane has closed. Its output and checks remain in this task.")
}

fn project_path(context: &Value) -> Result<PathBuf> {
    // Actions execute in the plugin directory. Never mistake it for the user's project.
    context
        .get("focused_pane_cwd")
        .and_then(Value::as_str)
        .or_else(|| context.get("workspace_cwd").and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .context("Open this action from a project pane")
}

fn repo_at(path: &Path) -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if output.status.success() {
        Ok(PathBuf::from(String::from_utf8(output.stdout)?.trim()))
    } else {
        Ok(path.canonicalize()?)
    }
}

pub fn dispatch(args: &[String]) -> Result<ExitCode> {
    let action = args.first().map(String::as_str).unwrap_or("");
    if args.len() != 1
        || !matches!(
            action,
            "ask" | "tasks" | "return" | "show-ask" | "show-tasks"
        )
    {
        bail!("usage: conductor herdr ask|tasks|return");
    }
    if !inside() {
        bail!("Run this command inside a Herdr pane.");
    }
    if action.starts_with("show-") {
        let root = std::env::var("CONDUCTOR_PROJECT_ROOT")
            .context("Plugin pane has no project context")?;
        let h = super::herdr_handle();
        let own = std::env::var("HERDR_PANE_ID")?;
        if action == "show-tasks" {
            let info = pane(&h, &own)?;
            let tab = info["tab_id"]
                .as_str()
                .context("No tab for project tasks")?;
            let project = Path::new(&root)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("project");
            h.call(&["tab", "rename", tab, &format!("Tasks · {project}")])?;
            h.call(&[
                "pane",
                "report-metadata",
                &own,
                "--source",
                SOURCE,
                "--token",
                "conductor_home=1",
            ])?;
        }
        let result = super::ui(&[
            if action == "show-ask" {
                "--ask"
            } else {
                "--tasks"
            }
            .into(),
            "--repo".into(),
            root,
        ]);
        if action == "show-tasks" {
            let _ = h.call(&[
                "pane",
                "report-metadata",
                &own,
                "--source",
                SOURCE,
                "--clear-token",
                "conductor_home",
            ]);
        }
        return result;
    }
    let h = super::herdr_handle();
    let context: Value = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or(Value::Null);
    let own = context["focused_pane_id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| std::env::var("HERDR_PANE_ID").ok())
        .context("No current Herdr pane")?;
    let current = pane(&h, &own)?;
    if action == "return" {
        let target = current
            .pointer("/tokens/conductor_return")
            .and_then(Value::as_str)
            .context("Visit an agent with 'g Open agent' first")?;
        let back =
            pane(&h, target).context("The task view has closed; reopen it from project tasks")?;
        if back
            .pointer("/tokens/conductor_task")
            .and_then(Value::as_str)
            != current
                .pointer("/tokens/conductor_return_task")
                .and_then(Value::as_str)
            || back
                .pointer("/tokens/conductor_task")
                .and_then(Value::as_str)
                .is_none()
        {
            bail!(
                "The task view has closed or moved to another task; reopen it from project tasks"
            );
        }
        h.focus_pane(target)?;
        return Ok(ExitCode::SUCCESS);
    }
    let cwd = project_path(&context).or_else(|_| {
        current
            .get("foreground_cwd")
            .or_else(|| current.get("cwd"))
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .context("This pane has no working directory")
    })?;
    let repo = repo_at(&cwd)?;
    let workspace = current["workspace_id"]
        .as_str()
        .context("No workspace for this pane")?;
    // Reuse the project's home in this workspace. Metadata is tied to the live UI pane.
    if action == "tasks" {
        for p in h.panes()? {
            let Ok(info) = pane(&h, &p.pane_id) else {
                continue;
            };
            if info["workspace_id"].as_str() == Some(workspace)
                && info
                    .pointer("/tokens/conductor_home")
                    .and_then(Value::as_str)
                    == Some("1")
                && p.cwd.as_deref().is_some_and(|cwd| Path::new(cwd) == repo)
            {
                h.focus_pane(&p.pane_id)?;
                return Ok(ExitCode::SUCCESS);
            }
        }
    }
    let root = repo.to_str().context("Project path is not UTF-8")?;
    if action == "ask" {
        h.focus_pane(&own)?;
    }
    let project_env = format!("CONDUCTOR_PROJECT_ROOT={root}");
    let mut argv = vec![
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN,
        "--entrypoint",
        action,
        "--placement",
        if action == "ask" { "overlay" } else { "tab" },
        "--cwd",
        root,
        "--env",
        &project_env,
        "--focus",
    ];
    if action == "tasks" {
        argv.extend(["--workspace", workspace]);
    }
    h.call(&argv)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pane_context_beats_plugin_cwd_and_missing_context_is_an_error() {
        let c = serde_json::json!({"focused_pane_cwd":"/project/nested", "workspace_cwd":"/other"});
        assert_eq!(project_path(&c).unwrap(), PathBuf::from("/project/nested"));
        assert!(project_path(&Value::Null).is_err());
    }
    #[test]
    fn attention_is_lifecycle_not_a_check_verdict() {
        assert_eq!(lifecycle(State::NeedsInput), "blocked");
        assert_eq!(lifecycle(State::Working), "working");
        for s in [
            State::Finished,
            State::Stopped,
            State::Cancelled,
            State::Answered,
        ] {
            assert_eq!(lifecycle(s), "idle");
        }
    }
}
