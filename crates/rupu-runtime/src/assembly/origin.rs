//! Why a run exists ([`Origin`]) and the per-origin defaults derived from it
//! ([`defaults_for`]) — the single place they live (spec 2026-10-07 W3 §3.3).

use std::path::PathBuf;
use std::sync::Arc;

use rupu_tools::{AliasScope, RunIdentity, Surface};

use crate::usage_ledger::{LedgerTag, UsageLedger};

/// Why this run exists. Decides per-kind defaults. Adding a launch site =
/// adding a variant here and a row in [`defaults_for`].
#[derive(Clone)]
pub enum Origin {
    /// A top-level `rupu run` (what every host connector launches for a
    /// placed unit too).
    Standalone,
    /// A workflow step (or one `for_each` / `distribute` unit of it).
    WorkflowStep {
        workflow_run_id: String,
        workflow_name: String,
        step_id: String,
        unit: Option<UnitKey>,
        /// The step's `actions:` (connector narrowing).
        step_actions: Vec<String>,
        /// The findings scope the run reports under, when not the
        /// workflow's name (an agentiflow-launched workflow unit).
        scope_override: Option<String>,
    },
    /// An in-process child started by `dispatch_agent` /
    /// `dispatch_agents_parallel` (W7: `dispatch kind: subagent`).
    SubAgent { parent: ParentRun },
    /// One `rupu session` turn.
    SessionTurn {
        session_id: String,
        /// The session's directory (`<sessions>/<id>/`): its usage ledger.
        session_dir: PathBuf,
    },
    /// An agentiflow lead round.
    FlowLead { flow_id: String, flow_dir: PathBuf },
    /// `rupu run --fleet-run-dir <flow_dir> --fleet-participant <p>`: a unit
    /// that joins an agentiflow.
    FlowUnit {
        flow_id: String,
        flow_dir: PathBuf,
        participant: String,
    },
}

/// Which unit of a fanned-out step a run is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitKey {
    pub index: usize,
    pub key: Option<String>,
}

/// What a child run takes from the run that started it.
#[derive(Clone)]
pub struct ParentRun {
    /// The parent's identity: the child's surface, scope, depth and parent
    /// link come from it.
    pub identity: Arc<RunIdentity>,
    /// The ROOT run's usage ledger, which charges every descendant. `None`
    /// when the root keeps none.
    pub usage: Option<UsageLedger>,
    /// The root `rupu run`'s coverage stream; children append to it.
    pub coverage_stream: Option<PathBuf>,
}

/// Where a run's usage rows go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsageRoot {
    /// `<runs>/<run_id>/usage.jsonl` — the run's own.
    OwnRun,
    /// `<runs>/<workflow_run_id>/usage.jsonl`, tagged with the step/unit.
    WorkflowRun(String),
    /// The root run's ledger, handed down by the parent; rows carry the
    /// parent's run id so the fold attributes them to its step.
    Parent,
    /// `<dir>/usage.jsonl` — a session's or an agentiflow's directory.
    Dir(PathBuf),
}

/// Where a run's coverage is streamed for a coordinator to collect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverageStream {
    /// None: the run writes the coordinator's ledgers directly.
    None,
    /// `<runs>/<run_id>/coverage.jsonl` — every `rupu run`.
    Own,
    /// The parent's (the root `rupu run`'s).
    Parent,
}

/// Every default the launch sites used to disagree on, for one origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginDefaults {
    pub surface: Surface,
    /// `None` = the agent's own name ([`RunIdentity::scope`]).
    pub scope_name: Option<String>,
    pub step_id: Option<String>,
    pub step_actions: Vec<String>,
    pub alias_scope: AliasScope,
    pub usage: UsageRoot,
    pub ledger_tag: LedgerTag,
    pub coverage_stream: CoverageStream,
    /// The SCM/issue registry (MCP connector tools) is offered.
    pub scm: bool,
    /// The in-process dispatcher is offered when the launch provides one.
    pub launcher: bool,
    /// The run goes through machine-level admission.
    pub admission: bool,
}

/// The per-origin defaults table (spec §3.3). The ONE `match`: a default
/// that differs by origin lives here and nowhere else.
pub fn defaults_for(origin: &Origin) -> OriginDefaults {
    match origin {
        Origin::Standalone => OriginDefaults {
            surface: Surface::Agent,
            scope_name: None,
            step_id: None,
            step_actions: Vec::new(),
            alias_scope: AliasScope::Everywhere,
            usage: UsageRoot::OwnRun,
            ledger_tag: LedgerTag::default(),
            coverage_stream: CoverageStream::Own,
            scm: true,
            launcher: true,
            admission: true,
        },
        Origin::WorkflowStep {
            workflow_run_id,
            workflow_name,
            step_id,
            unit,
            step_actions,
            scope_override,
        } => OriginDefaults {
            surface: Surface::Workflow,
            scope_name: Some(
                scope_override
                    .clone()
                    .unwrap_or_else(|| workflow_name.clone()),
            ),
            step_id: Some(step_id.clone()),
            step_actions: step_actions.clone(),
            alias_scope: AliasScope::Everywhere,
            usage: UsageRoot::WorkflowRun(workflow_run_id.clone()),
            ledger_tag: LedgerTag {
                step_id: Some(step_id.clone()),
                unit_index: unit.as_ref().map(|u| u.index),
                unit_key: unit.as_ref().and_then(|u| u.key.clone()),
            },
            coverage_stream: CoverageStream::None,
            scm: true,
            launcher: true,
            admission: true,
        },
        // A child reports where its parent does (R4) and is charged to the
        // parent's root (R13).
        Origin::SubAgent { parent } => OriginDefaults {
            surface: parent.identity.surface,
            scope_name: Some(parent.identity.scope().to_string()),
            step_id: None,
            step_actions: Vec::new(),
            alias_scope: AliasScope::Everywhere,
            usage: UsageRoot::Parent,
            ledger_tag: LedgerTag::default(),
            coverage_stream: CoverageStream::Parent,
            scm: true,
            launcher: true,
            admission: true,
        },
        // Sessions key their ledger off the session id so several sessions
        // against one workspace stay distinct.
        Origin::SessionTurn {
            session_id,
            session_dir,
        } => OriginDefaults {
            surface: Surface::Session,
            scope_name: Some(session_id.clone()),
            step_id: None,
            step_actions: Vec::new(),
            alias_scope: AliasScope::Everywhere,
            usage: UsageRoot::Dir(session_dir.clone()),
            ledger_tag: LedgerTag::default(),
            coverage_stream: CoverageStream::None,
            scm: true,
            launcher: true,
            admission: true,
        },
        Origin::FlowLead { flow_id, flow_dir } => OriginDefaults {
            surface: Surface::Agentiflow,
            scope_name: Some(flow_id.clone()),
            step_id: None,
            step_actions: Vec::new(),
            alias_scope: AliasScope::FlowLead,
            usage: UsageRoot::Dir(flow_dir.clone()),
            ledger_tag: LedgerTag::default(),
            coverage_stream: CoverageStream::None,
            scm: true,
            // The lead's launcher is the agentiflow's own unit supervisor
            // (W7 makes it the one launcher).
            launcher: false,
            admission: true,
        },
        // A fleet unit pools its findings under the agentiflow's scope and,
        // until W7 passes `--usage-root`, meters itself in its own run
        // directory, which the agentiflow folds into its budget.
        Origin::FlowUnit { flow_id, .. } => OriginDefaults {
            surface: Surface::Agentiflow,
            scope_name: Some(flow_id.clone()),
            step_id: None,
            step_actions: Vec::new(),
            alias_scope: AliasScope::Everywhere,
            usage: UsageRoot::OwnRun,
            ledger_tag: LedgerTag::default(),
            coverage_stream: CoverageStream::Own,
            scm: true,
            launcher: true,
            admission: true,
        },
    }
}

impl Origin {
    /// The in-process parent's run id, for a child.
    pub fn parent_run_id(&self) -> Option<&str> {
        match self {
            Origin::SubAgent { parent } => Some(&parent.identity.run_id),
            _ => None,
        }
    }
}
