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
mod run;
mod subprocess;
mod supervisor;
mod tools;
mod unit;

pub use budget::{Budget, BudgetEnforcer, BudgetStage, UsageSource};
pub use collectors::{lead_collectors, DirectiveCollector, MailboxCollector};
pub use coverage::{CoverageEvalError, CoverageEvaluator, CoverageOutcome, CoverageTarget};
pub use def::{
    AgentiflowDef, AssetSelector, FindingSelector, Goal, GoalTarget, Pool, RoundConfig, Scope,
    ScopeRoot,
};
pub use dispatch_tools::{fleet_dispatch_tools, fleet_dispatch_tools_at_depth, MAX_DEPTH};
pub use envelope::{
    Digest, Envelope, EnvelopeConfig, EnvelopeOutcome, LeadDriver, RoundContext, RoundOutcome,
    StopReason,
};
pub use error::AgentiflowError;
pub use goal::{GoalEvalError, GoalEvaluator, GoalOutcome};
pub use lead::{render_round_prompt, LeadConfig, ProviderFactory, RunAgentLeadDriver};
pub use operator::{OperatorMessage, OperatorQueue};
pub use proc::{pid_is_running, terminate_pid};
pub use run::{
    agentiflow_dir, new_run_id, run_agentiflow, AgentiflowRecord, GoalStatus, LeadInputs,
    RunAgentiflowOpts,
};
pub use subprocess::SubprocessUnitLauncher;
pub use supervisor::FleetSupervisor;
pub use tools::{fleet_tools, FleetToolCtx};
pub use unit::{
    MockUnitLauncher, UnitError, UnitId, UnitLauncher, UnitOutcome, UnitSpec, UnitStatus,
};
