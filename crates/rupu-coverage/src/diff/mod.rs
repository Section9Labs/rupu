//! Run-to-run diff over the Slice A coverage ledgers.
//!
//! Mirrors the `audit` module: pure analysis, re-exported at the crate
//! root (`run_diff`, `list_runs`). See the Slice B design spec.

pub mod generate;
pub mod types;

pub use types::{CellRef, FindingThemeRef, RunDiff, RunListEntry, VerdictFlip};
