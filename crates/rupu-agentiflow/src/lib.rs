//! The agentiflow envelope: the deterministic supervisor that parses an
//! `AgentiflowDef`, evaluates goal / coverage / budget stop conditions over
//! `rupu-coverage` evidence, and runs the round loop against a `LeadDriver`
//! port. Spec: docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md.

mod budget;
mod collectors;
mod coverage;
mod def;
mod dispatch_tools;
mod envelope;
mod error;
mod goal;
mod lead;
mod operator;
mod proc;
mod roster;
mod run;
mod status_tools;
mod subprocess;
mod supervisor;
mod tools;
mod unit;
mod usage;
#[cfg(test)]
mod verify_fixtures;

pub use budget::{Budget, BudgetEnforcer, BudgetStage, UsageSource};
pub use collectors::{lead_collectors, DirectiveCollector, MailboxCollector};
pub use coverage::{CoverageEvalError, CoverageEvaluator, CoverageOutcome, CoverageTarget};
pub use def::{
    load_agentiflow_def, AgentiflowDef, AssetSelector, FindingSelector, Goal, GoalTarget, Pool,
    RoundConfig, Scope, ScopeRoot, VerifyCheck,
};
pub use dispatch_tools::{
    fleet_dispatch_tools, fleet_dispatch_tools_at_depth, fleet_unit_tools, WorkflowToolCtx,
    MAX_DEPTH,
};
pub use envelope::{
    Digest, Envelope, EnvelopeConfig, EnvelopeOutcome, LeadDriver, RoundContext, RoundOutcome,
    StopReason,
};
pub use error::AgentiflowError;
pub use goal::{GoalEvalError, GoalEvaluator, GoalOutcome};
pub use lead::{
    render_round_prompt, GenerationCapability, GenerationProviderFactory, LeadConfig,
    ProviderFactory, RunAgentLeadDriver,
};
pub use operator::{OperatorMessage, OperatorQueue};
pub use proc::{kill_group, pid_is_running, terminate_group, terminate_pid};
pub use roster::{roster_collector, roster_tools, RosterCollector, RosterCtx};
pub use run::{
    agentiflow_dir, new_run_id, run_agentiflow, AgentiflowRecord, GoalStatus, LeadInputs,
    RunAgentiflowOpts,
};
pub use status_tools::{status_tools, BudgetProbe};
pub use subprocess::SubprocessUnitLauncher;
pub use supervisor::{units_on_disk, FleetSupervisor, UnitOnDisk};
pub use tools::{fleet_tools, FleetToolCtx};
pub use unit::{
    MockUnitLauncher, Spawned, UnitError, UnitId, UnitKind, UnitLauncher, UnitOutcome, UnitSpec,
    UnitStatus,
};
pub use usage::{fold_tokens, LedgerUsageSource, TokenTotals, Tokens};
