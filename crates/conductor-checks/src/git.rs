//! The few git facts checks need. Read-only: nothing here changes a repository.

use std::path::Path;
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("could not run git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("`git {args}` failed: {stderr}")]
    Failed { args: String, stderr: String },
}

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    Ok(out.stdout)
}

fn split_nul(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

pub fn head(dir: &Path) -> Result<String, GitError> {
    Ok(String::from_utf8_lossy(&git(dir, &["rev-parse", "HEAD"])?)
        .trim()
        .to_owned())
}

/// Every path that differs from `base`: committed, staged, unstaged and untracked.
///
/// Renames are reported as a deletion plus an addition, so moving a frozen file away counts
/// as touching it.
pub fn changed_files(dir: &Path, base: &str) -> Result<Vec<String>, GitError> {
    let mut paths = split_nul(&git(
        dir,
        &["diff", "--name-only", "--no-renames", "-z", base],
    )?);
    paths.extend(split_nul(&git(
        dir,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?));
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// A file's contents at a commit, such as a workflow read from the base rather than from the
/// agent's worktree (SPEC §7).
pub fn show(dir: &Path, rev: &str, path: &str) -> Result<Vec<u8>, GitError> {
    git(dir, &["show", &format!("{rev}:{path}")])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(d.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        fs::create_dir_all(d.path().join("tests")).unwrap();
        fs::write(d.path().join("tests/locked.rs"), "a").unwrap();
        fs::write(d.path().join("README.md"), "r").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "base"]);
        d
    }

    #[test]
    fn edits_new_files_and_moves_all_count_as_changes() {
        let d = repo();
        let base = head(d.path()).unwrap();
        fs::write(d.path().join("README.md"), "changed").unwrap();
        fs::write(d.path().join("new.rs"), "n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(d.path())
            .args(["mv", "tests/locked.rs", "tests/moved.rs"])
            .output()
            .unwrap();
        let changed = changed_files(d.path(), &base).unwrap();
        assert_eq!(
            changed,
            vec!["README.md", "new.rs", "tests/locked.rs", "tests/moved.rs"]
        );
    }

    #[test]
    fn a_clean_tree_has_no_changes() {
        let d = repo();
        let base = head(d.path()).unwrap();
        assert!(changed_files(d.path(), &base).unwrap().is_empty());
    }

    #[test]
    fn a_file_is_read_as_it_was_at_the_base() {
        let d = repo();
        let base = head(d.path()).unwrap();
        fs::write(d.path().join("README.md"), "edited by an agent").unwrap();
        assert_eq!(show(d.path(), &base, "README.md").unwrap(), b"r");
    }

    #[test]
    fn a_bad_revision_is_an_error() {
        let d = repo();
        assert!(changed_files(d.path(), "no-such-rev").is_err());
    }
}
