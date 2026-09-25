//! Starting a run from somewhere other than a shell: the TUI's launch tab.
//!
//! The run is `conductor run` in the background, so it outlives the screen that started
//! it. Its output goes to `.conductor/runs/launch.log`, and its id is the run directory
//! that appears.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

/// The repository's workflows, as paths relative to it, sorted.
pub fn workflows(repo: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(repo.join(".conductor/workflows"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".yaml") || n.ends_with(".yml"))
        .map(|n| format!(".conductor/workflows/{n}"))
        .collect();
    out.sort();
    out
}

/// Starts `bin run <workflow>` on `input`: `--spec` when it names a file in the repository,
/// else `-m`. `env` is added to the run's environment. Returns the new run's id once its
/// directory exists.
pub fn start(
    repo: &Path,
    bin: &Path,
    workflow: &str,
    input: &str,
    herdr: bool,
    env: &[(&str, &str)],
) -> Result<String, String> {
    let before: BTreeSet<String> = crate::list_runs_any(repo).into_iter().collect();
    let runs = repo.join(".conductor").join("runs");
    std::fs::create_dir_all(&runs)
        .map_err(|e| format!("could not create {}: {e}", runs.display()))?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(runs.join("launch.log"))
        .map_err(|e| format!("could not open .conductor/runs/launch.log: {e}"))?;
    let mut cmd = std::process::Command::new(bin);
    cmd.envs(env.iter().copied());
    cmd.arg("run").arg(workflow);
    if repo.join(input).is_file() {
        cmd.arg("--spec").arg(input);
    } else {
        cmd.arg("-m").arg(input);
    }
    cmd.arg("--executor")
        .arg(if herdr { "herdr" } else { "headless" })
        .current_dir(repo)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start conductor: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(id) = crate::list_runs_any(repo)
            .into_iter()
            .find(|id| !before.contains(id))
        {
            return Ok(id);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "conductor run ended ({status}) before recording a run; see .conductor/runs/launch.log"
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("the run did not appear within 15s; see .conductor/runs/launch.log".into())
}
