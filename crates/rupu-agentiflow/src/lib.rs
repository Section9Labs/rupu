//! The agentiflow envelope: the deterministic supervisor that parses an
//! `AgentiflowDef`, evaluates goal / coverage / budget stop conditions over
//! `rupu-coverage` evidence, and runs the round loop against a `LeadDriver`
//! port. Spec: docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md.

mod budget;
mod coverage;
mod def;
mod envelope;
mod error;
mod goal;
mod lead;
mod operator;

pub use budget::{Budget, BudgetEnforcer, BudgetStage, UsageSource};
pub use coverage::{CoverageEvalError, CoverageEvaluator, CoverageOutcome, CoverageTarget};
pub use def::{
    AgentiflowDef, AssetSelector, FindingSelector, Goal, GoalTarget, Pool, RoundConfig, Scope,
    ScopeRoot,
};
pub use envelope::{
    Digest, Envelope, EnvelopeConfig, EnvelopeOutcome, LeadDriver, RoundContext, RoundOutcome,
    StopReason,
};
pub use error::AgentiflowError;
pub use goal::{GoalEvalError, GoalEvaluator, GoalOutcome};
pub use lead::{render_round_prompt, LeadConfig, ProviderFactory, RunAgentLeadDriver};
pub use operator::{OperatorMessage, OperatorQueue};
