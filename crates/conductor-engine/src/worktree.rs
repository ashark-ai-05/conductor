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

/// Adds a worktree for the run under the repository's git directory, where `git status` in
/// the user's checkout never sees it, on a branch named for the run.
pub fn create(repo: &Path, run_id: &str, base: &str) -> Result<(PathBuf, String), GitError> {
    let common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let path = PathBuf::from(common)
        .join("conductor")
        .join("worktrees")
        .join(run_id);
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
