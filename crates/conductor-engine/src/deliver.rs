//! Delivery (SPEC §9, J5): a passed run becomes a pull request whose body is its receipt,
//! and whose last commit carries the run's record, so CI can re-check the receipt against
//! the code under review with nothing but git (`check_pr`).
//!
//! The branch a delivered run pushes looks like this:
//!
//! ```text
//!   base ── stage commits ── anchor (Conductor-Chain: <head>) ── record (Conductor-Receipt: <run>)
//! ```
//!
//! The record commit adds only `.conductor/receipts/<run>/`. A commit after it means the
//! code changed after it was checked, and `check_pr` fails.

use crate::store::RunDir;
use crate::worktree;
use conductor_model::{Chain, Event, Receipt, Verdict};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const RECEIPTS_DIR: &str = ".conductor/receipts";

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// The receipt as a pull request body.
pub fn markdown(r: &Receipt) -> String {
    let v = r.verdict();
    let mut s = format!(
        "## {} {} · conductor receipt `{}`\n\n",
        v.glyph(),
        v.word().to_uppercase(),
        r.run_id
    );
    s.push_str("### What was proven\n\n| | Claim | Evidence |\n|---|---|---|\n");
    for c in &r.checks {
        s.push_str(&format!(
            "| {} | {} | {} |\n",
            c.verdict.glyph(),
            c.claim.replace('|', "\\|"),
            c.detail.replace('|', "\\|")
        ));
    }
    if !r.survivors.is_empty() {
        s.push_str("\n### Look here first: bugs the tests missed\n\n");
        for x in &r.survivors {
            s.push_str(&format!("- `{}` {}\n", x.at, x.change));
        }
    }
    s.push_str("\n### Not checked\n\n");
    for n in &r.not_checked {
        s.push_str(&format!("- {n}\n"));
    }
    s.push_str("\n<details><summary>How it ran</summary>\n\n");
    for (k, val) in &r.how {
        s.push_str(&format!("- **{k}**: {val}\n"));
    }
    s.push_str(&format!(
        "- **record**: chain `{}`, anchored in {}\n\n</details>\n\n",
        r.integrity.chain_head,
        r.integrity.anchored_in.as_deref().unwrap_or("—")
    ));
    s.push_str(&format!(
        "The run's record is in `{RECEIPTS_DIR}/{}/`. `conductor check-pr` re-checks it against this branch; any commit after it fails that check.\n\n<!-- conductor-receipt run={} -->\n",
        r.run_id, r.run_id
    ));
    s
}

/// The worktree that holds a run's branch.
pub fn worktree_of(repo: &Path, run_id: &str) -> Result<PathBuf, String> {
    let branch = format!("refs/heads/conductor/{run_id}");
    let list = git(repo, &["worktree", "list", "--porcelain"])?;
    let mut path = None;
    for line in list.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if line.strip_prefix("branch ") == Some(branch.as_str()) {
            return path.ok_or_else(|| "git listed a branch before its worktree".into());
        }
    }
    Err(format!(
        "no worktree holds conductor/{run_id}; it may have been removed"
    ))
}

/// Commits the run's record on top of its anchor. Returns the new commit. Refuses unless the
/// run passed, its record verifies, and its branch still ends at the anchor.
pub fn commit_record(repo: &Path, run_id: &str) -> Result<String, String> {
    let dir = RunDir::for_run(repo, run_id);
    let receipt = crate::read_receipt(repo, run_id)?;
    if receipt.verdict() != Verdict::Passed {
        return Err(format!(
            "run {run_id} {}; only a passed run is delivered",
            receipt.verdict().word()
        ));
    }
    let v = crate::verify(repo, run_id);
    if !v.ok() {
        return Err(format!(
            "run {run_id} does not verify: {}",
            v.problems.join("; ")
        ));
    }
    let wt = worktree_of(repo, run_id)?;
    let anchor = receipt
        .integrity
        .anchored_in
        .clone()
        .ok_or("the receipt names no anchor commit")?;
    if anchor.len() < 7 {
        return Err(format!(
            "the receipt's anchor `{anchor}` is too short to identify a commit"
        ));
    }
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    let rel = format!("{RECEIPTS_DIR}/{run_id}");
    // The receipt keeps the anchor's short form.
    if !head.starts_with(&anchor) {
        let msg = git(&wt, &["log", "-1", "--format=%B", "HEAD"])?;
        if msg.contains(&format!("Conductor-Receipt: {run_id}")) {
            return Ok(head); // already committed by an earlier delivery
        }
        return Err(format!(
            "conductor/{run_id} has moved since the run ended (at {}, anchored at {}); \
             its receipt no longer describes it",
            &head[..12.min(head.len())],
            &anchor[..12.min(anchor.len())]
        ));
    }
    let out = wt.join(&rel);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    for (from, name) in [
        (dir.events(), "events.jsonl"),
        (dir.gates(), "gates.json"),
        (dir.receipt(), "receipt.json"),
    ] {
        std::fs::copy(&from, out.join(name))
            .map_err(|e| format!("copying {}: {e}", from.display()))?;
    }
    // Stage only the record: `-f` because a repository may ignore `.conductor/`.
    git(&wt, &["add", "-f", "--", &rel])?;
    let msg = format!(
        "conductor: receipt for run {run_id}\n\n{}\n\nConductor-Receipt: {run_id}",
        receipt.work
    );
    worktree::commit_as_conductor(&wt, &msg).map_err(|e| e.to_string())
}

/// `owner/repo` from a GitHub remote URL: https, ssh, or a proxy or enterprise host whose
/// path ends in the owner and repository.
pub fn github_slug(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let mut parts = url.rsplit(['/', ':']);
    let repo = parts.next().filter(|p| !p.is_empty())?;
    let owner = parts.next().filter(|p| !p.is_empty())?;
    Some(format!("{owner}/{repo}"))
}

/// A GitHub token: `GITHUB_TOKEN`, `GH_TOKEN`, or what `gh auth token` prints.
pub fn github_token() -> Option<String> {
    for k in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(t) = std::env::var(k)
            && !t.trim().is_empty()
        {
            return Some(t.trim().to_owned());
        }
    }
    let out = Command::new("gh").args(["auth", "token"]).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|t| !t.is_empty())
}

pub struct PullRequest<'a> {
    pub api: &'a str,
    pub token: &'a str,
    pub slug: &'a str,
    pub title: &'a str,
    pub head: &'a str,
    pub base: &'a str,
    pub body: &'a str,
    pub draft: bool,
}

/// Opens the pull request. Returns its URL.
pub fn open_pr(pr: &PullRequest) -> Result<String, String> {
    let url = format!("{}/repos/{}/pulls", pr.api.trim_end_matches('/'), pr.slug);
    let body = serde_json::json!({
        "title": pr.title,
        "head": pr.head,
        "base": pr.base,
        "body": pr.body,
        "draft": pr.draft,
    });
    let resp = crate::http::agent_for(&url, std::time::Duration::from_secs(30))
        .post(&url)
        .set("Authorization", &format!("Bearer {}", pr.token))
        .set("Accept", "application/vnd.github+json")
        .set("Content-Type", "application/json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .set("User-Agent", "conductor")
        .send_string(&body.to_string());
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            return Err(format!("GitHub answered {code}: {}", text.trim()));
        }
        Err(e) => return Err(format!("{url}: {e}")),
    };
    let text = resp
        .into_string()
        .map_err(|e| format!("reading GitHub's answer: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("GitHub's answer was not JSON: {e}"))?;
    v.get("html_url")
        .and_then(|u| u.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "GitHub's answer had no html_url".into())
}

/// One line of `check_pr`'s report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub ok: bool,
    pub what: String,
}

fn trailer(msg: &str, key: &str) -> Option<String> {
    msg.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .map(|v| v.trim().to_owned())
}

/// Re-checks, from git alone, that `head` is a delivered run's record commit and that its
/// receipt still describes the code: the check a pull request runs in CI.
pub fn check_pr(repo: &Path, head: &str) -> Result<Vec<Line>, String> {
    let mut out = Vec::new();
    let mut say = |ok: bool, what: String| out.push(Line { ok, what });

    // Find the newest record commit on the branch, and what came after it.
    let log = git(repo, &["log", "--format=%H", "-n", "200", head])?;
    let commits: Vec<&str> = log.lines().collect();
    let found = commits.iter().enumerate().find_map(|(i, sha)| {
        let msg = git(repo, &["log", "-1", "--format=%B", sha]).ok()?;
        trailer(&msg, "Conductor-Receipt").map(|run| (i, sha.to_string(), run))
    });
    let Some((after, record, run_id)) = found else {
        say(
            false,
            "no conductor receipt on this branch: deliver a run with `conductor deliver`".into(),
        );
        return Ok(out);
    };
    if after == 0 {
        say(true, format!("the branch ends at run {run_id}'s receipt"));
    } else {
        say(
            false,
            format!(
                "{after} commit(s) after run {run_id}'s receipt: the code changed after it was checked"
            ),
        );
    }

    // The record commit adds only the record.
    let rel = format!("{RECEIPTS_DIR}/{run_id}/");
    let touched = git(
        repo,
        &["diff", "--name-only", &format!("{record}^"), &record],
    )?;
    let stray: Vec<&str> = touched.lines().filter(|p| !p.starts_with(&rel)).collect();
    if stray.is_empty() {
        say(true, "the receipt commit adds only the run's record".into());
    } else {
        say(
            false,
            format!("the receipt commit also changes {}", stray.join(", ")),
        );
    }

    // Its parent is the run's anchor, whose trailer names the chain as it stood.
    let anchor = git(repo, &["rev-parse", &format!("{record}^")])?;
    let anchor_msg = git(repo, &["log", "-1", "--format=%B", &anchor])?;
    let chain_trailer = trailer(&anchor_msg, "Conductor-Chain");
    if trailer(&anchor_msg, "Conductor-Run").as_deref() == Some(run_id.as_str())
        && chain_trailer.is_some()
    {
        say(
            true,
            format!("its parent {} is the run's anchor", &anchor[..12]),
        );
    } else {
        say(
            false,
            "the receipt commit's parent is not the run's anchor".into(),
        );
    }

    let read = |name: &str| git(repo, &["show", &format!("{record}:{rel}{name}")]);
    let events: Vec<Event> = match read("events.jsonl") {
        Ok(text) => text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
        Err(e) => {
            say(false, format!("the record has no events: {e}"));
            return Ok(out);
        }
    };
    match Chain::verify(&events) {
        Ok(_) => say(
            true,
            format!("the event chain is intact ({} events)", events.len()),
        ),
        Err(e) => say(false, format!("the event chain is broken: {e}")),
    }
    let anchored = events.iter().enumerate().rev().find_map(|(i, e)| {
        e.what
            .strip_prefix("record anchored in commit ")
            .map(|sha| (i, sha.trim().to_owned()))
    });
    match anchored {
        Some((i, sha)) if sha == anchor => {
            let expected = if i == 0 {
                conductor_model::evidence::GENESIS.to_owned()
            } else {
                events[i - 1].hash.clone()
            };
            if chain_trailer.as_deref() == Some(expected.as_str()) {
                say(true, "the anchor's trailer matches the chain".into());
            } else {
                say(
                    false,
                    "the anchor's trailer does not match the chain".into(),
                );
            }
        }
        Some((_, sha)) => say(
            false,
            format!("the record says it was anchored in {sha}, not in this branch's anchor"),
        ),
        None => say(false, "the record never anchored".into()),
    }

    // Every stage commit the record names is in this branch's history.
    let named: Vec<&str> = events
        .iter()
        .filter_map(|e| e.what.split("; committed ").nth(1))
        .map(str::trim)
        .collect();
    let missing: Vec<&str> = named
        .iter()
        .copied()
        .filter(|sha| git(repo, &["merge-base", "--is-ancestor", sha, &anchor]).is_err())
        .collect();
    if missing.is_empty() {
        say(
            true,
            format!("all {} stage commits are in this branch", named.len()),
        );
    } else {
        say(
            false,
            format!("stage commits missing: {}", missing.join(", ")),
        );
    }

    match read("receipt.json")
        .ok()
        .and_then(|t| serde_json::from_str::<Receipt>(&t).ok())
    {
        Some(r) if r.run_id == run_id => {
            let v = r.verdict();
            say(
                v == Verdict::Passed,
                format!(
                    "the receipt's verdict, recomputed from its rows: {}",
                    v.word()
                ),
            );
        }
        _ => say(false, "the record has no readable receipt".into()),
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_remotes_name_their_repository() {
        for url in [
            "https://github.com/ashark-ai-05/conductor",
            "https://github.com/ashark-ai-05/conductor.git",
            "git@github.com:ashark-ai-05/conductor.git",
            "http://127.0.0.1:5000/git/ashark-ai-05/conductor",
        ] {
            assert_eq!(
                github_slug(url).as_deref(),
                Some("ashark-ai-05/conductor"),
                "{url}"
            );
        }
    }

    #[test]
    fn the_body_leads_with_the_verdict_and_carries_a_marker() {
        let r: Receipt = serde_json::from_str(include_str!(
            "../../../docs/examples/live-run-durations/receipt.json"
        ))
        .unwrap();
        let md = markdown(&r);
        assert!(md.starts_with("## ✓ PASSED · conductor receipt"), "{md}");
        assert!(md.contains("| ✓ |"), "{md}");
        assert!(md.contains("### Not checked"), "{md}");
        assert!(md.contains(&format!("<!-- conductor-receipt run={} -->", r.run_id)));
    }
}
