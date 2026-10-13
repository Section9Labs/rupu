//! What a launch site says it wants ([`LaunchSpec`]).

use std::path::PathBuf;
use std::sync::Arc;

use rupu_agent::{AgentSpec, Hooks, StreamOpts, TurnCollector, UserTurn};
use rupu_tools::{
    CatalogPort, LaunchPool, Launcher, PermissionMode, Prompter, RunStatus, WorkflowGenerator,
};

use super::origin::Origin;

/// A launch: this agent, this origin, this identity, this prompt, in this
/// workspace. Everything else is derived by [`super::RunAssembler`].
pub struct LaunchSpec {
    pub agent: AgentSpec,
    pub origin: Origin,
    /// Allocated by the site (`run_<ulid>`, a sub-run id, a session turn id).
    pub run_id: String,
    pub transcript_path: PathBuf,
    /// The run's codename, minted by the site; `None` = unnamed.
    pub codename: Option<String>,
    pub prompt: UserTurn,
    pub workspace: WorkspaceBinding,
    /// The run's permission mode (already resolved from flag / agent /
    /// config by the site), or a child's ceiling — see `ceiling`.
    pub mode: PermissionMode,
    /// A parent's cap (D7): when set, the run's mode is
    /// `min(ceiling, the agent's own permissionMode)` and `mode` is ignored.
    pub ceiling: Option<PermissionMode>,
    /// The operator to ask, for an interactive run.
    pub prompter: Option<Arc<dyn Prompter>>,
    pub overrides: Overrides,
    pub stream: StreamOpts,
    /// Callbacks the site hangs on the run. An `on_usage` here is called in
    /// addition to the run's usage-ledger hook, never instead of it.
    pub hooks: Hooks,
    pub pause: Option<tokio_util::sync::CancellationToken>,
    pub services: LaunchServices,
    pub collectors: Vec<Arc<dyn TurnCollector>>,
    /// A process child's link to the run that launched it
    /// (`--launch-depth` & co., W7 D18): its depth, parent, ceiling, caps and
    /// usage root. `None` for a top-level run.
    pub link: Option<crate::argv::LaunchLink>,
}

/// The workspace a run acts in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceBinding {
    /// The workspace record id (`rupu_workspace::upsert`).
    pub id: String,
    pub path: PathBuf,
}

/// What a site overrides of the agent's own settings.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    /// `--provider`.
    pub provider: Option<String>,
    /// `--model`. A model other than the agent's drops its model-specific
    /// pins ([`super::pins::PinRule`]).
    pub model: Option<String>,
    /// Appended to the agent's system prompt under `## Run target`.
    pub system_prompt_suffix: Option<String>,
    pub max_turns: Option<u32>,
    /// `--findings-profile` / a step's `findings_profile:`; wins over
    /// `findings_default` and the agent's own.
    pub findings_profile: Option<rupu_coverage::FindingProfile>,
    /// A workflow's default `findings_profile:`.
    pub findings_default: Option<rupu_coverage::FindingProfile>,
    /// A workflow's `concerns:`, which wins over the agent's.
    pub concerns: Option<rupu_coverage::ConcernsBlock>,
    /// Limits a session already resolved (and may have learned from an
    /// overflow): used as they are, unless unresolved.
    pub limits: Option<rupu_providers::model_limits::ModelLimits>,
    /// The launch's own `skills:` entries (a workflow step's), resolved
    /// beside the agent's with reason `step`.
    pub skills: Vec<String>,
    /// The launch's own `preloadSkills:` entries.
    pub preload_skills: Vec<String>,
}

/// Services the site provides (they belong to the root run, not this one).
#[derive(Default)]
pub struct LaunchServices {
    /// The launcher behind `dispatch` / `join` (`rupu_launch::RunLauncher`),
    /// offered when the origin's defaults allow it.
    pub launcher: Option<Arc<dyn Launcher>>,
    /// What `workflows.generate` authors with.
    pub workflow_generator: Option<Arc<dyn WorkflowGenerator>>,
    /// The agentiflow pool an agentiflow lead runs inside: what it (and its
    /// in-process children) may launch.
    pub flow_pool: Option<LaunchPool>,
    /// The run's netflow sink, when the site built it already (a top-level
    /// run's sink exists before its SCM registry and provider). `None` = the
    /// assembler builds one for the run.
    pub netflow: Option<Arc<dyn rupu_netflow::FlowSink>>,
    /// The run's address on its agentiflow's board and mailboxes
    /// (`board.*`, `msg.send`): an agentiflow lead's or unit's, built by
    /// the flow. Per run: it holds the run's participant id and claims.
    pub message_bus: Option<Arc<rupu_fleet::Bus>>,
    /// The flow's live goals, coverage and budget (`goal.*`,
    /// `budget.status`): an agentiflow lead's.
    pub run_status: Option<Arc<dyn RunStatus>>,
    /// A catalog for this launch instead of the context's
    /// ([`super::AssemblyContext::catalog`]): an agentiflow lead reads the
    /// one rooted at its workspace.
    pub catalog: Option<Arc<dyn CatalogPort>>,
    /// The run's provider, when the site prepared it early
    /// ([`super::RunAssembler::prepare_provider`], with `netflow` as its
    /// sink). `None` = the assembler builds it.
    pub provider: Option<super::PreparedProvider>,
    /// The run's skills, when the site resolved them already
    /// ([`super::RunAssembler::resolve_skills`]): used instead of resolving
    /// the agent's and the launch's. An agentiflow lead resolves its own once
    /// per flow, so every round's system prompt is the same.
    pub skills: Option<Arc<rupu_skills::ResolvedSkills>>,
}
