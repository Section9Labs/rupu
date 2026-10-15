//! The agentiflow envelope: the deterministic supervisor that parses an
//! `AgentiflowDef`, evaluates goal / coverage / budget stop conditions over
//! `rupu-coverage` evidence, and runs the round loop against a `LeadDriver`
//! port. Spec: docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md.

mod budget;
mod collectors;
mod coverage;
mod def;
mod envelope;
mod error;
mod flow_unit;
mod goal;
mod lead;
mod legacy_units;
mod operator;
pub mod reaper;
mod run;
mod status;
mod usage;
#[cfg(test)]
mod verify_fixtures;

pub use budget::{Budget, BudgetEnforcer, BudgetStage, UsageSource};
pub use collectors::{roster_collector, RosterCollector};
pub use coverage::{CoverageEvalError, CoverageEvaluator, CoverageOutcome, CoverageTarget};
pub use def::{
    load_agentiflow_def, AgentiflowDef, AssetSelector, FindingSelector, Goal, GoalTarget, Pool,
    RoundConfig, Scope, ScopeRoot, VerifyCheck,
};
pub use envelope::{
    Digest, Envelope, EnvelopeConfig, EnvelopeOutcome, LeadDriver, RoundContext, RoundOutcome,
    StopReason,
};
pub use error::AgentiflowError;
pub use flow_unit::FlowUnitServices;
pub use goal::{GoalEvalError, GoalEvaluator, GoalOutcome};
pub use lead::{
    render_round_prompt, GenerationCapability, GenerationProviderFactory, LeadConfig, LeadProvider,
    ProviderFactory, RunAgentLeadDriver,
};
pub use legacy_units::{units_on_disk, UnitKind, UnitOnDisk, UnitOutcome, UnitStatus};
pub use operator::{Control, OperatorQueue};
pub use reaper::{
    finalize_failed, hard_stop, hard_stop_with, reap_orphaned_agentiflows,
    reap_orphaned_agentiflows_with, HardStopGrace, HardStopOutcome, ReapSummary,
};
pub use run::{
    agentiflow_dir, new_run_id, run_agentiflow, AgentiflowRecord, GoalStatus, LeadInputs,
    RunAgentiflowOpts,
};
pub use rupu_launch::proc::{
    coordinator_alive, kill_group, pid_is_running, terminate_group, terminate_pid,
};
pub use status::{BudgetProbe, FlowStatus};
pub use usage::{
    fold_tokens, price_ledgers, LedgerUsageSource, PricedSpend, TokenTotals, Tokens, UnpricedModel,
};
