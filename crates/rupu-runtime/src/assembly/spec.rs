//! What a launch site says it wants ([`LaunchSpec`]).

use std::path::PathBuf;
use std::sync::Arc;

use rupu_agent::{AgentSpec, Hooks, StreamOpts, TurnCollector, UserTurn};
use rupu_tools::{AgentDispatcher, PermissionMode, Prompter, Tool};

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
}

/// Services the site provides (they belong to the root run, not this one).
#[derive(Default)]
pub struct LaunchServices {
    /// The in-process sub-agent dispatcher
    /// ([`crate::dispatch::InProcessDispatcher`]), offered when the origin's
    /// defaults allow it.
    pub dispatcher: Option<Arc<dyn AgentDispatcher>>,
    /// The run's netflow sink, when the site built it already (a top-level
    /// run's sink exists before its SCM registry and provider). `None` = the
    /// assembler builds one for the run.
    pub netflow: Option<Arc<dyn rupu_netflow::FlowSink>>,
    /// Pre-built tools injected into the run (`origin:injected`): the
    /// agentiflow coordination tools until W5 moves them into the catalog.
    pub extra_tools: Vec<Arc<dyn Tool>>,
    /// The run's provider, when the site prepared it early
    /// ([`super::RunAssembler::prepare_provider`], with `netflow` as its
    /// sink). `None` = the assembler builds it.
    pub provider: Option<super::PreparedProvider>,
}
