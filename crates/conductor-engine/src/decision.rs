//! A person's decision on a `human` stage: `conductor approve` or `reject` writes it, and
//! the run, waiting on it, reads it. One JSON file per stage under the run's record, so the
//! decision is part of the record and survives the process that waited for it.

use crate::store::RunDir;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub approved: bool,
    pub by: String,
    #[serde(default)]
    pub note: String,
    pub at: String,
}

pub fn path(dir: &RunDir, stage: &str) -> PathBuf {
    dir.root.join("decisions").join(format!("{stage}.json"))
}

pub fn read(dir: &RunDir, stage: &str) -> Option<Decision> {
    let text = std::fs::read_to_string(path(dir, stage)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Records the decision. A decision with no note is refused: the record has to say what was
/// checked, or why not. A second decision on the same stage is refused: the first one is
/// what the run acted on.
pub fn write(dir: &RunDir, stage: &str, d: &Decision) -> std::io::Result<()> {
    if d.note.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a decision needs a note: what you checked, or why not",
        ));
    }
    let p = path(dir, stage);
    if p.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("`{stage}` was already decided"),
        ));
    }
    std::fs::create_dir_all(p.parent().unwrap_or(&dir.root))?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(d)?)?;
    std::fs::rename(tmp, p)
}

impl Decision {
    pub fn word(&self) -> &'static str {
        if self.approved {
            "approved"
        } else {
            "rejected"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decision_without_a_note_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let dir = RunDir::create(d.path(), "R1").unwrap();
        let mut dec = Decision {
            approved: true,
            by: "PO".into(),
            note: "  ".into(),
            at: crate::store::now(),
        };
        assert_eq!(
            write(&dir, "review", &dec).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert!(read(&dir, "review").is_none());
        dec.note = "AC1-3 shown".into();
        write(&dir, "review", &dec).unwrap();
        assert_eq!(read(&dir, "review"), Some(dec));
    }
}
