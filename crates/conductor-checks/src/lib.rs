//! Checks conductor runs in its own process, never in an agent's pane.
//!
//! Each module does one thing and most are pure: [`runner`] executes a declared command,
//! [`git`] reads the facts a check needs, and the rest turn output into facts a gate can
//! decide on. A gate's verdict is only ever as good as these, which is why each is tested
//! against real output captured from the real tools.

pub mod assert;
pub mod cargo_test;
pub mod gate;
pub mod git;
pub mod junit;
pub mod mutants;
pub mod runner;
pub mod scope;
