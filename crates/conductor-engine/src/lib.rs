//! The conductor engine: runs a workflow and records everything it can prove.
//!
//! No model calls happen here. Agents are driven through [`executor`]s; every decision about
//! what happens next is a rule over recorded facts.

pub mod engine;
pub mod executor;
pub mod receipt;
pub mod store;
pub mod worktree;

pub use engine::{EngineError, Options, Outcome, run};

use conductor_model::{Chain, Receipt, Verdict};
use std::path::Path;

/// What `conductor verify` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    /// The event chain on disk is intact.
    pub chain_intact: bool,
    pub chain_head: Option<String>,
    /// The anchor commit's trailer matches the chain as it stood when anchored.
    pub anchored: bool,
    /// The verdict recomputed from the receipt's rows.
    pub verdict: Option<Verdict>,
    pub problems: Vec<String>,
}

impl Verification {
    pub fn ok(&self) -> bool {
        self.chain_intact && self.anchored && self.problems.is_empty()
    }
}

/// Re-checks a stored run with no model calls and no live queries.
pub fn verify(repo: &Path, run_id: &str) -> Verification {
    let dir = store::RunDir::for_run(repo, run_id);
    let mut v = Verification {
        chain_intact: false,
        chain_head: None,
        anchored: false,
        verdict: None,
        problems: vec![],
    };
    let events = match dir.read_events() {
        Ok(e) => e,
        Err(e) => {
            v.problems.push(format!("could not read events: {e}"));
            return v;
        }
    };
    match Chain::verify(&events) {
        Ok(head) => {
            v.chain_intact = true;
            v.chain_head = Some(head);
        }
        Err(e) => v.problems.push(e.to_string()),
    }
    // The anchor event names the commit; that commit's trailer must name the hash of the
    // event just before it.
    if let Some((i, anchor_event)) = events
        .iter()
        .enumerate()
        .rev()
        .find(|(_, e)| e.what.starts_with("record anchored in commit "))
    {
        let sha = anchor_event
            .what
            .trim_start_matches("record anchored in commit ")
            .trim();
        let expected = if i == 0 {
            conductor_model::evidence::GENESIS.to_owned()
        } else {
            events[i - 1].hash.clone()
        };
        match std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["log", "-1", "--format=%B", sha])
            .output()
        {
            Ok(o) if o.status.success() => {
                let body = String::from_utf8_lossy(&o.stdout);
                let trailer = body
                    .lines()
                    .find_map(|l| l.strip_prefix("Conductor-Chain: "))
                    .map(str::trim);
                if trailer == Some(expected.as_str()) {
                    v.anchored = true;
                } else {
                    v.problems.push(format!(
                        "commit {sha} carries chain {trailer:?}, but the record says {expected}"
                    ));
                }
            }
            _ => v
                .problems
                .push(format!("the anchor commit {sha} is not in this repository")),
        }
    } else {
        v.problems.push("the run was never anchored".into());
    }
    match read_receipt(repo, run_id) {
        Ok(r) => v.verdict = Some(r.verdict()),
        Err(e) => v.problems.push(e),
    }
    v
}

pub fn read_receipt(repo: &Path, run_id: &str) -> Result<Receipt, String> {
    let dir = store::RunDir::for_run(repo, run_id);
    let text = std::fs::read_to_string(dir.receipt())
        .map_err(|e| format!("no receipt for run {run_id}: {e}"))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("the receipt for {run_id} could not be read: {e}"))
}

/// Run ids in the repository that have a receipt, newest first.
pub fn list_runs(repo: &Path) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(repo.join(".conductor").join("runs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("receipt.json").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids.reverse();
    ids
}
