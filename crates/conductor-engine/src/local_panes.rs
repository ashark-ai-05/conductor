//! Panes without herdr (SPEC §11.2, headless runs): each "pane" is a background process
//! group whose output goes to a file, so a workflow that opens panes runs unchanged in CI.
//!
//! A pane is `<run dir>/panes/<id>.log` plus `<id>.pids`, one process group per command run
//! in it. Processes never share the agent's pipes (stdin is null, output goes to the log), so
//! the agent's own run ends when the agent does, and conductor stops them at stage end.

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The pseudo tab id that tells `conductor pane` it is in a headless run.
pub const TAB: &str = "headless";

pub fn dir(actions: &Path) -> PathBuf {
    actions.with_file_name("panes")
}

fn log_path(dir: &Path, pane: &str) -> PathBuf {
    dir.join(format!("{pane}.log"))
}

fn pids_path(dir: &Path, pane: &str) -> PathBuf {
    dir.join(format!("{pane}.pids"))
}

/// Starts `command` under `sh -c` in its own process group, appending its output to the
/// pane's log. Returns the group's id.
pub fn run(dir: &Path, pane: &str, command: &str, cwd: &Path) -> Result<u32, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let open = |p: PathBuf| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .map_err(|e| format!("opening {}: {e}", p.display()))
    };
    let out = open(log_path(dir, pane))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let child = Command::new("sh")
        .args(["-c", command])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("starting `{command}`: {e}"))?;
    let pid = child.id();
    // The child is not waited on here: `conductor pane run` returns at once, and conductor
    // stops the group at stage end. Once this process exits, init reaps it.
    drop(child);
    writeln!(open(pids_path(dir, pane))?, "{pid}").map_err(|e| e.to_string())?;
    Ok(pid)
}

/// The last `lines` lines of a pane's output.
pub fn read(dir: &Path, pane: &str, lines: usize) -> String {
    let text = std::fs::read_to_string(log_path(dir, pane)).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    let mut out = all[all.len().saturating_sub(lines)..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn groups(dir: &Path, pane: &str) -> Vec<i32> {
    std::fs::read_to_string(pids_path(dir, pane))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// Whether a process group has a live member. A killed process stays a zombie until its
/// parent reaps it, and an orphan's new parent may be slow to, so on Linux zombies are not
/// counted. Elsewhere the group's existence is the answer.
fn group_alive(pgid: i32) -> bool {
    let exists = match killpg(Pid::from_raw(pgid), None) {
        Ok(()) => true,
        Err(e) => e == nix::errno::Errno::EPERM,
    };
    if !exists {
        return false;
    }
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return true;
    };
    procs.flatten().any(|e| {
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
            return false;
        };
        // `pid (comm) state ppid pgrp …`; comm may contain spaces, so split after the last ')'.
        let Some((_, rest)) = stat.rsplit_once(')') else {
            return false;
        };
        let mut f = rest.split_whitespace();
        let state = f.next();
        let pgrp = f.nth(1).and_then(|g| g.parse::<i32>().ok());
        pgrp == Some(pgid) && state != Some("Z")
    })
}

/// Whether anything started in the pane is still running.
pub fn running(dir: &Path, pane: &str) -> bool {
    groups(dir, pane).into_iter().any(group_alive)
}

/// Stops every process group started in the pane: SIGTERM, then SIGKILL after a grace.
/// Returns how many were still running.
pub fn stop(dir: &Path, pane: &str) -> usize {
    let live: Vec<i32> = groups(dir, pane)
        .into_iter()
        .filter(|g| group_alive(*g))
        .collect();
    for g in &live {
        let _ = killpg(Pid::from_raw(*g), Signal::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && live.iter().any(|g| group_alive(*g)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    for g in &live {
        if group_alive(*g) {
            let _ = killpg(Pid::from_raw(*g), Signal::SIGKILL);
        }
    }
    live.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_runs_in_the_background_and_is_stopped() {
        let d = tempfile::tempdir().unwrap();
        let panes = d.path().join("panes");
        let pid = run(
            &panes,
            "local-1",
            "echo started-$((40+2)); sleep 30",
            d.path(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !read(&panes, "local-1", 10).contains("started-42") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(read(&panes, "local-1", 10).contains("started-42"));
        assert!(running(&panes, "local-1"));
        assert_eq!(stop(&panes, "local-1"), 1);
        // In a run, `conductor pane run` has exited and init reaps the child. Here the test
        // is its parent, so it reaps it, or the zombie would still count as running.
        let _ = nix::sys::wait::waitpid(Pid::from_raw(pid as i32), None);
        assert!(!running(&panes, "local-1"));
    }

    #[test]
    fn reading_keeps_the_last_lines() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(log_path(d.path(), "p"), "a\nb\nc\n").unwrap();
        assert_eq!(read(d.path(), "p", 2), "b\nc\n");
        assert_eq!(read(d.path(), "missing", 2), "");
    }
}
