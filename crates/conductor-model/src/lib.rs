//! Conductor's data model: workflows, the evidence chain, receipts, and the view models the
//! terminal UI renders.
//!
//! Nothing in here talks to an agent, herdr, git or the network. It is plain data plus the
//! pure functions that decide things about it, which is what lets a verdict be re-derived
//! from stored evidence with zero model calls.

pub mod demo;
pub mod evidence;
pub mod receipt;
pub mod view;
pub mod workflow;

pub use evidence::{Chain, ChainError, Event, Source};
pub use receipt::{CheckRow, Integrity, Place, Receipt, RunSummary, Survivor, Verdict};
pub use workflow::{Issue, Report, Workflow};
