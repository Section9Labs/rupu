//! The agentiflow envelope: the deterministic supervisor that parses an
//! `AgentiflowDef`, evaluates goal / coverage / budget stop conditions over
//! `rupu-coverage` evidence, and runs the round loop against a `LeadDriver`
//! port. Spec: docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md.

mod budget;
mod coverage;
mod def;
mod error;
mod goal;

pub use budget::Budget;
pub use coverage::{CoverageEvaluator, CoverageOutcome, CoverageTarget};
pub use def::{
    AgentiflowDef, AssetSelector, FindingSelector, Goal, GoalTarget, Pool, RoundConfig, Scope,
    ScopeRoot,
};
pub use error::AgentiflowError;
pub use goal::{GoalEvalError, GoalEvaluator, GoalOutcome};
