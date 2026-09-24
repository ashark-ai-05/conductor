//! Reads `mutants.out/outcomes.json` from cargo-mutants.
//!
//! A mutant is a small deliberate bug. A test suite that fails when it is injected has caught
//! it; one that still passes has let it survive. Survivors are the most useful thing a
//! receipt can show a reviewer: each is a line of code whose behaviour no test pins down.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Survivor {
    pub file: String,
    pub line: u64,
    /// cargo-mutants' own description, such as `replace > with >= in clamp`.
    pub what: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MutationReport {
    pub caught: u64,
    pub missed: u64,
    pub timeout: u64,
    pub unviable: u64,
    pub survivors: Vec<Survivor>,
    pub tool_version: Option<String>,
}

impl MutationReport {
    /// Caught (including timeouts, which stop the test run) over everything that compiled.
    /// `None` when no mutant was viable, because a score over nothing proves nothing.
    pub fn score(&self) -> Option<f64> {
        let tested = self.caught + self.missed + self.timeout;
        (tested > 0).then(|| (self.caught + self.timeout) as f64 / tested as f64)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("cargo-mutants output could not be read: {0}")]
pub struct ReadError(#[from] serde_json::Error);

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    caught: u64,
    #[serde(default)]
    missed: u64,
    #[serde(default)]
    timeout: u64,
    #[serde(default)]
    unviable: u64,
    #[serde(default)]
    cargo_mutants_version: Option<String>,
    #[serde(default)]
    outcomes: Vec<RawOutcome>,
}

#[derive(Deserialize)]
struct RawOutcome {
    scenario: serde_json::Value,
    summary: String,
}

pub fn parse(json: &str) -> Result<MutationReport, ReadError> {
    let raw: Raw = serde_json::from_str(json)?;
    let survivors = raw
        .outcomes
        .iter()
        .filter(|o| o.summary == "MissedMutant")
        .filter_map(|o| {
            let m = o.scenario.get("Mutant")?;
            let file = m.get("file")?.as_str()?.to_owned();
            let line = m.pointer("/span/start/line")?.as_u64()?;
            let name = m.get("name").and_then(|n| n.as_str()).unwrap_or_default();
            // The name repeats the location in front of the description; the receipt already
            // shows the location, so keep only what changed.
            let what = name
                .splitn(4, ':')
                .nth(3)
                .map_or(name, str::trim)
                .to_owned();
            Some(Survivor { file, line, what })
        })
        .collect();
    Ok(MutationReport {
        caught: raw.caught,
        missed: raw.missed,
        timeout: raw.timeout,
        unviable: raw.unviable,
        survivors,
        tool_version: raw.cargo_mutants_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = include_str!("../tests/fixtures/mutants-outcomes.json");

    #[test]
    fn a_real_run_is_counted_and_scored() {
        let r = parse(REAL).unwrap();
        assert_eq!((r.caught, r.missed, r.timeout, r.unviable), (13, 3, 0, 0));
        assert!((r.score().unwrap() - 13.0 / 16.0).abs() < 1e-9);
        assert_eq!(r.tool_version.as_deref(), Some("27.1.0"));
    }

    #[test]
    fn survivors_say_where_and_what_changed() {
        let r = parse(REAL).unwrap();
        assert_eq!(r.survivors.len(), 3);
        let s = r.survivors.iter().find(|s| s.line == 3).unwrap();
        assert_eq!(s.file, "src/lib.rs");
        assert_eq!(s.what, "replace > with >= in clamp");
    }

    #[test]
    fn no_viable_mutants_is_no_score_not_a_perfect_one() {
        let r = parse(r#"{"caught":0,"missed":0,"timeout":0,"unviable":4,"outcomes":[]}"#).unwrap();
        assert_eq!(r.score(), None);
    }

    #[test]
    fn junk_is_an_error() {
        assert!(parse("not json").is_err());
    }
}
