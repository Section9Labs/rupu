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
mod flow_unit;
mod goal;
mod lead;
mod operator;
mod proc;
pub mod reaper;
mod run;
mod status;
mod subprocess;
mod supervisor;
mod unit;
mod usage;
#[cfg(test)]
mod verify_fixtures;

pub use budget::{Budget, BudgetEnforcer, BudgetStage, UsageSource};
pub use collectors::{
    bus_collectors, lead_collectors, roster_collector, DirectiveCollector, MailboxCollector,
    RosterCollector,
};
pub use coverage::{CoverageEvalError, CoverageEvaluator, CoverageOutcome, CoverageTarget};
pub use def::{
    load_agentiflow_def, AgentiflowDef, AssetSelector, FindingSelector, Goal, GoalTarget, Pool,
    RoundConfig, Scope, ScopeRoot, VerifyCheck,
};
pub use dispatch_tools::{fleet_unit_tools, WorkflowToolCtx, MAX_DEPTH};
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
pub use operator::{OperatorMessage, OperatorQueue};
pub use proc::{coordinator_alive, kill_group, pid_is_running, terminate_group, terminate_pid};
pub use reaper::{
    finalize_failed, hard_stop, hard_stop_with, reap_orphaned_agentiflows,
    reap_orphaned_agentiflows_with, HardStopGrace, HardStopOutcome, ReapSummary,
};
pub use run::{
    agentiflow_dir, new_run_id, run_agentiflow, AgentiflowRecord, GoalStatus, LeadInputs,
    RunAgentiflowOpts,
};
pub use status::{BudgetProbe, FlowStatus};
pub use subprocess::SubprocessUnitLauncher;
pub use supervisor::{units_on_disk, FleetSupervisor, UnitOnDisk};
pub use unit::{
    MockUnitLauncher, Spawned, UnitError, UnitId, UnitKind, UnitLauncher, UnitOutcome, UnitSpec,
    UnitStatus,
};
pub use usage::{fold_tokens, LedgerUsageSource, TokenTotals, Tokens};
