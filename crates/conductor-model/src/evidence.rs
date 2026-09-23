//! The evidence chain: an append-only list of events, each hashed together with the hash of
//! the one before it.
//!
//! The chain is what a receipt stands on. Rewriting any event changes its hash and breaks
//! every link after it, so [`Chain::verify`] finds the edit. On its own that only protects
//! against someone who doesn't also recompute the rest of the chain; anchoring the head in a
//! pushed commit (SPEC §11.3) is what closes that gap.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// The hash the first event links to.
pub const GENESIS: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// Where a fact came from. Every event carries exactly one.
///
/// The labels are the product's honesty: a receipt may only call something proven when its
/// source is [`Source::Witnessed`]. [`Source::Inferred`] is shown, never relied on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Reported by the agent's own hooks: a tool call, a file write, a turn ending.
    Observed,
    /// Read from the agent's session log or headless JSON stream: tokens, model.
    Measured,
    /// A check conductor ran in its own process.
    Witnessed,
    /// herdr's reading of the terminal. Used for scheduling and display only.
    Inferred,
    /// Something a person did, from herdr or the conductor pane.
    Human,
    /// A pane or tab conductor saw appear but did not create or approve.
    Unmanaged,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Observed => "observed",
            Source::Measured => "measured",
            Source::Witnessed => "witnessed",
            Source::Inferred => "inferred",
            Source::Human => "human",
            Source::Unmanaged => "unmanaged",
        }
    }

    /// Whether a receipt may treat a fact from this source as proof.
    pub fn is_proof(self) -> bool {
        matches!(self, Source::Witnessed)
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One link in the chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    /// RFC 3339 timestamp. Supplied by the caller so the chain itself stays pure.
    pub at: String,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    pub what: String,
    pub prev: String,
    pub hash: String,
}

/// The fields a hash covers, in a fixed order, serialised as JSON.
#[derive(Serialize)]
struct Hashed<'a> {
    seq: u64,
    at: &'a str,
    source: Source,
    stage: Option<&'a str>,
    what: &'a str,
    prev: &'a str,
}

fn hash_of(
    seq: u64,
    at: &str,
    source: Source,
    stage: Option<&str>,
    what: &str,
    prev: &str,
) -> String {
    let body = serde_json::to_vec(&Hashed {
        seq,
        at,
        source,
        stage,
        what,
        prev,
    })
    .expect("a struct of strings and integers always serialises");
    format!("sha256:{}", hex::encode(Sha256::digest(&body)))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainError {
    #[error("event {seq} is out of order: expected {expected}")]
    Sequence { seq: u64, expected: u64 },
    #[error("event {seq} links to {found}, but the event before it hashes to {expected}")]
    BrokenLink {
        seq: u64,
        expected: String,
        found: String,
    },
    #[error(
        "event {seq} was changed after it was written: it now hashes to {actual}, not {recorded}"
    )]
    Tampered {
        seq: u64,
        recorded: String,
        actual: String,
    },
}

/// An append-only chain of events.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    events: Vec<Event>,
}

impl Chain {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an event and returns it. The only way to add to a chain.
    pub fn append(&mut self, at: &str, source: Source, stage: Option<&str>, what: &str) -> &Event {
        let seq = self.events.len() as u64;
        let prev = self.head().to_owned();
        let hash = hash_of(seq, at, source, stage, what, &prev);
        self.events.push(Event {
            seq,
            at: at.to_owned(),
            source,
            stage: stage.map(str::to_owned),
            what: what.to_owned(),
            prev,
            hash,
        });
        self.events.last().expect("just pushed")
    }

    /// The hash of the last event, or [`GENESIS`] for an empty chain. This is the value
    /// anchored in the run's final commit.
    pub fn head(&self) -> &str {
        self.events.last().map_or(GENESIS, |e| e.hash.as_str())
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Checks a list of events read back from disk. Returns the head on success.
    pub fn verify(events: &[Event]) -> Result<String, ChainError> {
        let mut prev = GENESIS.to_owned();
        for (i, e) in events.iter().enumerate() {
            let expected = i as u64;
            if e.seq != expected {
                return Err(ChainError::Sequence {
                    seq: e.seq,
                    expected,
                });
            }
            if e.prev != prev {
                return Err(ChainError::BrokenLink {
                    seq: e.seq,
                    expected: prev,
                    found: e.prev.clone(),
                });
            }
            let actual = hash_of(e.seq, &e.at, e.source, e.stage.as_deref(), &e.what, &e.prev);
            if actual != e.hash {
                return Err(ChainError::Tampered {
                    seq: e.seq,
                    recorded: e.hash.clone(),
                    actual,
                });
            }
            prev = e.hash.clone();
        }
        Ok(prev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Chain {
        let mut c = Chain::new();
        c.append(
            "2026-09-23T12:04:11Z",
            Source::Observed,
            Some("implement"),
            "edited src/status.rs",
        );
        c.append(
            "2026-09-23T12:04:20Z",
            Source::Witnessed,
            Some("implement"),
            "scope passed",
        );
        c.append(
            "2026-09-23T12:04:23Z",
            Source::Measured,
            None,
            "+18k tokens",
        );
        c
    }

    #[test]
    fn an_untouched_chain_verifies_to_its_head() {
        let c = sample();
        assert_eq!(Chain::verify(c.events()).unwrap(), c.head());
    }

    #[test]
    fn the_empty_chain_verifies_to_genesis() {
        assert_eq!(Chain::verify(&[]).unwrap(), GENESIS);
        assert_eq!(Chain::new().head(), GENESIS);
    }

    #[test]
    fn editing_an_event_is_found() {
        let mut events = sample().events().to_vec();
        events[1].what = "scope failed".into();
        assert!(matches!(
            Chain::verify(&events),
            Err(ChainError::Tampered { seq: 1, .. })
        ));
    }

    #[test]
    fn relabelling_a_guess_as_proof_is_found() {
        let mut events = sample().events().to_vec();
        events[2].source = Source::Witnessed;
        assert!(matches!(
            Chain::verify(&events),
            Err(ChainError::Tampered { seq: 2, .. })
        ));
    }

    #[test]
    fn deleting_an_event_is_found() {
        let mut events = sample().events().to_vec();
        events.remove(1);
        assert!(matches!(
            Chain::verify(&events),
            Err(ChainError::Sequence {
                seq: 2,
                expected: 1
            })
        ));
    }

    #[test]
    fn a_rehashed_event_still_breaks_the_next_link() {
        let mut events = sample().events().to_vec();
        events[0].what = "edited src/other.rs".into();
        events[0].hash = hash_of(
            0,
            &events[0].at,
            events[0].source,
            events[0].stage.as_deref(),
            &events[0].what,
            GENESIS,
        );
        assert!(matches!(
            Chain::verify(&events),
            Err(ChainError::BrokenLink { seq: 1, .. })
        ));
    }

    #[test]
    fn only_witnessed_facts_count_as_proof() {
        for s in [
            Source::Observed,
            Source::Measured,
            Source::Inferred,
            Source::Human,
            Source::Unmanaged,
        ] {
            assert!(!s.is_proof(), "{s} must not count as proof");
        }
        assert!(Source::Witnessed.is_proof());
    }

    #[test]
    fn events_round_trip_through_json() {
        let c = sample();
        let json = serde_json::to_string(c.events()).unwrap();
        let back: Vec<Event> = serde_json::from_str(&json).unwrap();
        assert_eq!(Chain::verify(&back).unwrap(), c.head());
    }
}
