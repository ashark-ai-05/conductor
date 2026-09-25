//! Where a run's record lives, and the recorder that writes it as it happens.
//!
//! Every event is appended to `events.jsonl` and flushed before the engine moves on, so a
//! crash leaves a readable, verifiable prefix rather than nothing.

use conductor_model::{Chain, Event, Source};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// `<repo>/.conductor/runs/<run_id>/`.
#[derive(Debug, Clone)]
pub struct RunDir {
    pub root: PathBuf,
}

impl RunDir {
    pub fn for_run(repo: &Path, run_id: &str) -> Self {
        RunDir {
            root: repo.join(".conductor").join("runs").join(run_id),
        }
    }

    pub fn create(repo: &Path, run_id: &str) -> io::Result<Self> {
        let d = Self::for_run(repo, run_id);
        fs::create_dir_all(&d.root)?;
        Ok(d)
    }

    pub fn events(&self) -> PathBuf {
        self.root.join("events.jsonl")
    }

    pub fn receipt(&self) -> PathBuf {
        self.root.join("receipt.json")
    }

    /// The receipt file's hash, as a trace carries it.
    pub fn receipt_sha256(&self) -> Option<String> {
        use sha2::Digest;
        let bytes = std::fs::read(self.receipt()).ok()?;
        Some(format!(
            "sha256:{}",
            hex::encode(sha2::Sha256::digest(&bytes))
        ))
    }

    pub fn gates(&self) -> PathBuf {
        self.root.join("gates.json")
    }

    /// What one attempt changed, as a patch: `attempts/<stage>-<attempt>.patch`.
    pub fn attempt_diff(&self, stage: &str, attempt: usize) -> PathBuf {
        self.root
            .join("attempts")
            .join(format!("{stage}-{attempt}.patch"))
    }

    pub fn scratch(&self, stage: &str, attempt: usize) -> PathBuf {
        self.root.join("scratch").join(format!("{stage}-{attempt}"))
    }

    pub fn write_json<T: Serialize>(&self, path: &Path, value: &T) -> io::Result<()> {
        let text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
        fs::write(path, text + "\n")
    }

    pub fn read_events(&self) -> io::Result<Vec<Event>> {
        let text = fs::read_to_string(self.events())?;
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(io::Error::other))
            .collect()
    }
}

/// Now, as RFC 3339 UTC.
pub fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

/// Appends events to the chain and to disk, and tells any watcher (the UI) about each one.
pub struct Recorder {
    chain: Chain,
    file: File,
    watcher: Option<Sender<Event>>,
}

impl Recorder {
    pub fn open(dir: &RunDir, watcher: Option<Sender<Event>>) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.events())?;
        Ok(Recorder {
            chain: Chain::new(),
            file,
            watcher,
        })
    }

    pub fn record(
        &mut self,
        source: Source,
        stage: Option<&str>,
        what: impl AsRef<str>,
    ) -> io::Result<Event> {
        let event = self
            .chain
            .append(&now(), source, stage, what.as_ref())
            .clone();
        let line = serde_json::to_string(&event).map_err(io::Error::other)?;
        writeln!(self.file, "{line}")?;
        self.file.flush()?;
        if let Some(w) = &self.watcher {
            let _ = w.send(event.clone());
        }
        Ok(event)
    }

    pub fn head(&self) -> &str {
        self.chain.head()
    }

    pub fn events(&self) -> &[Event] {
        self.chain.events()
    }
}

/// A run id that sorts by time: 13 base-36 digits of milliseconds and two of noise.
pub fn new_run_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut n = d.as_millis();
    let mut s = Vec::new();
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    for _ in 0..9 {
        s.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    s.reverse();
    let noise = (d.subsec_nanos() ^ std::process::id()) as usize;
    s.push(DIGITS[noise % 36]);
    s.push(DIGITS[(noise / 36) % 36]);
    String::from_utf8(s).expect("ascii")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_events_read_back_and_verify() {
        let repo = tempfile::tempdir().unwrap();
        let dir = RunDir::create(repo.path(), "R1").unwrap();
        let mut rec = Recorder::open(&dir, None).unwrap();
        rec.record(Source::Witnessed, None, "run started").unwrap();
        rec.record(Source::Observed, Some("spec"), "wrote spec.md")
            .unwrap();
        let back = dir.read_events().unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(Chain::verify(&back).unwrap(), rec.head());
    }

    #[test]
    fn run_ids_are_distinct_and_sortable() {
        let a = new_run_id();
        std::thread::sleep(std::time::Duration::from_millis(3));
        let b = new_run_id();
        assert_ne!(a, b);
        assert!(a[..9] <= b[..9]);
        assert_eq!(a.len(), 11);
    }
}
