//! The run's own git worktree: created from the base, one commit per passed stage, reset
//! between fresh attempts. The user's checkout is never touched.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("could not run git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("`git {args}` failed: {stderr}")]
    Failed { args: String, stderr: String },
}

fn git(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=conductor",
            "-c",
            "user.email=conductor@localhost",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn head(dir: &Path) -> Result<String, GitError> {
    git(dir, &["rev-parse", "HEAD"])
}

pub fn resolve(repo: &Path, rev: &str) -> Result<String, GitError> {
    git(
        repo,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )
}

/// Where conductor keeps its own state outside any repository: `$CONDUCTOR_HOME`, else
/// `$XDG_DATA_HOME/conductor`, else `~/.local/share/conductor`.
pub fn conductor_home() -> PathBuf {
    if let Some(h) = std::env::var_os("CONDUCTOR_HOME") {
        return PathBuf::from(h);
    }
    if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(x).join("conductor");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".local").join("share").join("conductor")
}

/// Adds a worktree for the run on a branch named for it.
///
/// The worktree lives outside the repository. Inside `.git` an agent may refuse to write at
/// all (Claude Code protects `.git`), and inside the checkout it would show up in
/// `git status` and could be mistaken for part of the project by build tools.
pub fn create(
    repo: &Path,
    run_id: &str,
    base: &str,
    home: &Path,
) -> Result<(PathBuf, String), GitError> {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let path = home.join("worktrees").join(&name).join(run_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let branch = format!("conductor/{run_id}");
    git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &branch,
            &path.display().to_string(),
            base,
        ],
    )?;
    Ok((path, branch))
}

/// Commits everything in the worktree. Returns the new head, or the old one if nothing
/// changed.
pub fn commit_all(wt: &Path, message: &str) -> Result<String, GitError> {
    git(wt, &["add", "-A"])?;
    if git(wt, &["status", "--porcelain"])?.is_empty() {
        return head(wt);
    }
    git(wt, &["commit", "-q", "-m", message])?;
    head(wt)
}

/// An empty commit whose trailer carries the chain head: the anchor (SPEC §11.3).
pub fn anchor(wt: &Path, run_id: &str, chain_head: &str) -> Result<String, GitError> {
    let msg = format!(
        "conductor: run {run_id} record\n\nConductor-Run: {run_id}\nConductor-Chain: {chain_head}"
    );
    git(wt, &["commit", "-q", "--allow-empty", "-m", &msg])?;
    head(wt)
}

/// Commits what is staged, as conductor. Returns the new commit.
pub fn commit_as_conductor(wt: &Path, msg: &str) -> Result<String, GitError> {
    git(wt, &["commit", "-q", "-m", msg])?;
    head(wt)
}

/// Everything the worktree changed since `base`, as one patch: tracked changes, plus each
/// new file against nothing, so an attempt's diff shows what it added as well as what it
/// edited.
pub fn diff_from(wt: &Path, base: &str) -> Result<String, GitError> {
    let mut patch = git(wt, &["diff", "--no-ext-diff", "--no-color", base, "--"])?;
    if !patch.is_empty() && !patch.ends_with('\n') {
        patch.push('\n');
    }
    let untracked = git(wt, &["ls-files", "--others", "--exclude-standard"])?;
    for f in untracked.lines().filter(|l| !l.is_empty()) {
        // `--no-index` exits 1 when the files differ, which they always do here.
        let out = Command::new("git")
            .arg("-C")
            .arg(wt)
            .args([
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--no-index",
                "--",
                "/dev/null",
                f,
            ])
            .output()
            .map_err(GitError::Spawn)?;
        patch.push_str(&String::from_utf8_lossy(&out.stdout));
    }
    Ok(patch)
}

/// Puts the worktree back exactly as it was at `sha`, keeping ignored files such as build
/// output so a fresh attempt doesn't rebuild from nothing.
pub fn reset(wt: &Path, sha: &str) -> Result<(), GitError> {
    git(wt, &["reset", "-q", "--hard", sha])?;
    git(wt, &["clean", "-q", "-fd"])?;
    Ok(())
}

pub fn show(repo: &Path, rev: &str, path: &str) -> Result<String, GitError> {
    git(repo, &["show", &format!("{rev}:{path}")])
}
