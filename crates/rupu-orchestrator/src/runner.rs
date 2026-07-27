//! Linear workflow runner.
//!
//! Per step:
//! 1. Render the step's `prompt:` template with `inputs.*` and prior
//!    `steps.<id>.output`.
//! 2. Build [`AgentRunOpts`] via a caller-supplied [`StepFactory`]
//!    (this lets tests inject the mock provider; the CLI in Plan 2
//!    Phase 3 wires real providers).
//! 3. Run the agent. Capture the final assistant message as the
//!    step's `output` and feed it forward to the next step's context.
//! 4. On step failure (provider error, agent abort), abort the
//!    workflow with the underlying error.
//!
//! Fan-out (`for_each:`) replaces step 2-3 with: render the for_each
//! expression to obtain a list of items, then dispatch the same
//! agent + prompt template per item with `{{item}}` and
//! `{{loop.*}}` bindings. Concurrency is capped by `max_parallel:`
//! (default 1, i.e. serial in declared order). Per-item results are
//! collected into `steps.<id>.results[*]`.

use crate::templates::{
    render_step_prompt, render_when_expression, LoopInfo, RenderError, RenderMode, StepContext,
    StepOutput,
};
use crate::workflow::{
    effective_workspace_mode, is_nonlinear, workflow_edges, yaml_scalar_to_string, InputType,
    Step, Workflow, WorkflowParseError, WorkspaceMode,
};
use async_trait::async_trait;
use rupu_agent::{run_agent, AgentRunOpts, RunError, RunResult};
use rupu_providers::types::Message;
use rupu_transcript::{Event, JsonlReader, JsonlWriter};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use ulid::Ulid;

// ---------------------------------------------------------------------------
// Remote unit dispatch port
// ---------------------------------------------------------------------------

/// Opaque file-change set a synced unit returns. The orchestrator never
/// interprets `payload` — a self-describing git patch/bundle or tar delta
/// produced by the workspace codec. `changed` / `deleted` are the affected
/// repo-relative paths, carried for observability/logging only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDelta {
    pub changed: Vec<String>,
    pub deleted: Vec<String>,
    pub payload: Vec<u8>,
}

/// Returned by `apply_workspace_deltas` when two units' changes conflict —
/// overlapping files (tar mode) or a conflicting hunk (git mode). Surfaced
/// as a step failure honoring `continue_on_error`.
#[derive(Debug, Error)]
#[error("workspace conflict on: {0:?}")]
pub struct WorkspaceConflict(pub Vec<String>);

/// Payload for one unit dispatched to a remote host.
#[derive(Debug)]
pub struct UnitDispatch {
    pub step_id: String,
    pub agent: String,
    pub rendered_prompt: String,
    pub index: usize,
    pub run_id: String,
    /// Set to `Some(coordinator workspace path)` when this unit's effective
    /// workspace mode is `Sync`. `None` ⇒ self-contained (unchanged).
    pub workspace_path: Option<PathBuf>,
}

/// Outcome of one unit dispatched to a remote host.
#[derive(Debug)]
pub struct UnitOutcome {
    pub output: String,
    pub success: bool,
    pub error: Option<String>,
    /// The unit's file changes when it ran with a synced workspace; `None`
    /// for a self-contained unit.
    pub workspace_delta: Option<WorkspaceDelta>,
}

/// Port that remote-fleet implementations plug into.
///
/// The orchestrator calls this when a `for_each:` step has a
/// `distribute:` placement. Each unit is dispatched to the named host;
/// results are aggregated exactly like local (inline) units. Local units
/// that have no placement NEVER go through this trait — they keep the
/// existing `dispatch_one` + `read_final_assistant_text` path unchanged.
#[async_trait]
pub trait UnitDispatcher: Send + Sync {
    /// Run one unit (an agent invocation) on `host` and return its output.
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError>;

    /// Apply collected unit workspace deltas to the coordinator workspace at
    /// `workspace_path`. Mode-aware (git 3-way merge / tar disjoint-copy);
    /// conflicts return `WorkspaceConflict`. Default is a no-op for
    /// dispatchers without workspace support.
    async fn apply_workspace_deltas(
        &self,
        _workspace_path: &Path,
        _deltas: &[WorkspaceDelta],
    ) -> Result<(), WorkspaceConflict> {
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum RunWorkflowError {
    #[error("parse: {0}")]
    Parse(#[from] WorkflowParseError),
    #[error("render step {step}: {source}")]
    Render {
        step: String,
        #[source]
        source: RenderError,
    },
    #[error("agent failure in step {step}: {source}")]
    Agent {
        step: String,
        #[source]
        source: RunError,
    },
    #[error("action step {step} failed: {source}")]
    Action {
        step: String,
        #[source]
        source: rupu_mcp::McpError,
    },
    #[error("input `{name}` is required but was not provided")]
    MissingRequiredInput { name: String },
    #[error("input `{name}`: value `{value}` is not in the declared `enum` ({allowed:?})")]
    InputNotInEnum {
        name: String,
        value: String,
        allowed: Vec<String>,
    },
    #[error("input `{name}`: value `{value}` is not a valid {ty}")]
    InputTypeMismatch {
        name: String,
        value: String,
        ty: &'static str,
    },
    #[error("input `{name}` is not declared in the workflow's `inputs:` block")]
    UndeclaredInput { name: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("fan-out item task in step `{step}` panicked or was cancelled: {source}")]
    FanoutJoin {
        step: String,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error(
        "resuming a workflow with a `workspace: sync` step is not supported (v1): re-run from the start instead"
    )]
    ResumeWithWorkspaceSync,
    // TODO(pause-workspace-sync): support delta-persisting resume so workspace:sync
    // workflows can pause/resume. Until then, pausing a workflow that contains a
    // `workspace: sync` step is refused: a mid-flight pause would checkpoint only
    // the coordinator's OUTPUTs, not the in-flight workspace deltas, so resuming
    // would silently lose file edits (same hazard as ResumeWithWorkspaceSync).
    #[error(
        "pausing a workflow with a `workspace: sync` step is not supported (v1): let it run to completion instead"
    )]
    PauseWithWorkspaceSync,
    #[error(
        "action step `{step}` requires runtime wiring — this entry point does not provide an SCM dispatcher"
    )]
    ActionDispatcherMissing { step: String },
    #[error("scheduler: a dispatched node task panicked or was cancelled: {0}")]
    SchedulerTaskJoin(#[from] tokio::task::JoinError),
    #[error(
        "run cancelled: {aborted} in-flight node(s) aborted; restart to resume from checkpoint"
    )]
    RunCancelled { aborted: usize },
    /// Task 3, spec §2c/§8g: `until` never held through `max_iterations`
    /// iterations and the loop's `on_max` is `fail` (the default) —
    /// fail-loudly rather than let a caller believe the last iteration's
    /// unconverged output is "the" answer.
    #[error("loop '{name}' exhausted {max_iterations} iterations, until never held")]
    LoopExhausted { name: String, max_iterations: u32 },
}

/// Trait the orchestrator uses to construct per-unit [`AgentRunOpts`].
/// Production impl wires real providers + the default tool registry;
/// tests inject mock providers.
///
/// `step_id` is the parent step id (used by the production impl to
/// look up step-level config); `agent_name` is the agent to load and
/// is the *sub-step's* agent for `parallel:` steps. For linear and
/// `for_each:` steps `agent_name` matches the parent step's `agent:`.
#[async_trait]
pub trait StepFactory: Send + Sync {
    // The signature is intentionally wide — every piece of context the
    // factory needs to load an agent + build its run opts. Wrapping
    // these in a struct adds friction for every test impl, so allow
    // the lint at the trait boundary.
    #[allow(clippy::too_many_arguments)]
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts;
}

/// `Clone` exists solely so [`run_scheduler`] can wrap one `Arc`-shared copy
/// (`Arc::new(opts.clone())`, cloned once per run) and hand a cheap
/// `Arc::clone` of it into every `tokio::spawn`ed node dispatch — the same
/// reason `run_parallel_step` clones individual `Arc` fields today, just
/// promoted to the whole struct now that dispatch happens at workflow scope,
/// not only inside one fan-out step. Every field is itself `Clone` (plain
/// data or an `Arc<dyn Trait + Send + Sync>`), so this is a cheap,
/// non-semantic addition — no behavior depends on it outside the scheduler.
#[derive(Clone)]
pub struct OrchestratorRunOpts {
    pub workflow: Workflow,
    pub inputs: BTreeMap<String, String>,
    pub workspace_id: String,
    pub workspace_path: PathBuf,
    /// Directory where per-step transcript files are written.
    pub transcript_dir: PathBuf,
    pub factory: Arc<dyn StepFactory>,
    /// Event payload that triggered this run, if any. Populated by
    /// the webhook receiver; `None` for manual / cron-triggered
    /// runs. Bound as `{{event.*}}` in step prompts and `when:`
    /// expressions.
    pub event: Option<serde_json::Value>,
    /// Pre-fetched issue payload, if the run-target resolved to an
    /// issue. Bound as `{{issue.*}}` in step prompts and `when:`
    /// expressions. The CLI calls `IssueConnector::get_issue` once
    /// at run-start and serializes the result here.
    pub issue: Option<serde_json::Value>,
    /// Stable text reference for the issue (`<tracker>:<project>/issues/<N>`),
    /// persisted on `RunRecord.issue_ref` so
    /// `rupu workflow runs --issue <ref>` can filter back. `None`
    /// for runs without an issue target.
    pub issue_ref: Option<String>,
    /// Optional persistent run-state store. When provided the runner
    /// creates a `RunRecord` at start, appends one `StepResultRecord`
    /// per completed step, and flips the record's status to
    /// `Completed` / `Failed` at the end. When `None` (e.g. a unit
    /// test wiring its own minimal harness) the runner skips
    /// persistence entirely.
    #[allow(clippy::missing_docs_in_private_items)]
    pub run_store: Option<Arc<crate::runs::RunStore>>,
    /// The workflow's YAML body, snapshotted into the run directory
    /// at start. Required when `run_store` is `Some`; ignored
    /// otherwise.
    pub workflow_yaml: Option<String>,
    /// When `Some`, this is a resume of a previously-paused run.
    /// The runner picks up where the original run left off rather
    /// than creating a new run record. Caller is responsible for
    /// populating this from the persisted `step_results.jsonl` +
    /// `run.json`.
    pub resume_from: Option<ResumeState>,
    /// Caller-supplied run-id used for idempotent dispatch (cron tick
    /// polled-events tier). `None` for normal manual runs, where the
    /// runner generates `run_<ULID>` instead. When `Some`, the runner
    /// passes the id straight to `RunStore::create`; if the run
    /// already exists, `RunStoreError::AlreadyExists` surfaces and
    /// the caller is expected to log + skip.
    pub run_id_override: Option<String>,
    /// When `true`, missing template variables abort rendering.
    pub strict_templates: bool,
    /// Optional event sink. When `Some`, the runner emits
    /// `Event::RunStarted` / `Event::StepStarted` / etc. at each
    /// transition. When `None`, behavior is unchanged (back-compat for
    /// any direct caller).
    pub event_sink: Option<std::sync::Arc<dyn crate::executor::EventSink>>,
    /// Optional remote unit dispatcher. When a `for_each:` step has
    /// `distribute:`, units are routed to hosts through this. `None` ⇒
    /// all units run locally (a `distribute:` step with `None` is a run
    /// error surfaced as a failed unit).
    pub unit_dispatcher: Option<Arc<dyn UnitDispatcher>>,
    /// Cooperative pause signal. When `Some` and the token is cancelled, the
    /// runner stops at the next safe boundary: mid-step for the in-flight
    /// *linear* agent run (the agent's partial turn is dropped / a running
    /// tool finishes, then the step is checkpointed as paused-incomplete), or
    /// at the *step boundary* (before the next step is dispatched) for every
    /// step shape. The run record flips to [`crate::runs::RunStatus::Paused`]
    /// and a `RunPaused` event is emitted. Resume is a fresh `run_workflow`
    /// with `resume_from` set (see [`ResumeState`]). `None` (default)
    /// preserves today's behavior exactly. Fan-out / panel / parallel steps
    /// pause only at the step boundary — mid-unit fan-out pause/resume is not
    /// supported in v1 (same class of limitation as `workspace: sync`).
    pub pause: Option<CancellationToken>,
    /// In-process MCP tool dispatcher for `action:` steps (spec §3.2). When
    /// `Some`, a workflow's `action:` steps execute for real by calling the
    /// dispatcher with the step's `with:` values (template-rendered first).
    /// `None` ⇒ any `action:` step fails loudly with
    /// [`RunWorkflowError::ActionDispatcherMissing`] — this entry point does
    /// not provide an SCM dispatcher (e.g. a test harness that never
    /// constructs one). Plan 2 wires this from `rupu-cli`; Plan 4's notify
    /// hooks reuse the same [`execute_action_step`] helper this drives.
    pub action_dispatcher: Option<Arc<rupu_mcp::ToolDispatcher>>,
}

/// Why a run paused. Threaded onto [`AwaitingInfo`] / [`ResumeState`] so the
/// single resume path (`run_workflow` with `resume_from`) can distinguish an
/// approval-gate pause (operator approves, then resumes) from a manual /
/// operator-requested pause (cooperative interrupt, then resumes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    /// Paused before a step whose `approval:` gate required sign-off.
    Approval,
    /// Paused by the cooperative pause signal ([`OrchestratorRunOpts::pause`]).
    Manual,
}

#[derive(Debug, Clone)]
pub struct StepResult {
    pub step_id: String,
    pub rendered_prompt: String,
    pub run_id: String,
    pub transcript_path: PathBuf,
    /// Final assistant text from this step (used as input for the
    /// next step's template). Empty for skipped steps and for steps
    /// that errored before producing output. For fan-out steps, this
    /// is the JSON-serialized array of per-item outputs.
    pub output: String,
    /// True when the step ran and finished without an agent error.
    /// For fan-out steps, true iff every item succeeded. For panel
    /// steps, true iff every panelist (and the fixer agent, if any)
    /// finished without an agent error — independent of whether the
    /// gate cleared (see `resolved`).
    pub success: bool,
    /// True when the step was skipped because its `when:` expression
    /// evaluated falsy. `success` is false in that case.
    pub skipped: bool,
    /// Workflow-step shape (linear / for_each / parallel / panel).
    /// Persisted into [`crate::runs::StepResultRecord`] so the
    /// line-stream printer can dispatch on it directly.
    pub kind: crate::runs::StepKind,
    /// Per-item records for fan-out steps. Empty for non-fan-out
    /// steps (and for skipped fan-out steps).
    pub items: Vec<ItemResult>,
    /// Aggregated findings for `panel:` steps. Empty for non-panel
    /// steps. Persisted into `StepOutput.findings` for downstream
    /// templates.
    pub findings: Vec<Finding>,
    /// Iteration count for panel steps with a `gate:` loop. `0` for
    /// non-panel steps and panel steps without a gate.
    pub iterations: u32,
    /// `true` when a panel step's gate cleared (or no gate was set).
    /// `false` when `max_iterations` was hit with findings still
    /// above the severity threshold. Always `true` for non-panel
    /// steps.
    pub resolved: bool,
    /// Task 4 (spec §3): which bounded-loop iteration produced this
    /// result, when this `StepResult` belongs to a loop member
    /// (`run_loop_node`'s recursive per-iteration call stamps it from
    /// `SchedulerScope::loop_progress`). `None` for every non-loop step
    /// and for the loop's own synthetic `"loop:<name>"` completion
    /// record — only an actual MEMBER's result carries this. Persisted
    /// into [`crate::runs::StepResultRecord::loop_iteration`]
    /// (`#[serde(default, skip_serializing_if = "Option::is_none")]` —
    /// absent, not `null`, for every legacy/non-loop record, so
    /// `step_results.jsonl` for a loop-free workflow is byte-identical
    /// to before this field existed).
    pub loop_iteration: Option<u32>,
}

/// Runtime form of one finding emitted by a panelist. Aggregated
/// across panelists into [`StepResult::findings`] and exposed to
/// downstream templates as `steps.<id>.findings[*]`.
#[derive(Debug, Clone)]
pub struct Finding {
    /// Panelist agent name that emitted this finding.
    pub source: String,
    pub severity: crate::workflow::Severity,
    pub title: String,
    pub body: String,
}

impl Default for StepResult {
    fn default() -> Self {
        Self {
            step_id: String::new(),
            rendered_prompt: String::new(),
            run_id: String::new(),
            transcript_path: PathBuf::new(),
            output: String::new(),
            success: false,
            skipped: false,
            kind: crate::runs::StepKind::Linear,
            items: Vec::new(),
            findings: Vec::new(),
            iterations: 0,
            // Non-panel steps that complete normally are "resolved";
            // panel-step constructors overwrite this when they
            // decide.
            resolved: true,
            loop_iteration: None,
        }
    }
}

/// One row per unit in a fan-out step — either a `for_each:` item or
/// a `parallel:` sub-step. Carries the same transcript pointer +
/// final-output information as a top-level step so callers (the CLI
/// summary, audit views) can drill into a specific unit's run.
#[derive(Debug, Clone)]
pub struct ItemResult {
    /// 0-based position in the rendered fan-out list (for both shapes,
    /// in declared order).
    pub index: usize,
    /// For `for_each:`: the item value as bound to `{{item}}`. For
    /// `parallel:`: `serde_json::Value::Null` (sub-steps don't have
    /// per-unit data; see `sub_id` instead).
    pub item: serde_json::Value,
    /// For `parallel:`: the sub-step's declared id. Empty for
    /// `for_each:`. When non-empty, this becomes the key in
    /// `steps.<id>.sub_results.<sub_id>`.
    pub sub_id: String,
    pub rendered_prompt: String,
    pub run_id: String,
    pub transcript_path: PathBuf,
    pub output: String,
    pub success: bool,
}

#[derive(Debug, Clone)]
pub struct OrchestratorRunResult {
    pub step_results: Vec<StepResult>,
    /// `run_<ULID>` when a `run_store` was configured; empty
    /// otherwise. Lets the CLI print "rupu workflow show-run <id>"
    /// at the end of a run.
    pub run_id: String,
    /// `Some` when the run paused at an approval gate.
    /// `None` when it ran to completion (or to a hard failure
    /// surfaced as `Err` from `run_workflow`).
    pub awaiting: Option<AwaitingInfo>,
}

/// Snapshot of the state a paused run is waiting for. Returned to
/// the caller so the CLI can print the right hint and operators can
/// see what they're approving.
#[derive(Debug, Clone)]
pub struct AwaitingInfo {
    pub step_id: String,
    pub prompt: String,
    /// When the pending approval expires. `None` when the awaited
    /// step has no `timeout_seconds:` set.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Why the run paused (approval gate vs manual/cooperative pause).
    pub reason: PauseReason,
    /// Seed transcript for a paused-*incomplete* step (a manual pause that
    /// landed mid-step). The caller round-trips this into
    /// [`ResumeState::paused_step`] so the resumed run re-runs that exact step
    /// from where the agent left off. Empty for approval and step-boundary
    /// pauses (nothing to seed — the step runs fresh / was fully completed).
    pub resume_seed: Vec<Message>,
    /// Units of the paused fan-out (`for_each`/`distribute:`) step that
    /// SUCCEEDED before the pause landed (merged with any units already
    /// replayed from an earlier resume). Keyed by 0-based unit index. The
    /// caller round-trips this into `ResumeState::completed_units[step_id]`
    /// so the resumed run re-dispatches ONLY the paused / not-yet-started
    /// units. Empty for every pause shape except a manual pause that landed
    /// mid-fan-out.
    pub fanout_completed_units: std::collections::BTreeMap<usize, ItemResult>,
    /// Every gate parked in this pause (Phase 2, Task 5b-1, spec §7).
    /// `step_id`/`prompt`/`expires_at` above mirror this set's FIRST
    /// element — the "single-gate convenience" existing callers (the CLI's
    /// `result.awaiting.step_id`/`.prompt`) keep reading unchanged. Empty
    /// for a `PauseReason::Manual` pause (the gate-awaiting-set model is
    /// specific to approval gates); one element for every approval pause
    /// through the legacy single-cursor loop or a DAG run whose batch-park
    /// wave reached exactly one gate; more than one for a DAG run whose
    /// wave reached several concurrent gates at once.
    pub gates: Vec<crate::runs::AwaitingGate>,
}

/// Caller-supplied resume context. When `OrchestratorRunOpts.resume_from`
/// is `Some`, the runner skips run-record creation, treats every
/// step in `prior_step_results` as already done (replays their
/// outputs into the context), and dispatches the awaited step
/// without re-asking for approval.
#[derive(Debug, Clone)]
pub struct ResumeState {
    pub run_id: String,
    pub prior_step_results: Vec<StepResult>,
    /// The step that was awaiting approval (and is now approved).
    /// The approval check is suppressed for this exact step id —
    /// every other approval gate in the workflow still fires
    /// normally.
    pub approved_step_id: String,
    /// Per-step set of unit indices that already SUCCEEDED in a prior
    /// run. A partially-completed fan-out step is NOT in
    /// `already_done`, so it re-runs — but these units are replayed
    /// from disk instead of re-dispatched. Map `step_id` → {unit
    /// index → its prior `ItemResult`}. Empty for the approval-resume
    /// path (which has no partially-completed fan-out steps).
    pub completed_units:
        std::collections::BTreeMap<String, std::collections::BTreeMap<usize, ItemResult>>,
    /// Why the original run paused. `Approval` (default) drives the existing
    /// approval-resume behavior unchanged; `Manual` marks a cooperative-pause
    /// resume (emits `RunResumed` / `StepResumed`).
    pub reason: PauseReason,
    /// The step that paused mid-run (a manual pause that landed inside a
    /// linear step). On resume this exact step re-runs seeded with its
    /// persisted transcript (role-alternation-safe). `None` for approval and
    /// step-boundary pauses.
    pub paused_step: Option<PausedStep>,
    /// The operator's rejection reason, when this `ResumeState` was built by
    /// [`ResumeState::from_rejection`]. `None` for every other constructor.
    /// Not consulted by [`run_reject_cleanup`] itself (its caller already
    /// has the reason as a plain `&str` argument) — carried here so a
    /// caller that only holds the `ResumeState` (e.g. a future cp-serve
    /// reject worker) doesn't need to thread the reason through separately.
    pub rejected_reason: Option<String>,
}

/// A linear step that paused mid-run, carried on [`ResumeState`] so the
/// resumed run re-seeds the agent from where it left off.
#[derive(Debug, Clone)]
pub struct PausedStep {
    pub step_id: String,
    /// The paused agent's transcript at the pause boundary (its
    /// `RunResult::final_messages`). Ends at the last complete message /
    /// tool result, ready to seed a resume.
    pub seed_messages: Vec<Message>,
}

impl ResumeState {
    /// Resume context that only carries prior step results + the
    /// approved step id (the original approval-resume shape). No
    /// per-unit fan-out checkpoints.
    pub fn from_approval(
        run_id: String,
        prior_step_results: Vec<StepResult>,
        approved_step_id: String,
    ) -> Self {
        Self {
            run_id,
            prior_step_results,
            approved_step_id,
            completed_units: std::collections::BTreeMap::new(),
            reason: PauseReason::Approval,
            paused_step: None,
            rejected_reason: None,
        }
    }

    /// Resume context carrying the run id + prior step results for
    /// [`run_reject_cleanup`] to persist against — the same disk state
    /// `from_approval` carries for approve-resume, plus the operator's
    /// rejection reason. Not a "pause" in the [`PauseReason`] sense (a
    /// rejected gate is terminal, never re-entering [`run_workflow`]'s
    /// main loop), so `reason` stays `PauseReason::Approval` — no new
    /// pause-reason variant is introduced for rejection.
    pub fn from_rejection(
        run_id: String,
        prior_step_results: Vec<StepResult>,
        rejected_step_id: String,
        reason: String,
    ) -> Self {
        Self {
            run_id,
            prior_step_results,
            approved_step_id: rejected_step_id,
            completed_units: std::collections::BTreeMap::new(),
            reason: PauseReason::Approval,
            paused_step: None,
            rejected_reason: Some(reason),
        }
    }
}

pub async fn run_workflow(
    opts: OrchestratorRunOpts,
) -> Result<OrchestratorRunResult, RunWorkflowError> {
    // Phase 2 (Task 5): `is_nonlinear` used to be an honesty GATE here —
    // reject before any work started. It is now a ROUTER instead (spec §9):
    // a workflow whose control flow is more than a single line (split/join,
    // a fork, or a reconverge) executes through the concurrent ready-set
    // scheduler (`run_scheduler`); a legacy/linear workflow (including
    // today's branches and an explicit-but-linear `next` chain) continues
    // through the untouched declaration-order loop (`run_steps_inner`).
    // The actual routing decision is made once, at the `run_steps_inner`/
    // `run_scheduler` call site below — see the comment there. Both return
    // the same `InnerOutcome`, so every consumer below that point (events,
    // awaiting-parking, persistence) is unchanged and shared between the
    // two paths.

    std::fs::create_dir_all(&opts.transcript_dir)?;
    let resolved_inputs = resolve_inputs(&opts.workflow, &opts.inputs)?;
    let workflow_default_continue = opts.workflow.defaults.continue_on_error.unwrap_or(false);

    // Guard: checkpoint-resuming a workflow that has a `workspace: sync` step
    // is not supported in v1.  Replaying from disk checkpoints restores only
    // the unit's OUTPUT, not its workspace delta, so already-succeeded units'
    // file edits would be silently lost.  Refuse loudly rather than let the
    // caller believe the coordinator workspace is up-to-date.
    //
    // This check fires only on the checkpoint-resume path (`resume_from`
    // is Some).  The non-resume path and resume of non-sync workflows are
    // unaffected.
    if opts.resume_from.is_some() && workflow_has_sync_step(&opts) {
        return Err(RunWorkflowError::ResumeWithWorkspaceSync);
    }

    // Persistent run-state setup. Two paths:
    //
    // - Fresh run: `run_store: Some` and `resume_from: None`. We
    //   create a new RunRecord in `<global>/runs/<run-id>/` and
    //   start with an empty step-results list.
    // - Resume: `resume_from: Some`. We reuse the prior run id,
    //   load no new record (the on-disk one is mutated in place),
    //   and seed `step_results` from the persisted history.
    // - In-memory (no run store): an empty `run_id`; persistence
    //   helpers no-op.
    let (run_id, mut step_results, approved_step_id) =
        if let Some(resume) = opts.resume_from.clone() {
            (
                resume.run_id,
                resume.prior_step_results,
                Some(resume.approved_step_id),
            )
        } else if opts.run_store.is_some() {
            // Caller-supplied id (cron-tick polled tier) wins; otherwise
            // generate a fresh ULID-suffixed id.
            let id = opts
                .run_id_override
                .clone()
                .unwrap_or_else(|| format!("run_{}", Ulid::new()));
            (id, Vec::new(), None)
        } else {
            (String::new(), Vec::new(), None)
        };

    // Create the on-disk record only on a fresh run. On resume the
    // record already exists and is mutated by the CLI's approve
    // path before we re-enter the loop.
    let mut run_record_opt = if opts.resume_from.is_none() {
        if let Some(store) = &opts.run_store {
            let yaml = opts.workflow_yaml.as_deref().unwrap_or("");
            let record = crate::runs::RunRecord {
                id: run_id.clone(),
                workflow_name: opts.workflow.name.clone(),
                status: crate::runs::RunStatus::Running,
                inputs: resolved_inputs.clone(),
                event: opts.event.clone(),
                workspace_id: opts.workspace_id.clone(),
                workspace_path: opts.workspace_path.clone(),
                transcript_dir: opts.transcript_dir.clone(),
                started_at: chrono::Utc::now(),
                finished_at: None,
                error_message: None,
                awaiting: Vec::new(),
                awaiting_step_id: None,
                approval_prompt: None,
                awaiting_since: None,
                expires_at: None,
                issue_ref: opts.issue_ref.clone(),
                issue: opts.issue.clone(),
                parent_run_id: None,
                backend_id: None,
                worker_id: None,
                artifact_manifest_path: None,
                runner_pid: Some(std::process::id()),
                source_wake_id: None,
                active_step_id: None,
                active_step_kind: None,
                active_step_agent: None,
                active_step_transcript_path: None,
                resume_requested_at: None,
                resume_claimed_at: None,
                resume_claimed_by: None,
                resume_mode: None,
                resume_gate_id: None,
                final_output: None,
                loop_progress: std::collections::BTreeMap::new(),
            };
            Some(store.create(record, yaml).map_err(map_run_store_err)?)
        } else {
            None
        }
    } else if let Some(store) = &opts.run_store {
        // Resume path: load the existing record so the terminal-flip
        // block at the bottom of the function can update it.
        match store.load(&run_id) {
            Ok(mut rec) => {
                rec.runner_pid = Some(std::process::id());
                if let Err(e) = store.update(&rec) {
                    warn!(error = %e, "failed to persist resumed runner pid");
                }
                Some(rec)
            }
            Err(e) => {
                warn!(error = %e, "failed to load resumed run record");
                None
            }
        }
    } else {
        None
    };

    // Emit RunStarted before entering the step loop.
    if let Some(sink) = opts.event_sink.as_ref() {
        sink.emit(
            &run_id,
            &crate::executor::Event::RunStarted {
                event_version: 1,
                run_id: run_id.clone(),
                workflow_path: opts.workspace_path.join(&opts.workflow.name),
                started_at: chrono::Utc::now(),
            },
        );
        // A manual-pause resume additionally announces `RunResumed`. The
        // approval-resume path (`PauseReason::Approval`) is left byte-for-byte
        // unchanged — no extra event.
        if opts
            .resume_from
            .as_ref()
            .is_some_and(|r| r.reason == PauseReason::Manual)
        {
            sink.emit(
                &run_id,
                &crate::executor::Event::RunResumed {
                    run_id: run_id.clone(),
                },
            );
        }
    }

    // The router (spec §2, §9): a non-linear workflow (split/join/fork/
    // reconverge — `is_nonlinear`) runs through the concurrent ready-set
    // scheduler; every other (legacy/linear) workflow continues through
    // the untouched declaration-order loop — byte-identical by
    // construction, since it's the exact same function call this crate
    // has always made here. This `if`/`else` is the ONLY place the two
    // paths diverge; everything below (event emission, awaiting-parking,
    // run-record persistence) consumes the shared `InnerOutcome` and does
    // not know or care which path produced it.
    //
    // `cancel: None` — a whole-run cancel signal (spec §8) is not yet
    // threaded through `run_workflow`'s public surface; that wiring, and
    // the CP/CLI-facing plumbing to trigger it, is out of this task's
    // scope.
    //
    // Concurrent-gate boundary (T5b): a non-linear workflow whose
    // concurrent paths each reach an approval gate does NOT park all of
    // them independently yet — `run_scheduler`'s ready-drain loop returns
    // `InnerOutcome::Paused` on the FIRST gate it reaches (same as today),
    // so multiple gates are handled sequentially across resumes rather
    // than concurrently. This is correct (every gate is eventually
    // handled, nothing silently skipped) but not the full "awaiting set"
    // model spec §7 describes — that migration (`RunRecord.awaiting_
    // step_id` -> `awaiting: Vec<AwaitingGate>`, approve/reject-by-gate-id,
    // the CP UI, and the gate sweep) is Task 5b's job, not this one's.
    let outcome = if is_nonlinear(&opts.workflow) {
        run_scheduler(
            &opts,
            &run_id,
            &resolved_inputs,
            workflow_default_continue,
            approved_step_id.as_deref(),
            &mut step_results,
            None,
        )
        .await
    } else {
        run_steps_inner(
            &opts,
            &run_id,
            &resolved_inputs,
            workflow_default_continue,
            approved_step_id.as_deref(),
            &mut step_results,
        )
        .await
    };

    // Map the inner outcome onto the persisted terminal status.
    // Paused = `AwaitingApproval` and the record carries the
    // awaiting_step_id + approval_prompt; Done = `Completed`;
    // Error = `Failed`.
    let mut awaiting: Option<AwaitingInfo> = None;
    if let (Some(store), Some(record)) = (opts.run_store.as_ref(), run_record_opt.as_mut()) {
        // Task 4, spec §3: `run_loop_node`'s own checkpoint writes
        // (`persist_loop_progress`) go straight to disk via their own
        // load/mutate/update cycle WHILE the scheduler above was
        // running — `record` here is a snapshot taken/created BEFORE
        // that (at `run_workflow`'s own start), so it's stale on this
        // one field. Refresh it from disk before this terminal-state
        // update below writes `record` back out, or it would clobber
        // every mid-run loop checkpoint back to empty (this run's own
        // `RunRecord`, so no cross-run race — nothing else touches
        // `loop_progress` between the scheduler returning and here).
        if let Ok(fresh) = store.load(&record.id) {
            record.loop_progress = fresh.loop_progress;
        }
        match &outcome {
            Ok(InnerOutcome::Done) => {
                record.status = crate::runs::RunStatus::Completed;
                record.finished_at = Some(chrono::Utc::now());
                record.awaiting_step_id = None;
                record.approval_prompt = None;
                record.awaiting_since = None;
                record.expires_at = None;
                record.runner_pid = None;
                record.active_step_id = None;
                record.active_step_kind = None;
                record.active_step_agent = None;
                record.active_step_transcript_path = None;
                // Defensive: a completed run has no further use for a
                // paused-step seed (there shouldn't be one, but a stale
                // sidecar from an earlier pause/resume cycle must not
                // leak into a future, unrelated pause).
                if let Err(e) = store.clear_paused_seed(&record.id) {
                    warn!(error = %e, "failed to clear paused-step seed on completion");
                }
            }
            Ok(InnerOutcome::Paused {
                step_id,
                prompt,
                reason,
                seed,
                fanout_completed_units,
                gates,
                // `timeout_seconds` is superseded by `gates` (each gate
                // carries its own) for computing `expires_at` below.
                ..
            }) => {
                let now = chrono::Utc::now();
                // Approval → non-terminal `AwaitingApproval` (existing shape).
                // Manual   → non-terminal `Paused`.
                record.status = match reason {
                    PauseReason::Approval => crate::runs::RunStatus::AwaitingApproval,
                    PauseReason::Manual => crate::runs::RunStatus::Paused,
                };
                // Batch-parking (Task 5b-1, spec §7): for an Approval pause,
                // `gates` carries EVERY gate the scheduler reached in this
                // drain wave (one element for the legacy single-cursor loop
                // and for a DAG run whose wave reached exactly one gate;
                // several for a DAG run whose wave reached concurrent gates
                // on independent paths). All gates in one wave share the
                // same `since` (this `now`) — the run paused once. A Manual
                // pause is a single-step concept orthogonal to the gate
                // model (`gates` is always empty for it, by construction —
                // see `GateParked`'s doc) — `record.awaiting` stays EMPTY
                // and only the legacy compat fields carry the paused-mid-
                // step id, exactly as before this task.
                // Task 5b-2a (5b-1 Minor #3): a gate that was ALREADY parked
                // before this resume cycle must keep its ORIGINAL `since`/
                // `expires_at` rather than restarting its timeout clock. On
                // resume, `record` (loaded from disk above, before the
                // scheduler ran) still carries the PRE-resume awaiting set —
                // e.g. after approving gate A, `record.awaiting` here is
                // exactly `[gate B]` with gate B's original park instant.
                // The scheduler recomputes the ready-set from scratch and
                // re-reports every still-parked gate (including B) in
                // `gates` for THIS pass, so without this lookup B would get
                // a fresh `since: now` / `expires_at: now + timeout` every
                // single resume — silently extending its deadline forever.
                // Only a genuinely NEW gate (not in `prior_gates`) gets a
                // fresh clock.
                let prior_gates = record.awaiting.clone();
                let gate_set: Vec<crate::runs::AwaitingGate> = match reason {
                    PauseReason::Approval => gates
                        .iter()
                        .map(|g| match prior_gates.iter().find(|p| p.step_id == g.step_id) {
                            Some(prior) => crate::runs::AwaitingGate {
                                step_id: g.step_id.clone(),
                                prompt: Some(g.prompt.clone()),
                                since: prior.since,
                                expires_at: prior.expires_at,
                            },
                            None => crate::runs::AwaitingGate {
                                step_id: g.step_id.clone(),
                                prompt: Some(g.prompt.clone()),
                                since: now,
                                expires_at: g
                                    .timeout_seconds
                                    .map(|secs| now + chrono::Duration::seconds(secs as i64)),
                            },
                        })
                        .collect(),
                    PauseReason::Manual => Vec::new(),
                };
                record.awaiting = gate_set;
                if *reason == PauseReason::Approval {
                    record.sync_awaiting_compat();
                } else {
                    record.awaiting_step_id = Some(step_id.clone());
                    record.approval_prompt = None;
                    record.awaiting_since = Some(now);
                    record.expires_at = None;
                }
                record.runner_pid = None;
                record.active_step_id = None;
                record.active_step_kind = None;
                record.active_step_agent = None;
                record.active_step_transcript_path = None;
                // Persist the mid-step seed transcript (if any) so a resume
                // in a fresh process (the CP-driven resume worker spawns a
                // new `rupu workflow resume` subprocess) can reconstruct
                // `ResumeState::paused_step` from disk. Empty for approval
                // and step-boundary pauses — nothing to persist.
                if *reason == PauseReason::Manual && !seed.is_empty() {
                    if let Err(e) = store.write_paused_seed(&record.id, seed) {
                        warn!(error = %e, "failed to persist paused-step seed");
                    }
                }
                // Don't set finished_at — the run hasn't ended.
                awaiting = Some(AwaitingInfo {
                    step_id: step_id.clone(),
                    prompt: prompt.clone(),
                    expires_at: record.expires_at,
                    reason: *reason,
                    resume_seed: seed.clone(),
                    fanout_completed_units: fanout_completed_units.clone(),
                    gates: record.awaiting.clone(),
                });
            }
            Err(e) => {
                record.status = crate::runs::RunStatus::Failed;
                record.finished_at = Some(chrono::Utc::now());
                record.error_message = Some(e.to_string());
                record.runner_pid = None;
                record.active_step_id = None;
                record.active_step_kind = None;
                record.active_step_agent = None;
                record.active_step_transcript_path = None;
                if let Err(e) = store.clear_paused_seed(&record.id) {
                    warn!(error = %e, "failed to clear paused-step seed on failure");
                }
            }
        }
        if let Err(persist_err) = store.update(record) {
            warn!(error = %persist_err, "failed to persist terminal run state");
        }
    } else if let Ok(InnerOutcome::Paused {
        step_id,
        prompt,
        timeout_seconds,
        reason,
        seed,
        fanout_completed_units,
        gates,
    }) = &outcome
    {
        // No store but the run paused (approval gate or manual pause) — surface
        // the paused state to the caller anyway.
        let now = chrono::Utc::now();
        let expires_at =
            timeout_seconds.map(|secs| now + chrono::Duration::seconds(secs as i64));
        let gate_set: Vec<crate::runs::AwaitingGate> = match reason {
            PauseReason::Approval => gates
                .iter()
                .map(|g| crate::runs::AwaitingGate {
                    step_id: g.step_id.clone(),
                    prompt: Some(g.prompt.clone()),
                    since: now,
                    expires_at: g
                        .timeout_seconds
                        .map(|secs| now + chrono::Duration::seconds(secs as i64)),
                })
                .collect(),
            PauseReason::Manual => Vec::new(),
        };
        awaiting = Some(AwaitingInfo {
            step_id: step_id.clone(),
            prompt: prompt.clone(),
            expires_at,
            reason: *reason,
            resume_seed: seed.clone(),
            fanout_completed_units: fanout_completed_units.clone(),
            gates: gate_set,
        });
    }

    // Emit terminal run events (skip for Paused — StepAwaitingApproval
    // was already emitted by run_steps_inner).
    if let Some(sink) = opts.event_sink.as_ref() {
        match &outcome {
            Ok(InnerOutcome::Done) => {
                sink.emit(
                    &run_id,
                    &crate::executor::Event::RunCompleted {
                        run_id: run_id.clone(),
                        status: crate::runs::RunStatus::Completed,
                        finished_at: chrono::Utc::now(),
                    },
                );
            }
            Err(e) => {
                sink.emit(
                    &run_id,
                    &crate::executor::Event::RunFailed {
                        run_id: run_id.clone(),
                        error: e.to_string(),
                        finished_at: chrono::Utc::now(),
                    },
                );
            }
            Ok(InnerOutcome::Paused { reason, .. }) => match reason {
                PauseReason::Approval => {
                    // StepAwaitingApproval was already emitted before returning
                    // from run_steps_inner; no additional run-level event here.
                }
                PauseReason::Manual => {
                    // A cooperative pause. `StepPaused` (mid-step) was already
                    // emitted by run_steps_inner; announce the run-level pause.
                    sink.emit(
                        &run_id,
                        &crate::executor::Event::RunPaused {
                            run_id: run_id.clone(),
                        },
                    );
                }
            },
        }
    }

    outcome?;
    Ok(OrchestratorRunResult {
        step_results,
        run_id,
        awaiting,
    })
}

/// True when a cooperative pause has been requested (the token exists and
/// is cancelled). `false` for the no-pause path (token is `None`), so every
/// pause check is a cheap no-op there.
fn pause_triggered(pause: &Option<CancellationToken>) -> bool {
    pause.as_ref().is_some_and(|t| t.is_cancelled())
}

/// True when a whole-run cancel has been requested (Task 4, spec §8) — the
/// harder sibling of [`pause_triggered`]: pause lets in-flight work reach
/// its own safe boundary and is left completely unaffected by this
/// function; cancel aborts in-flight work immediately (see
/// [`cancel_finalize`]). `false` for the no-cancel path (`None`), so this
/// is a cheap no-op for every call site until [`run_scheduler`] is handed
/// a real token.
fn cancel_requested(cancel: Option<&CancellationToken>) -> bool {
    cancel.is_some_and(|t| t.is_cancelled())
}

/// True when any step in the workflow resolves to `workspace: sync`. Used to
/// refuse both checkpoint-resume and pause of sync workflows (their in-flight
/// deltas can't be checkpointed in v1).
fn workflow_has_sync_step(opts: &OrchestratorRunOpts) -> bool {
    opts.workflow
        .steps
        .iter()
        .any(|s| effective_workspace_mode(s, &opts.workflow.defaults) == WorkspaceMode::Sync)
}

/// Outcome of a single linear step: it either completed (success or a
/// tolerated failure) or paused mid-run (cooperative pause landed inside the
/// agent turn). The paused arm carries the seed transcript so the resumed run
/// can continue from where the agent left off.
enum LinearStepOutcome {
    Completed(StepResult),
    Paused {
        step_id: String,
        /// The paused agent's `final_messages` (transcript through the last
        /// complete message / tool result).
        seed: Vec<Message>,
    },
}

/// Outcome of a distributed fan-out (`for_each:` / `distribute:`) step: it
/// either completed (every unit dispatched — success, tolerated failure, or
/// replayed from a prior checkpoint) or paused MID-FAN-OUT (the cooperative
/// pause token fired while units were still in flight). The paused arm
/// carries every unit that already succeeded (freshly dispatched this pass
/// plus any replayed from an earlier resume) so the next resume re-dispatches
/// ONLY the paused / not-yet-started units — never a unit that already has a
/// good result.
enum FanoutStepOutcome {
    Completed(StepResult),
    Paused {
        step_id: String,
        completed_units: std::collections::BTreeMap<usize, ItemResult>,
    },
}

/// Split a paused agent's seed transcript into `(initial_messages, user_message)`
/// for a resumed [`run_agent`].
///
/// `run_agent` appends `Message::user(user_message)` on top of
/// `initial_messages` ONLY when `user_message` is non-empty (an empty message
/// is treated as "seed-only" — the transcript is already complete). We exploit
/// that here: the resumed agent is seeded with the FULL paused transcript
/// AS-IS and handed an EMPTY `user_message`, so exactly one fresh provider
/// request is issued from the intact transcript with no extra turn.
///
/// This is lossless for BOTH pause shapes:
///   * mid-stream pause — the seed ends in a plain-text user message (the seed
///     prompt; partial assistant text was discarded on pause). Replaying it
///     verbatim preserves role alternation.
///   * tool-boundary pause — the seed ends in a user message carrying a
///     `ToolResult` that pairs with the `ToolUse` block in the immediately
///     preceding assistant message. Replaying it verbatim keeps the
///     `tool_use`/`tool_result` pair intact — no dangling `tool_use`, so no
///     Anthropic 400 "tool_use ids without tool_result blocks".
///
/// (If the seed instead ends in an assistant message, or is empty, an empty
/// `user_message` likewise appends nothing and the request still alternates.)
fn split_seed_for_resume(seed: Vec<Message>) -> (Vec<Message>, String) {
    (seed, String::new())
}

/// Inner loop's terminal state. Distinguishes "ran to completion"
/// from "paused at an approval gate" without forcing the caller to
/// inspect persisted state.
#[derive(Debug)]
enum InnerOutcome {
    Done,
    Paused {
        step_id: String,
        prompt: String,
        /// Optional `timeout_seconds:` from the awaited step's
        /// `approval:` block. When `Some`, the runner persists
        /// `expires_at = now() + timeout` so subsequent
        /// `rupu workflow approve` / `runs` can honor it.
        timeout_seconds: Option<u64>,
        /// Approval-gate pause vs manual/cooperative pause.
        reason: PauseReason,
        /// Seed transcript for a manual pause that landed mid-step (the
        /// paused-incomplete linear step's `final_messages`). Empty for
        /// approval and step-boundary pauses.
        seed: Vec<Message>,
        /// Units of a paused fan-out step that already succeeded (see
        /// [`AwaitingInfo::fanout_completed_units`]). Empty except for a
        /// manual pause that landed mid-fan-out.
        fanout_completed_units: std::collections::BTreeMap<usize, ItemResult>,
        /// Every gate parked in this pause (Task 5b-1, spec §7). For
        /// every return site except `run_scheduler`'s batch-park path
        /// this mirrors `step_id`/`prompt`/`timeout_seconds` as a single
        /// element (the "single-gate convenience"); `run_scheduler`'s
        /// batch-park path is the only producer of >1. Empty for a
        /// `PauseReason::Manual` pause — see [`GateParked`]'s doc.
        gates: Vec<GateParked>,
    },
}

/// One gate reached in a batch-park wave, carried on
/// [`InnerOutcome::Paused`] until `run_workflow` turns it into a
/// [`crate::runs::AwaitingGate`] (which needs a `since`/`expires_at`
/// timestamp `run_workflow`, not the scheduler, computes — see that
/// function's Paused-handling arm). Deliberately NOT reused for a
/// `PauseReason::Manual` pause: the awaiting-SET model (spec §7) is
/// specific to approval gates, a manual/cooperative pause is a single-step
/// concept, so every Manual return site leaves this empty rather than
/// synthesizing a manual "gate."
#[derive(Debug, Clone)]
struct GateParked {
    step_id: String,
    prompt: String,
    timeout_seconds: Option<u64>,
}

/// The actual per-step loop, factored out so the surrounding
/// run-store bookkeeping (create-on-start / flip-on-end) can wrap
/// it cleanly.
///
/// - `run_id` is empty when no run-store is configured (legacy
///   in-memory mode); persistence helpers no-op in that case.
/// - `approved_step_id` is set on a resume — the step with that id
///   skips its `approval:` gate (the operator already approved).
///   All other approval gates in the workflow still fire normally.
/// - `step_results` may be pre-seeded on resume; in that case the
///   loop skips any step whose id already appears (replaying their
///   outputs into the context for `{{ steps.<id>.output }}`).
async fn run_steps_inner(
    opts: &OrchestratorRunOpts,
    run_id: &str,
    resolved_inputs: &BTreeMap<String, String>,
    workflow_default_continue: bool,
    approved_step_id: Option<&str>,
    step_results: &mut Vec<StepResult>,
) -> Result<InnerOutcome, RunWorkflowError> {
    let order: Vec<&Step> = opts.workflow.steps.iter().collect();
    run_steps_over(
        &order,
        opts,
        run_id,
        resolved_inputs,
        workflow_default_continue,
        approved_step_id,
        step_results,
    )
    .await
}

/// Effectively-unbounded concurrency cap used when a workflow declares no
/// `max_concurrency:` (§4/§11 of the Phase-2 scheduler design doc: default
/// is unbounded — every ready node runs, including a `split:`'s full
/// fan-out, never artificially serialized). Comfortably under
/// `tokio::sync::Semaphore`'s internal permit ceiling; no real workflow's
/// graph width will ever approach it.
const UNBOUNDED_MAX_CONCURRENCY: usize = 1 << 20;

/// Concurrent ready-set scheduler (Phase-2 design doc §2 "the scheduler",
/// §4 "concurrency"). Unlike [`run_steps_inner`]/[`run_steps_over`] (which
/// dispatch `opts.workflow.steps` one at a time in declaration order), this
/// dispatches EVERY currently-ready node at once and launches each
/// newly-ready node the instant its dependencies clear — not a
/// per-declaration-order walk and not a rigid wave barrier.
///
/// **Readiness / implicit all-join (§2, D2).** A node is ready when every
/// inbound edge from [`workflow_edges`] (control edges — `next`/`split`/
/// branch arms — UNION inferred `steps.X.*` data edges, the same graph
/// [`is_nonlinear`]'s topological check and Task 1's `ready_set_order` used)
/// is `done`. Tracked as a plain Kahn's-algorithm indegree array: a node
/// with N inbound edges simply needs N decrements before it's ready. This
/// covers Task 2's "implicit all-join" requirement with NO separate join
/// code path — a regular node with two predecessors just has indegree 2.
///
/// **`split` (§2, §4).** A `split:` node fans into N targets. It carries no
/// agent (enforced at parse time by `validate_graph`'s
/// `OrchestrationNodeHasWork` check), so it resolves synchronously as an
/// orchestration no-op — an instant `StepResult`, no `tokio::spawn` — the
/// same way `branch` already resolves inline below. Its kind is
/// [`crate::runs::StepKind::Split`] (Task 5b-2b-ii: previously reused
/// [`crate::runs::StepKind::Branch`] to avoid rippling `rupu-cli`/`rupu-cp`'s
/// exhaustive `StepKind` matches while split was still test-only; now that
/// split is live via [`is_nonlinear`] workflows through this same
/// `run_workflow` router, every consumer has its own `Split` arm and the
/// dedicated variant is no longer deferred). Marking the split `done` unlocks every one of its
/// targets in the SAME indegree decrement pass, so they all go ready
/// together and get dispatched concurrently on the very next drain below.
/// (The bounded-loop super-node below — see the `if let Some(loop_suffix)
/// = step.id.strip_prefix("loop:")` branch — historically piggybacked on
/// this exact `StepKind::Split` value too, once loop execution landed on
/// top of this already-live code path; Phase 3 gave it its own
/// [`crate::runs::StepKind::Loop`] instead, so this variant is once again
/// exclusively split's.)
///
/// **Concurrency (§4).** Real dispatch (linear / panel / parallel /
/// for_each / action, via [`run_node`]) is the only work that
/// `tokio::spawn`s, bounded by a workflow-scope [`Semaphore`] sized from
/// `opts.workflow.max_concurrency` (default [`UNBOUNDED_MAX_CONCURRENCY`])
/// — exactly the `Semaphore` + `tokio::spawn` + join pattern
/// [`run_parallel_step`] already uses for one step's internal fan-out,
/// promoted here to the whole graph. A `tokio::task::JoinSet` (keyed by
/// declaration index) holds every in-flight dispatch; the main loop
/// alternates between draining every ready-but-undispatched node
/// (resolving control-flow synchronously, spawning real work) and awaiting
/// exactly the next task to finish (`JoinSet::join_next`, not a
/// wait-for-everyone barrier) — so a node with one predecessor still
/// running launches the moment THAT ONE predecessor completes, without
/// waiting on unrelated siblings. `opts` is cloned once into an `Arc`
/// (`OrchestratorRunOpts` derives `Clone` for exactly this) so every
/// spawned task gets a cheap `Arc::clone` instead of needing `opts` to
/// outlive the function as a bare reference.
///
/// For a legacy linear chain (graph width 1, every sample under
/// `.rupu/workflows/*.yaml`) exactly one node is ever ready at a time, so
/// this degenerates to the original one-at-a-time dispatch — proven
/// byte-for-byte against [`run_steps_inner`] by `dag_scheduler_golden`
/// below, now exercising the concurrent implementation directly (there is
/// no separate sequential-only code path anymore).
///
/// **Persist order.** `step_results` is appended to in the order each
/// node's outcome is drained from the `JoinSet` (completion order) — same
/// as `run_parallel_step`'s own join loop. For a linear chain, completion
/// order IS dispatch order IS declaration order (only one node in flight
/// ever), so this is byte-for-byte unaffected. For a genuine fork,
/// completion order between unrelated siblings is a real race; this is a
/// deliberate choice, not forced into a declaration-order re-sort, because
/// nothing in this crate treats `step_results`' order as positionally
/// meaningful — downstream templates key `steps.<id>` by NAME
/// (`base_context_for_step` builds a `BTreeMap`), so no template-visible
/// behavior depends on inter-sibling order.
///
/// **Error / pause unwinding.** A hard failure (`run_node` returning `Err`
/// — i.e. NOT `continue_on_error`) or any pause (approval-gate parking, the
/// legacy inline gate, the step-boundary check, or a mid-dispatch pause
/// bubbling out as `NodeOutcome::Paused`) stops launching new nodes and
/// returns immediately — "like the loop does" per this task's brief. Any
/// sibling tasks still in the `JoinSet` are ABORTED when it drops (tokio's
/// documented `JoinSet` drop behavior), not gracefully drained to a safe
/// boundary; full in-flight cancellation (a token threaded through
/// `dispatch_one`, §8 of the design doc) is later in this arc. Every
/// sample this task's golden test covers has graph width 1, so there is
/// never another task in flight when this fires — unobservable there.
async fn run_scheduler(
    opts: &OrchestratorRunOpts,
    run_id: &str,
    resolved_inputs: &BTreeMap<String, String>,
    workflow_default_continue: bool,
    approved_step_id: Option<&str>,
    step_results: &mut Vec<StepResult>,
    cancel: Option<&CancellationToken>,
) -> Result<InnerOutcome, RunWorkflowError> {
    run_scheduler_scoped(
        opts,
        run_id,
        resolved_inputs,
        workflow_default_continue,
        approved_step_id,
        step_results,
        cancel,
        SchedulerScope::whole_workflow(),
    )
    .await
}

/// Task 3 (bounded loops, spec §2b-§2e): restricts one [`run_scheduler`]
/// invocation to less than "every step in `opts.workflow.steps`, over
/// `workflow_edges`/`collapsed_graph_edges`". [`SchedulerScope::
/// whole_workflow`] is the zero-cost default every pre-Task-3 call site
/// uses (via the thin [`run_scheduler`] wrapper above) — `only`/`edges`
/// both `None` means every branch below that reads `scope.*` takes
/// exactly the value it always computed, so the legacy/golden path is
/// untouched code, not a parameterized special case of it.
///
/// [`run_loop_node`] is the only OTHER constructor: one loop iteration
/// scopes `only` to the loop's member ids and `edges` to
/// [`loop_forward_edges`] (intra-iteration dependencies only — the
/// feedback back-reference is deliberately excluded, see that function's
/// doc), and seeds `extra_context`/`loop_progress` for the render-context
/// surface members and `until` see.
struct SchedulerScope {
    /// `None` — every step in `wf.steps` is a scheduling candidate
    /// (today's behavior). `Some(ids)` — only ids in this set may ever
    /// enter `ready` / be dispatched for the DURATION of this call; every
    /// other index simply never gets there (never seeded `done`, since
    /// nothing here changes `step_results` seeding — it just never enters
    /// `ready`).
    only: Option<std::collections::BTreeSet<String>>,
    /// `None` — build the graph from `workflow_edges`/
    /// `collapsed_graph_edges` exactly as `graph_mode` already decided.
    /// `Some(edges)` — use exactly this edge list instead (both current
    /// callers that pass `Some` also pass a matching `only`).
    edges: Option<Vec<(String, String)>>,
    /// Extra `StepResult` history merged into every context build
    /// ALONGSIDE `step_results` (never INTO it — this never affects
    /// done/indegree seeding). [`run_loop_node`] uses this to carry the
    /// outer run's history at the moment the loop became ready plus every
    /// PRIOR iteration's member outputs (spec §2d's feedback mechanic),
    /// without those entries causing THIS iteration's members to be
    /// seeded as already-done.
    extra_context: Vec<StepResult>,
    /// The `loops.<name>.{iteration,converged}` render-context surface
    /// (spec §2e) every context build in this call exposes.
    loop_progress: std::collections::BTreeMap<String, crate::templates::LoopProgress>,
}

impl SchedulerScope {
    fn whole_workflow() -> Self {
        Self {
            only: None,
            edges: None,
            extra_context: Vec::new(),
            loop_progress: std::collections::BTreeMap::new(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_scheduler_scoped(
    opts: &OrchestratorRunOpts,
    run_id: &str,
    resolved_inputs: &BTreeMap<String, String>,
    workflow_default_continue: bool,
    approved_step_id: Option<&str>,
    step_results: &mut Vec<StepResult>,
    // Task 4, spec §8: a whole-run cancel signal, DISTINCT from
    // `opts.pause` (the pre-existing cooperative "stop at the next safe
    // boundary" signal — unchanged, still respected below). When `Some`
    // and cancelled, the scheduler stops launching new ready nodes AND
    // aborts every currently in-flight dispatched node immediately (via
    // `cancel_finalize`, reusing Task 3's `AbortHandle` map — no parallel
    // cancellation mechanism), persisting a `"cancelled"` marker for each
    // one so a resume replays it as not-live instead of re-dispatching it.
    // `None` (every call site before this task, and every legacy/golden
    // test) preserves prior behavior exactly — this parameter is
    // additive. See this task's report for exactly what "cancel" reaches
    // (the `tokio::spawn`ed task boundary) vs. what stays detached.
    cancel: Option<&CancellationToken>,
    scope: SchedulerScope,
) -> Result<InnerOutcome, RunWorkflowError> {
    // Task 3 (spec §2b): the outer scheduler treats a loop as one
    // super-node collapsed from its members. `wants_loop_supernodes` is
    // true ONLY for the top-level call over a workflow that HAS loops
    // (`scope.edges.is_none()` — a recursive per-iteration call always
    // passes `Some`, so it never re-triggers this). When true, `wf`
    // becomes an owned clone of `opts.workflow` with one bare synthetic
    // `Step` appended per loop (`id: "loop:<name>"`, every other field
    // empty/None) — see `augment_workflow_with_loop_supernodes`'s doc for
    // why this, rather than touching every `wf.steps[idx]` site below,
    // is the surgical way to give a loop super-node a valid index without
    // forking any of this function's join/branch/cancel bookkeeping.
    let wants_loop_supernodes = scope.edges.is_none() && !opts.workflow.loops.is_empty();
    let augmented_wf_owned = if wants_loop_supernodes {
        Some(augment_workflow_with_loop_supernodes(&opts.workflow))
    } else {
        None
    };
    let wf: &Workflow = augmented_wf_owned.as_ref().unwrap_or(&opts.workflow);
    let n = wf.steps.len();
    let index_of: BTreeMap<&str, usize> = wf
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();

    // Task 3, CRITICAL: every loop's own members must NEVER be directly
    // dispatchable by THIS (outer) call — they run exclusively through
    // `run_loop_node`'s own recursive call. Without this, a member has
    // indegree 0 in the collapsed outer graph (which never mentions it —
    // by construction, `collapsed_graph_edges` rewires every edge
    // touching it to the loop's super-node) and is NOT `done`, so it
    // would ALSO be dispatched straight through the outer ready-drain
    // loop's normal real-dispatch path (via `run_node`, with none of the
    // loop's context/iteration surface) — a double-dispatch race against
    // `run_loop_node`'s own scheduling of that exact member. `scope.only`
    // itself is never set for the outer call (the thin `run_scheduler`
    // wrapper always passes `None`), so this is computed here rather than
    // trusted from the caller.
    let effective_only: Option<std::collections::BTreeSet<String>> = if wants_loop_supernodes {
        let member_ids: std::collections::BTreeSet<&str> = opts
            .workflow
            .loops
            .values()
            .flat_map(|d| d.nodes.iter().map(|s| s.as_str()))
            .collect();
        Some(
            wf.steps
                .iter()
                .map(|s| s.id.clone())
                .filter(|id| !member_ids.contains(id.as_str()))
                .collect(),
        )
    } else {
        scope.only.clone()
    };

    // Captured once — both the graph-construction choice just below and
    // the branch-pruning arm further down (which needs REAL reachability
    // over the graph, not the author-declared `branch_skipped` set — Task
    // 3, spec §6) must agree on which mode this run is in. A scoped call
    // (`scope.edges.is_some()`, i.e. `run_loop_node`'s per-iteration
    // sub-DAG) always uses the explicit-graph machinery below, regardless
    // of what `is_nonlinear` alone would say about the whole workflow.
    let graph_mode = scope.edges.is_some() || is_nonlinear(wf);
    let mut indegree: Vec<usize> = vec![0; n];
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    // Reverse of `successors` — every node's direct predecessors. Used by
    // an explicit `join`'s inbound-set (Task 3, spec §5) and by
    // loser-cancellation's ancestor-exclusivity check (Task 3, spec §8).
    // Only meaningful in graph mode (chain mode's `successors` is a
    // synthesized linear chain that doesn't encode a join's real inbound
    // edges at all — see the `else` arm below).
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); n];
    if graph_mode {
        // Genuinely non-linear (split/join/fork, or explicit edges whose
        // declaration order isn't already topological): use the real
        // dependency graph — control edges union inferred data edges —
        // exactly as authored. Independent nodes are MEANT to run
        // concurrently here; that's the whole point of Phase 2.
        //
        // Task 3: `scope.edges` (when present) REPLACES this graph
        // entirely — `run_loop_node`'s per-iteration forward edges, or
        // (when absent but the workflow has loops) the collapsed outer
        // graph over `collapsed_graph_edges` instead of the raw
        // `workflow_edges`. Neither branch fires for a loop-free
        // workflow, so `workflow_edges(wf)` below is the exact
        // pre-Task-3 call, unchanged.
        let edges: Vec<(String, String)> = match &scope.edges {
            Some(e) => e.clone(),
            None if wants_loop_supernodes => crate::workflow::collapsed_graph_edges(&opts.workflow),
            None => workflow_edges(wf),
        };
        for (a, b) in &edges {
            if let (Some(&ai), Some(&bi)) = (index_of.get(a.as_str()), index_of.get(b.as_str())) {
                successors[ai].push(bi);
                predecessors[bi].push(ai);
                indegree[bi] += 1;
            }
        }
    } else {
        // Every workflow the LIVE boundary (`run_workflow`'s `is_nonlinear`
        // honesty gate, ~line 564) treats as linear — edge-free, OR
        // carrying explicit edges that still happen to be declaration-
        // order-topological (e.g. `a (next:[b]); b; c`, an independent
        // trailing step alongside a partial explicit chain) — MUST get the
        // exact declaration-order chain here, not `workflow_edges`' graph.
        // Gating on `workflow_has_explicit_edges` instead of `is_nonlinear`
        // is wrong: it would route that `a;b;c` example into the
        // explicit-edges branch below, where Kahn's algorithm sees `a` and
        // `c` as simultaneously ready (no edge relates them) and dispatches
        // them concurrently — reordering `step_results` vs
        // `run_steps_inner`'s deterministic `a, b, c` the moment a later
        // task lifts the `is_nonlinear` gate and this becomes the live
        // path. `workflow_edges`' inferred data edges are ALSO not a
        // complete substitute for declaration order even in the plain
        // edge-free case: two declaration-adjacent steps that don't
        // reference each other's `steps.*` output and aren't joined by any
        // `next:` have NO edge between them in that graph, yet the legacy
        // contract still requires the earlier one to fully finish before
        // the later one starts (e.g. `gate-demo.yaml`'s `assess` step and
        // its unrelated `ship_gate` approval gate — this exact gap broke
        // the golden test before this fix, since both would otherwise read
        // as simultaneously "ready"). Synthesizing the full chain
        // `steps[i] -> steps[i+1]` is strictly sufficient (a superset of
        // any real data or partial-explicit edge: by the time node `i+1`
        // is ready every one of `0..=i` is already done) and keeps graph
        // width at 1 throughout, so `run_scheduler` degenerates to the
        // exact one-at-a-time order `run_steps_inner` walks.
        for i in 0..n.saturating_sub(1) {
            successors[i].push(i + 1);
            predecessors[i + 1].push(i);
            indegree[i + 1] += 1;
        }
    }

    // Explicit `join` nodes (Task 3, spec §5) — the ONE rule: a join's
    // inbound set is exactly its `predecessors` row (edges that point at
    // it), and its wait POLICY determines the threshold of inbound paths
    // that must land before it fires. A `join:` anywhere makes
    // `is_nonlinear` true (see that function), so `join_threshold` is
    // always empty when `!graph_mode` — chain-mode workflows never
    // exercise any of this task's join/prune/cancel machinery, which is
    // exactly how the golden test's byte-for-byte equality stays intact
    // without a single line of it needing to special-case "am I in chain
    // mode".
    let mut join_threshold: BTreeMap<usize, usize> = BTreeMap::new();
    // Total inbound-edge count per join, independent of `threshold` —
    // needed so a join can still fire once every inbound path is
    // ACCOUNTED FOR (live or pruned/skipped/cancelled), even when too many
    // of them turned out non-live to ever reach `threshold` via live
    // arrivals alone (post-review fix: see `JoinScheduling`'s doc).
    let mut join_inbound_total: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, step) in wf.steps.iter().enumerate() {
        let Some(j) = &step.join else { continue };
        let inbound = predecessors[i].len();
        let threshold = match &j.wait {
            crate::workflow::JoinWait::Keyword(crate::workflow::JoinWaitKeyword::All) => inbound,
            crate::workflow::JoinWait::Keyword(crate::workflow::JoinWaitKeyword::Any) => 1,
            // `validate_graph`'s `JoinCountExceedsInbound` check already
            // rejects `count > inbound` at parse time; nothing to clamp.
            crate::workflow::JoinWait::Count { count } => *count as usize,
        };
        join_threshold.insert(i, threshold);
        join_inbound_total.insert(i, inbound);
    }

    // Resume seeding: steps already in a caller-pre-seeded `step_results`
    // are `done` up front (mirrors `run_steps_over`'s `already_done`), and
    // an already-done `branch:` step's persisted taken-arm output
    // reconstructs `branch_skipped` the same way `run_steps_over` does —
    // see that function's comment for why (an untaken arm not yet reached
    // at the original pause is neither `done` nor `branch_skipped`
    // otherwise, and would wrongly go ready on resume).
    let mut done: Vec<bool> = vec![false; n];
    let mut branch_skipped: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // Real-reachability branch pruning (Task 3, spec §6, graph mode
    // only — see the branch-handling arm below) and join
    // loser-cancellation markers for a node that hasn't been dispatched
    // yet (Task 3, spec §5/§8). Task 4, spec §3: rebuilt from the
    // persisted `"pruned"`/`"cancelled"` `StepResult.output` markers below
    // — a resumed run must REPLAY these decisions, not just rely on
    // `done[i]` already being `true` for that one node (which alone
    // would still stop IT from re-dispatching, but leaves these sets
    // inconsistent with reality for anything else that inspects them).
    let mut pruned: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut cancel_state = Cancellation {
        cancelled: std::collections::BTreeSet::new(),
        in_flight_abort: BTreeMap::new(),
        task_id_to_index: BTreeMap::new(),
        cancelled_by_us: std::collections::BTreeSet::new(),
    };
    for sr in step_results.iter() {
        if let Some(&i) = index_of.get(sr.step_id.as_str()) {
            done[i] = true;
            if let Some(branch) = &wf.steps[i].branch {
                match sr.output.as_str() {
                    "then" => branch_skipped.extend(branch.r#else.iter().cloned()),
                    "else" => branch_skipped.extend(branch.then.iter().cloned()),
                    _ => {}
                }
            }
            // Guarded on `sr.skipped` (always `true` for these two
            // markers, always `false` for a real dispatch/branch/split/
            // join/gate result) so a real agent turn that happens to
            // output the literal text "pruned" or "cancelled" can never
            // be misread as one of these markers.
            if sr.skipped {
                match sr.output.as_str() {
                    "pruned" => {
                        pruned.insert(sr.step_id.clone());
                    }
                    "cancelled" => {
                        cancel_state.cancelled.insert(sr.step_id.clone());
                    }
                    _ => {}
                }
            }
        }
    }
    for (i, &is_done) in done.iter().enumerate() {
        if is_done {
            for &s in &successors[i] {
                indegree[s] = indegree[s].saturating_sub(1);
            }
        }
    }

    // Task 3: `effective_only` excludes every id NOT in the set from ever
    // entering `ready` — used by the outer loop-supernode call to keep a
    // loop's own members from being dispatched directly (they run only
    // through `run_loop_node`'s recursive call, which itself scopes
    // `only` to exactly that loop's members via `scope.only`, folded into
    // `effective_only` above). A `None` (every pre-Task-3 call, and every
    // loop-free workflow) makes this a no-op filter.
    let only_ok = |i: usize| -> bool {
        effective_only
            .as_ref()
            .is_none_or(|set| set.contains(wf.steps[i].id.as_str()))
    };
    let mut ready: std::collections::BTreeSet<usize> = (0..n)
        .filter(|&i| !done[i] && indegree[i] == 0 && !join_threshold.contains_key(&i) && only_ok(i))
        .collect();

    // Task 3 (spec §2e): as each loop super-node resolves in the
    // real-dispatch interception below, its final `loops.<name>.
    // {iteration,converged}` lands here — visible to every LATER context
    // build in THIS SAME call (a loop's outer successors, e.g. `ship`).
    // Stays empty for every pre-Task-3 call and for a scoped per-iteration
    // call (which never dispatches a loop super-node itself — v1 forbids
    // nesting).
    let mut resolved_loop_progress: std::collections::BTreeMap<
        String,
        crate::templates::LoopProgress,
    > = std::collections::BTreeMap::new();

    // Join arrival tracking (Task 3, spec §5) + loser-cancellation
    // bookkeeping (Task 3, spec §8, first use in this arc):
    // `in_flight_abort` lets a join resolution reach into an in-flight
    // sibling's `JoinSet` slot and abort it directly; `task_id_to_index`
    // recovers which of OUR node indices a `JoinError` belongs to (the
    // error itself only carries tokio's own `task::Id`); `cancelled_by_us`
    // distinguishes an EXPECTED abort (swallow it, keep scheduling) from a
    // genuine panic/cancellation (propagate as `SchedulerTaskJoin`, same
    // as before this task). See this task's report for exactly what
    // "cancel" reaches today (the scheduler/`JoinSet` boundary — the
    // dispatched task is aborted, so a MockProvider-backed test agent
    // stops at its next await point) vs. Task 4's deeper agent-run
    // interruption.
    //
    // Task 4, spec §3 (the resume gap Task 3's own report flagged): a join
    // that already fired pre-pause has its own MERGED `StepResult` on disk
    // (`done[j]` is already `true`) — seed `resolved` with every such join
    // up front so it's never re-drained, THEN replay every already-done
    // node's arrival into whatever join(s) it directly feeds (exactly what
    // `mark_done_and_track_joins` would have recorded live). Without this,
    // a `wait: all` join with SOME (not all) inbound paths already done
    // pre-pause resumes with `arrived`/`resolved_count` both at zero and
    // can wait forever for arrivals that already happened.
    let resolved_joins_from_disk: std::collections::BTreeSet<usize> = join_threshold
        .keys()
        .copied()
        .filter(|&j| done[j])
        .collect();
    let mut join_state = JoinScheduling {
        threshold: join_threshold,
        inbound_total: join_inbound_total,
        arrived: BTreeMap::new(),
        resolved_count: BTreeMap::new(),
        resolved: resolved_joins_from_disk,
    };
    let mut resume_join_worklist: Vec<usize> = Vec::new();
    for sr in step_results.iter() {
        if let Some(&i) = index_of.get(sr.step_id.as_str()) {
            resume_join_worklist.extend(track_join_arrivals(
                i,
                !sr.skipped,
                &successors,
                &mut join_state,
            ));
        }
    }
    if !resume_join_worklist.is_empty() {
        drain_joins(
            resume_join_worklist,
            wf,
            opts,
            run_id,
            step_results,
            &mut done,
            &successors,
            &predecessors,
            &mut indegree,
            &mut ready,
            &mut join_state,
            &mut cancel_state,
        );
    }

    let resume_paused_step_id: Option<&str> = opts
        .resume_from
        .as_ref()
        .and_then(|r| r.paused_step.as_ref())
        .map(|ps| ps.step_id.as_str());

    let max_concurrency = wf
        .max_concurrency
        .map(|m| m as usize)
        .unwrap_or(UNBOUNDED_MAX_CONCURRENCY)
        .max(1);
    let semaphore = Arc::new(Semaphore::new(max_concurrency));
    let opts_arc = Arc::new(opts.clone());

    let mut in_flight: tokio::task::JoinSet<(usize, Result<NodeOutcome, RunWorkflowError>)> =
        tokio::task::JoinSet::new();

    // Batch-parking (Task 5b-1, spec §7): every approval gate this wave
    // reaches is recorded here instead of returning immediately. This lets
    // the loop keep dispatching every OTHER ready node and keep draining
    // `in_flight` to completion — a sibling on an unrelated concurrent path
    // that's about to finish is never aborted just because some other path
    // hit a gate. Deliberate non-goal: this is NOT full async parking
    // ("keep computing indefinitely while a gate waits for a human") — the
    // whole run still pauses once no further non-gate progress is
    // possible (both `ready` and `in_flight` empty); it just pauses with
    // EVERY gate reached in the wave in the set, not only the first.
    let mut parked: Vec<GateParked> = Vec::new();

    // Task 4 (spec §3): which bounded-loop iteration THIS call's own
    // dispatches belong to, if any. Non-empty (exactly one entry) only
    // for `run_loop_node`'s per-iteration recursive call — the outer
    // call (and every pre-Task-4 call) always has `scope.loop_progress`
    // empty, so this is `None` there, and every `StepResult` this call
    // persists keeps `loop_iteration: None` (its `Default`) — the
    // legacy/byte-equality guarantee. Stamped onto a member's result at
    // every completion site below (skip / branch / split / real
    // dispatch) right before it's persisted, so `step_results.jsonl`
    // can tell which iteration produced it (Task 4's resume re-entry
    // reads this back via `StepResult::loop_iteration`).
    let current_loop_iteration: Option<u32> = if scope.loop_progress.len() == 1 {
        scope.loop_progress.values().next().map(|p| p.iteration)
    } else {
        None
    };

    loop {
        // Drain every currently-ready, not-yet-dispatched node. Resolving
        // one synchronously (skip / branch / split / gate-approved) can
        // unlock a LATER-indexed node in the same `ready` set — the
        // `while let` keeps draining until nothing more is ready, so a
        // chain of instant no-ops (e.g. several `when:`-skips in a row)
        // still resolves in one pass, exactly like the sequential loop.
        while let Some(&i) = ready.iter().next() {
            // Task 4, spec §8: a whole-run cancel stops launching NEW
            // ready nodes immediately (harder than `pause_triggered`
            // below, which only applies at THIS same boundary for the
            // soft form) — leave `i` in `ready` (about to be abandoned
            // anyway) and fall through to `cancel_finalize` below.
            if cancel_requested(cancel) {
                break;
            }
            ready.remove(&i);
            let step = &wf.steps[i];

            let is_pruned = pruned.contains(&step.id);
            let is_cancelled = !is_pruned && cancel_state.cancelled.contains(&step.id);
            if branch_skipped.contains(&step.id) || is_pruned || is_cancelled {
                // Task 3: a node can now be skipped for three distinct
                // reasons — the pre-existing author-declared branch-arm
                // skip-set (chain mode only), real-reachability branch
                // pruning (spec §6, graph mode — see the branch-handling
                // arm below), or join loser-cancellation (spec §5/§8 —
                // see `drain_joins`). Each persists a distinguishing
                // `output` marker so a later resume (Task 4) can tell
                // them apart on disk.
                let (marker, reason): (&str, &str) = if is_pruned {
                    ("pruned", "pruned (unreachable via the taken branch arm)")
                } else if is_cancelled {
                    (
                        "cancelled",
                        "cancelled (a join resolved without waiting for this path)",
                    )
                } else {
                    ("", "not taken by branch")
                };
                info!(step = %step.id, reason, "skipping");
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepSkipped {
                            run_id: run_id.to_string(),
                            step_id: step.id.clone(),
                            reason: reason.to_string(),
                        },
                    );
                }
                let result = StepResult {
                    step_id: step.id.clone(),
                    rendered_prompt: String::new(),
                    run_id: String::new(),
                    transcript_path: PathBuf::new(),
                    output: marker.to_string(),
                    success: false,
                    skipped: true,
                    kind: step_kind_for_run_record(step),
                    items: Vec::new(),
                    loop_iteration: current_loop_iteration,
                    ..Default::default()
                };
                complete_and_drain_joins(
                    i,
                    false, // skip / prune / cancel no-op — never a live join arrival
                    result,
                    wf,
                    opts,
                    run_id,
                    step_results,
                    &mut done,
                    &successors,
                    &predecessors,
                    &mut indegree,
                    &mut ready,
                    &mut join_state,
                    &mut cancel_state,
                );
                continue;
            }

            if pause_triggered(&opts.pause) {
                if workflow_has_sync_step(opts) {
                    return Err(RunWorkflowError::PauseWithWorkspaceSync);
                }
                info!(step = %step.id, "cooperative pause at step boundary");
                // Manual pause is a hard, immediate return (unlike the
                // approval-gate batch-park above) — the awaiting-set model
                // is specific to approval gates (spec §7), so any gate
                // ALREADY collected into `parked` this wave is dropped
                // here rather than folded into this Manual-reason pause
                // (which the `gates` field below leaves empty). Nothing is
                // corrupted: a dropped-but-parked gate was never persisted
                // (no `StepResult`), so it simply re-parks on the next
                // resume exactly as if this wave had reached it first.
                return Ok(InnerOutcome::Paused {
                    step_id: step.id.clone(),
                    prompt: String::new(),
                    timeout_seconds: None,
                    reason: PauseReason::Manual,
                    seed: Vec::new(),
                    fanout_completed_units: std::collections::BTreeMap::new(),
                    gates: Vec::new(),
                });
            }

            // Task 3 (spec §2d/§2e): `scope.extra_context` (empty for
            // every pre-Task-3 call) merges in ALONGSIDE `step_results` —
            // never INTO it, so it never affects the done/indegree
            // seeding above. `run_loop_node` uses this to carry the
            // feedback mechanic: chained BEFORE `step_results` so a fresh
            // completion THIS call recorded (latest, later in the chain)
            // overwrites a same-id entry `extra_context` carried in from
            // an earlier iteration (`base_context_for_step` builds its
            // map by iterating in order and inserting — last write wins).
            let mut ctx = if scope.extra_context.is_empty() {
                base_context_for_step(
                    resolved_inputs,
                    opts.event.as_ref(),
                    opts.issue.as_ref(),
                    step_results,
                )
            } else {
                let combined: Vec<StepResult> = scope
                    .extra_context
                    .iter()
                    .cloned()
                    .chain(step_results.iter().cloned())
                    .collect();
                base_context_for_step(
                    resolved_inputs,
                    opts.event.as_ref(),
                    opts.issue.as_ref(),
                    &combined,
                )
            };
            // `scope.loop_progress` (this call's own seed, non-empty only
            // for a per-iteration scoped call) and `resolved_loop_progress`
            // (loops THIS call has already resolved, non-empty only for
            // the outer call) are mutually exclusive in practice — merge
            // both so either shape works without an extra branch.
            if !scope.loop_progress.is_empty() || !resolved_loop_progress.is_empty() {
                ctx.loops = scope
                    .loop_progress
                    .iter()
                    .chain(resolved_loop_progress.iter())
                    .map(|(k, v)| (k.clone(), *v))
                    .collect();
            }

            if let Some(when_expr) = &step.when {
                let take =
                    render_when_expression(when_expr, &ctx, render_mode(opts.strict_templates))
                        .map_err(|e| RunWorkflowError::Render {
                            step: step.id.clone(),
                            source: e,
                        })?;
                if !take {
                    info!(step = %step.id, "skipping (when: expression is falsy)");
                    if let Some(sink) = opts.event_sink.as_ref() {
                        sink.emit(
                            run_id,
                            &crate::executor::Event::StepSkipped {
                                run_id: run_id.to_string(),
                                step_id: step.id.clone(),
                                reason: "when: expression evaluated to false".into(),
                            },
                        );
                    }
                    let result = StepResult {
                        step_id: step.id.clone(),
                        rendered_prompt: String::new(),
                        run_id: String::new(),
                        transcript_path: PathBuf::new(),
                        output: String::new(),
                        success: false,
                        skipped: true,
                        items: Vec::new(),
                        loop_iteration: current_loop_iteration,
                        ..Default::default()
                    };
                    complete_and_drain_joins(
                        i,
                        false, // skip / prune / cancel no-op — never a live join arrival
                        result,
                        wf,
                        opts,
                        run_id,
                        step_results,
                        &mut done,
                        &successors,
                        &predecessors,
                        &mut indegree,
                        &mut ready,
                        &mut join_state,
                        &mut cancel_state,
                    );
                    continue;
                }
            }

            if crate::workflow::is_approval_gate(step) {
                let ap = step.approval.as_ref().expect("gate has approval");
                let gate_suppressed = approved_step_id == Some(step.id.as_str());
                let prompt = match &ap.prompt {
                    Some(t) => render_step_prompt(t, &ctx, render_mode(opts.strict_templates))
                        .map_err(|e| RunWorkflowError::Render {
                            step: step.id.clone(),
                            source: e,
                        })?,
                    None => format!(
                        "Approve gate `{}` of workflow `{}`?",
                        step.id, opts.workflow.name
                    ),
                };

                if gate_suppressed {
                    info!(step = %step.id, "gate: resuming with human approval");
                    emit_gate_result(
                        opts,
                        run_id,
                        step,
                        "approved",
                        "human",
                        None,
                        step_results,
                        current_loop_iteration,
                    );
                    let worklist = mark_done_and_track_joins(
                        i,
                        true, // gate resolved with a real decision — a live arrival
                        &successors,
                        &mut indegree,
                        &mut ready,
                        &mut done,
                        &mut join_state,
                    );
                    drain_joins(
                        worklist,
                        wf,
                        opts,
                        run_id,
                        step_results,
                        &mut done,
                        &successors,
                        &predecessors,
                        &mut indegree,
                        &mut ready,
                        &mut join_state,
                        &mut cancel_state,
                    );
                    continue;
                }
                if let Some(expr) = &ap.auto_approve {
                    let truthy =
                        render_when_expression(expr, &ctx, render_mode(opts.strict_templates))
                            .map_err(|e| RunWorkflowError::Render {
                                step: step.id.clone(),
                                source: e,
                            })?;
                    if truthy {
                        info!(step = %step.id, "gate auto-approved");
                        emit_gate_result(
                            opts,
                            run_id,
                            step,
                            "approved",
                            "auto",
                            None,
                            step_results,
                            current_loop_iteration,
                        );
                        let worklist = mark_done_and_track_joins(
                            i,
                            true, // gate resolved with a real decision — a live arrival
                            &successors,
                            &mut indegree,
                            &mut ready,
                            &mut done,
                            &mut join_state,
                        );
                        drain_joins(
                            worklist,
                            wf,
                            opts,
                            run_id,
                            step_results,
                            &mut done,
                            &successors,
                            &predecessors,
                            &mut indegree,
                            &mut ready,
                            &mut join_state,
                            &mut cancel_state,
                        );
                        continue;
                    }
                }
                info!(step = %step.id, "gate: pausing for approval");
                fire_notify_hooks(
                    opts,
                    &step.id,
                    &ap.notify,
                    &ctx,
                    render_mode(opts.strict_templates),
                )
                .await;
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepAwaitingApproval {
                            run_id: run_id.to_string(),
                            step_id: step.id.clone(),
                            reason: prompt.clone(),
                        },
                    );
                }
                // Batch-park (spec §7): record this gate and keep draining
                // the rest of `ready` — do NOT return yet. A sibling on an
                // independent path that also hits a gate in this same wave
                // joins the same `parked` set instead of only ever seeing
                // the first one; a sibling that's genuinely dispatchable
                // still gets dispatched below in this same pass.
                parked.push(GateParked {
                    step_id: step.id.clone(),
                    prompt,
                    timeout_seconds: ap.timeout_seconds,
                });
                continue;
            }

            if let Some(approval) = &step.approval {
                let gate_suppressed = approved_step_id == Some(step.id.as_str())
                    || resume_paused_step_id == Some(step.id.as_str());
                if approval.required && !gate_suppressed {
                    let prompt = match &approval.prompt {
                        Some(template) => render_step_prompt(
                            template,
                            &ctx,
                            render_mode(opts.strict_templates),
                        )
                        .map_err(|e| RunWorkflowError::Render {
                            step: step.id.clone(),
                            source: e,
                        })?,
                        None => format!(
                            "Approve step `{}` of workflow `{}`?",
                            step.id, opts.workflow.name
                        ),
                    };
                    info!(step = %step.id, "pausing for approval");
                    if let Some(sink) = opts.event_sink.as_ref() {
                        sink.emit(
                            run_id,
                            &crate::executor::Event::StepAwaitingApproval {
                                run_id: run_id.to_string(),
                                step_id: step.id.clone(),
                                reason: prompt.clone(),
                            },
                        );
                    }
                    // Batch-park (spec §7) — see the gate-node arm above.
                    parked.push(GateParked {
                        step_id: step.id.clone(),
                        prompt,
                        timeout_seconds: approval.timeout_seconds,
                    });
                    continue;
                }
            }

            if let Some(branch) = &step.branch {
                let branch_timer = std::time::Instant::now();
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepStarted {
                            run_id: run_id.to_string(),
                            step_id: step.id.clone(),
                            kind: crate::runs::StepKind::Branch,
                            agent: None,
                            host: None,
                        },
                    );
                }
                let take = render_when_expression(
                    &branch.condition,
                    &ctx,
                    render_mode(opts.strict_templates),
                )
                .map_err(|e| RunWorkflowError::Render {
                    step: step.id.clone(),
                    source: e,
                })?;
                let taken = if take { "then" } else { "else" };
                if graph_mode {
                    // Task 3, spec §6 (post-review fix): real reachability
                    // over the graph (`successors`, control edges union
                    // inferred data edges), NOT the author-declared
                    // `branch_skipped` set the chain-mode `else` arm below
                    // still relies on, and NOT a naive "reachable from the
                    // untaken arm minus reachable from the taken arm" diff
                    // either — see `branch_prune_set`'s doc for why that
                    // naive version silently over-prunes a node fed by an
                    // UNRELATED live predecessor (not either branch arm).
                    let untaken_ids: &[String] = if take { &branch.r#else } else { &branch.then };
                    let untaken_idxs: Vec<usize> = untaken_ids
                        .iter()
                        .filter_map(|id| index_of.get(id.as_str()).copied())
                        .collect();
                    for idx in branch_prune_set(i, &untaken_idxs, &successors, &predecessors) {
                        pruned.insert(wf.steps[idx].id.clone());
                    }
                } else if take {
                    branch_skipped.extend(branch.r#else.iter().cloned());
                } else {
                    branch_skipped.extend(branch.then.iter().cloned());
                }
                info!(step = %step.id, arm = taken, "branch: took arm");
                let result = StepResult {
                    step_id: step.id.clone(),
                    output: taken.to_string(),
                    success: true,
                    skipped: false,
                    kind: crate::runs::StepKind::Branch,
                    loop_iteration: current_loop_iteration,
                    ..Default::default()
                };
                let duration_ms = branch_timer.elapsed().as_millis() as u64;
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepCompleted {
                            run_id: run_id.to_string(),
                            step_id: step.id.clone(),
                            success: true,
                            duration_ms,
                            host: None,
                        },
                    );
                }
                complete_and_drain_joins(
                    i,
                    true, // branch resolution is a real completion — a live arrival
                    result,
                    wf,
                    opts,
                    run_id,
                    step_results,
                    &mut done,
                    &successors,
                    &predecessors,
                    &mut indegree,
                    &mut ready,
                    &mut join_state,
                    &mut cancel_state,
                );
                continue;
            }

            // `split:` orchestration no-op — see this function's doc.
            if step.split.is_some() {
                let split_timer = std::time::Instant::now();
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepStarted {
                            run_id: run_id.to_string(),
                            step_id: step.id.clone(),
                            kind: crate::runs::StepKind::Split,
                            agent: None,
                            host: None,
                        },
                    );
                }
                let result = StepResult {
                    step_id: step.id.clone(),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    kind: crate::runs::StepKind::Split,
                    loop_iteration: current_loop_iteration,
                    ..Default::default()
                };
                let duration_ms = split_timer.elapsed().as_millis() as u64;
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepCompleted {
                            run_id: run_id.to_string(),
                            step_id: step.id.clone(),
                            success: true,
                            duration_ms,
                            host: None,
                        },
                    );
                }
                complete_and_drain_joins(
                    i,
                    true, // split resolution is a real completion — a live arrival
                    result,
                    wf,
                    opts,
                    run_id,
                    step_results,
                    &mut done,
                    &successors,
                    &predecessors,
                    &mut indegree,
                    &mut ready,
                    &mut join_state,
                    &mut cancel_state,
                );
                continue;
            }

            // Task 3 (spec §2b/§2c): a loop super-node (`step.id ==
            // "loop:<name>"`, only ever present when `wants_loop_
            // supernodes` augmented `wf` above — a real step can never
            // collide here, see `augment_workflow_with_loop_supernodes`'s
            // doc) is driven by `run_loop_node`'s bounded iteration
            // driver instead of `run_node`. Awaited INLINE here (not
            // spawned into `in_flight`) rather than through the
            // `tokio::spawn`+`JoinSet` real-dispatch path below: no
            // required behavior needs a loop to run concurrently with an
            // unrelated OUTER sibling (only *within* one iteration, which
            // `run_loop_node`'s own recursive `run_scheduler_scoped` call
            // already gets for free), and reusing this path would mean
            // widening `NodeOutcome`'s pause/abort-handle plumbing to a
            // second dispatch shape for no required gain — see this
            // task's report for the tradeoff (Task 4 may revisit).
            //
            // Phase 3 fix: this `StepStarted` (and the `StepResult`
            // `run_loop_node` returns below) now carries
            // [`crate::runs::StepKind::Loop`], not the reused
            // [`crate::runs::StepKind::Split`] loop execution originally
            // shipped with — see [`crate::runs::StepKind::Loop`]'s doc for
            // why it was `Split` in the first place.
            if let Some(loop_suffix) = step.id.strip_prefix("loop:") {
                if let Some(loop_def) = opts.workflow.loops.get(loop_suffix) {
                    if let Some(sink) = opts.event_sink.as_ref() {
                        sink.emit(
                            run_id,
                            &crate::executor::Event::StepStarted {
                                run_id: run_id.to_string(),
                                step_id: step.id.clone(),
                                kind: crate::runs::StepKind::Loop,
                                agent: None,
                                host: None,
                            },
                        );
                    }
                    let loop_timer = std::time::Instant::now();
                    let loop_result = Box::pin(run_loop_node(
                        loop_suffix,
                        loop_def,
                        opts,
                        run_id,
                        resolved_inputs,
                        workflow_default_continue,
                        step_results,
                        cancel,
                        approved_step_id,
                    ))
                    .await;
                    match loop_result {
                        Ok(LoopNodeOutcome::Completed(result, progress)) => {
                            let duration_ms = loop_timer.elapsed().as_millis() as u64;
                            if let Some(sink) = opts.event_sink.as_ref() {
                                sink.emit(
                                    run_id,
                                    &crate::executor::Event::StepCompleted {
                                        run_id: run_id.to_string(),
                                        step_id: step.id.clone(),
                                        success: result.success,
                                        duration_ms,
                                        host: None,
                                    },
                                );
                            }
                            // Spec §2e: visible to every LATER context
                            // build in this (outer) call — this loop's
                            // own successors (e.g. `ship`) and any other
                            // node's `when:`/prompt.
                            resolved_loop_progress.insert(loop_suffix.to_string(), progress);
                            complete_and_drain_joins(
                                i,
                                true, // the loop's own resolution is a real completion
                                result,
                                wf,
                                opts,
                                run_id,
                                step_results,
                                &mut done,
                                &successors,
                                &predecessors,
                                &mut indegree,
                                &mut ready,
                                &mut join_state,
                                &mut cancel_state,
                            );
                        }
                        // Task 4 (spec §3/§5): a member (or a gate on
                        // one) paused mid-iteration. NOT a failure —
                        // forward it as the exact same hard-return every
                        // other pause site in this function uses (no
                        // `StepFailed`/`StepCompleted` event for the
                        // loop itself; the paused member's own event was
                        // already emitted deep inside the recursive
                        // call). A later resume re-enters this loop at
                        // its checkpointed iteration (`run_loop_node`'s
                        // `load_loop_start_iteration`) and re-runs only
                        // that iteration's not-done members.
                        Ok(LoopNodeOutcome::Paused {
                            step_id,
                            prompt,
                            timeout_seconds,
                            reason,
                            seed,
                            fanout_completed_units,
                            gates,
                        }) => {
                            return Ok(InnerOutcome::Paused {
                                step_id,
                                prompt,
                                timeout_seconds,
                                reason,
                                seed,
                                fanout_completed_units,
                                gates,
                            });
                        }
                        Err(e) => {
                            if let Some(sink) = opts.event_sink.as_ref() {
                                sink.emit(
                                    run_id,
                                    &crate::executor::Event::StepFailed {
                                        run_id: run_id.to_string(),
                                        step_id: step.id.clone(),
                                        error: e.to_string(),
                                    },
                                );
                            }
                            return Err(e);
                        }
                    }
                    continue;
                }
            }

            // Real dispatch: linear / panel / parallel / for_each / action —
            // spawned concurrently, bounded by `semaphore`. Everything up to
            // and including the `StepStarted` emit mirrors `run_steps_over`
            // exactly; only WHERE the dispatch itself runs changed.
            let effective_continue_on_error =
                step.continue_on_error.unwrap_or(workflow_default_continue);
            persist_active_step(opts, run_id, step, None);
            let step_kind = step_kind_for_run_record(step);
            if let Some(sink) = opts.event_sink.as_ref() {
                sink.emit(
                    run_id,
                    &crate::executor::Event::StepStarted {
                        run_id: run_id.to_string(),
                        step_id: step.id.clone(),
                        kind: step_kind,
                        agent: step.agent.clone(),
                        host: step.host.clone(),
                    },
                );
            }
            if resume_paused_step_id == Some(step.id.as_str()) {
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepResumed {
                            run_id: run_id.to_string(),
                            step_id: step.id.clone(),
                        },
                    );
                }
            }

            let permit_sem = Arc::clone(&semaphore);
            let opts_task = Arc::clone(&opts_arc);
            let run_id_owned = run_id.to_string();
            let step_owned = step.clone();
            let ctx_owned = ctx;
            let abort_handle = in_flight.spawn(async move {
                let _permit = permit_sem
                    .acquire_owned()
                    .await
                    .expect("semaphore not closed");
                let outcome = run_node(
                    &run_id_owned,
                    &step_owned,
                    &ctx_owned,
                    &opts_task,
                    effective_continue_on_error,
                )
                .await;
                (i, outcome)
            });
            // Task 3, spec §8: recorded so a join resolving via `first`/
            // `count` can reach into this exact in-flight slot and abort
            // it (`cancel_state.in_flight_abort`), and so the `JoinError`
            // that abort produces can be traced back to node `i` when it
            // surfaces below (`cancel_state.task_id_to_index` — the
            // `JoinError` itself only carries tokio's own `task::Id`).
            cancel_state.task_id_to_index.insert(abort_handle.id(), i);
            cancel_state.in_flight_abort.insert(i, abort_handle);
        }

        // Task 4, spec §8: the ready-drain loop above only catches the
        // cancel signal at ITS OWN boundary (between draining one
        // ready-but-undispatched node and the next). A node with no
        // predecessor ready to unblock it might sit here for a while with
        // `in_flight` non-empty and `ready` empty — check again so a
        // cancel fired while every currently-ready node had already been
        // drained still gets caught before we settle in to await the next
        // completion.
        if cancel_requested(cancel) {
            return cancel_finalize(
                opts,
                run_id,
                step_results,
                &mut in_flight,
                &mut cancel_state,
                current_loop_iteration,
            )
            .await;
        }

        if in_flight.is_empty() {
            break;
        }

        // Task 4, spec §8: race the next dispatch completion against the
        // cancel signal itself, so a cancel fired WHILE we're awaiting a
        // long-running in-flight node is caught immediately rather than
        // only after that node happens to finish on its own.
        let join_next_result = if let Some(token) = cancel {
            tokio::select! {
                res = in_flight.join_next() => res,
                () = token.cancelled() => {
                    return cancel_finalize(
                        opts, run_id, step_results, &mut in_flight, &mut cancel_state,
                        current_loop_iteration,
                    )
                    .await;
                }
            }
        } else {
            in_flight.join_next().await
        };

        let (i, node_result) = match join_next_result.expect("in_flight is non-empty") {
            Ok(pair) => {
                cancel_state.in_flight_abort.remove(&pair.0);
                pair
            }
            Err(join_err) => {
                let idx = cancel_state.task_id_to_index.remove(&join_err.id());
                if join_err.is_cancelled() {
                    if let Some(ix) = idx {
                        if cancel_state.cancelled_by_us.remove(&ix) {
                            // Expected: this is a join loser we
                            // deliberately aborted (spec §5/§8) —
                            // `in_flight_abort` was already removed at
                            // the moment we called `.abort()`. This node
                            // itself deliberately gets NO `StepResult` of
                            // its own (unchanged from Task 3 — it never
                            // ran, so there's nothing to record, and
                            // `join_and_prune`'s tests assert exactly
                            // this for a single-hop loser).
                            //
                            // **Post-review fix (Task 4 follow-up):** but
                            // its own bookkeeping MUST still run —
                            // `unlock_successors` (decrement its
                            // successors' indegree) and
                            // `track_join_arrivals` (non-live: resolve
                            // any join it directly feeds without counting
                            // as an arrival). Without this, a MULTI-HOP
                            // losing closure (e.g. a losing arm
                            // `A -> B -> C` behind a `wait: any` join,
                            // where `A` is the one actually in-flight and
                            // aborted here) silently strands `B`/`C`:
                            // `drain_joins` already marked them
                            // `cancel_state.cancelled`, but with `A` never
                            // decrementing `B`'s indegree, `B` never
                            // reaches `ready` at all — it never hits the
                            // skip-persist check, never gets a terminal
                            // `StepResult`, and the run still reports
                            // `Ok(Done)` having silently dropped a
                            // reachable node. Running this now lets `B`
                            // (and, cascading, `C`) flow into `ready` and
                            // resolve through the EXISTING
                            // `cancel_state.cancelled` skip-persist path
                            // exactly like a loser that was never
                            // dispatched at all.
                            let worklist = mark_done_and_track_joins(
                                ix,
                                false, // aborted, not a live arrival
                                &successors,
                                &mut indegree,
                                &mut ready,
                                &mut done,
                                &mut join_state,
                            );
                            if !worklist.is_empty() {
                                drain_joins(
                                    worklist,
                                    wf,
                                    opts,
                                    run_id,
                                    step_results,
                                    &mut done,
                                    &successors,
                                    &predecessors,
                                    &mut indegree,
                                    &mut ready,
                                    &mut join_state,
                                    &mut cancel_state,
                                );
                            }
                            continue;
                        }
                    }
                }
                // A genuine panic, or a cancellation we did NOT
                // initiate (e.g. the whole `JoinSet` being dropped) —
                // propagate exactly as before this task.
                return Err(RunWorkflowError::SchedulerTaskJoin(join_err));
            }
        };
        match node_result? {
            NodeOutcome::Paused {
                step_id,
                seed,
                fanout_completed_units,
            } => {
                // Manual/mid-dispatch pause — same hard-return, `parked`-
                // dropping contract as the step-boundary Manual pause
                // above.
                return Ok(InnerOutcome::Paused {
                    step_id,
                    prompt: String::new(),
                    timeout_seconds: None,
                    reason: PauseReason::Manual,
                    seed,
                    fanout_completed_units,
                    gates: Vec::new(),
                });
            }
            NodeOutcome::Completed(mut result) => {
                let done_step_id = wf.steps[i].id.clone();
                clear_active_step(opts, run_id, &done_step_id);
                if cancel_state.cancelled_by_us.remove(&i) {
                    // Lost the abort race — tokio's own docs note a task
                    // that had already finished when `.abort()` was
                    // called still yields `Ok`. This node was already
                    // written off as a join loser (the join resolved
                    // without it); discard the straggler result rather
                    // than double-counting it or re-triggering a join
                    // that already fired.
                    continue;
                }
                // Task 4 (spec §3): stamp which iteration this real
                // dispatch belongs to (`None` outside a loop's own
                // per-iteration recursive call — see
                // `current_loop_iteration`'s doc).
                result.loop_iteration = current_loop_iteration;
                complete_and_drain_joins(
                    i,
                    true, // real dispatch completed — a live arrival
                    result,
                    wf,
                    opts,
                    run_id,
                    step_results,
                    &mut done,
                    &successors,
                    &predecessors,
                    &mut indegree,
                    &mut ready,
                    &mut join_state,
                    &mut cancel_state,
                );
            }
        }
    }

    // The loop above only `break`s once BOTH `ready` and `in_flight` are
    // empty — i.e. no further non-gate progress is possible (the batch-
    // parking non-goal stated on `parked`'s declaration: this is NOT
    // "keep computing while a gate waits", it's "pause once nothing else
    // CAN run"). If any gate was reached along the way, the run pauses now
    // with the FULL set; `step_id`/`prompt`/`timeout_seconds` mirror the
    // first-reached gate for the single-gate callers (`AwaitingInfo`, the
    // legacy CLI printer) that only ever look at those three fields.
    if let Some(first) = parked.first() {
        return Ok(InnerOutcome::Paused {
            step_id: first.step_id.clone(),
            prompt: first.prompt.clone(),
            timeout_seconds: first.timeout_seconds,
            reason: PauseReason::Approval,
            seed: Vec::new(),
            fanout_completed_units: std::collections::BTreeMap::new(),
            gates: parked,
        });
    }

    Ok(InnerOutcome::Done)
}

/// Task 3 (spec §2b): returns an owned clone of `wf` with one bare
/// synthetic [`Step`] appended per loop — `id: "loop:<name>"`, every
/// other field empty/`None`. This is the surgical way to give a loop
/// super-node a valid index into `wf.steps[idx]` without touching any of
/// [`run_scheduler_scoped`]'s (or its helpers': `drain_joins`,
/// `branch_prune_set`, `complete_and_drain_joins`, …) many existing
/// `wf.steps[idx]` reads — a synthetic step has no `when`/`approval`/
/// `branch`/`split`/`join`/`agent`/`action`, so it flows harmlessly
/// through every one of those special-case checks straight to the "real
/// dispatch" site, which is the ONE place that special-cases it (checking
/// `step.id.strip_prefix("loop:")` and redirecting to [`run_loop_node`]).
/// Never called when `wf.loops.is_empty()` (every call site gates on
/// that), so a loop-free workflow's scheduling is completely unaffected —
/// this function doesn't even run for it.
fn augment_workflow_with_loop_supernodes(wf: &Workflow) -> Workflow {
    let mut augmented = wf.clone();
    for name in wf.loops.keys() {
        augmented.steps.push(Step {
            id: format!("loop:{name}"),
            agent: None,
            actions: Vec::new(),
            when: None,
            continue_on_error: None,
            for_each: None,
            parallel: None,
            max_parallel: None,
            prompt: None,
            approval: None,
            panel: None,
            branch: None,
            contract: None,
            distribute: None,
            host: None,
            workspace: None,
            next: Vec::new(),
            depends_on: Vec::new(),
            split: None,
            join: None,
            action: None,
            with: None,
        });
    }
    augmented
}

/// Splits a loop's full internal edge set ([`crate::workflow::
/// loop_internal_edges`] — control UNION data, the feedback data edge
/// included) into exactly the subset safe to feed [`run_loop_node`]'s
/// per-iteration recursive [`run_scheduler_scoped`] call as its
/// intra-iteration dependency graph.
///
/// A control edge ([`crate::workflow::loop_control_edges`]) is always
/// kept — Task 2's `validate_loop_subgraph_acyclic` already proved that
/// subgraph acyclic at parse time, so there's nothing to decide. A DATA
/// edge `(a, b)` (`b`'s template references `steps.a`) is kept ONLY when
/// `a` is a control-DAG ANCESTOR of `b` — a genuine same-iteration
/// dependency, safe to schedule on (e.g. `test` referencing `steps.gen`
/// when a `gen -> test` control edge, or a longer control path, already
/// makes `gen` an ancestor of `test`). Every OTHER data edge — `a` is NOT
/// a control ancestor of `b`, whether unrelated or (the refine-loop
/// shape) a downstream node feeding back upstream — is the loop's
/// controlled FEEDBACK reference (spec §2d): dropped here entirely, so it
/// never gates `b`'s readiness. `run_loop_node` resolves it from the
/// PRIOR iteration's `StepResult` via `extra_context` instead.
fn loop_forward_edges(wf: &Workflow, loop_name: &str) -> Vec<(String, String)> {
    let control = crate::workflow::loop_control_edges(wf, loop_name);
    let control_set: std::collections::BTreeSet<(String, String)> =
        control.iter().cloned().collect();

    // The ACCEPTED graph, seeded with the control edges (Task 2 already
    // proved that subgraph acyclic) and grown by every data edge accepted
    // below — so a later data edge's cycle check sees every earlier
    // accepted one, not just the control edges. `path_exists` answers "is
    // there already a path from `from` to `to`" over this growing graph.
    let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (a, b) in &control {
        adj.entry(a.clone()).or_default().push(b.clone());
    }
    fn path_exists(adj: &BTreeMap<String, Vec<String>>, from: &str, to: &str) -> bool {
        if from == to {
            return true;
        }
        let mut stack = vec![from];
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        while let Some(n) = stack.pop() {
            for next in adj.get(n).map(|v| v.as_slice()).unwrap_or(&[]) {
                if next == to {
                    return true;
                }
                if seen.insert(next.as_str()) {
                    stack.push(next.as_str());
                }
            }
        }
        false
    }

    let mut forward = control.clone();
    for (a, b) in crate::workflow::loop_internal_edges(wf, loop_name) {
        if control_set.contains(&(a.clone(), b.clone())) {
            continue; // already counted via `control` above
        }
        // spec §2d: a member->member DATA ref stays a real intra-iteration
        // (forward) dependency UNLESS adding it would close a cycle over
        // the internal control+data DAG accepted so far — i.e. there's
        // already a path b -> ... -> a. Only THAT edge is the loop's
        // controlled feedback back-reference (resolved from the prior
        // iteration instead, in `run_loop_node`'s `extra_context`); every
        // other data edge is scheduled exactly like `workflow_edges`
        // schedules one everywhere else in this crate.
        if path_exists(&adj, &b, &a) {
            continue; // feedback — excluded from scheduling
        }
        adj.entry(a.clone()).or_default().push(b.clone());
        forward.push((a, b));
    }
    forward
}

/// The bounded iteration driver (spec §2b-§2e). Runs `loop_def`'s member
/// sub-DAG through [`run_scheduler_scoped`] itself — recursively (boxed:
/// async recursion needs an explicit heap indirection to break the
/// otherwise-infinite future-size cycle), restricted to the loop's
/// members and [`loop_forward_edges`] — once per iteration, evaluates
/// `until` against the accumulated context after each one, and converges
/// / re-runs / applies `on_max`. Returns a synthetic [`StepResult`] for
/// the loop's OWN super-node id (`"loop:<name>"`) so its caller
/// ([`run_scheduler_scoped`]'s real-dispatch interception) can feed it
/// through the exact same [`complete_and_drain_joins`] bookkeeping every
/// other node resolution uses.
///
/// **The feedback mechanic (spec §2d), concretely.** `feedback_history`
/// starts as a snapshot of the OUTER `step_results` at the moment the
/// loop became ready (so a member's `{{ steps.seed.output }}` sees the
/// run's real history) and, after each iteration, gains that iteration's
/// own member `StepResult`s appended — NEVER removed/cleared ("carry
/// forward, only overwrite" per the plan). Each iteration's recursive
/// call is handed `feedback_history` as `SchedulerScope::extra_context`,
/// merged into every context build ALONGSIDE that call's own
/// (freshly-empty) `step_results`. A member's context therefore sees:
/// (a) this iteration's already-completed upstream siblings, via that
/// call's own growing `step_results` (normal `base_context_for_step`
/// latest-wins, later in the chain than `extra_context`), and (b)
/// everything else — external history + every PRIOR iteration's member
/// outputs — via `extra_context`. A non-upstream sibling ref therefore
/// resolves to the PRIOR iteration's value (empty on iteration 0), never
/// to a not-yet-run value and never as a live cycle.
///
/// Handing the recursive call a FRESH, empty `step_results` (rather than
/// `feedback_history` itself) is what lets a member re-run every
/// iteration: `run_scheduler_scoped`'s resume-seeding marks `done[i] =
/// true` for every id present in the `step_results` it's given — if a
/// member's PRIOR-iteration result lived in that same vector, it would
/// never be scheduled again this iteration.
///
/// **Resume / checkpoint (Task 4, spec §3).** Not fresh-empty ANYMORE,
/// actually — see below: the recursive call's seed `step_results` is
/// empty only in the common (non-resumed) case; on resume it's exactly
/// this iteration's already-done members, reconstructed from
/// `outer_step_results`.
///
/// The re-entry point is `start_iteration`
/// ([`load_loop_start_iteration`], reading `RunRecord.loop_progress`):
/// `0` for a fresh run (no entry yet) or the LAST iteration a prior
/// process of this same run persisted as in-flight
/// ([`persist_loop_progress`], called at the top of the loop below,
/// BEFORE that iteration's recursive call — so even a pause that landed
/// before any member of it ran still checkpoints correctly at that
/// iteration, not the previous one). A loop whose OWN `"loop:<name>"`
/// synthetic result is already in `outer_step_results` (i.e. it already
/// converged, or exhausted with `on_max: proceed`) is never re-entered
/// at all — `run_scheduler_scoped`'s existing resume-seeding
/// (`done[loop_idx] = true`) keeps the outer ready-drain loop from ever
/// reaching this function for it, with zero code here needed to special-
/// case that (requirement #4 falls out of the pre-existing Phase-2
/// mechanism for free).
///
/// For iteration `start_iteration` specifically, `iteration_results` is
/// seeded from `outer_step_results` with every record matching (a) this
/// loop's members and (b) `loop_iteration == Some(start_iteration)` —
/// i.e. exactly the members THIS iteration already finished before the
/// prior process stopped. Handed to the recursive `run_scheduler_scoped`
/// call as its OWN `step_results`, this makes its resume-seeding mark
/// those members `done` (so they are NOT re-dispatched) while anything
/// else in the iteration still runs. For every iteration AFTER
/// `start_iteration` the same filter naturally returns empty (no
/// records exist yet for an iteration that hasn't started), so this is
/// one code path for both the fresh and resumed cases — not a special
/// resume branch.
///
/// `feedback_history` starts as `outer_step_results.clone()` exactly as
/// before, which already carries every earlier iteration's (and this
/// iteration's own already-done) member results straight from disk on
/// resume — so a member's prior-iteration feedback read reconstructs
/// correctly with no extra bookkeeping.
///
/// A pause/approval gate landing on a member mid-iteration
/// (`InnerOutcome::Paused` from the recursive call) is forwarded
/// verbatim as [`LoopNodeOutcome::Paused`] instead of the fail-loud
/// `LoopIterationPaused` Task 3 shipped — see that variant's doc and
/// this function's caller for how it becomes a normal, resumable
/// `InnerOutcome::Paused` at the outer scheduler level. **Pause
/// granularity decision (spec §3, requirement #5): mid-iteration, not
/// just between-iterations.** This works out to the finer granularity
/// for free (not the simpler between-iterations-only alternative)
/// because the checkpoint primitives above — `loop_iteration`-tagged
/// per-member persistence (already true since every member dispatch
/// funnels through the SAME `persist_step_result` path as any other
/// step) plus the `RunRecord.loop_progress` counter recorded before the
/// iteration starts — are together already sufficient to reconstruct
/// "which members of the in-flight iteration are done" on resume; there
/// is no extra state a between-iterations-only design would avoid
/// needing.
#[allow(clippy::too_many_arguments)]
async fn run_loop_node(
    loop_name: &str,
    loop_def: &crate::workflow::LoopDef,
    opts: &OrchestratorRunOpts,
    run_id: &str,
    resolved_inputs: &BTreeMap<String, String>,
    workflow_default_continue: bool,
    outer_step_results: &mut Vec<StepResult>,
    cancel: Option<&CancellationToken>,
    // Task 4, spec §3/§5 fix: the id of a gate an approve-resume just
    // suppressed, forwarded from `run_workflow`/`run_scheduler_scoped`'s
    // own parameter of the same name. Task 3 hardcoded `None` on the
    // recursive call below — harmless for that task (no test exercised
    // an approval gate INSIDE a loop) but silently broken for one: a
    // loop member gate could never be suppressed on resume, so
    // approving it would just re-park it forever. Threaded through
    // unchanged; `run_scheduler_scoped`'s own gate-suppression check
    // (`approved_step_id == Some(step.id.as_str())`) does the rest.
    approved_step_id: Option<&str>,
) -> Result<LoopNodeOutcome, RunWorkflowError> {
    let wf = &opts.workflow;
    let member_ids: std::collections::BTreeSet<String> = loop_def.nodes.iter().cloned().collect();
    let forward_edges = loop_forward_edges(wf, loop_name);

    let mut feedback_history: Vec<StepResult> = outer_step_results.clone();
    let mut converged = false;
    let start_iteration = load_loop_start_iteration(opts, run_id, loop_name);
    let mut last_iteration = start_iteration;

    for iteration in start_iteration..loop_def.max_iterations {
        last_iteration = iteration;
        // Task 4, spec §3: checkpoint BEFORE this iteration's recursive
        // call — a pause/cancel landing anywhere inside it (even before
        // any member has run) resumes at THIS iteration, never the
        // previous one.
        persist_loop_progress(opts, run_id, loop_name, iteration);

        let mut loop_progress = std::collections::BTreeMap::new();
        loop_progress.insert(
            loop_name.to_string(),
            crate::templates::LoopProgress {
                iteration,
                converged: false,
            },
        );
        let scope = SchedulerScope {
            only: Some(member_ids.clone()),
            edges: Some(forward_edges.clone()),
            extra_context: feedback_history.clone(),
            loop_progress,
        };

        // Task 4, spec §3: seeded with this iteration's already-done
        // members on resume (empty otherwise — see this function's
        // doc); the recursive call's own resume-seeding treats these
        // exactly like Phase 2 treats any other pre-seeded
        // `step_results`, so they are NOT re-dispatched.
        let mut iteration_results: Vec<StepResult> = outer_step_results
            .iter()
            .filter(|sr| sr.loop_iteration == Some(iteration) && member_ids.contains(&sr.step_id))
            .cloned()
            .collect();
        // Everything in `iteration_results` at this point is a SEED —
        // already present in `outer_step_results`/`feedback_history`
        // (that's where it was cloned from). Only the entries the
        // recursive call itself APPENDS past this point are genuinely
        // NEW this iteration; merging the seed back in too would
        // duplicate it (Task 4 resume bug: a resumed, already-done
        // member counted twice in `step_results`, once from the
        // original persist and once from THIS re-merge).
        let seeded_count = iteration_results.len();
        let outcome = Box::pin(run_scheduler_scoped(
            opts,
            run_id,
            resolved_inputs,
            workflow_default_continue,
            approved_step_id,
            &mut iteration_results,
            cancel,
            scope,
        ))
        .await;

        // Fold whatever THIS iteration NEWLY produced — whether it ran
        // to completion, paused partway through, or the recursive call
        // itself returned an `Err` (e.g. `RunCancelled` — Task 4, spec
        // §8/§3: a whole-run cancel aborts an in-flight member, but
        // `iteration_results` already has whatever completed
        // SYNCHRONOUSLY before the abort landed, mutated through the
        // `&mut` regardless of the call's own return value) — into the
        // cross-iteration history (latest-wins via append — never
        // cleared; see this function's doc) AND into the OUTER run's
        // `step_results`, BEFORE inspecting `outcome` or propagating an
        // error. Task 4, spec §3: unconditionally, for every iteration
        // outcome (NOT only a converged/`Done` one) — a member that
        // finished right before a mid-iteration pause/cancel is real,
        // persisted history and must be visible the same way a
        // converged iteration's members are, exactly ONCE (hence
        // `[seeded_count..]`, not the whole vector — see above).
        outer_step_results.extend(iteration_results[seeded_count..].iter().cloned());
        feedback_history.extend(iteration_results[seeded_count..].iter().cloned());
        let outcome = outcome?;

        match outcome {
            InnerOutcome::Done => {}
            InnerOutcome::Paused {
                step_id,
                prompt,
                timeout_seconds,
                reason,
                seed,
                fanout_completed_units,
                gates,
            } => {
                // Task 4, spec §3/§5: a member (or a gate on one) paused
                // mid-iteration. Every member that finished before this
                // point in THIS iteration is already persisted (the
                // recursive call's own dispatch path) and tagged with
                // `loop_iteration`, and just got merged into
                // `outer_step_results`/`feedback_history` above —
                // nothing extra to checkpoint here beyond the
                // `persist_loop_progress` call already made above.
                // Forward the pause verbatim rather than failing the
                // run: the caller turns this into a normal
                // `InnerOutcome::Paused`, and a later resume re-enters
                // this exact iteration via `start_iteration` above,
                // re-running only what didn't finish.
                return Ok(LoopNodeOutcome::Paused {
                    step_id,
                    prompt,
                    timeout_seconds,
                    reason,
                    seed,
                    fanout_completed_units,
                    gates,
                });
            }
        }

        // Everything below only runs on `InnerOutcome::Done` — the
        // `Paused` arm above already returned. `iteration_results` was
        // already merged into `outer_step_results`/`feedback_history`
        // above, before the match.
        let mut until_progress = std::collections::BTreeMap::new();
        until_progress.insert(
            loop_name.to_string(),
            crate::templates::LoopProgress {
                iteration,
                converged: false,
            },
        );
        let mut until_ctx = base_context_for_step(
            resolved_inputs,
            opts.event.as_ref(),
            opts.issue.as_ref(),
            &feedback_history,
        );
        until_ctx.loops = until_progress;
        let holds = render_when_expression(
            &loop_def.until,
            &until_ctx,
            render_mode(opts.strict_templates),
        )
        .map_err(|e| RunWorkflowError::Render {
            step: format!("loop:{loop_name}.until"),
            source: e,
        })?;

        if holds {
            // This (converged) iteration's member results are already in
            // `outer_step_results` (merged unconditionally above) — its
            // successors (e.g. `ship`) see them via the normal
            // `base_context_for_step` path. Never re-persisted here —
            // each member was already persisted once by its own dispatch
            // inside the recursive call above.
            converged = true;
            break;
        }
        if iteration + 1 >= loop_def.max_iterations {
            match loop_def.on_max {
                crate::workflow::OnMax::Fail => {
                    return Err(RunWorkflowError::LoopExhausted {
                        name: loop_name.to_string(),
                        max_iterations: loop_def.max_iterations,
                    });
                }
                crate::workflow::OnMax::Proceed => {
                    converged = false;
                    break;
                }
            }
        }
        // else: `until` didn't hold and the cap isn't reached yet — run
        // the next iteration.
    }

    let output =
        serde_json::json!({ "iteration": last_iteration, "converged": converged }).to_string();
    let progress = crate::templates::LoopProgress {
        iteration: last_iteration,
        converged,
    };
    Ok(LoopNodeOutcome::Completed(
        StepResult {
            step_id: format!("loop:{loop_name}"),
            output,
            success: converged || matches!(loop_def.on_max, crate::workflow::OnMax::Proceed),
            skipped: false,
            // Phase 3 fix: [`crate::runs::StepKind::Loop`], not the
            // reused [`crate::runs::StepKind::Split`] — see that
            // variant's doc.
            kind: crate::runs::StepKind::Loop,
            // Deliberately `None`, not `current_loop_iteration`-style —
            // this is the loop's OWN super-node record, not a member's.
            // See `StepResult::loop_iteration`'s doc.
            ..Default::default()
        },
        progress,
    ))
}

/// [`run_loop_node`]'s result (Task 4, spec §3/§5): either it resolved
/// (converged, or exhausted with `on_max: proceed`) with its synthetic
/// `"loop:<name>"` [`StepResult`] + final [`crate::templates::
/// LoopProgress`], or a member (or a gate on one) hit a real pause
/// boundary mid-iteration — forwarded verbatim from the recursive
/// `run_scheduler_scoped` call's own `InnerOutcome::Paused` (same field
/// set, deliberately not reusing `InnerOutcome` itself so this type
/// can't also carry `InnerOutcome::Done`, which `run_loop_node` never
/// produces this way — it uses `Completed` instead). The caller
/// (`run_scheduler_scoped`'s loop-supernode dispatch interception)
/// turns `Paused` into a normal `InnerOutcome::Paused` hard-return —
/// this is what makes a mid-loop pause resumable instead of the
/// fail-loud `LoopIterationPaused` Task 3 shipped (now removed).
enum LoopNodeOutcome {
    Completed(StepResult, crate::templates::LoopProgress),
    Paused {
        step_id: String,
        prompt: String,
        timeout_seconds: Option<u64>,
        reason: PauseReason,
        seed: Vec<Message>,
        fanout_completed_units: std::collections::BTreeMap<usize, ItemResult>,
        gates: Vec<GateParked>,
    },
}

/// Task 4 (spec §3): the loop-resume checkpoint counter, mirroring
/// [`persist_active_step`]'s load/mutate/store shape. Writes
/// `RunRecord.loop_progress[loop_name] = iteration`, overwriting any
/// prior value for this loop (the running process is always the
/// authority on "what iteration is in flight now"). A no-op when
/// there's no run store or run id (in-memory harness / unit tests
/// without persistence) — the loop still runs correctly in a single
/// process, it just has nothing to resume FROM if the process exits.
fn persist_loop_progress(opts: &OrchestratorRunOpts, run_id: &str, loop_name: &str, iteration: u32) {
    let Some(store) = &opts.run_store else { return };
    if run_id.is_empty() {
        return;
    }
    let Ok(mut record) = store.load(run_id) else {
        return;
    };
    record.loop_progress.insert(loop_name.to_string(), iteration);
    if let Err(e) = store.update(&record) {
        warn!(loop_name, iteration, error = %e, "failed to persist loop progress checkpoint");
    }
}

/// Task 4 (spec §3): the resume-re-entry counterpart to
/// [`persist_loop_progress`] — `0` when there's no run store / run id
/// (fresh in-memory run) or no recorded entry for this loop yet (a
/// truly fresh run, including a fresh run that HAS a run store but
/// hasn't reached this loop before). Otherwise the iteration a prior
/// process of THIS SAME run id last checkpointed as in-flight.
///
/// Deliberately unconditional — this function does not (and cannot,
/// from a plain counter alone) distinguish "fresh run" from "resumed
/// run": both cases return `0` for a loop this run id has never
/// checkpointed, which is exactly correct for a fresh run and would
/// also be correct for a resumed run whose pause landed before this
/// loop's very first iteration was checkpointed (nothing to skip
/// either way).
fn load_loop_start_iteration(opts: &OrchestratorRunOpts, run_id: &str, loop_name: &str) -> u32 {
    let Some(store) = &opts.run_store else { return 0 };
    if run_id.is_empty() {
        return 0;
    }
    store
        .load(run_id)
        .ok()
        .and_then(|r| r.loop_progress.get(loop_name).copied())
        .unwrap_or(0)
}

/// Task 4, spec §8: a whole-run cancel signal fired mid-schedule. Aborts
/// every currently in-flight dispatched node (reuses Task 3's own
/// `AbortHandle` map — no parallel cancellation mechanism is introduced)
/// and drains the `JoinSet` to completion. A node that raced ahead and
/// actually finished before its abort landed is still recorded with its
/// REAL result — tokio's own documented behavior is that `.abort()` on an
/// already-finished task is a no-op, so it surfaces as `Ok`, not `Err`,
/// and there is no reason to discard a good result just because a cancel
/// was in flight at the same moment.
///
/// **Deliberately does NOT persist anything for a node that was actually
/// aborted mid-flight.** Spec §8 is explicit that restart-from-checkpoint
/// means: a node that checkpointed (a fan-out's `completed_units`) picks
/// up from its checkpoint; **a node with no checkpoint restarts CLEAN**.
/// A plain aborted node has no checkpoint (a hard `.abort()` drops its
/// future instantly — no graceful mid-turn checkpoint extraction is
/// possible, unlike the cooperative `opts.pause` path `run_linear_step`/
/// `run_fanout_step` already support), so the only spec-correct
/// resume behavior is "not in `step_results` → re-runs on resume",
/// EXACTLY the existing pause-time-in-flight contract (spec §3: "none of
/// those are `done`, so they simply re-run"). This is why cancel here is
/// NOT symmetric with a join's loser-cancellation (§5, `drain_joins`),
/// which DOES persist a `"cancelled"` marker: a join loser is
/// permanently moot (the join already resolved without it — re-running
/// it later would be pointless), whereas a whole-run-cancelled node has
/// no such permanent reason to stay dead.
///
/// **What this reaches, precisely (see this task's report for the full
/// writeup):** the `tokio::spawn`ed task is aborted at its very next
/// `.await` point — for a real agent dispatch that's wherever
/// `rupu_agent::run_agent`'s own internal awaits are (a provider call, a
/// tool run). The in-flight work is DROPPED, not gracefully interrupted
/// mid-turn. A `for_each`/`distribute:` fan-out node's OWN
/// `completed_units` checkpoint is unaffected by any of this: that
/// mechanism is keyed off `opts.resume_from` (a cooperative-pause
/// checkpoint built by [`run_fanout_step`] itself via `opts.pause`,
/// independent of the scheduler-level hard-cancel signal here) and is
/// exercised unchanged — see this task's report for how a fan-out node
/// restarts from checkpoint under the scheduler.
async fn cancel_finalize(
    opts: &OrchestratorRunOpts,
    run_id: &str,
    step_results: &mut Vec<StepResult>,
    in_flight: &mut tokio::task::JoinSet<(usize, Result<NodeOutcome, RunWorkflowError>)>,
    cancel_state: &mut Cancellation,
    // Task 4 (spec §3): same stamp `complete_and_drain_joins`'s call
    // sites apply — a node that raced to completion just as a
    // whole-run cancel landed still belongs to the iteration this call
    // is scoped to (or `None` outside a loop).
    loop_iteration: Option<u32>,
) -> Result<InnerOutcome, RunWorkflowError> {
    let to_abort: Vec<usize> = cancel_state.in_flight_abort.keys().copied().collect();
    for i in &to_abort {
        if let Some(handle) = cancel_state.in_flight_abort.remove(i) {
            handle.abort();
            cancel_state.cancelled_by_us.insert(*i);
        }
    }
    let aborted = to_abort.len();
    while let Some(joined) = in_flight.join_next().await {
        match joined {
            Ok((_, Ok(NodeOutcome::Completed(mut result)))) => {
                result.loop_iteration = loop_iteration;
                persist_step_result(opts, run_id, &result);
                step_results.push(result);
            }
            Ok((_, Ok(NodeOutcome::Paused { .. }))) => {
                // A node independently noticed its OWN cooperative-pause
                // check (`opts.pause`, unrelated to this cancel signal)
                // while we were tearing down. There's no clean partial
                // state to record for it here — the whole run is ending
                // as `RunCancelled` regardless of which signal a given
                // node happened to notice first.
            }
            Ok((_, Err(e))) => return Err(e),
            Err(join_err) => {
                let idx = cancel_state.task_id_to_index.remove(&join_err.id());
                if join_err.is_cancelled() {
                    if let Some(ix) = idx {
                        if cancel_state.cancelled_by_us.remove(&ix) {
                            // Genuinely aborted by this cancel — no
                            // checkpoint to extract, no marker to
                            // persist (see the doc comment above): the
                            // node simply stays absent from
                            // `step_results` so a resume re-runs it
                            // clean.
                            continue;
                        }
                    }
                }
                // A genuine panic, or a cancellation this function did
                // NOT initiate — propagate rather than mask it behind
                // `RunCancelled`.
                return Err(RunWorkflowError::SchedulerTaskJoin(join_err));
            }
        }
    }
    Err(RunWorkflowError::RunCancelled { aborted })
}

/// Decrement node `i`'s successors' indegree (it just completed — sync
/// resolution or an awaited async dispatch) and insert any that reach 0
/// into `ready`. Shared by every completion path in [`run_scheduler`].
///
/// A successor that's an explicit `join` node (present as a key in
/// `join_threshold`) is deliberately never inserted into `ready` here,
/// even at indegree 0 — Task 3, spec §5: a join's readiness is governed
/// by its wait POLICY (tracked separately in [`JoinScheduling`] and
/// resolved by [`drain_joins`]), not by "every inbound edge resolved".
/// For the default `wait: all` policy the two conditions coincide
/// (`threshold == predecessors.len()`), so a join still fires at exactly
/// the same moment it would have under plain indegree tracking — it's
/// only `first`/`count` that need to fire EARLIER, before indegree
/// reaches 0, which is why joins are excluded from `ready` uniformly
/// rather than only for those two policies.
fn unlock_successors(
    i: usize,
    successors: &[Vec<usize>],
    indegree: &mut [usize],
    ready: &mut std::collections::BTreeSet<usize>,
    join_threshold: &BTreeMap<usize, usize>,
) {
    for &s in &successors[i] {
        indegree[s] = indegree[s].saturating_sub(1);
        if indegree[s] == 0 && !join_threshold.contains_key(&s) {
            ready.insert(s);
        }
    }
}

/// Per-run join-scheduling state (Task 3, spec §5): `threshold[j]` is how
/// many LIVE inbound paths must land before join `j` fires
/// (`predecessors.len()` for `all`, `1` for `any`/`first`, `count` for the
/// count form — see [`run_scheduler`]'s setup). `arrived[j]` is the list of
/// predecessor node indices that completed LIVE (actually ran — NOT
/// pruned/`when:`-skipped/cancelled) so far, in COMPLETION order
/// (meaningful for `first`: it's the single fastest LIVE path; harmless for
/// `all`/`count`, which don't care about order).
///
/// **Post-review fix:** a pruned/skipped/cancelled predecessor must still
/// resolve the join's indegree (so the join can fire on its live arms —
/// [`unlock_successors`] is unconditional) but must NEVER be counted in
/// `arrived` or merged into the join's `results`/`sub_results` — see
/// [`mark_done_and_track_joins`]'s `live` parameter. Because a pruned
/// predecessor is invisible to `arrived`, `threshold` alone can no longer
/// be the only fire condition for `all` (or for `count`/`any` when enough
/// of a join's inbound get pruned that live arrivals can never reach
/// `threshold`): `resolved_count[j]` tracks EVERY inbound resolution
/// (live or not) against `inbound_total[j]` (`predecessors[j].len()`,
/// fixed at setup), and a join fires when EITHER `arrived.len() >=
/// threshold` (the live-count reaches policy) OR `resolved_count[j] >=
/// inbound_total[j]` (everything inbound is accounted for, whether or not
/// enough was live) — the second clause is what prevents a join from
/// hanging forever when too many of its paths get pruned to ever satisfy
/// the first. `resolved` guards against processing the same join twice —
/// necessary because both fire conditions can become true on the same
/// call, and because [`drain_joins`] can cascade (a join feeding another
/// join).
struct JoinScheduling {
    threshold: BTreeMap<usize, usize>,
    inbound_total: BTreeMap<usize, usize>,
    arrived: BTreeMap<usize, Vec<usize>>,
    resolved_count: BTreeMap<usize, usize>,
    resolved: std::collections::BTreeSet<usize>,
}

/// Loser-cancellation bookkeeping (Task 3, spec §8, first use in this arc).
/// `cancelled` is the string (step-id) set the ready-drain loop's skip
/// check consults for a node that was never dispatched at all — the same
/// shape as `branch_skipped`/`pruned`. `in_flight_abort` holds every
/// currently-spawned node's `AbortHandle`, keyed by OUR node index, so a
/// join resolution can reach in and cancel one directly.
/// `task_id_to_index` is the reverse lookup a `JoinError` needs (it only
/// carries tokio's own `task::Id`, not our index). `cancelled_by_us`
/// distinguishes an abort WE initiated (swallow the resulting `JoinError`,
/// or discard a straggler `Ok` that won the race against the abort) from
/// a genuine panic or an externally-initiated cancellation (still a hard
/// error, exactly as before this task).
///
/// **What "cancel" reaches today, vs. Task 4:** aborting the
/// `tokio::spawn`ed task stops the dispatch at its next `.await` point —
/// for a real agent run that's wherever `rupu_agent::run_agent`'s own
/// internal awaits are (a provider call, a tool run), so the in-flight
/// HTTP/tool work is dropped, not gracefully interrupted mid-turn. Task
/// 4's "cancel is restart-aware" (spec §8) — checkpointing partial
/// progress so a restart resumes rather than reruns from scratch — is
/// NOT implemented here; a cancelled node simply never gets a
/// `StepResult` and is never retried within the same run.
struct Cancellation {
    cancelled: std::collections::BTreeSet<String>,
    in_flight_abort: BTreeMap<usize, tokio::task::AbortHandle>,
    task_id_to_index: BTreeMap<tokio::task::Id, usize>,
    cancelled_by_us: std::collections::BTreeSet<usize>,
}

/// Transitive closure (including every start node) reachable by following
/// `adj` from every node in `starts`. Shared by two Task-3 reachability
/// questions that are mirror images of each other:
/// - Branch pruning (spec §6): forward, over `successors` — "everything
///   downstream of the untaken arm".
/// - Join loser-cancellation's ancestor-exclusivity check (spec §5/§8):
///   backward, over `predecessors` — "everything upstream of a losing
///   inbound path".
///
/// In both cases, a node belongs EXCLUSIVELY to one side of a fork iff
///   it's reachable from that side and NOT reachable from the other — see
///   [`branch_prune_set`] and [`drain_joins`], which both compute a set
///   difference of two `reachable_via` calls.
fn reachable_via(
    starts: &[usize],
    adj: &[Vec<usize>],
) -> std::collections::BTreeSet<usize> {
    let mut seen: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut stack: Vec<usize> = starts.to_vec();
    while let Some(n) = stack.pop() {
        if seen.insert(n) {
            stack.extend(adj[n].iter().copied());
        }
    }
    seen
}

/// Like [`reachable_via`], but any edge FROM `cut_from` INTO a node in
/// `cut_to` is skipped during traversal — the one specific edge (or set of
/// edges) this computation is entitled to treat as severed. Lets
/// [`branch_prune_set`] ask "if the branch's untaken-arm edge(s) didn't
/// exist, would this node still be reachable from somewhere?" without
/// actually mutating the graph.
fn reachable_via_excluding(
    starts: &[usize],
    adj: &[Vec<usize>],
    cut_from: usize,
    cut_to: &std::collections::BTreeSet<usize>,
) -> std::collections::BTreeSet<usize> {
    let mut seen: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut stack: Vec<usize> = starts.to_vec();
    while let Some(n) = stack.pop() {
        if seen.insert(n) {
            for &m in &adj[n] {
                if n == cut_from && cut_to.contains(&m) {
                    continue;
                }
                stack.push(m);
            }
        }
    }
    seen
}

/// Task 3, spec §6. **Post-review fix — replaces the original naive
/// "reachable from untaken minus reachable from taken" diff, which had a
/// real silent-drop bug:** that version correctly excluded a reconverge
/// (reachable via BOTH arms) but WRONGLY pruned a node that's reachable
/// via the untaken arm AND via some entirely UNRELATED live predecessor
/// that has nothing to do with either branch arm (e.g. an independent
/// entry step feeding the same downstream node) — that node has a
/// perfectly good way to become ready regardless of this branch's
/// decision, so pruning it silently starved whatever's downstream of it.
///
/// **The fix:** instead of comparing against "reachable from the taken
/// arm specifically", compare `reach_untaken` against "reachable from ANY
/// graph entry point (a node with no inbound edges at all) once ONLY
/// `branch_idx`'s edge(s) to `untaken_targets` are cut" — via
/// [`reachable_via_excluding`]. This is a strict superset of the old
/// taken-arm-only reachability (an entry point can always still reach the
/// taken arm, since that edge is untouched) that ALSO covers any
/// unrelated live path into the same node. A node is pruned only if
/// EVERY structural path into it required the specific edge being cut —
/// which both correctly excludes a reconverge (still reachable via the
/// taken arm) AND correctly excludes a node fed by an unrelated live
/// predecessor (still reachable via that predecessor).
///
/// Only meaningful in graph mode (`successors`/`predecessors` built from
/// [`workflow_edges`], which includes branch-arm edges); chain mode never
/// calls this — see [`run_scheduler`]'s branch-handling arm.
fn branch_prune_set(
    branch_idx: usize,
    untaken_targets: &[usize],
    successors: &[Vec<usize>],
    predecessors: &[Vec<usize>],
) -> std::collections::BTreeSet<usize> {
    let reach_untaken = reachable_via(untaken_targets, successors);
    let untaken_set: std::collections::BTreeSet<usize> = untaken_targets.iter().copied().collect();
    let entries: Vec<usize> = (0..predecessors.len())
        .filter(|&i| predecessors[i].is_empty())
        .collect();
    let reach_from_entries_reduced =
        reachable_via_excluding(&entries, successors, branch_idx, &untaken_set);
    reach_untaken
        .difference(&reach_from_entries_reduced)
        .copied()
        .collect()
}

/// The non-persisting half of "a node just resolved": mark it `done`,
/// unlock its successors' indegree (a pruned/skipped/cancelled node's
/// resolution counts exactly like a real completion for this purpose —
/// Task 3, spec §6's requirement that a pruned predecessor not block a
/// downstream reconverge), and record this node's arrival against every
/// join it directly feeds (Task 3, spec §5) — but ONLY as a live arrival
/// when `live` is `true`. Returns the indices of joins newly satisfied —
/// the caller ([`drain_joins`]) resolves those. Split out from
/// [`finish_node`] so the two call sites that persist via
/// [`emit_gate_result`] (which already pushes a `StepResult` itself)
/// don't push a second one.
///
/// **`live` — post-review fix, CRITICAL.** `live` must be `false` for a
/// node resolved via the skip/prune/cancel no-op path (branch pruning,
/// `when:`-skip, join loser-cancellation) and `true` for everything that
/// actually ran (real dispatch, `branch`/`split`'s own synchronous
/// resolution, an approval gate's decision, a join's own merge). A
/// NON-live resolution still counts toward `resolved_count` (so a join
/// waiting on it doesn't hang) but is NEVER pushed into `arrived` — i.e.
/// never merged into a join's `results`/`sub_results`, and never poisons
/// its `success`. Before this fix, every completing node — pruned or not
/// — was recorded as a join arrival unconditionally, so a `wait: all`
/// join downstream of a branch merged the DEAD arm's `"pruned"` output
/// and forced `success = false` even though the live arm succeeded.
#[allow(clippy::too_many_arguments)]
fn mark_done_and_track_joins(
    i: usize,
    live: bool,
    successors: &[Vec<usize>],
    indegree: &mut [usize],
    ready: &mut std::collections::BTreeSet<usize>,
    done: &mut [bool],
    joins: &mut JoinScheduling,
) -> Vec<usize> {
    done[i] = true;
    unlock_successors(i, successors, indegree, ready, &joins.threshold);
    track_join_arrivals(i, live, successors, joins)
}

/// The join-bookkeeping half of [`mark_done_and_track_joins`], split out
/// (Task 4) so [`run_scheduler`]'s resume-seeding can REPLAY it for every
/// node that's already `done` from a prior process's persisted
/// `step_results` — exactly the accounting a join with some (not all)
/// inbound paths already resolved pre-pause needs, or it resumes with
/// `arrived`/`resolved_count` both at zero and can wait forever for
/// arrivals that already happened. See [`mark_done_and_track_joins`] for
/// what `live` must be at each live call site; the resume replay passes
/// `!step_result.skipped` (every skip/prune/cancel marker sets `skipped:
/// true`, matching exactly the `live: false` those call sites already
/// pass).
fn track_join_arrivals(
    i: usize,
    live: bool,
    successors: &[Vec<usize>],
    joins: &mut JoinScheduling,
) -> Vec<usize> {
    let mut newly_satisfied = Vec::new();
    for &s in &successors[i] {
        if joins.resolved.contains(&s) {
            continue;
        }
        let Some(&threshold) = joins.threshold.get(&s) else {
            continue;
        };
        let resolved_count = joins.resolved_count.entry(s).or_insert(0);
        *resolved_count += 1;
        if live {
            let arrived = joins.arrived.entry(s).or_default();
            if !arrived.contains(&i) {
                arrived.push(i);
            }
        }
        let arrived_len = joins.arrived.get(&s).map(Vec::len).unwrap_or(0);
        let inbound_total = joins.inbound_total.get(&s).copied().unwrap_or(0);
        // Fire on whichever condition comes first: enough LIVE arrivals to
        // satisfy the wait policy, OR every inbound path accounted for
        // (live or not) — the second clause is what stops a join from
        // hanging forever when too many of its paths were pruned/skipped
        // to ever reach `threshold` via live arrivals alone.
        if arrived_len >= threshold || *resolved_count >= inbound_total {
            newly_satisfied.push(s);
        }
    }
    newly_satisfied
}

/// Persist + push a completed/skipped/pruned/cancelled node's
/// `StepResult`, then run [`mark_done_and_track_joins`]. The shared tail
/// every completion path in [`run_scheduler`] calls (mirroring what the
/// pre-Task-3 code did inline at each site — skip / when-skip / branch /
/// split / the async `Completed` arm — now also join-aware). See
/// [`mark_done_and_track_joins`] for what `live` must be at each call site.
#[allow(clippy::too_many_arguments)]
fn finish_node(
    i: usize,
    live: bool,
    result: StepResult,
    opts: &OrchestratorRunOpts,
    run_id: &str,
    step_results: &mut Vec<StepResult>,
    done: &mut [bool],
    successors: &[Vec<usize>],
    indegree: &mut [usize],
    ready: &mut std::collections::BTreeSet<usize>,
    joins: &mut JoinScheduling,
) -> Vec<usize> {
    persist_step_result(opts, run_id, &result);
    step_results.push(result);
    mark_done_and_track_joins(i, live, successors, indegree, ready, done, joins)
}

/// Resolve every join in `worklist` (Task 3, spec §5), cascading if
/// resolving one join newly satisfies another (a join feeding a join).
/// For each join `j`:
/// 1. **Winners** are `joins.arrived[j]` (already exactly `threshold`
///    entries, in arrival order — the scheduler processes one node
///    completion at a time, so there's no race within this single-
///    threaded bookkeeping). **Losers** are `j`'s other direct
///    predecessors.
/// 2. **Cancellation** (spec §8): start from the ancestor-exclusive
///    closure of the losers (reachable backward from `losers`, minus
///    reachable backward from `winners` — the same `reachable_via`
///    set-difference [`branch_prune_set`] uses forward). **Post-review
///    fix:** a candidate is then PROTECTED (removed from the closure,
///    left to run normally) if cancelling it would silently strand a
///    live consumer outside this join's losing path — i.e. it has a
///    successor that's neither another still-cancellable candidate nor
///    the join `j` itself (the one edge this resolution is entitled to
///    cut). Protecting a candidate also protects its own candidate
///    ancestors transitively (they must still run to feed it), so this
///    runs to a fixed point. Only what survives that fixed point is
///    actually cancelled: aborted if currently in-flight
///    (`cancel.in_flight_abort`), or marked `cancelled` (if not yet
///    dispatched) so the ready-drain loop's skip check resolves it as a
///    no-op instead of ever running it.
/// 3. **Merge** (spec §5's ONE rule): the join's own `StepResult.items`
///    is one [`ItemResult`] per winner, `sub_id` = that winner's step id
///    (source-keyed) — `base_context_for_step` turns this into BOTH
///    `steps.<join>.results` (ordered list of output strings, winner
///    arrival order) and `steps.<join>.sub_results.<source_id>.output`
///    (keyed lookup) for free, with no new `StepOutput` field. See this
///    task's report for the exact shape + a template example.
#[allow(clippy::too_many_arguments)]
fn drain_joins(
    mut worklist: Vec<usize>,
    wf: &Workflow,
    opts: &OrchestratorRunOpts,
    run_id: &str,
    step_results: &mut Vec<StepResult>,
    done: &mut [bool],
    successors: &[Vec<usize>],
    predecessors: &[Vec<usize>],
    indegree: &mut [usize],
    ready: &mut std::collections::BTreeSet<usize>,
    joins: &mut JoinScheduling,
    cancel: &mut Cancellation,
) {
    while let Some(j) = worklist.pop() {
        if !joins.resolved.insert(j) {
            continue;
        }
        let winners = joins.arrived.get(&j).cloned().unwrap_or_default();
        let losers: Vec<usize> = predecessors[j]
            .iter()
            .copied()
            .filter(|p| !winners.contains(p))
            .collect();

        if !losers.is_empty() {
            let reach_losers = reachable_via(&losers, predecessors);
            let reach_winners = reachable_via(&winners, predecessors);
            let mut cancel_candidates: std::collections::BTreeSet<usize> =
                reach_losers.difference(&reach_winners).copied().collect();
            // Post-review fix: protect (un-cancel) any candidate that
            // feeds a live consumer outside this closure — a successor
            // that's neither another candidate nor the join `j` itself
            // (the one edge we're entitled to cut). Protecting a node
            // also protects its own candidate ancestors transitively
            // (they must still run to feed it), so iterate to a fixed
            // point rather than a single pass.
            loop {
                let protect: Vec<usize> = cancel_candidates
                    .iter()
                    .copied()
                    .filter(|&x| {
                        successors[x]
                            .iter()
                            .any(|&s| s != j && !cancel_candidates.contains(&s))
                    })
                    .collect();
                if protect.is_empty() {
                    break;
                }
                for x in protect {
                    cancel_candidates.remove(&x);
                }
            }
            for idx in cancel_candidates {
                if done[idx] {
                    continue;
                }
                if let Some(handle) = cancel.in_flight_abort.remove(&idx) {
                    handle.abort();
                    cancel.cancelled_by_us.insert(idx);
                } else {
                    cancel.cancelled.insert(wf.steps[idx].id.clone());
                    ready.remove(&idx);
                }
            }
        }

        let join_timer = std::time::Instant::now();
        let mut items = Vec::with_capacity(winners.len());
        let mut outputs = Vec::with_capacity(winners.len());
        let mut all_success = true;
        for (idx0, &w) in winners.iter().enumerate() {
            let source_id = wf.steps[w].id.clone();
            let src = step_results
                .iter()
                .rev()
                .find(|sr| sr.step_id == source_id)
                .expect("a join's winner must already have a StepResult");
            all_success = all_success && src.success;
            outputs.push(src.output.clone());
            items.push(ItemResult {
                index: idx0,
                item: serde_json::Value::Null,
                sub_id: source_id,
                rendered_prompt: String::new(),
                run_id: src.run_id.clone(),
                transcript_path: src.transcript_path.clone(),
                output: src.output.clone(),
                success: src.success,
            });
        }
        let join_step = &wf.steps[j];
        if let Some(sink) = opts.event_sink.as_ref() {
            sink.emit(
                run_id,
                &crate::executor::Event::StepStarted {
                    run_id: run_id.to_string(),
                    step_id: join_step.id.clone(),
                    kind: crate::runs::StepKind::Join,
                    agent: None,
                    host: None,
                },
            );
        }
        let output = serde_json::to_string(&outputs).unwrap_or_else(|_| "[]".into());
        let join_result = StepResult {
            step_id: join_step.id.clone(),
            output,
            success: all_success,
            skipped: false,
            kind: crate::runs::StepKind::Join,
            items,
            ..Default::default()
        };
        let duration_ms = join_timer.elapsed().as_millis() as u64;
        if let Some(sink) = opts.event_sink.as_ref() {
            sink.emit(
                run_id,
                &crate::executor::Event::StepCompleted {
                    run_id: run_id.to_string(),
                    step_id: join_step.id.clone(),
                    success: all_success,
                    duration_ms,
                    host: None,
                },
            );
        }
        let more = finish_node(
            j, true, join_result, opts, run_id, step_results, done, successors, indegree, ready,
            joins,
        );
        worklist.extend(more);
    }
}

/// [`finish_node`] followed by [`drain_joins`] — the common case (every
/// completion path except the two `emit_gate_result`-backed gate-approval
/// sites, which push their own `StepResult` and call
/// [`mark_done_and_track_joins`] + [`drain_joins`] directly).
#[allow(clippy::too_many_arguments)]
fn complete_and_drain_joins(
    i: usize,
    live: bool,
    result: StepResult,
    wf: &Workflow,
    opts: &OrchestratorRunOpts,
    run_id: &str,
    step_results: &mut Vec<StepResult>,
    done: &mut [bool],
    successors: &[Vec<usize>],
    predecessors: &[Vec<usize>],
    indegree: &mut [usize],
    ready: &mut std::collections::BTreeSet<usize>,
    joins: &mut JoinScheduling,
    cancel: &mut Cancellation,
) {
    let worklist = finish_node(
        i, live, result, opts, run_id, step_results, done, successors, indegree, ready, joins,
    );
    drain_joins(
        worklist,
        wf,
        opts,
        run_id,
        step_results,
        done,
        successors,
        predecessors,
        indegree,
        ready,
        joins,
        cancel,
    );
}

/// The actual per-step loop body — [`run_steps_inner`]'s (and only
/// [`run_steps_inner`]'s, since Task 2) shared implementation, iterating
/// `order` (declaration order) directly. Behavior is IDENTICAL to what was,
/// before the Task-1 extraction, `run_steps_inner`'s own body iterating
/// `&opts.workflow.steps` directly.
async fn run_steps_over(
    order: &[&Step],
    opts: &OrchestratorRunOpts,
    run_id: &str,
    resolved_inputs: &BTreeMap<String, String>,
    workflow_default_continue: bool,
    approved_step_id: Option<&str>,
    step_results: &mut Vec<StepResult>,
) -> Result<InnerOutcome, RunWorkflowError> {
    let already_done: std::collections::BTreeSet<String> =
        step_results.iter().map(|sr| sr.step_id.clone()).collect();

    // The step (if any) that paused mid-run in a prior process and is being
    // re-run now. Its `approval:` gate is suppressed and it is re-seeded from
    // its persisted transcript (see `run_linear_step`).
    let resume_paused_step_id: Option<&str> = opts
        .resume_from
        .as_ref()
        .and_then(|r| r.paused_step.as_ref())
        .map(|ps| ps.step_id.as_str());

    // Step ids on branch arms that were NOT taken. Populated when a
    // `branch:` step's condition renders (see the branch arm below); each
    // such step is skipped-in-place when the loop reaches it, mirroring the
    // `when:`-skip shape.
    let mut branch_skipped: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for step in order.iter().copied() {
        // Resume: skip steps that already ran in the prior process.
        if already_done.contains(&step.id) {
            // If the already-done step is a `branch:` step, reconstruct the
            // not-taken arm's skip-set from its PERSISTED result BEFORE we
            // `continue` past it. On resume `branch_skipped` starts empty and
            // the branch arm below (which normally populates it) never runs
            // for an already-done branch. Without this, a not-taken arm step
            // that had not yet been reached when the run paused is neither in
            // `already_done` nor in `branch_skipped`, so it would EXECUTE on
            // resume — dispatching the agent the branch explicitly excluded.
            // The branch's persisted result carries `output == "then"`/`"else"`
            // (the taken arm); mirror the arm-selection logic in the branch
            // arm below so taken/not-taken stays consistent.
            if let Some(branch) = &step.branch {
                let taken = step_results
                    .iter()
                    .find(|sr| sr.step_id == step.id)
                    .map(|sr| sr.output.as_str());
                match taken {
                    Some("then") => branch_skipped.extend(branch.r#else.iter().cloned()),
                    Some("else") => branch_skipped.extend(branch.then.iter().cloned()),
                    _ => {}
                }
            }
            info!(step = %step.id, "resume: skipping already-completed step");
            continue;
        }

        // Branch-skip: this step lies on a branch arm the runner did NOT
        // take. Persist it as skipped (empty output) so downstream
        // reconvergence and `when:` chains observe a real, skipped result —
        // the same shape the `when:`-skip block produces below. Runs before
        // this step's own `when:` / approval / dispatch.
        if branch_skipped.contains(&step.id) {
            info!(step = %step.id, "skipping (not taken by branch)");
            if let Some(sink) = opts.event_sink.as_ref() {
                sink.emit(
                    run_id,
                    &crate::executor::Event::StepSkipped {
                        run_id: run_id.to_string(),
                        step_id: step.id.clone(),
                        reason: "not taken by branch".to_string(),
                    },
                );
            }
            let result = StepResult {
                step_id: step.id.clone(),
                rendered_prompt: String::new(),
                run_id: String::new(),
                transcript_path: PathBuf::new(),
                output: String::new(),
                success: false,
                skipped: true,
                kind: step_kind_for_run_record(step),
                items: Vec::new(),
                ..Default::default()
            };
            persist_step_result(opts, run_id, &result);
            step_results.push(result);
            continue;
        }

        // Step-boundary pause: if a cooperative pause was requested, stop
        // before dispatching the next step. Every step shape pauses cleanly
        // here (fan-out / panel / parallel steps run to completion, then pause
        // at the following boundary). A paused `workspace: sync` workflow is
        // refused loudly — checkpointing it would drop in-flight deltas.
        if pause_triggered(&opts.pause) {
            if workflow_has_sync_step(opts) {
                return Err(RunWorkflowError::PauseWithWorkspaceSync);
            }
            info!(step = %step.id, "cooperative pause at step boundary");
            return Ok(InnerOutcome::Paused {
                step_id: step.id.clone(),
                prompt: String::new(),
                timeout_seconds: None,
                reason: PauseReason::Manual,
                seed: Vec::new(),
                fanout_completed_units: std::collections::BTreeMap::new(),
                gates: Vec::new(),
            });
        }

        // Build template context from inputs + prior step outputs.
        let ctx = base_context_for_step(
            resolved_inputs,
            opts.event.as_ref(),
            opts.issue.as_ref(),
            step_results,
        );

        // `when:` gate. Evaluated against the same context the prompt
        // sees; falsy result skips the step. The skipped step still
        // appears in `step_results` so downstream `when:` chains can
        // observe it.
        if let Some(when_expr) = &step.when {
            let take = render_when_expression(when_expr, &ctx, render_mode(opts.strict_templates))
                .map_err(|e| RunWorkflowError::Render {
                    step: step.id.clone(),
                    source: e,
                })?;
            if !take {
                info!(step = %step.id, "skipping (when: expression is falsy)");
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepSkipped {
                            run_id: run_id.to_string(),
                            step_id: step.id.clone(),
                            reason: "when: expression evaluated to false".into(),
                        },
                    );
                }
                let result = StepResult {
                    step_id: step.id.clone(),
                    rendered_prompt: String::new(),
                    run_id: String::new(),
                    transcript_path: PathBuf::new(),
                    output: String::new(),
                    success: false,
                    skipped: true,
                    items: Vec::new(),
                    ..Default::default()
                };
                persist_step_result(opts, run_id, &result);
                step_results.push(result);
                continue;
            }
        }

        // ── Approval GATE NODE (spec §4.1) ─────────────────────────────
        // A standalone `approval:` step (no agent/prompt/for_each/parallel/
        // panel/branch/action — see `is_approval_gate`). Distinct from the
        // legacy inline `approval:` option handled just below, which stays
        // on an agent-bearing step. Runs BEFORE that legacy check so a gate
        // step never falls through to it (a gate step's `step.approval` is
        // always `Some`, so it would otherwise also match the legacy block
        // and pause twice).
        if crate::workflow::is_approval_gate(step) {
            let ap = step.approval.as_ref().expect("gate has approval");
            let gate_suppressed = approved_step_id == Some(step.id.as_str());
            let prompt = match &ap.prompt {
                Some(t) => render_step_prompt(t, &ctx, render_mode(opts.strict_templates))
                    .map_err(|e| RunWorkflowError::Render {
                        step: step.id.clone(),
                        source: e,
                    })?,
                None => format!(
                    "Approve gate `{}` of workflow `{}`?",
                    step.id, opts.workflow.name
                ),
            };

            if gate_suppressed {
                info!(step = %step.id, "gate: resuming with human approval");
                emit_gate_result(
                    opts, run_id, step, "approved", "human", None, step_results, None,
                );
                continue;
            }
            if let Some(expr) = &ap.auto_approve {
                let truthy =
                    render_when_expression(expr, &ctx, render_mode(opts.strict_templates))
                        .map_err(|e| RunWorkflowError::Render {
                            step: step.id.clone(),
                            source: e,
                        })?;
                if truthy {
                    info!(step = %step.id, "gate auto-approved");
                    emit_gate_result(
                        opts, run_id, step, "approved", "auto", None, step_results, None,
                    );
                    continue;
                }
            }
            info!(step = %step.id, "gate: pausing for approval");
            fire_notify_hooks(
                opts,
                &step.id,
                &ap.notify,
                &ctx,
                render_mode(opts.strict_templates),
            )
            .await;
            if let Some(sink) = opts.event_sink.as_ref() {
                sink.emit(
                    run_id,
                    &crate::executor::Event::StepAwaitingApproval {
                        run_id: run_id.to_string(),
                        step_id: step.id.clone(),
                        reason: prompt.clone(),
                    },
                );
            }
            return Ok(InnerOutcome::Paused {
                step_id: step.id.clone(),
                prompt: prompt.clone(),
                timeout_seconds: ap.timeout_seconds,
                reason: PauseReason::Approval,
                seed: Vec::new(),
                fanout_completed_units: std::collections::BTreeMap::new(),
                gates: vec![GateParked {
                    step_id: step.id.clone(),
                    prompt,
                    timeout_seconds: ap.timeout_seconds,
                }],
            });
        }

        // Approval gate: pause BEFORE dispatching the step. The
        // outer `run_workflow` flips the persisted RunRecord to
        // AwaitingApproval and exits cleanly. On resume the step's
        // id matches `approved_step_id`, so we skip the gate this
        // pass.
        if let Some(approval) = &step.approval {
            // Suppress the gate on resume for the approved step AND for a
            // paused-mid-run step being re-run (it already cleared its gate
            // in the prior process).
            let gate_suppressed = approved_step_id == Some(step.id.as_str())
                || resume_paused_step_id == Some(step.id.as_str());
            if approval.required && !gate_suppressed {
                let prompt = match &approval.prompt {
                    Some(template) => {
                        render_step_prompt(template, &ctx, render_mode(opts.strict_templates))
                            .map_err(|e| RunWorkflowError::Render {
                                step: step.id.clone(),
                                source: e,
                            })?
                    }
                    None => format!(
                        "Approve step `{}` of workflow `{}`?",
                        step.id, opts.workflow.name
                    ),
                };
                info!(step = %step.id, "pausing for approval");
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepAwaitingApproval {
                            run_id: run_id.to_string(),
                            step_id: step.id.clone(),
                            reason: prompt.clone(),
                        },
                    );
                }
                return Ok(InnerOutcome::Paused {
                    step_id: step.id.clone(),
                    prompt: prompt.clone(),
                    timeout_seconds: approval.timeout_seconds,
                    reason: PauseReason::Approval,
                    seed: Vec::new(),
                    fanout_completed_units: std::collections::BTreeMap::new(),
                    gates: vec![GateParked {
                        step_id: step.id.clone(),
                        prompt,
                        timeout_seconds: approval.timeout_seconds,
                    }],
                });
            }
        }

        // Branch step: render the condition, record which arm was taken,
        // and mark every step on the OTHER (not-taken) arm as branch-skipped
        // so the loop skips them when it reaches them. A branch dispatches no
        // agent — it only routes — so it never goes through `run_linear_step`
        // (which panics on the absent prompt) or the shared dispatch
        // machinery below; it emits its own StepStarted/StepCompleted pair.
        if let Some(branch) = &step.branch {
            let branch_timer = std::time::Instant::now();
            if let Some(sink) = opts.event_sink.as_ref() {
                sink.emit(
                    run_id,
                    &crate::executor::Event::StepStarted {
                        run_id: run_id.to_string(),
                        step_id: step.id.clone(),
                        kind: crate::runs::StepKind::Branch,
                        agent: None,
                        host: None,
                    },
                );
            }
            let take = render_when_expression(
                &branch.condition,
                &ctx,
                render_mode(opts.strict_templates),
            )
            .map_err(|e| RunWorkflowError::Render {
                step: step.id.clone(),
                source: e,
            })?;
            let taken = if take { "then" } else { "else" };
            // The arm NOT taken is skipped. `then`/`else` are validated
            // (parse-time) to be forward references and non-overlapping.
            if take {
                branch_skipped.extend(branch.r#else.iter().cloned());
            } else {
                branch_skipped.extend(branch.then.iter().cloned());
            }
            info!(step = %step.id, arm = taken, "branch: took arm");
            let result = StepResult {
                step_id: step.id.clone(),
                output: taken.to_string(),
                success: true,
                skipped: false,
                kind: crate::runs::StepKind::Branch,
                ..Default::default()
            };
            let duration_ms = branch_timer.elapsed().as_millis() as u64;
            if let Some(sink) = opts.event_sink.as_ref() {
                sink.emit(
                    run_id,
                    &crate::executor::Event::StepCompleted {
                        run_id: run_id.to_string(),
                        step_id: step.id.clone(),
                        success: true,
                        duration_ms,
                        host: None,
                    },
                );
            }
            persist_step_result(opts, run_id, &result);
            step_results.push(result);
            continue;
        }

        let effective_continue_on_error =
            step.continue_on_error.unwrap_or(workflow_default_continue);
        persist_active_step(opts, run_id, step, None);

        let step_kind = step_kind_for_run_record(step);
        if let Some(sink) = opts.event_sink.as_ref() {
            sink.emit(
                run_id,
                &crate::executor::Event::StepStarted {
                    run_id: run_id.to_string(),
                    step_id: step.id.clone(),
                    kind: step_kind,
                    agent: step.agent.clone(),
                    host: step.host.clone(),
                },
            );
        }
        // Resume: announce the paused-mid-run step is picking back up.
        if resume_paused_step_id == Some(step.id.as_str()) {
            if let Some(sink) = opts.event_sink.as_ref() {
                sink.emit(
                    run_id,
                    &crate::executor::Event::StepResumed {
                        run_id: run_id.to_string(),
                        step_id: step.id.clone(),
                    },
                );
            }
        }
        match run_node(run_id, step, &ctx, opts, effective_continue_on_error).await? {
            NodeOutcome::Paused {
                step_id,
                seed,
                fanout_completed_units,
            } => {
                return Ok(InnerOutcome::Paused {
                    step_id,
                    prompt: String::new(),
                    timeout_seconds: None,
                    reason: PauseReason::Manual,
                    seed,
                    fanout_completed_units,
                    gates: Vec::new(),
                });
            }
            NodeOutcome::Completed(result) => {
                persist_step_result(opts, run_id, &result);
                clear_active_step(opts, run_id, &step.id);
                step_results.push(result);
            }
        }
    }
    Ok(InnerOutcome::Done)
}

/// Outcome of dispatching one workflow node — panel / parallel / for_each /
/// action / linear — via [`run_node`]. Distinct from [`InnerOutcome`]'s
/// `Paused` variant, which ALSO covers the step-boundary and approval-gate
/// pauses that happen in [`run_steps_over`] BEFORE `run_node` is ever
/// called; those never produce a `NodeOutcome`.
enum NodeOutcome {
    /// The node ran to completion — success or a tolerated
    /// (`continue_on_error`) failure.
    Completed(StepResult),
    /// A cooperative pause landed mid-dispatch (mid-fan-out or
    /// mid-linear-step). Carries exactly the fields that vary for THIS
    /// pause shape — `prompt` is always empty, `timeout_seconds` always
    /// `None`, and `reason` is always [`PauseReason::Manual`] whenever a
    /// pause reaches this point (see [`InnerOutcome::Paused`]'s doc), so
    /// the caller fills those in when it re-wraps this into an
    /// `InnerOutcome::Paused`.
    Paused {
        step_id: String,
        seed: Vec<Message>,
        fanout_completed_units: std::collections::BTreeMap<usize, ItemResult>,
    },
}

/// Dispatch exactly one step (panel / parallel / for_each / action /
/// linear) and return its outcome. Lifted verbatim out of
/// [`run_steps_over`]'s per-step dispatch — a pure extraction, not a
/// behavior change (see this crate's Phase-2 DAG-scheduler foundation
/// work, `run_scheduler`).
///
/// The `StepCompleted`/`StepFailed` emission that used to sit immediately
/// after this block inline in the loop now lives at the end of THIS
/// function instead of in the caller, so it stays colocated with the point
/// where `dispatch_result` is actually finalized — that matters because
/// the two early `return`s below (the fan-out/linear "paused" arms,
/// including the `workflow_has_sync_step` guard) bypass it entirely,
/// exactly as they bypassed the equivalent code when it lived inline in
/// the loop.
async fn run_node(
    run_id: &str,
    step: &Step,
    ctx: &StepContext,
    opts: &OrchestratorRunOpts,
    effective_continue_on_error: bool,
) -> Result<NodeOutcome, RunWorkflowError> {
    let step_timer = std::time::Instant::now();

    let dispatch_result: Result<StepResult, RunWorkflowError> = if step.panel.is_some() {
        run_panel_step(run_id, step, ctx, opts, effective_continue_on_error).await
    } else if step.parallel.is_some() {
        run_parallel_step(step, ctx, opts, effective_continue_on_error).await
    } else if step.for_each.is_some() {
        // A distributed fan-out honors the cooperative pause token
        // MID-UNIT (not just at the step boundary): a paused-incomplete
        // fan-out unwinds here carrying the units that already succeeded,
        // so the resumed run re-dispatches only the paused / not-yet-
        // started ones.
        match run_fanout_step(run_id, step, ctx, opts, effective_continue_on_error).await {
            Ok(FanoutStepOutcome::Paused {
                step_id,
                completed_units,
            }) => {
                info!(step = %step_id, "cooperative pause mid-fan-out");
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepPaused {
                            run_id: run_id.to_string(),
                            step_id: step_id.clone(),
                        },
                    );
                }
                clear_active_step(opts, run_id, &step.id);
                return Ok(NodeOutcome::Paused {
                    step_id,
                    seed: Vec::new(),
                    fanout_completed_units: completed_units,
                });
            }
            Ok(FanoutStepOutcome::Completed(sr)) => Ok(sr),
            Err(e) => Err(e),
        }
    } else if step.action.is_some() {
        // Action steps never pause mid-run — a single dispatcher call is
        // either fast enough to run to completion or fails outright; no
        // cooperative-pause checkpoint shape exists for it.
        match opts.action_dispatcher.as_ref() {
            Some(dispatcher) => {
                execute_action_step(
                    dispatcher,
                    step,
                    ctx,
                    render_mode(opts.strict_templates),
                    effective_continue_on_error,
                    &opts.transcript_dir,
                )
                .await
            }
            None => Err(RunWorkflowError::ActionDispatcherMissing {
                step: step.id.clone(),
            }),
        }
    } else {
        // The linear path is the only other shape that pauses mid-step
        // (its agent run carries the cooperative pause token). A
        // paused-incomplete step unwinds here into a manual-pause
        // checkpoint carrying the seed.
        match run_linear_step(run_id, step, ctx, opts, effective_continue_on_error).await {
            Ok(LinearStepOutcome::Paused { step_id, seed }) => {
                // A `workspace: sync` workflow is refused loudly here too
                // — checkpointing mid-step would drop in-flight deltas
                // the same way a step-boundary/fan-out pause would.
                // Mirrors the step-boundary guard above and the fan-out
                // guard in `run_fanout_step`.
                if workflow_has_sync_step(opts) {
                    return Err(RunWorkflowError::PauseWithWorkspaceSync);
                }
                info!(step = %step_id, "cooperative pause mid-step");
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        run_id,
                        &crate::executor::Event::StepPaused {
                            run_id: run_id.to_string(),
                            step_id: step_id.clone(),
                        },
                    );
                }
                clear_active_step(opts, run_id, &step.id);
                return Ok(NodeOutcome::Paused {
                    step_id,
                    seed,
                    fanout_completed_units: std::collections::BTreeMap::new(),
                });
            }
            Ok(LinearStepOutcome::Completed(sr)) => Ok(sr),
            Err(e) => Err(e),
        }
    };

    let duration_ms = step_timer.elapsed().as_millis() as u64;

    match &dispatch_result {
        Ok(result) => {
            if let Some(sink) = opts.event_sink.as_ref() {
                sink.emit(
                    run_id,
                    &crate::executor::Event::StepCompleted {
                        run_id: run_id.to_string(),
                        step_id: step.id.clone(),
                        success: result.success,
                        duration_ms,
                        host: step.host.clone(),
                    },
                );
            }
        }
        Err(e) => {
            if let Some(sink) = opts.event_sink.as_ref() {
                sink.emit(
                    run_id,
                    &crate::executor::Event::StepFailed {
                        run_id: run_id.to_string(),
                        step_id: step.id.clone(),
                        error: e.to_string(),
                    },
                );
            }
        }
    }

    Ok(NodeOutcome::Completed(dispatch_result?))
}

/// Append one step's record to the run's `step_results.jsonl`. A
/// failure to persist is logged but doesn't abort the in-memory run
/// — the in-flight result still carries forward to the next step's
/// template context. No-op when `run_store` is `None` or `run_id`
/// is empty (in-memory mode).
fn persist_step_result(opts: &OrchestratorRunOpts, run_id: &str, result: &StepResult) {
    let Some(store) = &opts.run_store else { return };
    if run_id.is_empty() {
        return;
    }
    let record = crate::runs::StepResultRecord::from(result);
    if let Err(e) = store.append_step_result(run_id, &record) {
        warn!(step = %result.step_id, error = %e, "failed to append step record");
    }
}

fn map_run_store_err(e: crate::runs::RunStoreError) -> RunWorkflowError {
    RunWorkflowError::Io(std::io::Error::other(format!("run-store: {e}")))
}

/// Build the read-only template context that a (linear) step or
/// fan-out unit sees: workflow inputs + event payload + every prior
/// step's published output (including per-unit `results[*]` and the
/// `sub_results.<sub_id>` map for `parallel:` steps).
fn base_context_for_step(
    inputs: &BTreeMap<String, String>,
    event: Option<&serde_json::Value>,
    issue: Option<&serde_json::Value>,
    prior: &[StepResult],
) -> StepContext {
    let mut ctx = StepContext::new();
    ctx.inputs = inputs.clone();
    ctx.event = event.cloned();
    ctx.issue = issue.cloned();
    for sr in prior {
        let results: Vec<String> = sr.items.iter().map(|i| i.output.clone()).collect();
        let sub_results: std::collections::BTreeMap<String, crate::templates::SubResult> = sr
            .items
            .iter()
            .filter(|i| !i.sub_id.is_empty())
            .map(|i| {
                (
                    i.sub_id.clone(),
                    crate::templates::SubResult {
                        output: i.output.clone(),
                        success: i.success,
                    },
                )
            })
            .collect();
        ctx.steps.insert(
            sr.step_id.clone(),
            StepOutput {
                output: sr.output.clone(),
                success: sr.success,
                skipped: sr.skipped,
                results,
                sub_results,
                findings: sr
                    .findings
                    .iter()
                    .map(|f| crate::templates::FindingView {
                        source: f.source.clone(),
                        severity: f.severity.as_str().to_string(),
                        title: f.title.clone(),
                        body: f.body.clone(),
                    })
                    .collect(),
                max_severity: sr
                    .findings
                    .iter()
                    .map(|f| f.severity)
                    .max()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                iterations: sr.iterations,
                resolved: sr.resolved,
                decision: gate_decision(sr),
            },
        );
    }
    ctx
}

/// Extract the `decision` field out of an approval-gate step's `output`
/// JSON (spec §3.1: `{"decision": "approved"|"rejected", ...}`) so
/// downstream templates can write `{{ steps.<id>.decision }}` instead of
/// hand-parsing the JSON string. Empty for non-gate steps, or if a gate's
/// `output` somehow fails to parse (defensive — the value is always
/// produced by `emit_gate_resolved`, never author-supplied).
fn gate_decision(sr: &StepResult) -> String {
    if sr.kind != crate::runs::StepKind::ApprovalGate {
        return String::new();
    }
    serde_json::from_str::<serde_json::Value>(&sr.output)
        .ok()
        .and_then(|v| v.get("decision").and_then(|d| d.as_str()).map(String::from))
        .unwrap_or_default()
}

fn step_kind_for_run_record(step: &Step) -> crate::runs::StepKind {
    if crate::workflow::is_approval_gate(step) {
        crate::runs::StepKind::ApprovalGate
    } else if step.branch.is_some() {
        crate::runs::StepKind::Branch
    } else if step.split.is_some() {
        crate::runs::StepKind::Split
    } else if step.join.is_some() {
        crate::runs::StepKind::Join
    } else if step.panel.is_some() {
        crate::runs::StepKind::Panel
    } else if step.parallel.is_some() {
        crate::runs::StepKind::Parallel
    } else if step.for_each.is_some() {
        crate::runs::StepKind::ForEach
    } else if step.action.is_some() {
        crate::runs::StepKind::Action
    } else {
        crate::runs::StepKind::Linear
    }
}

/// Deep-walk `with:` (a step's action arguments) and render every JSON
/// STRING leaf through the same template machinery `prompt:` uses
/// ([`render_step_prompt`]) — objects and arrays recurse; numbers, bools,
/// and null pass through unchanged. `None` (no `with:` block at all) becomes
/// an empty object, a valid call for any tool whose schema has no required
/// parameters.
fn render_action_args(
    with: Option<&serde_json::Value>,
    ctx: &StepContext,
    mode: RenderMode,
) -> Result<serde_json::Value, RenderError> {
    fn walk(
        value: &serde_json::Value,
        ctx: &StepContext,
        mode: RenderMode,
    ) -> Result<serde_json::Value, RenderError> {
        match value {
            serde_json::Value::String(s) => {
                Ok(serde_json::Value::String(render_step_prompt(s, ctx, mode)?))
            }
            serde_json::Value::Array(items) => Ok(serde_json::Value::Array(
                items
                    .iter()
                    .map(|item| walk(item, ctx, mode))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            serde_json::Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (key, val) in map {
                    out.insert(key.clone(), walk(val, ctx, mode)?);
                }
                Ok(serde_json::Value::Object(out))
            }
            // Numbers, bools, null: no template surface, pass through as-is.
            other => Ok(other.clone()),
        }
    }
    match with {
        None => Ok(serde_json::json!({})),
        Some(v) => walk(v, ctx, mode),
    }
}

/// Execute one `action:` step through the in-process MCP dispatcher: render
/// `with:` against `ctx`, call `dispatcher.call(tool, args)`, and package the
/// outcome into a `StepResult` (kind `Action`).
///
/// Shared by the main step loop and the `on_reject` cleanup mirror
/// (`run_reject_cleanup`) — and, per Plan 4's design, will also be the call
/// a gate's `notify:` hooks make. Deliberately carries **no** main-loop-only
/// state (no event sink, no run-store persistence, no pause token): every
/// caller owns emitting its own events and persisting the returned
/// `StepResult` — this function only does the render + dispatch + package.
///
/// `continue_on_error` mirrors the linear step's failure handling exactly:
/// a dispatcher error becomes `Ok(StepResult { success: false, .. })` when
/// `true`, or `Err(RunWorkflowError::Action)` when `false`. A `with:`
/// template-render failure always propagates as `Err` regardless of
/// `continue_on_error` — same as every other step shape's render failures,
/// which are treated as author/config errors, not runtime tool failures.
///
/// Writes exactly one audit-trail transcript line at a fresh path under
/// `transcript_dir` — an `Event::ActionEmitted` record, the same envelope
/// shape agent-run transcripts already carry for this event (see
/// `output/workflow_printer.rs` / `output/live_run.rs`, which render it
/// today with no production writer yet). `allowed` is `false` only for a
/// `McpError::PermissionDenied` (the call never reached the connector);
/// any other dispatch error still reached the connector, so `allowed:
/// true, applied: false`. The line is written once, after the dispatch
/// call resolves and before the `continue_on_error` branch, so both the
/// tolerated-failure and hard-abort paths get the same audit record. No
/// `run_start` preamble: `JsonlReader`'s `read_transcript_run_start` /
/// `JsonlReader::summary` both tolerate (rather than error on) a first
/// line that isn't `RunStart` — an action step has no
/// agent/provider/model to put in one anyway, so a bare single-line file
/// is the correct shape, not a gap.
async fn execute_action_step(
    dispatcher: &rupu_mcp::ToolDispatcher,
    step: &Step,
    ctx: &StepContext,
    mode: RenderMode,
    continue_on_error: bool,
    transcript_dir: &Path,
) -> Result<StepResult, RunWorkflowError> {
    let tool = step
        .action
        .as_deref()
        .expect("execute_action_step called for a non-action step");
    let args = render_action_args(step.with.as_ref(), ctx, mode).map_err(|e| {
        RunWorkflowError::Render {
            step: step.id.clone(),
            source: e,
        }
    })?;

    let transcript_path = transcript_dir.join(format!("run_{}.jsonl", Ulid::new()));
    let call_result = dispatcher.call(tool, args.clone()).await;

    let (allowed, applied, reason) = match &call_result {
        Ok(_) => (true, true, None),
        Err(rupu_mcp::McpError::PermissionDenied { reason, .. }) => {
            (false, false, Some(reason.clone()))
        }
        Err(e) => (true, false, Some(e.to_string())),
    };
    // `blocked` reuses `ToolDispatcher::is_blocked` (rupu-mcp) rather than
    // re-deriving it from `allowed` — the SAME classifier the agent-tool-call
    // audit path would use if action nodes had an agent grant to check.
    // An action node's tool is always explicit (it IS `step.action`), so
    // `declared`/`granted`/`restricted` are always `true` here — there is
    // no separate allowlist to narrow against (T1's `ActionsOnActionStep`
    // validation already forbids a non-empty `actions:` on an action step).
    let tool_audit_blocked = rupu_mcp::ToolDispatcher::is_blocked(&call_result);
    match JsonlWriter::create(&transcript_path) {
        Ok(mut writer) => {
            if let Err(e) = writer.write(&Event::ActionEmitted {
                kind: tool.to_string(),
                payload: args,
                allowed,
                applied,
                reason,
            }) {
                warn!(step = %step.id, error = %e, "failed to write action audit transcript line");
            } else if let Err(e) = writer.write(&Event::ToolAudit {
                tool: tool.to_string(),
                declared: true,
                granted: true,
                blocked: tool_audit_blocked,
                restricted: true,
            }) {
                warn!(step = %step.id, error = %e, "failed to write tool_audit transcript line");
            } else if let Err(e) = writer.flush() {
                warn!(step = %step.id, error = %e, "failed to flush action audit transcript");
            }
        }
        Err(e) => {
            warn!(step = %step.id, error = %e, "failed to create action audit transcript file");
        }
    }

    match call_result {
        Ok(output) => Ok(StepResult {
            step_id: step.id.clone(),
            output,
            success: true,
            skipped: false,
            kind: crate::runs::StepKind::Action,
            transcript_path,
            ..Default::default()
        }),
        Err(source) => {
            if continue_on_error {
                warn!(
                    step = %step.id,
                    error = %source,
                    "action step failed but continue_on_error is set; proceeding"
                );
                Ok(StepResult {
                    step_id: step.id.clone(),
                    output: String::new(),
                    success: false,
                    skipped: false,
                    kind: crate::runs::StepKind::Action,
                    transcript_path,
                    ..Default::default()
                })
            } else {
                Err(RunWorkflowError::Action {
                    step: step.id.clone(),
                    source,
                })
            }
        }
    }
}

/// Fire a gate's `notify:` hooks best-effort, right as the gate is about to
/// actually park (never on auto-approve, never on a resume-suppressed
/// gate — callers only reach here on the real pause path). Each hook is a
/// throwaway `action:`-shaped step run through the same
/// [`execute_action_step`] the main loop uses; every failure (missing
/// dispatcher, render error, dispatch error) is logged and swallowed —
/// notify never changes the park outcome, never blocks it, and never
/// surfaces as a run error.
async fn fire_notify_hooks(
    opts: &OrchestratorRunOpts,
    step_id: &str,
    notify: &[crate::workflow::NotifyAction],
    ctx: &StepContext,
    mode: RenderMode,
) {
    if notify.is_empty() {
        return;
    }
    let Some(dispatcher) = opts.action_dispatcher.as_ref() else {
        warn!(step = %step_id, "notify skipped: no action dispatcher");
        return;
    };
    for n in notify {
        let synth = Step {
            id: format!("{step_id}.notify"),
            agent: None,
            actions: Vec::new(),
            when: None,
            continue_on_error: None,
            for_each: None,
            parallel: None,
            max_parallel: None,
            prompt: None,
            approval: None,
            panel: None,
            branch: None,
            contract: None,
            distribute: None,
            host: None,
            workspace: None,
            next: Vec::new(),
            depends_on: Vec::new(),
            split: None,
            join: None,
            action: Some(n.action.clone()),
            with: Some(n.with.clone()),
        };
        if let Err(e) =
            execute_action_step(dispatcher, &synth, ctx, mode, true, &opts.transcript_dir).await
        {
            warn!(step = %step_id, action = %n.action, error = %e, "gate notify hook failed; continuing");
        }
    }
}

/// Record a resolved gate node's result: `StepStarted` + `StepCompleted`
/// events, a `StepResult` whose `output` is the decision JSON (spec §3.1),
/// persisted like any other step. `decision` is `"approved"` or
/// `"rejected"`; `via` is `"human"` (approve-resume / operator reject) or
/// `"auto"` (auto_approve truthy — always paired with `decision:
/// "approved"`); `reason` is the operator's rejection reason (`Some` only
/// for a rejected decision, `None` otherwise, matching spec §3.1's
/// `"reason": null` for approvals).
#[allow(clippy::too_many_arguments)]
fn emit_gate_result(
    opts: &OrchestratorRunOpts,
    run_id: &str,
    step: &Step,
    decision: &str,
    via: &str,
    reason: Option<&str>,
    step_results: &mut Vec<StepResult>,
    // Task 4 (spec §3): stamped onto this gate's persisted `StepResult`
    // when it resolved as a member INSIDE a loop iteration (`Some` only
    // from `run_scheduler_scoped`'s two in-loop call sites, via its own
    // `current_loop_iteration`). `None` for every other caller (the
    // legacy single-cursor loop, and `run_reject_cleanup`'s rejected-
    // gate record) — preserves the legacy absent-field shape exactly.
    loop_iteration: Option<u32>,
) {
    if let Some(sink) = opts.event_sink.as_ref() {
        sink.emit(
            run_id,
            &crate::executor::Event::StepStarted {
                run_id: run_id.to_string(),
                step_id: step.id.clone(),
                kind: crate::runs::StepKind::ApprovalGate,
                agent: None,
                host: None,
            },
        );
    }
    let output = serde_json::json!({
        "decision": decision,
        "via": via,
        "reason": reason,
        "decided_at": chrono::Utc::now().to_rfc3339(),
    })
    .to_string();
    let result = StepResult {
        step_id: step.id.clone(),
        output,
        success: true,
        skipped: false,
        kind: crate::runs::StepKind::ApprovalGate,
        loop_iteration,
        ..Default::default()
    };
    if let Some(sink) = opts.event_sink.as_ref() {
        sink.emit(
            run_id,
            &crate::executor::Event::StepCompleted {
                run_id: run_id.to_string(),
                step_id: step.id.clone(),
                success: true,
                duration_ms: 0,
                host: None,
            },
        );
    }
    persist_step_result(opts, run_id, &result);
    step_results.push(result);
}

/// Execute a rejected gate's `on_reject` chain (spec §4.1). Called by
/// the rejecting process AFTER `RunStore::reject` finalized the run.
/// Failures inside the chain are logged per-step (`continue`), never
/// returned — the run is already terminal.
///
/// `opts.resume_from` carries the run id + prior step results this
/// cleanup persists against — built via [`ResumeState::from_rejection`]
/// the same way approve-resume's `opts.resume_from` carries
/// [`ResumeState::from_approval`]'s. When `resume_from` is `None` (a
/// caller that didn't wire it), persistence + event emission silently
/// no-op (same as `run_workflow`'s in-memory mode) since there is no
/// run id to key off of.
///
/// `via` is the gate output's decision provenance (spec §3.1): `"human"`
/// for an operator-issued reject (`rupu workflow reject`), `"timeout"`
/// for a gate's own `on_timeout: reject` policy firing (whether observed
/// via `rupu workflow runs`'s lazy-expiry sweep or via an `approve` call
/// that lands on an already-overdue `on_timeout: reject` gate). Callers
/// must pass the value that matches how this rejection actually came
/// about — it is persisted verbatim into the gate's `StepResult` output.
pub async fn run_reject_cleanup(
    opts: OrchestratorRunOpts,
    rejected_step_id: &str,
    reason: &str,
    via: &str,
) -> Result<(), RunWorkflowError> {
    let Some(gate) = opts.workflow.steps.iter().find(|s| s.id == rejected_step_id) else {
        return Ok(()); // legacy inline approval or unknown id — nothing to run
    };
    if !crate::workflow::is_approval_gate(gate) {
        return Ok(());
    }
    let chain = gate
        .approval
        .as_ref()
        .map(|a| a.on_reject.clone())
        .unwrap_or_default();

    // 1. Prior results + run id come from `opts.resume_from` (the CLI
    //    reject path builds it with `ResumeState::from_rejection` after
    //    reading `step_results.jsonl` back — the same loader
    //    approve-resume uses; see `crates/rupu-cli/src/resume.rs`). This
    //    mirrors how `run_workflow` itself pulls `run_id` +
    //    `prior_step_results` off `opts.resume_from` at its own top
    //    (~line 519).
    let run_id = opts
        .resume_from
        .as_ref()
        .map(|r| r.run_id.clone())
        .unwrap_or_default();
    let mut step_results: Vec<StepResult> = opts
        .resume_from
        .as_ref()
        .map(|r| r.prior_step_results.clone())
        .unwrap_or_default();

    // 2. The gate's own rejected result, recorded BEFORE the chain runs
    //    (a `via`/`decision` variant of Task 3's now-generalized
    //    `emit_gate_result`).
    emit_gate_result(
        &opts,
        &run_id,
        gate,
        "rejected",
        via,
        Some(reason),
        &mut step_results,
        None,
    );

    // 3. Dispatch each on_reject step through the same per-step
    //    machinery `run_workflow`'s linear arm uses (StepStarted event →
    //    factory build_opts_for_step → agent run → StepCompleted /
    //    StepFailed event → persist_step_result → push). Mirrored inline
    //    rather than extracted into a shared helper: `run_linear_step`
    //    (~1726, now further down this file) also carries host-redirect,
    //    cooperative-pause, and resume-seed logic that a terminal cleanup
    //    chain never needs (cleanup steps always run local, uninterrupted,
    //    fresh — no host:, no opts.pause, no mid-step resume-seed), so
    //    extracting a shared helper would either drag that machinery along
    //    unused or require new plumbing on `run_linear_step` itself. The
    //    subset actually needed here is small enough to mirror directly.
    let resolved_inputs = resolve_inputs(&opts.workflow, &opts.inputs)?;
    for step in &chain {
        if step.action.is_some() {
            // Action steps dispatch through the same `execute_action_step`
            // helper the main loop uses (Plan 2). `continue_on_error:
            // false` here so a dispatcher failure comes back as `Err`
            // (matching the main loop's own default) — the `Err` arm
            // below is what actually implements this function's
            // never-abort contract: it logs, emits `StepFailed` with the
            // real error, records a failed `StepResult`, and the `for`
            // loop continues to the next cleanup step regardless. Passing
            // `true` here would have silently swallowed the error inside
            // `execute_action_step` itself (an `Ok(StepResult { success:
            // false })` with no error string anywhere), so the chain
            // still continues either way — this just makes sure the real
            // error reaches `StepFailed` instead of being discarded.
            let ctx = base_context_for_step(
                &resolved_inputs,
                opts.event.as_ref(),
                opts.issue.as_ref(),
                &step_results,
            );
            if let Some(sink) = opts.event_sink.as_ref() {
                sink.emit(
                    &run_id,
                    &crate::executor::Event::StepStarted {
                        run_id: run_id.clone(),
                        step_id: step.id.clone(),
                        kind: crate::runs::StepKind::Action,
                        agent: None,
                        host: None,
                    },
                );
            }
            let step_timer = std::time::Instant::now();
            let outcome = match opts.action_dispatcher.as_ref() {
                Some(dispatcher) => {
                    execute_action_step(
                        dispatcher,
                        step,
                        &ctx,
                        render_mode(opts.strict_templates),
                        false,
                        &opts.transcript_dir,
                    )
                    .await
                }
                None => Err(RunWorkflowError::ActionDispatcherMissing {
                    step: step.id.clone(),
                }),
            };
            let duration_ms = step_timer.elapsed().as_millis() as u64;
            let result = match outcome {
                Ok(result) => {
                    if let Some(sink) = opts.event_sink.as_ref() {
                        sink.emit(
                            &run_id,
                            &crate::executor::Event::StepCompleted {
                                run_id: run_id.clone(),
                                step_id: step.id.clone(),
                                success: result.success,
                                duration_ms,
                                host: None,
                            },
                        );
                    }
                    result
                }
                Err(e) => {
                    warn!(
                        step = %step.id,
                        error = %e,
                        "on_reject cleanup action step failed; continuing chain"
                    );
                    if let Some(sink) = opts.event_sink.as_ref() {
                        sink.emit(
                            &run_id,
                            &crate::executor::Event::StepFailed {
                                run_id: run_id.clone(),
                                step_id: step.id.clone(),
                                error: e.to_string(),
                            },
                        );
                    }
                    StepResult {
                        step_id: step.id.clone(),
                        success: false,
                        skipped: false,
                        kind: crate::runs::StepKind::Action,
                        ..Default::default()
                    }
                }
            };
            persist_step_result(&opts, &run_id, &result);
            step_results.push(result);
            continue;
        }

        let ctx = base_context_for_step(
            &resolved_inputs,
            opts.event.as_ref(),
            opts.issue.as_ref(),
            &step_results,
        );
        let agent_name = step.agent.as_deref().unwrap_or_default();
        let prompt = step.prompt.as_deref().unwrap_or_default();
        let step_run_id = format!("run_{}", Ulid::new());
        let transcript_path = opts.transcript_dir.join(format!("{step_run_id}.jsonl"));

        if let Some(sink) = opts.event_sink.as_ref() {
            sink.emit(
                &run_id,
                &crate::executor::Event::StepStarted {
                    run_id: run_id.clone(),
                    step_id: step.id.clone(),
                    kind: crate::runs::StepKind::Linear,
                    agent: step.agent.clone(),
                    host: step.host.clone(),
                },
            );
        }

        let rendered = match render_step_prompt(prompt, &ctx, render_mode(opts.strict_templates))
        {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    step = %step.id,
                    error = %e,
                    "on_reject cleanup step: prompt render failed; continuing chain"
                );
                if let Some(sink) = opts.event_sink.as_ref() {
                    sink.emit(
                        &run_id,
                        &crate::executor::Event::StepFailed {
                            run_id: run_id.clone(),
                            step_id: step.id.clone(),
                            error: e.to_string(),
                        },
                    );
                }
                let result = StepResult {
                    step_id: step.id.clone(),
                    success: false,
                    skipped: false,
                    kind: crate::runs::StepKind::Linear,
                    ..Default::default()
                };
                persist_step_result(&opts, &run_id, &result);
                step_results.push(result);
                continue;
            }
        };

        let step_timer = std::time::Instant::now();
        let outcome = dispatch_one(
            &opts.factory,
            &step.id,
            agent_name,
            rendered.clone(),
            step_run_id.clone(),
            opts.workspace_id.clone(),
            opts.workspace_path.clone(),
            transcript_path.clone(),
            None,
            None,
            None,
        )
        .await;
        let duration_ms = step_timer.elapsed().as_millis() as u64;

        let (success, error_text) = match outcome {
            Ok(_) => (true, String::new()),
            Err(e) => {
                warn!(
                    step = %step.id,
                    error = %e,
                    "on_reject cleanup step failed; continuing chain"
                );
                (false, e.to_string())
            }
        };
        if let Some(sink) = opts.event_sink.as_ref() {
            if success {
                sink.emit(
                    &run_id,
                    &crate::executor::Event::StepCompleted {
                        run_id: run_id.clone(),
                        step_id: step.id.clone(),
                        success: true,
                        duration_ms,
                        host: None,
                    },
                );
            } else {
                sink.emit(
                    &run_id,
                    &crate::executor::Event::StepFailed {
                        run_id: run_id.clone(),
                        step_id: step.id.clone(),
                        error: error_text,
                    },
                );
            }
        }
        let output = read_final_assistant_text(&transcript_path, success, &step_run_id, &step.id);
        let result = StepResult {
            step_id: step.id.clone(),
            rendered_prompt: rendered,
            run_id: step_run_id,
            transcript_path,
            output,
            success,
            skipped: false,
            kind: crate::runs::StepKind::Linear,
            ..Default::default()
        };
        persist_step_result(&opts, &run_id, &result);
        step_results.push(result);
    }

    // 4. Cleanup never changes the terminal status itself — that was
    //    already decided by whichever `reject_gate`/`expire_gate_if_overdue`
    //    call preceded this cleanup. Pre-5b-2a that was ALWAYS `Rejected`
    //    (the sole gate, emptying the set). Task 5b-2a (spec §7): several
    //    concurrent gates can each carry their own `on_reject` chain —
    //    rejecting one gate of a still-parked set only removes it, and
    //    the run stays `AwaitingApproval` until every gate resolves. This
    //    cleanup call is scoped to ONE gate's chain regardless of which
    //    case applies; step 5 below is what has to tell the difference.

    // 5. `RunStore::reject_gate`/`expire_gate_if_overdue` already appended
    //    a terminal `RunCompleted` event IF rejecting this gate emptied
    //    the awaiting set (step 1 doc comment above), but
    //    `emit_gate_result` (step 2 above) unconditionally emits the
    //    gate's own `StepStarted`/`StepCompleted` events after that, and
    //    every cleanup step dispatched in the loop above appends its own
    //    `StepStarted`/`StepCompleted`/`StepFailed` events on top of
    //    those — so `events.jsonl` ends with step events, not a terminal
    //    one, even when the chain is empty. Newest-event-fold consumers
    //    (Situation Room's live event stream) fold the log by treating
    //    its last event as the run's current state; with a step event
    //    trailing, a rejected run would briefly render as "active" until
    //    a fresh terminal event lands. Re-append the same
    //    `RunCompleted(Rejected)` event here — after the chain — so the
    //    log ends closed. This is a deliberate duplicate: it is NOT
    //    deduped downstream, it is simply an accepted trailing marker
    //    that keeps the log's last line authoritative.
    //
    //    Task 5b-2a: this must NOT fire while the run is still
    //    `AwaitingApproval` — a sibling gate rejected earlier in the same
    //    set must not falsely close the event log for a run that's still
    //    genuinely active on another path. Reload the persisted record
    //    and gate the re-append on it actually being terminal. Skipped
    //    entirely when `opts.run_store` is `None` (in-memory runs have no
    //    `events.jsonl` to close) or the record can't be reloaded (same
    //    best-effort contract `append_terminal_event` itself already has).
    if let Some(store) = &opts.run_store {
        let still_awaiting = store
            .load(&run_id)
            .map(|r| r.status == crate::runs::RunStatus::AwaitingApproval)
            .unwrap_or(false);
        if !still_awaiting {
            store.append_terminal_event(
                &run_id,
                &crate::executor::Event::RunCompleted {
                    run_id: run_id.clone(),
                    status: crate::runs::RunStatus::Rejected,
                    finished_at: chrono::Utc::now(),
                },
            );
        }
    }

    Ok(())
}

fn persist_active_step(
    opts: &OrchestratorRunOpts,
    workflow_run_id: &str,
    step: &Step,
    transcript_path: Option<PathBuf>,
) {
    let Some(store) = &opts.run_store else { return };
    if workflow_run_id.is_empty() {
        return;
    }
    let Ok(mut record) = store.load(workflow_run_id) else {
        return;
    };
    record.active_step_id = Some(step.id.clone());
    record.active_step_kind = Some(step_kind_for_run_record(step));
    record.active_step_agent = step.agent.clone();
    record.active_step_transcript_path = transcript_path;
    if let Err(e) = store.update(&record) {
        warn!(step = %step.id, error = %e, "failed to persist active step");
    }
}

fn clear_active_step(opts: &OrchestratorRunOpts, workflow_run_id: &str, step_id: &str) {
    let Some(store) = &opts.run_store else { return };
    if workflow_run_id.is_empty() {
        return;
    }
    let Ok(mut record) = store.load(workflow_run_id) else {
        return;
    };
    if record.active_step_id.as_deref() != Some(step_id) {
        return;
    }
    record.active_step_id = None;
    record.active_step_kind = None;
    record.active_step_agent = None;
    record.active_step_transcript_path = None;
    if let Err(e) = store.update(&record) {
        warn!(step = %step_id, error = %e, "failed to clear active step");
    }
}

/// Run a host-placed linear step as a single remote unit through the
/// [`UnitDispatcher`] port (index 0). Mirrors the fan-out remote path:
/// `Ok(success:true)` → that output; `Ok(success:false)` or `Err` → a
/// failed step honoring `continue_on_error`. There is **no reassignment**
/// — a single named host has no alternate. Absence of a dispatcher is a
/// hard configuration error (coordinator without fleet access), surfaced
/// clearly with no silent local fallback.
#[allow(clippy::too_many_arguments)]
async fn dispatch_placed_step(
    host: &str,
    step: &Step,
    agent_name: &str,
    rendered: &str,
    run_id: &str,
    opts: &OrchestratorRunOpts,
    continue_on_error: bool,
    sync: bool,
) -> Result<(String, bool), RunWorkflowError> {
    let Some(dispatcher) = opts.unit_dispatcher.as_ref() else {
        let source =
            RunError::Provider("host placement requires fleet access — run via the CP".into());
        let output = source.to_string();
        return placed_failure(step, host, output, source, continue_on_error);
    };
    // When sync mode is active, pass the coordinator workspace to the unit so
    // the remote side can mount / apply it. None ⇒ self-contained (unchanged).
    let workspace_path_opt = sync.then(|| opts.workspace_path.clone());
    let unit = UnitDispatch {
        step_id: step.id.clone(),
        agent: agent_name.to_string(),
        rendered_prompt: rendered.to_string(),
        index: 0,
        run_id: run_id.to_string(),
        workspace_path: workspace_path_opt.clone(),
    };
    match dispatcher.dispatch_unit(unit, host).await {
        Ok(outcome) if outcome.success => {
            let output = outcome.output;
            let ws_delta = outcome.workspace_delta;
            // Apply the unit's workspace delta back to the coordinator before
            // the step is considered complete. Guard on both sync mode (workspace_path_opt
            // is Some) and a dispatcher being present (always true here, but keeps
            // the guard symmetric with the fan-out path).
            if let Some(delta) = ws_delta {
                if let (Some(disp), Some(ws)) =
                    (opts.unit_dispatcher.as_ref(), workspace_path_opt.as_ref())
                {
                    if let Err(conflict) = disp.apply_workspace_deltas(ws, &[delta]).await {
                        let src = RunError::Provider(conflict.to_string());
                        return placed_failure(
                            step,
                            host,
                            conflict.to_string(),
                            src,
                            continue_on_error,
                        );
                    }
                }
            }
            Ok((output, true))
        }
        Ok(outcome) => {
            // Agent ran but reported failure: preserve its output, but
            // synthesize a raw error so the abort below fires — symmetric
            // with the fan-out remote path.
            let source = RunError::Provider(
                outcome
                    .error
                    .clone()
                    .unwrap_or_else(|| "remote step failed".into()),
            );
            placed_failure(step, host, outcome.output, source, continue_on_error)
        }
        Err(source) => {
            let output = source.to_string();
            placed_failure(step, host, output, source, continue_on_error)
        }
    }
}

/// Apply `continue_on_error` to a failed placement: tolerate (record a
/// failed `(output, false)`) or abort with the same `RunWorkflowError::Agent`
/// a local step failure produces.
fn placed_failure(
    step: &Step,
    host: &str,
    output: String,
    source: RunError,
    continue_on_error: bool,
) -> Result<(String, bool), RunWorkflowError> {
    if continue_on_error {
        warn!(
            step = %step.id,
            host = %host,
            error = %source,
            "placed step failed but continue_on_error is set; proceeding"
        );
        Ok((output, false))
    } else {
        Err(RunWorkflowError::Agent {
            step: step.id.clone(),
            source,
        })
    }
}

/// Single-shot linear step: render the prompt, build agent opts via
/// the factory, run the agent, capture final assistant text, return
/// a `StepResult`.
async fn run_linear_step(
    workflow_run_id: &str,
    step: &Step,
    ctx: &StepContext,
    opts: &OrchestratorRunOpts,
    continue_on_error: bool,
) -> Result<LinearStepOutcome, RunWorkflowError> {
    let prompt = step
        .prompt
        .as_deref()
        .expect("validate_step_shape guarantees prompt for linear steps");
    let agent_name = step
        .agent
        .as_deref()
        .expect("validate_step_shape guarantees agent for linear steps");
    let rendered =
        render_step_prompt(prompt, ctx, render_mode(opts.strict_templates)).map_err(|e| {
            RunWorkflowError::Render {
                step: step.id.clone(),
                source: e,
            }
        })?;
    let run_id = format!("run_{}", Ulid::new());
    let transcript_path = opts.transcript_dir.join(format!("{run_id}.jsonl"));
    persist_active_step(opts, workflow_run_id, step, Some(transcript_path.clone()));
    // Announce the running step's transcript path on the live event stream.
    // A linear step generates this path lazily (after the outer-loop
    // `StepStarted`), so the UI has no way to learn it until the step
    // completes and a `step_result` is persisted — surface it now so the
    // run view can select and tail the file in real time.
    if let Some(sink) = opts.event_sink.as_ref() {
        sink.emit(
            workflow_run_id,
            &crate::executor::Event::StepWorking {
                run_id: workflow_run_id.to_string(),
                step_id: step.id.clone(),
                note: None,
                transcript_path: Some(transcript_path.clone()),
            },
        );
    }

    let (output, success) = match step.host.as_deref() {
        Some(host) => {
            let sync =
                effective_workspace_mode(step, &opts.workflow.defaults) == WorkspaceMode::Sync;
            dispatch_placed_step(
                host,
                step,
                agent_name,
                &rendered,
                &run_id,
                opts,
                continue_on_error,
                sync,
            )
            .await?
        }
        None => {
            // --- Existing local (inline) path — UNCHANGED ---
            let on_tool_call: Option<rupu_agent::OnToolCallCallback> =
                opts.event_sink.as_ref().map(|sink| {
                    let sink = sink.clone();
                    let wf_run_id = workflow_run_id.to_string();
                    let step_id = step.id.clone();
                    std::sync::Arc::new(move |_caller_step_id: &str, tool_name: &str, blocked: bool| {
                        let note = if blocked {
                            format!("{tool_name} — blocked")
                        } else {
                            tool_name.to_string()
                        };
                        sink.emit(
                            &wf_run_id,
                            &crate::executor::Event::StepWorking {
                                run_id: wf_run_id.clone(),
                                step_id: step_id.clone(),
                                note: Some(note),
                                transcript_path: None,
                            },
                        );
                    }) as rupu_agent::OnToolCallCallback
                });

            // Resume-seed: if this exact step paused mid-run in a prior
            // process, re-seed the agent from its persisted transcript
            // (role-alternation-safe — see `split_seed_for_resume`).
            let resume_seed = opts
                .resume_from
                .as_ref()
                .and_then(|r| r.paused_step.as_ref())
                .filter(|ps| ps.step_id == step.id)
                .map(|ps| split_seed_for_resume(ps.seed_messages.clone()));

            let outcome = dispatch_one(
                &opts.factory,
                &step.id,
                agent_name,
                rendered.clone(),
                run_id.clone(),
                opts.workspace_id.clone(),
                opts.workspace_path.clone(),
                transcript_path.clone(),
                on_tool_call,
                opts.pause.clone(),
                resume_seed,
            )
            .await;

            let success = match outcome {
                // NOTE 2: branch on the paused outcome BEFORE the Ok/Err
                // success check. A paused agent run is neither success nor
                // failure — it unwinds into a manual-pause checkpoint.
                Ok(rr) if rr.paused => {
                    return Ok(LinearStepOutcome::Paused {
                        step_id: step.id.clone(),
                        seed: rr.final_messages,
                    });
                }
                Ok(_) => true,
                Err(source) => {
                    if continue_on_error {
                        warn!(
                            step = %step.id,
                            error = %source,
                            "step failed but continue_on_error is set; proceeding"
                        );
                        false
                    } else {
                        return Err(RunWorkflowError::Agent {
                            step: step.id.clone(),
                            source,
                        });
                    }
                }
            };

            let output = read_final_assistant_text(&transcript_path, success, &run_id, &step.id);
            (output, success)
        }
    };

    Ok(LinearStepOutcome::Completed(StepResult {
        step_id: step.id.clone(),
        rendered_prompt: rendered,
        run_id,
        transcript_path,
        output,
        success,
        skipped: false,
        items: Vec::new(),
        ..Default::default()
    }))
}

/// Fan-out step: render `for_each:` to a list, then dispatch the
/// step's agent + prompt template per item. Items run with up to
/// `max_parallel` concurrency (default 1). Per-item failures honor
/// `continue_on_error`: when set, failed items are recorded with
/// `success=false` and the rest still run; otherwise the first
/// failed item aborts the workflow.
async fn run_fanout_step(
    workflow_run_id: &str,
    step: &Step,
    ctx: &StepContext,
    opts: &OrchestratorRunOpts,
    continue_on_error: bool,
) -> Result<FanoutStepOutcome, RunWorkflowError> {
    let for_each_expr = step
        .for_each
        .as_ref()
        .expect("run_fanout_step called for a non-fan-out step");
    let rendered_list = render_step_prompt(for_each_expr, ctx, render_mode(opts.strict_templates))
        .map_err(|e| RunWorkflowError::Render {
            step: step.id.clone(),
            source: e,
        })?;
    let items = parse_fanout_items(&rendered_list);

    if items.is_empty() {
        info!(step = %step.id, "for_each rendered to an empty list; recording as success with no items");
        return Ok(FanoutStepOutcome::Completed(StepResult {
            step_id: step.id.clone(),
            rendered_prompt: String::new(),
            run_id: String::new(),
            transcript_path: PathBuf::new(),
            output: "[]".into(),
            success: true,
            skipped: false,
            kind: crate::runs::StepKind::ForEach,
            items: Vec::new(),
            ..Default::default()
        }));
    }

    let max_parallel = step.max_parallel.unwrap_or(1).max(1) as usize;
    let semaphore = Arc::new(Semaphore::new(max_parallel));
    let total = items.len();
    // Effective workspace mode for this step — if Sync, units receive the
    // coordinator workspace path and return deltas that are applied once
    // after all units finish.
    let sync = effective_workspace_mode(step, &opts.workflow.defaults) == WorkspaceMode::Sync;

    // Resume: units that already SUCCEEDED in a prior run are replayed
    // from disk rather than re-dispatched. `completed_units[step.id]`
    // is keyed by the unit's 0-based index in the rendered list. The
    // list is deterministic on resume, so the index is a stable key —
    // but if the rendered list length differs from what was
    // checkpointed (the underlying for_each source changed), we can't
    // trust the index mapping, so we fall back to re-running every unit.
    let mut resumed: std::collections::BTreeMap<usize, ItemResult> =
        std::collections::BTreeMap::new();
    if let Some(prior) = opts
        .resume_from
        .as_ref()
        .and_then(|r| r.completed_units.get(&step.id))
    {
        let checkpointed_len = prior.keys().copied().max().map(|m| m + 1).unwrap_or(0);
        if checkpointed_len > total {
            warn!(
                step = %step.id,
                checkpointed = checkpointed_len,
                rendered = total,
                "resume: checkpointed fan-out length exceeds rendered list; re-running all units"
            );
        } else {
            for (idx, item_result) in prior {
                if *idx >= total {
                    continue;
                }
                if item_result.success {
                    resumed.insert(*idx, item_result.clone());
                }
            }
            if !resumed.is_empty() {
                info!(
                    step = %step.id,
                    replayed = resumed.len(),
                    total,
                    "resume: replaying succeeded fan-out units from disk"
                );
            }
        }
    }

    // Render each item's prompt up front so a per-item template
    // error is reported before any agent dispatches. Each item gets
    // its own clone of the parent context with `item` + `loop` bound.
    // Units already replayed from a prior run's checkpoint are skipped.
    let mut prepared: Vec<(usize, serde_json::Value, String, String, PathBuf)> =
        Vec::with_capacity(total);
    for (idx, item) in items.iter().enumerate() {
        if resumed.contains_key(&idx) {
            continue;
        }
        let item_ctx = ctx.clone().with_item(
            item.clone(),
            LoopInfo {
                index: idx + 1,
                index0: idx,
                length: total,
                first: idx == 0,
                last: idx + 1 == total,
            },
        );
        let item_prompt = step
            .prompt
            .as_deref()
            .expect("validate_step_shape guarantees prompt for for_each steps");
        let rendered =
            render_step_prompt(item_prompt, &item_ctx, render_mode(opts.strict_templates))
                .map_err(|e| RunWorkflowError::Render {
                    step: format!("{}[{}]", step.id, idx),
                    source: e,
                })?;
        let run_id = format!("run_{}", Ulid::new());
        let transcript_path = opts.transcript_dir.join(format!("{run_id}.jsonl"));
        prepared.push((idx, item.clone(), rendered, run_id, transcript_path));
    }

    // Spawn each item with the concurrency cap. We want declared
    // ordering of results regardless of finish order, so we collect
    // (idx, ItemResult) and sort by idx at the end.
    let agent_name_root = step
        .agent
        .as_deref()
        .expect("validate_step_shape guarantees agent for for_each steps")
        .to_string();
    // Pre-extract distribute hosts (if any) so we can compute per-unit
    // placement and fallback host before entering each spawned task.
    let distribute_hosts: Option<Vec<String>> = step.distribute.as_ref().map(|d| d.hosts.clone());
    // Clone the dispatcher Arc once; each spawned task gets its own ref.
    let unit_dispatcher = opts.unit_dispatcher.clone();
    // Cooperative pause token, cloned once; each spawned task gets its own
    // handle. Threaded into the LOCAL agent dispatch below so an in-flight
    // unit honors it mid-turn (same mechanism as a linear step's agent —
    // see T2). Checked again after each unit acquires its semaphore permit
    // so a unit that hasn't started yet is never dispatched (local OR
    // remote) once a pause has landed.
    let unit_pause = opts.pause.clone();
    let mut handles = Vec::with_capacity(total);
    for (idx, item_value, rendered, run_id, transcript_path) in prepared {
        // Compute host placement for this unit. `None` → local inline path
        // (unchanged). `Some(host)` → remote dispatch via `UnitDispatcher`.
        let placement: Option<String> = distribute_hosts
            .as_ref()
            .map(|hosts| hosts[idx % hosts.len()].clone());
        // Fallback host for the single retry on primary-host failure.
        // Computed eagerly outside the task to avoid capturing the full
        // hosts list in every closure.
        let fallback_host: Option<String> = distribute_hosts
            .as_ref()
            .map(|hosts| hosts[(idx + 1) % hosts.len()].clone());
        let permit_sem = semaphore.clone();
        let factory = Arc::clone(&opts.factory);
        let step_id = step.id.clone();
        let agent_name = agent_name_root.clone();
        let workspace_id = opts.workspace_id.clone();
        let workspace_path = opts.workspace_path.clone();
        let rendered_clone = rendered.clone();
        let run_id_clone = run_id.clone();
        let transcript_clone = transcript_path.clone();
        // Per-unit live-view events. Cloned into the task so emission
        // ordering reflects the unit's REAL start/finish under
        // `max_parallel` concurrency (the started/completed pair brackets
        // the dispatch inside the spawned future, after the semaphore
        // permit is held).
        let event_sink = opts.event_sink.clone();
        let workflow_run_id = workflow_run_id.to_string();
        let unit_key = fanout_unit_key(&item_value);
        let unit_agent = agent_name_root.clone();
        let dispatcher_for_task = unit_dispatcher.clone();
        let pause_for_task = unit_pause.clone();
        // Workspace path forwarded to the remote unit when sync mode is active.
        // None ⇒ self-contained; Some ⇒ unit mounts this path and returns a delta.
        let unit_workspace_path = sync.then(|| opts.workspace_path.clone());

        handles.push(tokio::spawn(async move {
            // Held for the duration of this item's run; dropping it
            // releases a slot back to the pool.
            let _permit = permit_sem
                .acquire_owned()
                .await
                .expect("semaphore not closed");
            // Save placement before the `if let Some(host) = placement`
            // branch consumes it, so both events and FanoutItemOutcome
            // carry the same host attribution.
            let placement_host = placement.clone();

            // Cooperative pause, checked the instant this unit's semaphore
            // permit is granted — i.e. BEFORE any work (local or remote) is
            // dispatched. A unit that hasn't started is never a "safe
            // boundary" problem: skip it outright so `run_fanout_step` can
            // report it as not-yet-started and the next resume redispatches
            // it fresh. Units already past this check when the pause lands
            // keep running to their own safe boundary (a local unit's agent
            // loop honors the SAME token mid-turn; a remote unit — no
            // cancellation channel exists over the wire — runs to
            // completion, which is its safe boundary).
            if pause_triggered(&pause_for_task) {
                return FanoutItemOutcome {
                    idx,
                    item: item_value,
                    rendered_prompt: rendered,
                    run_id,
                    transcript_path,
                    output: String::new(),
                    success: false,
                    error: None,
                    raw_error: None,
                    host: placement_host,
                    workspace_delta: None,
                    paused: true,
                };
            }

            if let Some(sink) = event_sink.as_ref() {
                sink.emit(
                    &workflow_run_id,
                    &crate::executor::Event::UnitStarted {
                        run_id: workflow_run_id.clone(),
                        step_id: step_id.clone(),
                        index: idx,
                        unit_key: unit_key.clone(),
                        agent: Some(unit_agent.clone()),
                        transcript_path: transcript_clone.clone(),
                        host: placement_host.clone(),
                    },
                );
            }

            // Branch: remote (placed) vs local (inline) path.
            let (output, success, error_str, raw_error, workspace_delta, paused) =
                if let Some(host) = placement {
                    // --- Remote dispatch path ---
                    //
                    // `distribute:` requires a `UnitDispatcher`. Its absence is a
                    // configuration error — the caller must supply one when running
                    // a workflow with `distribute:`.
                    match dispatcher_for_task {
                        None => {
                            let err = RunError::Provider(
                                "distribute requires fleet access — run via the CP".into(),
                            );
                            let msg = err.to_string();
                            // Minor 3: reuse `msg` instead of duplicating the literal.
                            (msg.clone(), false, Some(msg), Some(err), None, false)
                        }
                        Some(dispatcher) => {
                            let unit = UnitDispatch {
                                step_id: step_id.clone(),
                                agent: agent_name.clone(),
                                rendered_prompt: rendered_clone.clone(),
                                index: idx,
                                run_id: run_id_clone.clone(),
                                workspace_path: unit_workspace_path.clone(),
                            };
                            match dispatcher.dispatch_unit(unit, &host).await {
                                Ok(outcome) => {
                                    // Important fix: when the agent ran but failed
                                    // (success=false), synthesize a raw_error so
                                    // the continue_on_error:false abort below fires
                                    // — symmetric with the local Err path.
                                    let err_str = outcome.error.clone();
                                    let raw = if !outcome.success {
                                        Some(RunError::Provider(
                                            outcome
                                                .error
                                                .clone()
                                                .unwrap_or_else(|| "remote unit failed".into()),
                                        ))
                                    } else {
                                        None
                                    };
                                    let ws_delta = outcome.workspace_delta;
                                    (
                                        outcome.output,
                                        outcome.success,
                                        err_str,
                                        raw,
                                        ws_delta,
                                        false,
                                    )
                                }
                                Err(first_err) => {
                                    // Reassign once to the next host and retry.
                                    let retry_host = fallback_host.as_deref().unwrap_or(&host);
                                    let retry_unit = UnitDispatch {
                                        step_id: step_id.clone(),
                                        agent: agent_name.clone(),
                                        rendered_prompt: rendered_clone.clone(),
                                        index: idx,
                                        run_id: run_id_clone.clone(),
                                        workspace_path: unit_workspace_path.clone(),
                                    };
                                    warn!(
                                        step = %step_id,
                                        index = idx,
                                        host = %host,
                                        retry = %retry_host,
                                        error = %first_err,
                                        "unit dispatch failed; retrying on next host"
                                    );
                                    match dispatcher.dispatch_unit(retry_unit, retry_host).await {
                                        Ok(outcome) => {
                                            // Same fix as primary path: synthesize
                                            // raw_error for a failed-but-Ok outcome.
                                            let err_str = outcome.error.clone();
                                            let raw = if !outcome.success {
                                                Some(RunError::Provider(
                                                    outcome.error.clone().unwrap_or_else(|| {
                                                        "remote unit failed".into()
                                                    }),
                                                ))
                                            } else {
                                                None
                                            };
                                            let ws_delta = outcome.workspace_delta;
                                            (
                                                outcome.output,
                                                outcome.success,
                                                err_str,
                                                raw,
                                                ws_delta,
                                                false,
                                            )
                                        }
                                        Err(second_err) => {
                                            let msg = second_err.to_string();
                                            (
                                                msg.clone(),
                                                false,
                                                Some(msg),
                                                Some(second_err),
                                                None,
                                                false,
                                            )
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // --- Local (inline) path ---
                    //
                    // The pause token IS threaded through here (unlike the
                    // pre-T6 shape): a local unit's agent loop honors it
                    // cooperatively mid-turn (same primitive a linear step's
                    // agent uses — see T2), so an in-flight local unit
                    // genuinely pauses instead of running to completion.
                    let outcome = dispatch_one(
                        &factory,
                        &step_id,
                        &agent_name,
                        rendered_clone.clone(),
                        run_id_clone.clone(),
                        workspace_id,
                        workspace_path,
                        transcript_clone.clone(),
                        None,
                        pause_for_task.clone(),
                        None,
                    )
                    .await;
                    match outcome {
                        // A cooperative pause landed mid-turn. Not a success,
                        // not a failure — the unit is incomplete and must be
                        // re-dispatched (fresh) on resume.
                        Ok(rr) if rr.paused => (String::new(), false, None, None, None, true),
                        Ok(_) => {
                            let out = read_final_assistant_text(
                                &transcript_clone,
                                true,
                                &run_id_clone,
                                &step_id,
                            );
                            (out, true, None, None, None, false)
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            let out = read_final_assistant_text(
                                &transcript_clone,
                                false,
                                &run_id_clone,
                                &step_id,
                            );
                            (out, false, Some(msg), Some(e), None, false)
                        }
                    }
                };

            if !paused {
                if let Some(sink) = event_sink.as_ref() {
                    // Tokens are not available from the dispatch result
                    // (`dispatch_one` returns `Result<()>`); emit 0 — the live
                    // view still tails the unit transcript for token deltas.
                    sink.emit(
                        &workflow_run_id,
                        &crate::executor::Event::UnitCompleted {
                            run_id: workflow_run_id.clone(),
                            step_id: step_id.clone(),
                            index: idx,
                            unit_key: unit_key.clone(),
                            success,
                            tokens_in: 0,
                            tokens_out: 0,
                            host: placement_host.clone(),
                        },
                    );
                }
            }
            FanoutItemOutcome {
                idx,
                item: item_value,
                rendered_prompt: rendered,
                run_id,
                transcript_path,
                output,
                success,
                error: error_str,
                raw_error,
                host: placement_host,
                workspace_delta,
                paused,
            }
        }));
    }

    let mut item_outcomes: Vec<FanoutItemOutcome> = Vec::with_capacity(total);
    for handle in handles {
        match handle.await {
            Ok(o) => item_outcomes.push(o),
            Err(join_err) => {
                // Task panic or cancellation. Surface as a typed
                // workflow error regardless of continue_on_error —
                // a panicked task means we don't have an agent
                // RunError to report, so the orchestrator-level
                // tolerance flag doesn't apply.
                return Err(RunWorkflowError::FanoutJoin {
                    step: step.id.clone(),
                    source: join_err,
                });
            }
        }
    }
    item_outcomes.sort_by_key(|o| o.idx);

    // Persist every freshly-dispatched, NON-PAUSED unit's checkpoint as soon
    // as the fan-out's tasks have joined — BEFORE the `continue_on_error`
    // abort check below, so a crash/early-return mid-fan-out still
    // leaves the finished units (success AND failure) durable on disk
    // for `rupu workflow resume`. Replayed (`resumed`) units are
    // already on disk from the prior run, so we don't re-append them. A
    // paused unit (skipped outright, or cancelled mid-turn) did NOT
    // finish — it gets no checkpoint entry, so a subsequent `read_unit_
    // checkpoints` naturally omits it and the next resume redispatches it
    // fresh (same "absent = incomplete" contract the existing failed-unit
    // path already relies on). `workflow_run_id` is empty in the in-memory
    // (no run-store) mode.
    if let Some(store) = &opts.run_store {
        if !workflow_run_id.is_empty() {
            for o in item_outcomes.iter().filter(|o| !o.paused) {
                let checkpoint = crate::runs::UnitCheckpoint {
                    step_id: step.id.clone(),
                    index: o.idx,
                    item: o.item.clone(),
                    run_id: o.run_id.clone(),
                    transcript_path: o.transcript_path.clone(),
                    output: o.output.clone(),
                    success: o.success,
                    finished_at: chrono::Utc::now(),
                    host: o.host.clone(),
                };
                if let Err(e) = store.append_unit_checkpoint(workflow_run_id, &checkpoint) {
                    warn!(step = %step.id, index = o.idx, error = %e, "failed to append unit checkpoint");
                }
            }
        }
    }

    // Apply `continue_on_error`: if not set, the first REAL failure (not a
    // paused unit — that's incomplete, not failed) aborts the workflow. We
    // surface the original RunError.
    if !continue_on_error {
        if let Some(failed) = item_outcomes.iter_mut().find(|o| !o.success && !o.paused) {
            if let Some(err) = failed.raw_error.take() {
                return Err(RunWorkflowError::Agent {
                    step: format!("{}[{}]", step.id, failed.idx),
                    source: err,
                });
            }
        }
    }

    // Cooperative pause landed mid-fan-out: at least one unit was skipped
    // outright (not yet started) or paused mid-turn. Stop here — don't
    // aggregate a step result, don't apply sync workspace deltas (a
    // `workspace: sync` step pausing mid-flight would otherwise silently
    // drop the in-flight/not-yet-dispatched units' deltas, so it's refused
    // instead, consistent with the existing step-boundary/checkpoint-resume
    // guards for sync workflows). Report every unit that DID succeed this
    // pass, merged with anything already replayed from an earlier resume,
    // so the caller redispatches ONLY the paused / not-yet-started units.
    if item_outcomes.iter().any(|o| o.paused) {
        if sync {
            return Err(RunWorkflowError::PauseWithWorkspaceSync);
        }
        let mut completed_units = resumed;
        for o in &item_outcomes {
            if o.paused || !o.success {
                continue;
            }
            completed_units.insert(
                o.idx,
                ItemResult {
                    index: o.idx,
                    item: o.item.clone(),
                    sub_id: String::new(),
                    rendered_prompt: o.rendered_prompt.clone(),
                    run_id: o.run_id.clone(),
                    transcript_path: o.transcript_path.clone(),
                    output: o.output.clone(),
                    success: true,
                },
            );
        }
        return Ok(FanoutStepOutcome::Paused {
            step_id: step.id.clone(),
            completed_units,
        });
    }

    // Merge freshly-dispatched outcomes with units replayed from a
    // prior run's checkpoint, then sort so the assembled step result
    // is identical in shape to a fresh run (all units present, in
    // declared order).
    let mut items_vec: Vec<ItemResult> = item_outcomes
        .iter()
        .map(|o| ItemResult {
            index: o.idx,
            item: o.item.clone(),
            sub_id: String::new(),
            rendered_prompt: o.rendered_prompt.clone(),
            run_id: o.run_id.clone(),
            transcript_path: o.transcript_path.clone(),
            output: o.output.clone(),
            success: o.success,
        })
        .collect();
    items_vec.extend(resumed.into_values());
    items_vec.sort_by_key(|i| i.index);
    let outputs: Vec<String> = items_vec.iter().map(|i| i.output.clone()).collect();
    let aggregate_output = serde_json::to_string(&outputs).unwrap_or_else(|_| "[]".into());
    let mut success = items_vec.iter().all(|i| i.success);

    if !success {
        warn!(
            step = %step.id,
            failed = items_vec.iter().filter(|i| !i.success).count(),
            total,
            "fan-out completed with failed items (continue_on_error tolerated)"
        );
    }

    // Apply workspace deltas once (after all units finish) when sync mode is
    // active. Deltas are collected in unit-index order from the sorted outcomes.
    if sync {
        let deltas: Vec<WorkspaceDelta> = item_outcomes
            .iter()
            .filter_map(|o| o.workspace_delta.clone())
            .collect();
        if !deltas.is_empty() {
            if let Some(dispatcher) = &opts.unit_dispatcher {
                if let Err(conflict) = dispatcher
                    .apply_workspace_deltas(&opts.workspace_path, &deltas)
                    .await
                {
                    let src = RunError::Provider(conflict.to_string());
                    if !continue_on_error {
                        return Err(RunWorkflowError::Agent {
                            step: step.id.clone(),
                            source: src,
                        });
                    }
                    warn!(
                        step = %step.id,
                        error = %conflict,
                        "workspace conflict on fan-out but continue_on_error is set; marking step failed"
                    );
                    success = false;
                }
            }
        }
    }

    Ok(FanoutStepOutcome::Completed(StepResult {
        step_id: step.id.clone(),
        // The for_each-rendered list of items doubles as the
        // top-level "rendered prompt" for audit purposes; per-item
        // prompts live on each ItemResult.
        rendered_prompt: rendered_list,
        run_id: String::new(),
        transcript_path: PathBuf::new(),
        output: aggregate_output,
        success,
        skipped: false,
        kind: crate::runs::StepKind::ForEach,
        items: items_vec,
        ..Default::default()
    }))
}

/// Parallel step: render each sub-step's prompt against the same
/// shared context, then dispatch all sub-steps with the configured
/// `max_parallel:` cap. Sub-steps run independently — there's no
/// shared per-unit binding (no `{{item}}`); each sub-step's prompt
/// is just rendered against the parent context. Per-sub-step
/// results land in both `steps.<id>.results[*]` (positional, in
/// declared order) and `steps.<id>.sub_results.<sub_id>` (named).
async fn run_parallel_step(
    step: &Step,
    ctx: &StepContext,
    opts: &OrchestratorRunOpts,
    continue_on_error: bool,
) -> Result<StepResult, RunWorkflowError> {
    let subs = step
        .parallel
        .as_ref()
        .expect("run_parallel_step called for a non-parallel step");
    let total = subs.len();
    let max_parallel = step.max_parallel.unwrap_or(1).max(1) as usize;
    let semaphore = Arc::new(Semaphore::new(max_parallel));

    // Render all sub-step prompts up front so a per-sub template
    // error reports cleanly before any agent dispatches.
    let mut prepared: Vec<(usize, String, String, String, String, PathBuf)> =
        Vec::with_capacity(total);
    for (idx, sub) in subs.iter().enumerate() {
        let rendered = render_step_prompt(&sub.prompt, ctx, render_mode(opts.strict_templates))
            .map_err(|e| RunWorkflowError::Render {
                step: format!("{}.{}", step.id, sub.id),
                source: e,
            })?;
        let run_id = format!("run_{}", Ulid::new());
        let transcript_path = opts.transcript_dir.join(format!("{run_id}.jsonl"));
        prepared.push((
            idx,
            sub.id.clone(),
            sub.agent.clone(),
            rendered,
            run_id,
            transcript_path,
        ));
    }

    let mut handles = Vec::with_capacity(total);
    for (idx, sub_id, sub_agent_name, rendered, run_id, transcript_path) in prepared {
        let permit_sem = semaphore.clone();
        let factory = Arc::clone(&opts.factory);
        let workspace_id = opts.workspace_id.clone();
        let workspace_path = opts.workspace_path.clone();
        let rendered_clone = rendered.clone();
        let run_id_clone = run_id.clone();
        let transcript_clone = transcript_path.clone();
        let parent_step_id = step.id.clone();

        handles.push(tokio::spawn(async move {
            let _permit = permit_sem
                .acquire_owned()
                .await
                .expect("semaphore not closed");
            let outcome = dispatch_one(
                &factory,
                // Parent step id (for the factory's step lookup)
                // plus the sub-step's resolved agent name (which is
                // what actually loads + runs).
                &parent_step_id,
                &sub_agent_name,
                rendered_clone.clone(),
                run_id_clone.clone(),
                workspace_id,
                workspace_path,
                transcript_clone.clone(),
                None,
                // Parallel sub-steps pause at the step boundary, not mid-unit.
                None,
                None,
            )
            .await;
            let (success, error_str, raw_error) = match outcome {
                Ok(_) => (true, None, None),
                Err(e) => (false, Some(e.to_string()), Some(e)),
            };
            let output = read_final_assistant_text(
                &transcript_clone,
                success,
                &run_id_clone,
                &parent_step_id,
            );
            ParallelSubOutcome {
                idx,
                sub_id,
                rendered_prompt: rendered,
                run_id,
                transcript_path,
                output,
                success,
                error: error_str,
                raw_error,
            }
        }));
    }

    let mut outcomes: Vec<ParallelSubOutcome> = Vec::with_capacity(total);
    for handle in handles {
        match handle.await {
            Ok(o) => outcomes.push(o),
            Err(join_err) => {
                return Err(RunWorkflowError::FanoutJoin {
                    step: step.id.clone(),
                    source: join_err,
                });
            }
        }
    }
    outcomes.sort_by_key(|o| o.idx);

    if !continue_on_error {
        if let Some(failed) = outcomes.iter_mut().find(|o| !o.success) {
            if let Some(err) = failed.raw_error.take() {
                return Err(RunWorkflowError::Agent {
                    step: format!("{}.{}", step.id, failed.sub_id),
                    source: err,
                });
            }
        }
    }

    let items_vec: Vec<ItemResult> = outcomes
        .iter()
        .map(|o| ItemResult {
            index: o.idx,
            item: serde_json::Value::Null,
            sub_id: o.sub_id.clone(),
            rendered_prompt: o.rendered_prompt.clone(),
            run_id: o.run_id.clone(),
            transcript_path: o.transcript_path.clone(),
            output: o.output.clone(),
            success: o.success,
        })
        .collect();
    let outputs: Vec<String> = items_vec.iter().map(|i| i.output.clone()).collect();
    let aggregate_output = serde_json::to_string(&outputs).unwrap_or_else(|_| "[]".into());
    let success = items_vec.iter().all(|i| i.success);

    if !success {
        warn!(
            step = %step.id,
            failed = items_vec.iter().filter(|i| !i.success).count(),
            total,
            "parallel completed with failed sub-steps (continue_on_error tolerated)"
        );
    }

    Ok(StepResult {
        step_id: step.id.clone(),
        rendered_prompt: String::new(),
        run_id: String::new(),
        transcript_path: PathBuf::new(),
        output: aggregate_output,
        success,
        skipped: false,
        kind: crate::runs::StepKind::Parallel,
        items: items_vec,
        ..Default::default()
    })
}

struct ParallelSubOutcome {
    idx: usize,
    sub_id: String,
    rendered_prompt: String,
    run_id: String,
    transcript_path: PathBuf,
    output: String,
    success: bool,
    #[allow(dead_code)]
    error: Option<String>,
    raw_error: Option<RunError>,
}

/// Internal fan-out task return type. Carries the typed `RunError`
/// separately from its display string so we can re-raise the original
/// error when `continue_on_error` isn't set.
struct FanoutItemOutcome {
    idx: usize,
    item: serde_json::Value,
    rendered_prompt: String,
    run_id: String,
    transcript_path: PathBuf,
    output: String,
    success: bool,
    /// String form, currently unused but kept for future structured
    /// per-item error reporting in `ItemResult`.
    #[allow(dead_code)]
    error: Option<String>,
    raw_error: Option<RunError>,
    /// Host placement for this unit (`None` = local). Threaded through
    /// from the per-unit `placement` computed in `run_fanout_step` so
    /// the checkpoint writer can record it without re-computing it.
    host: Option<String>,
    /// File-change set returned by a sync-mode unit. `None` for local
    /// (non-sync) units or when the unit returned no delta.
    workspace_delta: Option<WorkspaceDelta>,
    /// `true` when this unit did NOT complete because a cooperative pause
    /// landed — either it was skipped outright (pause already triggered when
    /// its semaphore permit was granted, so it was never dispatched) or its
    /// local agent loop paused mid-turn. `success` is always `false` when
    /// this is `true`, but a paused unit is NOT a failure: it must be
    /// excluded from the `continue_on_error` abort check and from the
    /// on-disk checkpoint, and re-dispatched fresh on resume.
    paused: bool,
}

/// Build the agent opts via the factory and dispatch one agent run.
/// Shared by the linear and fan-out paths. Returns the full [`RunResult`]
/// so callers can distinguish a cooperative pause (`RunResult::paused`) from
/// a completed run.
///
/// `pause` is the cooperative pause token, forced onto the factory-built opts
/// (factories default it to `None`). `resume_seed`, when `Some`, overrides the
/// factory-built `initial_messages` + `user_message` so a paused-incomplete
/// step re-runs from its persisted transcript with correct role alternation.
#[allow(clippy::too_many_arguments)]
async fn dispatch_one(
    factory: &Arc<dyn StepFactory>,
    step_id: &str,
    agent_name: &str,
    rendered_prompt: String,
    run_id: String,
    workspace_id: String,
    workspace_path: PathBuf,
    transcript_path: PathBuf,
    on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    pause: Option<CancellationToken>,
    resume_seed: Option<(Vec<Message>, String)>,
) -> Result<RunResult, RunError> {
    let mut agent_opts = factory
        .build_opts_for_step(
            step_id,
            agent_name,
            rendered_prompt,
            run_id,
            workspace_id,
            workspace_path,
            transcript_path,
            on_tool_call,
        )
        .await;
    // The orchestrator owns the pause signal, not the factory.
    agent_opts.pause = pause;
    if let Some((initial_messages, user_message)) = resume_seed {
        agent_opts.initial_messages = initial_messages;
        agent_opts.user_message = user_message;
    }
    run_agent(agent_opts).await
}

/// Read the just-finished transcript to extract the final assistant
/// text. The JSONL reader silently skips truncated lines, so this is
/// robust against half-written transcripts. We do this even on
/// failure so partial output is observable to downstream `when:`
/// gates.
pub fn read_final_assistant_text(
    transcript_path: &Path,
    success: bool,
    run_id: &str,
    step_id: &str,
) -> String {
    let mut output = String::new();
    if let Ok(iter) = JsonlReader::iter(transcript_path) {
        for ev in iter.flatten() {
            if let Event::AssistantMessage { content, .. } = ev {
                output = content;
            }
        }
    } else if success {
        warn!(
            run_id = %run_id,
            "transcript missing after step {}; using empty output",
            step_id
        );
    }
    output
}

/// Parse the rendered `for_each:` string into a list of items. We
/// support two shapes:
/// - JSON array (string starts with `[`): parsed via serde_json.
///   Items can be strings, numbers, bools, or objects — whatever
///   shape the workflow author provides — and are passed through to
///   `{{item}}` verbatim.
/// - One non-empty line per item otherwise. Lines are trimmed; blank
///   lines are skipped. This is the shape produced by minijinja's
///   `for x in xs` loops or by simple comma-less newline lists.
fn parse_fanout_items(rendered: &str) -> Vec<serde_json::Value> {
    let trimmed = rendered.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.starts_with('[') {
        if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(trimmed) {
            return arr;
        }
        // Fall through to line-mode if the string starts with `[`
        // but isn't valid JSON — better to dispatch one item ("[bad")
        // than swallow the value silently.
    }
    trimmed
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| serde_json::Value::String(s.to_string()))
        .collect()
}

/// Render a fan-out item value to a short, single-line live-view label.
fn fanout_unit_key(item: &serde_json::Value) -> String {
    const MAX: usize = 60;
    let raw = match item {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= MAX {
        one_line
    } else {
        let mut out: String = one_line.chars().take(MAX - 1).collect();
        out.push('…');
        out
    }
}

/// Validate user-provided `inputs` against the workflow's declared
/// `inputs:` block: required-ness, enum membership, and per-type
/// coercion. Returns the effective input map (declared defaults
/// applied for missing entries) used by every step's template
/// context.
pub fn resolve_inputs(
    wf: &Workflow,
    user: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, RunWorkflowError> {
    // 1. Reject undeclared inputs (only when the workflow declared a
    //    schema — if `inputs:` is empty we accept any user-provided
    //    inputs as opaque strings, preserving the pre-typed behavior).
    if !wf.inputs.is_empty() {
        for name in user.keys() {
            if !wf.inputs.contains_key(name) {
                return Err(RunWorkflowError::UndeclaredInput { name: name.clone() });
            }
        }
    }

    let mut effective: BTreeMap<String, String> = BTreeMap::new();
    for (name, def) in &wf.inputs {
        let user_val = user.get(name);
        let value = match (user_val, &def.default, def.required) {
            (Some(v), _, _) => v.clone(),
            (None, Some(d), _) => yaml_scalar_to_string(d),
            (None, None, true) => {
                return Err(RunWorkflowError::MissingRequiredInput { name: name.clone() });
            }
            (None, None, false) => continue, // omit from context
        };

        // Type coercion check
        match def.ty {
            InputType::String => { /* anything stringifies */ }
            InputType::Int => {
                if value.parse::<i64>().is_err() {
                    return Err(RunWorkflowError::InputTypeMismatch {
                        name: name.clone(),
                        value: value.clone(),
                        ty: "int",
                    });
                }
            }
            InputType::Bool => {
                if !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "true" | "false" | "1" | "0" | "yes" | "no" | "on" | "off"
                ) {
                    return Err(RunWorkflowError::InputTypeMismatch {
                        name: name.clone(),
                        value: value.clone(),
                        ty: "bool",
                    });
                }
            }
        }

        if !def.allowed.is_empty() && !def.allowed.contains(&value) {
            return Err(RunWorkflowError::InputNotInEnum {
                name: name.clone(),
                value,
                allowed: def.allowed.clone(),
            });
        }

        effective.insert(name.clone(), value);
    }

    // For workflows that don't declare inputs, fall through to the
    // legacy behavior: user inputs are passed through untyped.
    if wf.inputs.is_empty() {
        for (k, v) in user {
            effective.insert(k.clone(), v.clone());
        }
    }

    Ok(effective)
}

// -- Panel step (kind: panel) -----------------------------------------------

/// Panel step. Dispatches every panelist in parallel against a shared
/// rendered subject, parses each panelist's findings JSON from its
/// final assistant text, aggregates by source, and (if a `gate:`
/// loop is configured) iterates with a fixer agent until the gate
/// clears or `max_iterations` is reached.
///
/// The runtime contract for a panelist's final assistant message:
///
///   ```json
///   { "findings": [
///       { "severity": "high", "title": "<short>", "body": "<details>" },
///       ...
///   ] }
///   ```
///
/// Surrounding prose is allowed — the parser scans for the first
/// `{ ... "findings": [...] ... }` JSON object that decodes cleanly.
/// Panelists that emit no parseable findings contribute zero findings
/// and a warning is logged.
async fn run_panel_step(
    workflow_run_id: &str,
    step: &Step,
    ctx: &StepContext,
    opts: &OrchestratorRunOpts,
    continue_on_error: bool,
) -> Result<StepResult, RunWorkflowError> {
    let panel = step
        .panel
        .as_ref()
        .expect("run_panel_step called for a non-panel step");

    // Monotonic unit index across every panelist + fixer dispatch in
    // this panel run, so the live view's `UnitState` slots grow in a
    // stable order (iter1 panelists, then iter1 fixer, then iter2…).
    let mut unit_index: usize = 0;

    // Render the initial subject once against the parent context.
    // When a `gate:` loop is configured, subsequent iterations
    // re-bind the subject to the fixer agent's output.
    let initial_subject =
        render_step_prompt(&panel.subject, ctx, render_mode(opts.strict_templates)).map_err(
            |e| RunWorkflowError::Render {
                step: format!("{}.subject", step.id),
                source: e,
            },
        )?;

    // No gate → run a single panel pass and return.
    let Some(gate) = &panel.gate else {
        return run_panel_iteration(
            workflow_run_id,
            1,
            &mut unit_index,
            step,
            panel,
            ctx,
            opts,
            continue_on_error,
            &initial_subject,
        )
        .await
        .map(|p| p.into_step_result(step, &initial_subject, 1, true));
    };

    // Gate loop. Each iteration:
    //   1. Run the panel against the current subject.
    //   2. If max severity < threshold, exit (resolved=true).
    //   3. If iterations >= max_iterations, exit (resolved=false,
    //      keep accumulated findings + items).
    //   4. Otherwise dispatch `fix_with` against the findings; the
    //      fixer's output becomes the next iteration's subject.
    let mut subject = initial_subject.clone();
    let mut iterations = 0u32;
    let (final_pass, resolved) = loop {
        iterations += 1;
        if let Some(sink) = opts.event_sink.as_ref() {
            sink.emit(
                workflow_run_id,
                &crate::executor::Event::PanelRound {
                    run_id: workflow_run_id.to_owned(),
                    step_id: step.id.clone(),
                    round: iterations,
                    max_iterations: gate.max_iterations,
                    max_severity_remaining: None,
                },
            );
        }
        let pass = run_panel_iteration(
            workflow_run_id,
            iterations,
            &mut unit_index,
            step,
            panel,
            ctx,
            opts,
            continue_on_error,
            &subject,
        )
        .await?;
        let max_sev = pass.max_severity();
        let cleared = match max_sev {
            None => true,
            Some(s) => s < gate.until_no_findings_at_severity_or_above,
        };
        if cleared {
            info!(step = %step.id, iterations, "panel gate cleared");
            break (pass, true);
        }
        if iterations >= gate.max_iterations {
            warn!(
                step = %step.id,
                iterations,
                threshold = %gate.until_no_findings_at_severity_or_above.as_str(),
                "panel gate did not clear within max_iterations"
            );
            break (pass, false);
        }
        // Dispatch the fixer with the findings as input. Its output
        // becomes the next iteration's subject.
        let fixer_subject = render_fixer_input(&subject, &pass.findings);
        let fixer_index = unit_index;
        unit_index += 1;
        let fixer_outcome = dispatch_fixer(
            workflow_run_id,
            iterations,
            fixer_index,
            step,
            &gate.fix_with,
            &fixer_subject,
            opts,
        )
        .await?;
        match fixer_outcome {
            FixerOutcome::Ok { output } => {
                subject = output;
                // Loop continues; pass is dropped — its findings are
                // about to be addressed by the fixer.
            }
            FixerOutcome::Failed(e) if !continue_on_error => {
                return Err(RunWorkflowError::Agent {
                    step: format!("{}.fixer({})", step.id, gate.fix_with),
                    source: e,
                });
            }
            FixerOutcome::Failed(e) => {
                warn!(step = %step.id, error = %e, "fixer agent failed; tolerating via continue_on_error");
                break (pass, false);
            }
        }
    };

    Ok(final_pass.into_step_result(step, &initial_subject, iterations, resolved))
}

/// Result of one panel iteration. Used both for the single-pass
/// (no-gate) path and as the loop body for the gate-loop path.
struct PanelPass {
    findings: Vec<Finding>,
    items: Vec<ItemResult>,
    success: bool,
}

impl PanelPass {
    /// Highest severity in `findings`, or `None` when empty.
    fn max_severity(&self) -> Option<crate::workflow::Severity> {
        self.findings.iter().map(|f| f.severity).max()
    }

    fn into_step_result(
        self,
        step: &Step,
        rendered_subject: &str,
        iterations: u32,
        resolved: bool,
    ) -> StepResult {
        let aggregate_output = serde_json::to_string(
            &self
                .findings
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "source": f.source,
                        "severity": f.severity.as_str(),
                        "title": f.title,
                        "body": f.body,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into());
        StepResult {
            step_id: step.id.clone(),
            rendered_prompt: rendered_subject.to_string(),
            run_id: String::new(),
            transcript_path: PathBuf::new(),
            output: aggregate_output,
            success: self.success,
            skipped: false,
            kind: crate::runs::StepKind::Panel,
            items: self.items,
            findings: self.findings,
            iterations,
            resolved,
            loop_iteration: None,
        }
    }
}

/// Outcome of one fixer-agent dispatch in the gate loop.
enum FixerOutcome {
    Ok { output: String },
    Failed(RunError),
}

/// Render a structured prompt for the fixer agent given the current
/// subject + the findings it should address. Format: original
/// subject, then a JSON-encoded `findings` array. The fixer agent's
/// system prompt should describe how to consume this.
fn render_fixer_input(subject: &str, findings: &[Finding]) -> String {
    let findings_json = serde_json::to_string_pretty(
        &findings
            .iter()
            .map(|f| {
                serde_json::json!({
                    "source": f.source,
                    "severity": f.severity.as_str(),
                    "title": f.title,
                    "body": f.body,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());
    format!(
        "Subject under review:\n{subject}\n\n\
         Panel findings to address:\n{findings_json}\n\n\
         Address every finding above and emit the revised subject."
    )
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_fixer(
    workflow_run_id: &str,
    iteration: u32,
    unit_index: usize,
    step: &Step,
    fixer_agent: &str,
    rendered_prompt: &str,
    opts: &OrchestratorRunOpts,
) -> Result<FixerOutcome, RunWorkflowError> {
    let run_id = format!("run_{}", Ulid::new());
    let transcript_path = opts.transcript_dir.join(format!("{run_id}.jsonl"));
    let unit_key = format!("iter{iteration}:fix:{fixer_agent}");
    if let Some(sink) = opts.event_sink.as_ref() {
        sink.emit(
            workflow_run_id,
            &crate::executor::Event::UnitStarted {
                run_id: workflow_run_id.to_string(),
                step_id: step.id.clone(),
                index: unit_index,
                unit_key: unit_key.clone(),
                agent: Some(fixer_agent.to_string()),
                transcript_path: transcript_path.clone(),
                host: None,
            },
        );
    }
    let outcome = dispatch_one(
        &opts.factory,
        &step.id,
        fixer_agent,
        rendered_prompt.to_string(),
        run_id.clone(),
        opts.workspace_id.clone(),
        opts.workspace_path.clone(),
        transcript_path.clone(),
        None,
        // Panel fixer runs pause at the step boundary, not mid-unit.
        None,
        None,
    )
    .await;
    let success = outcome.is_ok();
    if let Some(sink) = opts.event_sink.as_ref() {
        sink.emit(
            workflow_run_id,
            &crate::executor::Event::UnitCompleted {
                run_id: workflow_run_id.to_string(),
                step_id: step.id.clone(),
                index: unit_index,
                unit_key: unit_key.clone(),
                success,
                tokens_in: 0,
                tokens_out: 0,
                host: None,
            },
        );
    }
    match outcome {
        Ok(_) => {
            let output = read_final_assistant_text(&transcript_path, true, &run_id, &step.id);
            Ok(FixerOutcome::Ok { output })
        }
        Err(e) => Ok(FixerOutcome::Failed(e)),
    }
}

/// Run one panel iteration: dispatch all panelists in parallel
/// against `current_subject` and aggregate findings. Used by both
/// the single-pass and gate-loop paths.
#[allow(clippy::too_many_arguments)]
async fn run_panel_iteration(
    workflow_run_id: &str,
    iteration: u32,
    unit_index: &mut usize,
    step: &Step,
    panel: &crate::workflow::Panel,
    ctx: &StepContext,
    opts: &OrchestratorRunOpts,
    continue_on_error: bool,
    current_subject: &str,
) -> Result<PanelPass, RunWorkflowError> {
    let max_parallel = panel.max_parallel.unwrap_or(1).max(1) as usize;
    let total = panel.panelists.len();

    // Each panelist's prompt is either the per-step `prompt:`
    // template (rendered against the parent context plus a `subject`
    // binding) or — when omitted — the current subject verbatim. A
    // monotonic `unit_index` (shared across the whole panel run) is
    // assigned here so the live view slots grow in a stable order;
    // `unit_key` is `iter{N}:{panelist}` so re-runs across iterations
    // render as distinct rows.
    let mut prepared: Vec<(usize, usize, String, String, String, String, PathBuf)> =
        Vec::with_capacity(total);
    for (sub_idx, panelist) in panel.panelists.iter().enumerate() {
        let mut item_ctx = ctx.clone();
        item_ctx
            .inputs
            .insert("subject".to_string(), current_subject.to_string());
        let rendered = match &panel.prompt {
            Some(template) => {
                render_step_prompt(template, &item_ctx, render_mode(opts.strict_templates))
                    .map_err(|e| RunWorkflowError::Render {
                        step: format!("{}.{}", step.id, panelist),
                        source: e,
                    })?
            }
            None => current_subject.to_string(),
        };
        let run_id = format!("run_{}", Ulid::new());
        let transcript_path = opts.transcript_dir.join(format!("{run_id}.jsonl"));
        let view_index = *unit_index;
        *unit_index += 1;
        let unit_key = format!("iter{iteration}:{panelist}");
        prepared.push((
            sub_idx,
            view_index,
            unit_key,
            panelist.clone(),
            rendered,
            run_id,
            transcript_path,
        ));
    }

    // Spawn each panelist with the concurrency cap.
    let semaphore = Arc::new(Semaphore::new(max_parallel));
    let mut handles = Vec::with_capacity(total);
    for (idx, view_index, unit_key, agent_name, rendered, run_id, transcript_path) in prepared {
        let permit_sem = semaphore.clone();
        let factory = Arc::clone(&opts.factory);
        let parent_step_id = step.id.clone();
        let workspace_id = opts.workspace_id.clone();
        let workspace_path = opts.workspace_path.clone();
        let rendered_clone = rendered.clone();
        let run_id_clone = run_id.clone();
        let transcript_clone = transcript_path.clone();
        let agent_name_clone = agent_name.clone();
        // Per-unit live-view events. Cloned into the task so emission
        // brackets the panelist's REAL start/finish under the panel's
        // `max_parallel` concurrency, mirroring the fan-out path.
        let event_sink = opts.event_sink.clone();
        let workflow_run_id = workflow_run_id.to_string();
        let unit_agent = agent_name.clone();

        handles.push(tokio::spawn(async move {
            let _permit = permit_sem
                .acquire_owned()
                .await
                .expect("semaphore not closed");
            if let Some(sink) = event_sink.as_ref() {
                sink.emit(
                    &workflow_run_id,
                    &crate::executor::Event::UnitStarted {
                        run_id: workflow_run_id.clone(),
                        step_id: parent_step_id.clone(),
                        index: view_index,
                        unit_key: unit_key.clone(),
                        agent: Some(unit_agent.clone()),
                        transcript_path: transcript_clone.clone(),
                        host: None,
                    },
                );
            }
            let outcome = dispatch_one(
                &factory,
                &parent_step_id,
                &agent_name_clone,
                rendered_clone.clone(),
                run_id_clone.clone(),
                workspace_id,
                workspace_path,
                transcript_clone.clone(),
                None,
                // Panel panelists pause at the step boundary, not mid-unit.
                None,
                None,
            )
            .await;
            let (success, _err_str, raw_error) = match outcome {
                Ok(_) => (true, None, None),
                Err(e) => (false, Some(e.to_string()), Some(e)),
            };
            let output = read_final_assistant_text(
                &transcript_clone,
                success,
                &run_id_clone,
                &parent_step_id,
            );
            if let Some(sink) = event_sink.as_ref() {
                sink.emit(
                    &workflow_run_id,
                    &crate::executor::Event::UnitCompleted {
                        run_id: workflow_run_id.clone(),
                        step_id: parent_step_id.clone(),
                        index: view_index,
                        unit_key: unit_key.clone(),
                        success,
                        tokens_in: 0,
                        tokens_out: 0,
                        host: None,
                    },
                );
            }
            PanelOutcome {
                idx,
                source: agent_name,
                rendered_prompt: rendered,
                run_id,
                transcript_path,
                output,
                success,
                raw_error,
            }
        }));
    }

    let mut outcomes: Vec<PanelOutcome> = Vec::with_capacity(total);
    for handle in handles {
        match handle.await {
            Ok(o) => outcomes.push(o),
            Err(join_err) => {
                return Err(RunWorkflowError::FanoutJoin {
                    step: step.id.clone(),
                    source: join_err,
                });
            }
        }
    }
    outcomes.sort_by_key(|o| o.idx);

    // Surface a per-panelist agent error iff continue_on_error is
    // not set. Same semantics as parallel:.
    if !continue_on_error {
        if let Some(failed) = outcomes.iter_mut().find(|o| !o.success) {
            if let Some(err) = failed.raw_error.take() {
                return Err(RunWorkflowError::Agent {
                    step: format!("{}.{}", step.id, failed.source),
                    source: err,
                });
            }
        }
    }

    // Parse findings out of every panelist's final assistant text.
    // Failed panelists contribute zero findings.
    let mut findings: Vec<Finding> = Vec::new();
    for o in &outcomes {
        if !o.success {
            continue;
        }
        match parse_findings(&o.output) {
            Ok(parsed) => {
                for p in parsed {
                    findings.push(Finding {
                        source: o.source.clone(),
                        severity: p.severity,
                        title: p.title,
                        body: p.body,
                    });
                }
            }
            Err(e) => {
                warn!(panelist = %o.source, error = %e, "failed to parse findings JSON; counting as zero");
            }
        }
    }

    let items_vec: Vec<ItemResult> = outcomes
        .iter()
        .map(|o| ItemResult {
            index: o.idx,
            item: serde_json::Value::Null,
            sub_id: o.source.clone(),
            rendered_prompt: o.rendered_prompt.clone(),
            run_id: o.run_id.clone(),
            transcript_path: o.transcript_path.clone(),
            output: o.output.clone(),
            success: o.success,
        })
        .collect();
    let success = items_vec.iter().all(|i| i.success);

    if !success {
        warn!(
            step = %step.id,
            failed = items_vec.iter().filter(|i| !i.success).count(),
            total,
            "panel completed with failed panelists (continue_on_error tolerated)"
        );
    }

    Ok(PanelPass {
        findings,
        items: items_vec,
        success,
    })
}

/// Internal panel-task return type.
struct PanelOutcome {
    idx: usize,
    source: String,
    rendered_prompt: String,
    run_id: String,
    transcript_path: PathBuf,
    output: String,
    success: bool,
    raw_error: Option<RunError>,
}

/// One parsed finding. Lives only inside this module — the public
/// `Finding` struct adds the `source` (panelist agent name) on top.
struct ParsedFinding {
    severity: crate::workflow::Severity,
    title: String,
    body: String,
}

/// Extract findings from a panelist's final assistant text. Tries
/// strict-JSON first (the contract), then falls back to scanning
/// for a `{ "findings": [...] }` substring (so panelists can wrap
/// the JSON in narrative prose). Returns an empty vector if no
/// findings could be parsed — that's a legitimate "panelist saw
/// nothing problematic" outcome.
fn parse_findings(text: &str) -> Result<Vec<ParsedFinding>, ParseFindingsError> {
    let trimmed = text.trim();
    // Strict path: the entire output is one JSON object with a
    // `findings` array.
    if let Ok(parsed) = serde_json::from_str::<RawFindingsBag>(trimmed) {
        return Ok(parsed.into_findings());
    }
    // Loose path: scan for a `{...}` chunk that decodes. Matches
    // common LLM behavior of wrapping JSON in code fences or prose.
    if let Some(obj) = scan_for_json_object(trimmed) {
        if let Ok(parsed) = serde_json::from_str::<RawFindingsBag>(obj) {
            return Ok(parsed.into_findings());
        }
    }
    // No parseable findings — return empty rather than erroring.
    // Emit a debug log so authors can see during iteration.
    info!("no parseable findings JSON in panelist output");
    Ok(Vec::new())
}

fn render_mode(strict: bool) -> RenderMode {
    if strict {
        RenderMode::Strict
    } else {
        RenderMode::Permissive
    }
}

#[derive(Debug, thiserror::Error)]
enum ParseFindingsError {
    #[error("findings JSON: {0}")]
    #[allow(dead_code)]
    Json(String),
}

#[derive(serde::Deserialize)]
struct RawFindingsBag {
    #[serde(default)]
    findings: Vec<RawFinding>,
}

#[derive(serde::Deserialize)]
struct RawFinding {
    #[serde(default)]
    severity: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
}

impl RawFindingsBag {
    fn into_findings(self) -> Vec<ParsedFinding> {
        self.findings
            .into_iter()
            .map(|f| ParsedFinding {
                severity: crate::workflow::Severity::parse_lossy(&f.severity),
                title: f.title,
                body: f.body,
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Unit tests — distributed fan-out
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // A factory that panics if actually called.  For distributed fan-out
    // tests every unit has a host placement, so `dispatch_one` (and
    // therefore `build_opts_for_step`) must never be invoked.
    struct PanicFactory;

    #[async_trait]
    impl StepFactory for PanicFactory {
        async fn build_opts_for_step(
            &self,
            _step_id: &str,
            _agent_name: &str,
            _rendered_prompt: String,
            _run_id: String,
            _workspace_id: String,
            _workspace_path: PathBuf,
            _transcript_path: PathBuf,
            _on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> rupu_agent::AgentRunOpts {
            panic!("PanicFactory: build_opts_for_step must not be called for distributed units")
        }
    }

    /// Fake `UnitDispatcher` for tests.
    ///
    /// Records every `(unit.index, host)` pair it receives.  When
    /// `fail_first_host` is set, the first dispatch to that host returns
    /// `Err(RunError::Provider("host down"))`.
    struct FakeUnitDispatcher {
        calls: Mutex<Vec<(usize, String)>>,
        fail_first_host: Option<String>,
    }

    impl FakeUnitDispatcher {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail_first_host: None,
            }
        }

        fn with_failing_host(host: impl Into<String>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail_first_host: Some(host.into()),
            }
        }
    }

    #[async_trait]
    impl UnitDispatcher for FakeUnitDispatcher {
        async fn dispatch_unit(
            &self,
            unit: UnitDispatch,
            host: &str,
        ) -> Result<UnitOutcome, RunError> {
            self.calls
                .lock()
                .unwrap()
                .push((unit.index, host.to_string()));
            if self.fail_first_host.as_deref() == Some(host) {
                return Err(RunError::Provider("host down".into()));
            }
            Ok(UnitOutcome {
                output: format!("out-{}-on-{host}", unit.index),
                success: true,
                error: None,
                workspace_delta: None,
            })
        }
    }

    /// Build the minimal `OrchestratorRunOpts` for a distributed fan-out
    /// test.  Mirrors the pattern used by the integration tests in
    /// `tests/linear_runner.rs` but keeps `run_store: None` (no disk
    /// persistence) and injects a `UnitDispatcher`.
    fn make_opts(
        wf: Workflow,
        transcript_dir: PathBuf,
        dispatcher: Arc<dyn UnitDispatcher>,
    ) -> OrchestratorRunOpts {
        OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_test".into(),
            workspace_path: transcript_dir.clone(),
            transcript_dir,
            factory: Arc::new(PanicFactory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: Some(dispatcher),
            action_dispatcher: None,
            pause: None,
        }
    }

    // -----------------------------------------------------------------------
    // `render_action_args` — array-value template rendering
    // -----------------------------------------------------------------------

    /// An action step's `with:` array values must render each element
    /// through the same template machinery a string `with:` value gets —
    /// `walk`'s `Value::Array` arm recurses per-item rather than skipping
    /// arrays wholesale. Mirrors `issues.create`'s `labels:
    /// Option<Vec<String>>` schema param (`crates/rupu-mcp/src/tools/
    /// issues.rs`), the catalog tool with an array-typed field.
    #[test]
    fn render_action_args_renders_templates_inside_array_values() {
        let prior = vec![StepResult {
            step_id: "seed".into(),
            output: "bug-report".into(),
            success: true,
            skipped: false,
            kind: crate::runs::StepKind::Linear,
            ..Default::default()
        }];
        let ctx = base_context_for_step(&BTreeMap::new(), None, None, &prior);
        let with = serde_json::json!({
            "project": "acme/widget",
            "title": "found an issue",
            "body": "auto-filed",
            "labels": ["{{ steps.seed.output }}", "static"],
        });

        let rendered = render_action_args(Some(&with), &ctx, RenderMode::Permissive)
            .expect("array-valued with: renders");

        assert_eq!(rendered["labels"][0], "bug-report");
        assert_eq!(
            rendered["labels"][1], "static",
            "non-template array elements pass through unchanged"
        );
    }

    // -----------------------------------------------------------------------
    // Placed linear step tests
    // -----------------------------------------------------------------------

    const WF_PLACED: &str = r#"
name: placed-test
steps:
  - id: build
    agent: builder
    prompt: "build {{ inputs.what }}"
    host: worker-1
"#;

    const WF_PLACED_TWO_STEP: &str = r#"
name: placed-chain
steps:
  - id: build
    agent: builder
    prompt: "build it"
    host: worker-1
  - id: report
    agent: reporter
    prompt: "summarize {{ steps.build.output }}"
    host: worker-2
"#;

    #[tokio::test]
    async fn placed_linear_step_dispatched_through_port() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::new());
        let wf = Workflow::parse(WF_PLACED).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher.clone());
        opts.inputs.insert("what".into(), "rupu".into());

        let result = run_workflow(opts).await.expect("run ok");

        // The dispatcher saw exactly one unit at index 0 on worker-1.
        let calls = dispatcher.calls.lock().unwrap().clone();
        assert_eq!(calls, vec![(0, "worker-1".to_string())]);

        // The UnitOutcome.output became the step output.
        let sr = &result.step_results[0];
        assert_eq!(sr.step_id, "build");
        assert!(sr.success);
        assert_eq!(sr.output, "out-0-on-worker-1");
    }

    #[tokio::test]
    async fn placed_step_output_feeds_downstream() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::new());
        let wf = Workflow::parse(WF_PLACED_TWO_STEP).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), dispatcher.clone());

        let result = run_workflow(opts).await.expect("run ok");

        // Step 2 ran on worker-2, and its rendered prompt embedded step 1's output.
        let calls = dispatcher.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![(0, "worker-1".to_string()), (0, "worker-2".to_string())]
        );
        assert_eq!(result.step_results.len(), 2);
        assert_eq!(
            result.step_results[1].rendered_prompt,
            "summarize out-0-on-worker-1"
        );
    }

    #[tokio::test]
    async fn placed_step_remote_err_aborts_without_continue_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::with_failing_host("worker-1"));
        let wf = Workflow::parse(WF_PLACED).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);
        opts.inputs.insert("what".into(), "rupu".into());

        let err = run_workflow(opts).await.expect_err("must abort");
        assert!(matches!(err, RunWorkflowError::Agent { ref step, .. } if step == "build"));
    }

    #[tokio::test]
    async fn placed_step_remote_err_tolerated_with_continue_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::with_failing_host("worker-1"));
        let yaml = r#"
name: placed-tolerant
steps:
  - id: build
    agent: builder
    prompt: "build it"
    host: worker-1
    continue_on_error: true
"#;
        let wf = Workflow::parse(yaml).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);

        let result = run_workflow(opts).await.expect("tolerated");
        assert!(!result.step_results[0].success);
    }

    #[tokio::test]
    async fn placed_step_failed_outcome_aborts() {
        // Agent ran but reported success=false → still aborts under
        // continue_on_error:false (symmetric with the fan-out path).
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(AlwaysFailedOutcomeDispatcher);
        let wf = Workflow::parse(WF_PLACED).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);
        opts.inputs.insert("what".into(), "rupu".into());

        let err = run_workflow(opts)
            .await
            .expect_err("must abort on success=false");
        assert!(matches!(err, RunWorkflowError::Agent { ref step, .. } if step == "build"));
    }

    #[tokio::test]
    async fn placed_step_without_dispatcher_errors_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(WF_PLACED).unwrap();
        // make_opts requires a dispatcher; build opts with None directly.
        let mut opts = make_opts(
            wf,
            dir.path().to_path_buf(),
            Arc::new(FakeUnitDispatcher::new()),
        );
        opts.unit_dispatcher = None;
        opts.inputs.insert("what".into(), "rupu".into());

        let err = run_workflow(opts)
            .await
            .expect_err("must error without fleet");
        let msg = err.to_string();
        assert!(
            msg.contains("fleet"),
            "expected clear fleet-access error, got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Distributed fan-out tests
    // -----------------------------------------------------------------------

    const WF_DISTRIBUTED: &str = r#"
name: distributed-test
steps:
  - id: process
    for_each: "a\nb\nc\nd"
    agent: dummy
    prompt: "Process {{ item }}"
    max_parallel: 4
    distribute:
      hosts: [h1, h2]
"#;

    #[tokio::test]
    async fn distributed_fanout_round_robins_and_aggregates() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::new());
        let wf = Workflow::parse(WF_DISTRIBUTED).unwrap();
        let opts = make_opts(wf, tmp.path().to_path_buf(), dispatcher.clone());

        let res = run_workflow(opts).await.expect("workflow should succeed");

        assert_eq!(res.step_results.len(), 1);
        let step = &res.step_results[0];
        assert!(step.success, "all units succeeded → step success");

        // Round-robin host assignment: idx 0→h1, 1→h2, 2→h1, 3→h2.
        let calls = dispatcher.calls.lock().unwrap().clone();
        let mut sorted = calls.clone();
        sorted.sort_by_key(|(idx, _)| *idx);
        assert_eq!(
            sorted,
            vec![
                (0, "h1".to_string()),
                (1, "h2".to_string()),
                (2, "h1".to_string()),
                (3, "h2".to_string()),
            ],
            "units dispatched round-robin by index; got: {sorted:?}"
        );

        // Aggregated results in index order.
        assert_eq!(step.items.len(), 4);
        assert_eq!(step.items[0].output, "out-0-on-h1");
        assert_eq!(step.items[1].output, "out-1-on-h2");
        assert_eq!(step.items[2].output, "out-2-on-h1");
        assert_eq!(step.items[3].output, "out-3-on-h2");
    }

    const WF_DISTRIBUTED_2: &str = r#"
name: distributed-retry-test
steps:
  - id: process
    for_each: "a\nb"
    agent: dummy
    prompt: "Process {{ item }}"
    max_parallel: 2
    continue_on_error: true
    distribute:
      hosts: [h1, h2]
"#;

    #[tokio::test]
    async fn distributed_fanout_reassigns_once_on_host_failure() {
        // h1 always returns an error.  Unit 0 is assigned h1 (idx=0 % 2),
        // should be retried on h2 (fallback = (0+1)%2=h2) and succeed.
        // Unit 1 is assigned h2 directly and succeeds on the first try.
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::with_failing_host("h1"));
        let wf = Workflow::parse(WF_DISTRIBUTED_2).unwrap();
        let opts = make_opts(wf, tmp.path().to_path_buf(), dispatcher.clone());

        let res = run_workflow(opts).await.expect("workflow should complete");
        let step = &res.step_results[0];

        let calls = dispatcher.calls.lock().unwrap().clone();

        // Unit 0: first call to h1 (fails), then retry to h2 (succeeds).
        // Unit 1: single call to h2 (succeeds).
        // Total calls = 3.
        let unit0_calls: Vec<&(usize, String)> = calls.iter().filter(|(i, _)| *i == 0).collect();
        assert_eq!(
            unit0_calls.len(),
            2,
            "unit 0 should be called twice (primary + retry); got {calls:?}"
        );
        assert_eq!(unit0_calls[0].1, "h1", "first call for unit 0 must be h1");
        assert_eq!(unit0_calls[1].1, "h2", "retry call for unit 0 must be h2");

        // After the retry, unit 0's output should come from h2.
        assert!(step.items[0].success, "unit 0 should succeed after retry");
        assert_eq!(step.items[0].output, "out-0-on-h2");

        // Unit 1 succeeded on h2 directly.
        let unit1_calls: Vec<&(usize, String)> = calls.iter().filter(|(i, _)| *i == 1).collect();
        assert_eq!(unit1_calls.len(), 1, "unit 1 needs only one call");
        assert_eq!(unit1_calls[0].1, "h2");
        assert!(step.items[1].success);
    }

    // -----------------------------------------------------------------------
    // Minor 1 — no dispatcher + distribute → clear error
    // -----------------------------------------------------------------------

    /// A workflow with `distribute:` but no `UnitDispatcher` must return a
    /// clear "distribute requires fleet access" error rather than silently
    /// completing or panicking.
    #[tokio::test]
    async fn distributed_fanout_no_dispatcher_returns_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(WF_DISTRIBUTED).unwrap();
        // Build opts directly (without `make_opts`) so we can set
        // `unit_dispatcher: None`.
        let opts = OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_test".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(PanicFactory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };

        let err = run_workflow(opts)
            .await
            .expect_err("should fail — distribute without fleet access");
        let msg = err.to_string();
        assert!(
            msg.contains("distribute requires fleet access"),
            "expected 'distribute requires fleet access' in error; got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Minor 2 — KEY regression test: Ok(UnitOutcome{success:false}) aborts
    // -----------------------------------------------------------------------

    /// A fake dispatcher that always returns a successful `Ok` envelope but
    /// with `success: false` inside — the "agent ran but failed" case.
    struct AlwaysFailedOutcomeDispatcher;

    #[async_trait]
    impl UnitDispatcher for AlwaysFailedOutcomeDispatcher {
        async fn dispatch_unit(
            &self,
            _unit: UnitDispatch,
            _host: &str,
        ) -> Result<UnitOutcome, RunError> {
            Ok(UnitOutcome {
                output: String::new(),
                success: false,
                error: Some("boom".into()),
                workspace_delta: None,
            })
        }
    }

    // `continue_on_error` is absent → defaults to false.
    const WF_DISTRIBUTED_NO_COE: &str = r#"
name: distributed-fail-abort-test
steps:
  - id: process
    for_each: "a\nb"
    agent: dummy
    prompt: "Process {{ item }}"
    max_parallel: 2
    distribute:
      hosts: [h1, h2]
"#;

    /// When a remote unit returns `Ok(UnitOutcome{success:false, …})` and
    /// `continue_on_error` is not set (defaults to false), the workflow must
    /// ABORT — not silently complete.  This is the regression test for the
    /// `raw_error` synthesis fix above.
    #[tokio::test]
    async fn distributed_fanout_failed_outcome_aborts_under_continue_on_error_false() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(AlwaysFailedOutcomeDispatcher);
        let wf = Workflow::parse(WF_DISTRIBUTED_NO_COE).unwrap();
        let opts = make_opts(wf, tmp.path().to_path_buf(), dispatcher);

        let err = run_workflow(opts)
            .await
            .expect_err("workflow must abort — remote unit failed and continue_on_error is false");
        let msg = err.to_string();
        assert!(
            msg.contains("boom") || msg.contains("remote unit failed"),
            "error should surface the unit failure reason; got: {msg}"
        );
    }

    #[test]
    fn workspace_delta_carries_paths_and_payload() {
        let d = WorkspaceDelta {
            changed: vec!["src/lib.rs".into()],
            deleted: vec!["old.txt".into()],
            payload: vec![1, 2, 3],
        };
        assert_eq!(d.changed, vec!["src/lib.rs".to_string()]);
        assert_eq!(d.deleted, vec!["old.txt".to_string()]);
        assert_eq!(d.payload, vec![1, 2, 3]);
    }

    #[test]
    fn workspace_conflict_displays_paths() {
        let c = WorkspaceConflict(vec!["src/shared.rs".into()]);
        assert!(c.to_string().contains("src/shared.rs"));
    }

    #[tokio::test]
    async fn default_apply_workspace_deltas_is_noop_ok() {
        // The 3a FakeUnitDispatcher does not override apply; the default is Ok.
        let d = FakeUnitDispatcher::new();
        let tmp = tempfile::tempdir().unwrap();
        let res = d.apply_workspace_deltas(tmp.path(), &[]).await;
        assert!(res.is_ok());
    }

    // -----------------------------------------------------------------------
    // T5 — workspace-sync routing tests
    // -----------------------------------------------------------------------

    /// A `UnitDispatcher` that:
    /// - Records whether each dispatched unit's `workspace_path` was `Some`.
    /// - Always returns a `UnitOutcome` with `workspace_delta: Some(...)`.
    /// - Records the number of deltas passed to each `apply_workspace_deltas` call.
    /// - When built with `with_conflict()`, returns `Err(WorkspaceConflict)`
    ///   from `apply_workspace_deltas`.
    struct WorkspaceFakeDispatcher {
        saw_ws_path: Mutex<Vec<bool>>,
        applied_counts: Mutex<Vec<usize>>,
        conflict_mode: bool,
    }

    impl WorkspaceFakeDispatcher {
        fn new() -> Self {
            Self {
                saw_ws_path: Mutex::new(Vec::new()),
                applied_counts: Mutex::new(Vec::new()),
                conflict_mode: false,
            }
        }

        fn with_conflict() -> Self {
            Self {
                saw_ws_path: Mutex::new(Vec::new()),
                applied_counts: Mutex::new(Vec::new()),
                conflict_mode: true,
            }
        }

        fn saw_workspace_path(&self) -> Vec<bool> {
            self.saw_ws_path.lock().unwrap().clone()
        }

        fn applied_delta_counts(&self) -> Vec<usize> {
            self.applied_counts.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl UnitDispatcher for WorkspaceFakeDispatcher {
        async fn dispatch_unit(
            &self,
            unit: UnitDispatch,
            _host: &str,
        ) -> Result<UnitOutcome, RunError> {
            self.saw_ws_path
                .lock()
                .unwrap()
                .push(unit.workspace_path.is_some());
            Ok(UnitOutcome {
                output: format!("out-{}", unit.index),
                success: true,
                error: None,
                workspace_delta: Some(WorkspaceDelta {
                    changed: vec![format!("u{}.txt", unit.index)],
                    deleted: vec![],
                    payload: vec![],
                }),
            })
        }

        async fn apply_workspace_deltas(
            &self,
            _workspace_path: &std::path::Path,
            deltas: &[WorkspaceDelta],
        ) -> Result<(), WorkspaceConflict> {
            self.applied_counts.lock().unwrap().push(deltas.len());
            if self.conflict_mode {
                Err(WorkspaceConflict(vec!["shared".into()]))
            } else {
                Ok(())
            }
        }
    }

    const WF_PLACED_SYNC: &str = r#"
name: placed-sync
steps:
  - id: edit
    agent: coder
    prompt: "edit"
    host: worker-1
    workspace: sync
"#;

    #[tokio::test]
    async fn placed_sync_step_sends_workspace_and_applies_delta() {
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::new());
        let wf = Workflow::parse(WF_PLACED_SYNC).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), disp.clone());
        let res = run_workflow(opts).await.expect("ok");
        assert!(res.step_results[0].success);
        // dispatched WITH a workspace_path
        assert_eq!(disp.saw_workspace_path(), vec![true]);
        // applied exactly one delta set (single writer)
        assert_eq!(disp.applied_delta_counts(), vec![1]);
    }

    #[tokio::test]
    async fn no_sync_step_sends_no_workspace_path() {
        // WF_PLACED (host: but no workspace:) must not set workspace_path.
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::new());
        let wf = Workflow::parse(WF_PLACED).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), disp.clone());
        opts.inputs.insert("what".into(), "x".into());
        run_workflow(opts).await.expect("ok");
        assert_eq!(disp.saw_workspace_path(), vec![false]);
        // apply never called when workspace_path is None
        assert!(disp.applied_delta_counts().is_empty());
    }

    #[tokio::test]
    async fn fanout_sync_collects_all_deltas_and_applies_once() {
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::new());
        let wf = Workflow::parse(
            r#"
name: fan-sync
steps:
  - id: edit
    for_each: "a\nb\nc"
    agent: coder
    prompt: "edit {{ item }}"
    max_parallel: 3
    workspace: sync
    distribute:
      hosts: [w1, w2]
"#,
        )
        .unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), disp.clone());
        let res = run_workflow(opts).await.expect("ok");
        assert!(res.step_results[0].success);
        // every unit saw a workspace_path (all 3 dispatches)
        assert_eq!(disp.saw_workspace_path(), vec![true, true, true]);
        // applied once, with all 3 deltas together
        assert_eq!(disp.applied_delta_counts(), vec![3]);
    }

    #[tokio::test]
    async fn workspace_conflict_aborts_without_continue_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::with_conflict());
        let wf = Workflow::parse(WF_PLACED_SYNC).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), disp);
        let err = run_workflow(opts).await.expect_err("conflict must abort");
        assert!(
            matches!(err, RunWorkflowError::Agent { ref step, .. } if step == "edit"),
            "expected Agent error for step 'edit', got: {err:?}"
        );
    }

    #[tokio::test]
    async fn workspace_conflict_tolerated_with_continue_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::with_conflict());
        let wf = Workflow::parse(
            r#"
name: placed-sync-tol
steps:
  - id: edit
    agent: coder
    prompt: "edit"
    host: worker-1
    workspace: sync
    continue_on_error: true
"#,
        )
        .unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), disp);
        let res = run_workflow(opts).await.expect("tolerated");
        assert!(!res.step_results[0].success);
    }

    // -----------------------------------------------------------------------
    // T6 — resume-with-workspace-sync guard
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resume_of_workspace_sync_workflow_is_refused() {
        // A workflow with a host-placed workspace:sync step.  Attempting to
        // checkpoint-resume it must return ResumeWithWorkspaceSync, not silently
        // drop the already-succeeded unit's file edits.
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::new());
        let wf = Workflow::parse(WF_PLACED_SYNC).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), disp);
        // Simulate a checkpoint resume (prior_step_results is empty — the guard
        // fires before any step runs, so the content doesn't matter).
        opts.resume_from = Some(ResumeState {
            run_id: "run_test_resume".into(),
            prior_step_results: Vec::new(),
            approved_step_id: String::new(),
            completed_units: std::collections::BTreeMap::new(),
            reason: PauseReason::Approval,
            paused_step: None,
            rejected_reason: None,
        });
        let err = run_workflow(opts)
            .await
            .expect_err("resume of sync workflow must be refused");
        assert!(
            matches!(err, RunWorkflowError::ResumeWithWorkspaceSync),
            "expected ResumeWithWorkspaceSync, got: {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // T3 — pause / resume (run + workflow)
    // -----------------------------------------------------------------------

    use rupu_agent::runner::{
        CapturingMockProvider, MockProvider, ScriptedTurn, DEFAULT_MAX_TOKENS,
    };
    use rupu_agent::{AgentRunOpts, BypassDecider};
    use rupu_providers::types::{
        ContentBlock, LlmRequest, LlmResponse, Role, StopReason, StreamEvent,
    };
    use rupu_providers::{LlmProvider, ProviderError, ProviderId};
    use std::time::Duration;

    /// A provider whose `send` blocks (effectively) forever, so a pause token
    /// wins the `run_agent` select! race deterministically.
    struct BlockingProvider;

    #[async_trait]
    impl LlmProvider for BlockingProvider {
        async fn send(&mut self, _req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            Err(ProviderError::Http("unreachable — pause should win".into()))
        }
        async fn stream(
            &mut self,
            req: &LlmRequest,
            _on_event: &mut (dyn FnMut(StreamEvent) + Send),
        ) -> Result<LlmResponse, ProviderError> {
            self.send(req).await
        }
        fn default_model(&self) -> &str {
            "mock-1"
        }
        fn provider_id(&self) -> ProviderId {
            ProviderId::Anthropic
        }
    }

    /// A `StepFactory` that hands out a single pre-built provider (once).
    struct OneShotFactory {
        provider: Mutex<Option<Box<dyn LlmProvider>>>,
    }
    impl OneShotFactory {
        fn new(p: Box<dyn LlmProvider>) -> Self {
            Self {
                provider: Mutex::new(Some(p)),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_agent_opts(
        provider: Box<dyn LlmProvider>,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        AgentRunOpts {
            agent_name: agent_name.to_string(),
            agent_system_prompt: "test".into(),
            agent_tools: None,
            provider,
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id,
            workspace_id,
            workspace_path,
            transcript_path,
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: rupu_tools::ToolContext::default(),
            user_message: rendered_prompt,
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            // The one-shot completions path races `provider.send` against the
            // pause token — the deterministic pause boundary for these tests.
            no_stream: true,
            suppress_stream_stdout: true,
            mcp_registry: None,
            effort: None,
            context_window: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: None,
            step_id: String::new(),
            on_tool_call,
            on_stream_event: None,
            concerns: None,
            max_tokens: DEFAULT_MAX_TOKENS,
            context_window_tokens: None,
            compact_at_percent: None,
            scope_name: None,
            surface_tag: None,
            pause: None,
        }
    }

    #[async_trait]
    impl StepFactory for OneShotFactory {
        async fn build_opts_for_step(
            &self,
            _step_id: &str,
            agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: PathBuf,
            transcript_path: PathBuf,
            on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> AgentRunOpts {
            let provider = self
                .provider
                .lock()
                .unwrap()
                .take()
                .expect("OneShotFactory: provider already taken");
            make_agent_opts(
                provider,
                agent_name,
                rendered_prompt,
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                on_tool_call,
            )
        }
    }

    /// Collecting event sink for pause/resume assertions.
    #[derive(Default)]
    struct CollectingSink {
        labels: Mutex<Vec<String>>,
    }
    impl CollectingSink {
        fn labels(&self) -> Vec<String> {
            self.labels.lock().unwrap().clone()
        }
    }
    impl crate::executor::EventSink for CollectingSink {
        fn emit(&self, _run_id: &str, ev: &crate::executor::Event) {
            let label = match ev {
                crate::executor::Event::RunPaused { .. } => "RunPaused",
                crate::executor::Event::RunResumed { .. } => "RunResumed",
                crate::executor::Event::StepPaused { .. } => "StepPaused",
                crate::executor::Event::StepResumed { .. } => "StepResumed",
                crate::executor::Event::RunCompleted { .. } => "RunCompleted",
                _ => return,
            };
            self.labels.lock().unwrap().push(label.to_string());
        }
    }

    fn pause_opts(
        wf: Workflow,
        dir: PathBuf,
        factory: Arc<dyn StepFactory>,
        sink: Arc<dyn crate::executor::EventSink>,
    ) -> OrchestratorRunOpts {
        OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_pause".into(),
            workspace_path: dir.clone(),
            transcript_dir: dir,
            factory,
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: Some(sink),
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        }
    }

    const WF_SOLO: &str = r#"
name: pause-solo
steps:
  - id: solo
    agent: worker
    prompt: "do work"
"#;

    #[tokio::test]
    async fn agent_run_pauses_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(WF_SOLO).unwrap();

        // --- Phase 1: pause mid-step. ---
        let token = CancellationToken::new();
        let token2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            token2.cancel();
        });
        let sink1 = Arc::new(CollectingSink::default());
        let factory1 = Arc::new(OneShotFactory::new(Box::new(BlockingProvider)));
        let mut opts1 = pause_opts(
            wf.clone(),
            dir.path().to_path_buf(),
            factory1,
            sink1.clone(),
        );
        opts1.pause = Some(token);

        let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
        let awaiting = res1.awaiting.expect("run must have paused");
        assert_eq!(awaiting.reason, PauseReason::Manual, "manual pause");
        assert_eq!(awaiting.step_id, "solo");
        assert!(
            !awaiting.resume_seed.is_empty(),
            "a mid-step pause carries a resume seed"
        );
        assert!(
            res1.step_results.is_empty(),
            "the paused step did not complete"
        );
        assert!(
            sink1.labels().contains(&"RunPaused".to_string()),
            "RunPaused must be emitted; got {:?}",
            sink1.labels()
        );
        assert!(
            sink1.labels().contains(&"StepPaused".to_string()),
            "StepPaused must be emitted; got {:?}",
            sink1.labels()
        );

        // --- Phase 2: resume → completes. ---
        let sink2 = Arc::new(CollectingSink::default());
        let factory2 = Arc::new(OneShotFactory::new(Box::new(MockProvider::new(vec![
            ScriptedTurn::AssistantText {
                text: "done".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            },
        ]))));
        let mut opts2 = pause_opts(wf, dir.path().to_path_buf(), factory2, sink2.clone());
        opts2.resume_from = Some(ResumeState {
            run_id: String::new(),
            prior_step_results: Vec::new(),
            approved_step_id: String::new(),
            completed_units: std::collections::BTreeMap::new(),
            reason: PauseReason::Manual,
            paused_step: Some(PausedStep {
                step_id: "solo".into(),
                seed_messages: awaiting.resume_seed,
            }),
            rejected_reason: None,
        });

        let res2 = run_workflow(opts2).await.expect("resume completes");
        assert!(res2.awaiting.is_none(), "resumed run runs to completion");
        assert_eq!(res2.step_results.len(), 1);
        assert!(res2.step_results[0].success);
        assert_eq!(res2.step_results[0].output, "done");
        let labels2 = sink2.labels();
        assert!(
            labels2.contains(&"RunResumed".to_string()),
            "RunResumed must be emitted; got {labels2:?}"
        );
        assert!(
            labels2.contains(&"StepResumed".to_string()),
            "StepResumed must be emitted; got {labels2:?}"
        );
    }

    /// A `UnitDispatcher` that cancels a pause token immediately after its
    /// first dispatch — so step 1 completes, then the workflow pauses at the
    /// boundary before step 2.
    struct CancelAfterFirstDispatcher {
        token: CancellationToken,
        calls: Mutex<Vec<(usize, String)>>,
    }
    #[async_trait]
    impl UnitDispatcher for CancelAfterFirstDispatcher {
        async fn dispatch_unit(
            &self,
            unit: UnitDispatch,
            host: &str,
        ) -> Result<UnitOutcome, RunError> {
            let first = self.calls.lock().unwrap().is_empty();
            self.calls
                .lock()
                .unwrap()
                .push((unit.index, host.to_string()));
            if first {
                self.token.cancel();
            }
            Ok(UnitOutcome {
                output: format!("out-{}-on-{host}", unit.index),
                success: true,
                error: None,
                workspace_delta: None,
            })
        }
    }

    #[tokio::test]
    async fn workflow_pauses_at_step_boundary_and_resumes_remaining() {
        let dir = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(WF_PLACED_TWO_STEP).unwrap();

        // --- Phase 1: run step 1, pause before step 2. ---
        let token = CancellationToken::new();
        let dispatcher1 = Arc::new(CancelAfterFirstDispatcher {
            token: token.clone(),
            calls: Mutex::new(Vec::new()),
        });
        let sink1 = Arc::new(CollectingSink::default());
        let factory: Arc<dyn StepFactory> = Arc::new(PanicFactory);
        let mut opts1 = pause_opts(wf.clone(), dir.path().to_path_buf(), factory, sink1.clone());
        opts1.unit_dispatcher = Some(dispatcher1.clone());
        opts1.pause = Some(token);

        let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
        let awaiting = res1.awaiting.expect("must pause at boundary");
        assert_eq!(awaiting.reason, PauseReason::Manual);
        assert_eq!(awaiting.step_id, "report", "paused BEFORE step 2");
        assert_eq!(res1.step_results.len(), 1, "only step 1 completed");
        assert_eq!(res1.step_results[0].step_id, "build");
        assert_eq!(
            dispatcher1.calls.lock().unwrap().clone(),
            vec![(0, "worker-1".to_string())],
            "only step 1 was dispatched"
        );
        assert!(sink1.labels().contains(&"RunPaused".to_string()));

        // --- Phase 2: resume → step 2 only. ---
        let dispatcher2 = Arc::new(FakeUnitDispatcher::new());
        let sink2 = Arc::new(CollectingSink::default());
        let factory2: Arc<dyn StepFactory> = Arc::new(PanicFactory);
        let mut opts2 = pause_opts(wf, dir.path().to_path_buf(), factory2, sink2.clone());
        opts2.unit_dispatcher = Some(dispatcher2.clone());
        opts2.resume_from = Some(ResumeState {
            run_id: String::new(),
            prior_step_results: res1.step_results.clone(),
            approved_step_id: String::new(),
            completed_units: std::collections::BTreeMap::new(),
            reason: PauseReason::Manual,
            paused_step: None,
            rejected_reason: None,
        });

        let res2 = run_workflow(opts2).await.expect("resume completes");
        assert!(res2.awaiting.is_none());
        assert_eq!(
            res2.step_results.len(),
            2,
            "both steps present after resume"
        );
        assert_eq!(res2.step_results[1].step_id, "report");
        assert_eq!(
            dispatcher2.calls.lock().unwrap().clone(),
            vec![(0, "worker-2".to_string())],
            "resume dispatched ONLY step 2"
        );
        assert!(sink2.labels().contains(&"RunResumed".to_string()));
    }

    #[tokio::test]
    async fn workspace_sync_workflow_pause_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::new());
        let wf = Workflow::parse(WF_PLACED_SYNC).unwrap();
        let sink = Arc::new(CollectingSink::default());
        let factory: Arc<dyn StepFactory> = Arc::new(PanicFactory);
        let mut opts = pause_opts(wf, dir.path().to_path_buf(), factory, sink);
        opts.unit_dispatcher = Some(disp);
        // Pre-cancelled token: the boundary check fires before the first step.
        let token = CancellationToken::new();
        token.cancel();
        opts.pause = Some(token);

        let err = run_workflow(opts)
            .await
            .expect_err("pause of a workspace:sync workflow must be refused");
        assert!(
            matches!(err, RunWorkflowError::PauseWithWorkspaceSync),
            "expected PauseWithWorkspaceSync, got: {err:?}"
        );
    }

    /// A workflow whose FIRST step is a plain local (non-placed) linear step
    /// and whose SECOND step is a placed `workspace: sync` step. The pause
    /// token is cancelled mid-first-step (via `BlockingProvider`, same
    /// mechanism as `agent_run_pauses_and_resumes`) — a step BOUNDARY was
    /// never crossed, so only the mid-linear-step pause path
    /// (`run_steps_inner`'s `LinearStepOutcome::Paused` arm) can catch this.
    /// `workflow_has_sync_step` looks at every step in the workflow, not just
    /// the currently-running one, so it still refuses.
    const WF_SOLO_THEN_SYNC: &str = r#"
name: pause-solo-then-sync
steps:
  - id: solo
    agent: worker
    prompt: "do work"
  - id: edit
    agent: coder
    prompt: "edit"
    host: worker-1
    workspace: sync
"#;

    #[tokio::test]
    async fn workspace_sync_workflow_mid_linear_step_pause_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(WF_SOLO_THEN_SYNC).unwrap();

        let token = CancellationToken::new();
        let token2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            token2.cancel();
        });
        let sink = Arc::new(CollectingSink::default());
        let factory = Arc::new(OneShotFactory::new(Box::new(BlockingProvider)));
        let mut opts = pause_opts(wf, dir.path().to_path_buf(), factory, sink);
        opts.unit_dispatcher = Some(Arc::new(WorkspaceFakeDispatcher::new()));
        opts.pause = Some(token);

        let err = run_workflow(opts).await.expect_err(
            "a mid-linear-step pause of a workflow containing a workspace:sync step must be refused",
        );
        assert!(
            matches!(err, RunWorkflowError::PauseWithWorkspaceSync),
            "expected PauseWithWorkspaceSync, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn resume_seed_preserves_role_alternation() {
        // NOTE 3 (mid-stream pause): a paused-incomplete step re-runs seeded
        // from its transcript. `run_agent` appends a fresh user turn only when
        // `user_message` is non-empty; the resume path seeds the FULL transcript
        // as-is with an EMPTY `user_message`, so no extra turn is appended and
        // the seed replays verbatim. This asserts the resumed request's messages
        // reconstruct the seed exactly and strictly alternate.
        let dir = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(WF_SOLO).unwrap();

        // Seed transcript ends in a USER message (a tool result → user turn),
        // the exact shape that would double-up without the fix.
        let seed = vec![
            Message::user("do work"),
            Message::assistant("let me check"),
            Message::user("tool result payload"),
        ];

        let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "final".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        let captured = provider.captured.clone();
        let factory = Arc::new(OneShotFactory::new(Box::new(provider)));
        let sink = Arc::new(CollectingSink::default());
        let mut opts = pause_opts(wf, dir.path().to_path_buf(), factory, sink);
        opts.resume_from = Some(ResumeState {
            run_id: String::new(),
            prior_step_results: Vec::new(),
            approved_step_id: String::new(),
            completed_units: std::collections::BTreeMap::new(),
            reason: PauseReason::Manual,
            paused_step: Some(PausedStep {
                step_id: "solo".into(),
                seed_messages: seed.clone(),
            }),
            rejected_reason: None,
        });

        run_workflow(opts).await.expect("resume completes");

        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 1, "resume issues exactly one fresh request");
        let msgs = &reqs[0].messages;
        assert_eq!(
            msgs.len(),
            seed.len(),
            "resumed request reconstructs the seed exactly (no extra turn)"
        );
        for pair in msgs.windows(2) {
            assert!(
                pair[0].role != pair[1].role,
                "messages must strictly alternate roles; got {:?}",
                msgs.iter().map(|m| &m.role).collect::<Vec<_>>()
            );
        }
        // Last turn is the replayed trailing user message.
        assert_eq!(msgs.last().unwrap().role, Role::User);
    }

    #[tokio::test]
    async fn resume_seed_preserves_tool_boundary_pairing() {
        // Tool-boundary pause: T2 lets a running tool finish, records its
        // `tool_result`, THEN pauses — so the seed transcript ends in a USER
        // message carrying a `ToolResult` block, preceded by an ASSISTANT
        // message whose `ToolUse` block it answers. The resume must replay this
        // pair INTACT: flattening the trailing `tool_result` to plain text (the
        // old behavior) would strip it and strand the assistant's `tool_use`
        // with no matching `tool_result` → real Anthropic returns 400
        // "tool_use ids without tool_result blocks". This asserts the
        // reconstructed request preserves the tool_use/tool_result pair, adds
        // no doubled user turn, and keeps valid role/tool pairing.
        let dir = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(WF_SOLO).unwrap();

        // Seed shape: user prompt → assistant(tool_use) → user(tool_result).
        let seed = vec![
            Message::user("do work"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "toolu_abc".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "README.md" }),
                }],
            },
            Message::tool_result("toolu_abc", "file contents here", false),
        ];

        let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "final".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        let captured = provider.captured.clone();
        let factory = Arc::new(OneShotFactory::new(Box::new(provider)));
        let sink = Arc::new(CollectingSink::default());
        let mut opts = pause_opts(wf, dir.path().to_path_buf(), factory, sink);
        opts.resume_from = Some(ResumeState {
            run_id: String::new(),
            prior_step_results: Vec::new(),
            approved_step_id: String::new(),
            completed_units: std::collections::BTreeMap::new(),
            reason: PauseReason::Manual,
            paused_step: Some(PausedStep {
                step_id: "solo".into(),
                seed_messages: seed.clone(),
            }),
            rejected_reason: None,
        });

        run_workflow(opts).await.expect("resume completes");

        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 1, "resume issues exactly one fresh request");
        let msgs = &reqs[0].messages;

        // No doubled user turn: the request is the seed verbatim.
        assert_eq!(
            msgs.len(),
            seed.len(),
            "resumed request reconstructs the seed exactly (no doubled user turn)"
        );

        // The trailing tool_result is preserved as a ToolResult block (NOT
        // flattened to plain text) and still references its tool_use id.
        let tool_result_id = msgs.last().unwrap().content.iter().find_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        });
        assert_eq!(
            tool_result_id.as_deref(),
            Some("toolu_abc"),
            "trailing tool_result must survive intact, got {:?}",
            msgs.last().unwrap().content
        );

        // The assistant tool_use it pairs with is still present — no dangling
        // tool_use. Every tool_use id must have a matching tool_result.
        let tool_use_ids: Vec<String> = msgs
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        let tool_result_ids: Vec<String> = msgs
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(tool_use_ids, vec!["toolu_abc".to_string()]);
        for id in &tool_use_ids {
            assert!(
                tool_result_ids.contains(id),
                "tool_use {id} has no matching tool_result (dangling tool_use)"
            );
        }

        // Role/tool pairing is valid: strict alternation holds.
        for pair in msgs.windows(2) {
            assert!(
                pair[0].role != pair[1].role,
                "messages must strictly alternate roles; got {:?}",
                msgs.iter().map(|m| &m.role).collect::<Vec<_>>()
            );
        }
    }

    // -----------------------------------------------------------------------
    // T6 — distributed fan-out pause/resume (mid-unit)
    // -----------------------------------------------------------------------

    const WF_FANOUT_DISTRIBUTE_PAUSE: &str = r#"
name: fanout-distribute-pause
steps:
  - id: process
    for_each: "a\nb\nc"
    agent: dummy
    prompt: "Process {{ item }}"
    max_parallel: 1
    distribute:
      hosts: [h1]
"#;

    /// Mirrors `resume_reruns_only_failed_fanout_units` (a real, disk-backed
    /// `RunStore`) but for a `distribute:` fan-out paused MID-FLIGHT rather
    /// than a unit that genuinely failed. `max_parallel: 1` plus
    /// `CancelAfterFirstDispatcher` (already used above for the step-boundary
    /// pause test) makes the ordering deterministic: the semaphore holds
    /// unit 1's permit until unit 0's ENTIRE dispatch — including the
    /// token cancellation — has returned, so unit 1 (and unit 2, behind it)
    /// always observe the pause as already triggered before they're ever
    /// dispatched.
    #[tokio::test]
    async fn distributed_fanout_pauses_mid_flight_and_resumes_only_incomplete_units() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::runs::RunStore::new(tmp.path().join("runs")));
        let wf = Workflow::parse(WF_FANOUT_DISTRIBUTE_PAUSE).unwrap();

        // --- Phase 1: pause mid-fan-out. ---
        let token = CancellationToken::new();
        let dispatcher1 = Arc::new(CancelAfterFirstDispatcher {
            token: token.clone(),
            calls: Mutex::new(Vec::new()),
        });
        let sink1 = Arc::new(CollectingSink::default());
        let opts1 = OrchestratorRunOpts {
            workflow: wf.clone(),
            inputs: BTreeMap::new(),
            workspace_id: "ws_fanout_pause".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().join("transcripts"),
            factory: Arc::new(PanicFactory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(WF_FANOUT_DISTRIBUTE_PAUSE.to_string()),
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: Some(sink1.clone()),
            unit_dispatcher: Some(dispatcher1.clone()),
            action_dispatcher: None,
            pause: Some(token),
        };

        let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
        let awaiting = res1.awaiting.expect("must pause mid-fan-out");
        assert_eq!(awaiting.reason, PauseReason::Manual);
        assert_eq!(awaiting.step_id, "process");
        assert!(
            res1.step_results.is_empty(),
            "the fan-out step did not complete"
        );
        assert!(sink1.labels().contains(&"RunPaused".to_string()));
        assert!(sink1.labels().contains(&"StepPaused".to_string()));

        // Only unit 0 ever reached the dispatcher — units 1 and 2 were
        // never started.
        assert_eq!(
            dispatcher1.calls.lock().unwrap().clone(),
            vec![(0, "h1".to_string())],
            "only the first unit reached the dispatcher; the rest were never dispatched"
        );

        // `AwaitingInfo` also carries the completed-unit map directly.
        assert_eq!(awaiting.fanout_completed_units.len(), 1);
        assert!(awaiting.fanout_completed_units.contains_key(&0));

        // The run record itself is durably `Paused`.
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        let run_id = listed[0].id.clone();
        assert_eq!(listed[0].status, crate::runs::RunStatus::Paused);

        // Exactly ONE unit checkpoint on disk — the completed unit. The
        // not-yet-started units are simply ABSENT (incomplete), not
        // recorded as failed.
        let checkpoints = store.read_unit_checkpoints(&run_id).unwrap();
        assert_eq!(
            checkpoints.len(),
            1,
            "only the completed unit is checkpointed"
        );
        assert_eq!(checkpoints[0].index, 0);
        assert!(checkpoints[0].success);

        // Build resume state the way `rupu workflow resume` does: successful
        // checkpoints replay from disk; everything else re-dispatches.
        let mut completed_units: BTreeMap<String, BTreeMap<usize, ItemResult>> = BTreeMap::new();
        for cp in checkpoints.iter().filter(|c| c.success) {
            completed_units
                .entry(cp.step_id.clone())
                .or_default()
                .insert(
                    cp.index,
                    ItemResult {
                        index: cp.index,
                        item: cp.item.clone(),
                        sub_id: String::new(),
                        rendered_prompt: String::new(),
                        run_id: cp.run_id.clone(),
                        transcript_path: cp.transcript_path.clone(),
                        output: cp.output.clone(),
                        success: true,
                    },
                );
        }

        // Flip the record back to Running (mirrors the CLI's `resume_run`).
        let mut record = store.load(&run_id).unwrap();
        record.status = crate::runs::RunStatus::Running;
        record.finished_at = None;
        store.update(&record).unwrap();

        // --- Phase 2: resume. ---
        let dispatcher2 = Arc::new(FakeUnitDispatcher::new());
        let sink2 = Arc::new(CollectingSink::default());
        let opts2 = OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: record.workspace_id.clone(),
            workspace_path: record.workspace_path.clone(),
            transcript_dir: record.transcript_dir.clone(),
            factory: Arc::new(PanicFactory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(WF_FANOUT_DISTRIBUTE_PAUSE.to_string()),
            resume_from: Some(ResumeState {
                run_id: run_id.clone(),
                prior_step_results: Vec::new(),
                approved_step_id: String::new(),
                completed_units,
                reason: PauseReason::Manual,
                paused_step: None,
                rejected_reason: None,
            }),
            run_id_override: None,
            strict_templates: false,
            event_sink: Some(sink2.clone()),
            unit_dispatcher: Some(dispatcher2.clone()),
            action_dispatcher: None,
            pause: None,
        };

        let res2 = run_workflow(opts2).await.expect("resume completes");
        assert!(res2.awaiting.is_none(), "resumed run runs to completion");
        assert_eq!(res2.step_results.len(), 1);
        let step = &res2.step_results[0];
        assert!(step.success);

        // Resume dispatched ONLY units 1 and 2 — unit 0 is NOT re-run.
        assert_eq!(
            dispatcher2.calls.lock().unwrap().clone(),
            vec![(1, "h1".to_string()), (2, "h1".to_string())],
            "resume must re-dispatch only the paused/not-yet-started units"
        );

        // All three items present, in declared order; unit 0's output is
        // the one preserved from the checkpoint (not re-dispatched).
        assert_eq!(step.items.len(), 3);
        assert_eq!(step.items[0].output, "out-0-on-h1");
        assert_eq!(step.items[1].output, "out-1-on-h1");
        assert_eq!(step.items[2].output, "out-2-on-h1");
        assert!(sink2.labels().contains(&"RunResumed".to_string()));
    }

    // A `for_each` (no `distribute:`) fan-out whose units run LOCALLY
    // through the real agent loop (T2's cooperative pause), rather than
    // through a `UnitDispatcher`. Item 0 answers immediately; item 1 (the
    // "SLOW" one) hangs on `BlockingProvider` until the pause token wins the
    // race inside `run_agent`'s `select!` (see `agent_run_pauses_and_resumes`
    // for the solo-step version of this same mechanism); item 2 must never
    // be dispatched at all. `block_slow` lets phase 2 hand the SAME item
    // ("SLOW-1") a normal, fast-completing provider instead.
    struct FanoutPauseFactory {
        block_slow: bool,
        seen: Mutex<Vec<String>>,
    }
    impl FanoutPauseFactory {
        fn new(block_slow: bool) -> Self {
            Self {
                block_slow,
                seen: Mutex::new(Vec::new()),
            }
        }
    }
    #[async_trait]
    impl StepFactory for FanoutPauseFactory {
        async fn build_opts_for_step(
            &self,
            _step_id: &str,
            agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: PathBuf,
            transcript_path: PathBuf,
            on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> AgentRunOpts {
            self.seen.lock().unwrap().push(rendered_prompt.clone());
            let provider: Box<dyn LlmProvider> =
                if self.block_slow && rendered_prompt.contains("SLOW") {
                    Box::new(BlockingProvider)
                } else {
                    Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText {
                        text: format!("done: {rendered_prompt}"),
                        stop: StopReason::EndTurn,
                        input_tokens: 1,
                        output_tokens: 1,
                    }]))
                };
            make_agent_opts(
                provider,
                agent_name,
                rendered_prompt,
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                on_tool_call,
            )
        }
    }

    const WF_FANOUT_LOCAL_PAUSE: &str = r#"
name: fanout-local-pause
steps:
  - id: process
    for_each: "ok-0\nSLOW-1\nok-2"
    agent: worker
    prompt: "{{ item }}"
    max_parallel: 1
"#;

    #[tokio::test]
    async fn fanout_pauses_mid_local_unit_and_resumes_only_incomplete_units() {
        let dir = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(WF_FANOUT_LOCAL_PAUSE).unwrap();

        // --- Phase 1: pause while unit 1 is mid-turn. ---
        let token = CancellationToken::new();
        let token2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            token2.cancel();
        });

        let sink1 = Arc::new(CollectingSink::default());
        let factory1 = Arc::new(FanoutPauseFactory::new(true));
        let mut opts1 = pause_opts(
            wf.clone(),
            dir.path().to_path_buf(),
            factory1.clone(),
            sink1.clone(),
        );
        opts1.pause = Some(token);

        let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
        let awaiting = res1.awaiting.expect("must pause mid-fan-out");
        assert_eq!(awaiting.reason, PauseReason::Manual);
        assert_eq!(awaiting.step_id, "process");
        assert!(
            res1.step_results.is_empty(),
            "the fan-out step did not complete"
        );
        assert!(sink1.labels().contains(&"RunPaused".to_string()));
        assert!(sink1.labels().contains(&"StepPaused".to_string()));

        // Unit 2 ("ok-2") was never dispatched — only units 0 and 1 were.
        assert_eq!(
            factory1.seen.lock().unwrap().clone(),
            vec!["ok-0".to_string(), "SLOW-1".to_string()],
            "the not-yet-started unit must never reach the factory"
        );

        // Only the succeeded unit (index 0) is carried forward.
        assert_eq!(awaiting.fanout_completed_units.len(), 1);
        assert!(awaiting.fanout_completed_units.contains_key(&0));

        // --- Phase 2: resume. ---
        let sink2 = Arc::new(CollectingSink::default());
        let factory2 = Arc::new(FanoutPauseFactory::new(false));
        let mut opts2 = pause_opts(
            wf,
            dir.path().to_path_buf(),
            factory2.clone(),
            sink2.clone(),
        );
        let mut completed_units = BTreeMap::new();
        completed_units.insert(
            "process".to_string(),
            awaiting.fanout_completed_units.clone(),
        );
        opts2.resume_from = Some(ResumeState {
            run_id: String::new(),
            prior_step_results: Vec::new(),
            approved_step_id: String::new(),
            completed_units,
            reason: PauseReason::Manual,
            paused_step: None,
            rejected_reason: None,
        });

        let res2 = run_workflow(opts2).await.expect("resume completes");
        assert!(res2.awaiting.is_none(), "resumed run runs to completion");
        assert_eq!(res2.step_results.len(), 1);
        let step = &res2.step_results[0];
        assert!(step.success);

        // Resume dispatched ONLY units 1 and 2 — unit 0 is NOT re-run.
        assert_eq!(
            factory2.seen.lock().unwrap().clone(),
            vec!["SLOW-1".to_string(), "ok-2".to_string()],
            "resume must re-dispatch only the paused/not-yet-started units"
        );
        assert_eq!(step.items.len(), 3);
        assert_eq!(step.items[0].output, "done: ok-0");
        assert_eq!(step.items[1].output, "done: SLOW-1");
        assert_eq!(step.items[2].output, "done: ok-2");
        assert!(sink2.labels().contains(&"RunResumed".to_string()));
    }

    // -----------------------------------------------------------------------
    // Phase 2 router (Task 5) — non-linear workflows now run for real
    // through the live scheduler instead of being rejected. This fixture
    // and test used to prove the honesty gate fired
    // (`run_workflow_rejects_a_split_workflow_before_any_execution`,
    // asserting `RunWorkflowError::NonlinearNotYetSupported`); flipped
    // here to prove the opposite — a real end-to-end execution.
    // -----------------------------------------------------------------------

    const WF_SPLIT: &str = r#"
name: nonlinear-split
steps:
  - id: s
    split: [a, b]
  - id: a
    agent: x
    prompt: p
    next: [d]
  - id: b
    agent: x
    prompt: p
    next: [d]
  - id: d
    agent: x
    prompt: "merged a={{ steps.a.output }} b={{ steps.b.output }}"
"#;

    #[tokio::test]
    async fn run_workflow_runs_a_split_join_workflow_live_through_the_scheduler() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::new());
        let wf = Workflow::parse(WF_SPLIT).unwrap();
        assert!(
            is_nonlinear(&wf),
            "fixture must be non-linear to exercise the live scheduler router"
        );
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);
        opts.factory = Arc::new(PerCallMockFactory);

        let res = run_workflow(opts).await.expect(
            "a split/join workflow must now run to completion through run_workflow, not be rejected",
        );
        assert!(
            res.awaiting.is_none(),
            "no gate in this workflow — must reach Completed, not park"
        );

        // The split node itself never dispatches (an orchestration
        // no-op); `a` and `b` both ran (concurrently — dispatch order
        // between them isn't asserted, only that both completed), and
        // `d` (the implicit all-join reconverge) sees BOTH of their real
        // outputs.
        //
        // Task 5b-2b-ii: the split node's own persisted `StepResult` must
        // carry `StepKind::Split`, not the reused `StepKind::Branch` — this
        // is the exact render-correctness defect this task fixes.
        let s = res
            .step_results
            .iter()
            .find(|sr| sr.step_id == "s")
            .expect("the split node itself has a persisted StepResult");
        assert_eq!(
            s.kind,
            crate::runs::StepKind::Split,
            "a live split node must persist kind: Split, not the reused Branch"
        );
        let a = res
            .step_results
            .iter()
            .find(|sr| sr.step_id == "a")
            .expect("a ran");
        let b = res
            .step_results
            .iter()
            .find(|sr| sr.step_id == "b")
            .expect("b ran");
        let d = res
            .step_results
            .iter()
            .find(|sr| sr.step_id == "d")
            .expect("d ran");
        assert!(!a.skipped && a.success);
        assert!(!b.skipped && b.success);
        assert!(!d.skipped && d.success);
        assert!(
            d.output.contains("a=done:p") && d.output.contains("b=done:p"),
            "d must see both a's and b's real output: {}",
            d.output
        );
    }

    /// Hands out a fresh `MockProvider` per call (unlike `OneShotFactory`,
    /// which panics on a second call) — needed here because the linear
    /// chain has two real (non-placed) agent steps, each dispatched
    /// through `build_opts_for_step`. Output is deterministic from the
    /// rendered prompt so downstream assertions can prove genuine
    /// sequential execution rather than a stubbed pass-through.
    struct PerCallMockFactory;
    #[async_trait]
    impl StepFactory for PerCallMockFactory {
        async fn build_opts_for_step(
            &self,
            _step_id: &str,
            agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: PathBuf,
            transcript_path: PathBuf,
            on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> AgentRunOpts {
            let provider = Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText {
                text: format!("done:{rendered_prompt}"),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]));
            make_agent_opts(
                provider,
                agent_name,
                rendered_prompt,
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                on_tool_call,
            )
        }
    }

    #[tokio::test]
    async fn run_workflow_still_runs_a_linear_chain_with_explicit_next() {
        // Same `next:`-bearing shape Task 1/2 introduced, but strictly
        // linear (no fork/reconverge/split/join) — must run exactly as
        // before, proving the gate doesn't over-fire on graph-schema use.
        let yaml = r#"
name: linear-with-next
steps:
  - id: a
    agent: x
    prompt: p
    next: [b]
  - id: b
    agent: x
    prompt: "then {{ steps.a.output }}"
"#;
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::new());
        let wf = Workflow::parse(yaml).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);
        opts.factory = Arc::new(PerCallMockFactory);

        let result = run_workflow(opts)
            .await
            .expect("linear chain with explicit next must run past the gate to completion");
        assert_eq!(result.step_results.len(), 2);
        assert_eq!(result.step_results[0].output, "done:p");
        assert_eq!(
            result.step_results[1].output, "done:then done:p",
            "step b ran with step a's real output, proving genuine sequential execution"
        );
    }

    /// Task 3: `loops:` execution is wired — the Task-2 honesty gate
    /// (`LoopRuntimeNotYetWired`) is gone; a workflow declaring `loops:`
    /// now runs to completion through the live scheduler exactly like any
    /// other `is_nonlinear` workflow. `until: "{{ steps.test.output }}"`
    /// is truthy on `PerCallMockFactory`'s very first (non-empty) output,
    /// so this exercises exactly one iteration end-to-end — the dedicated
    /// `bounded_loops` test module below covers multi-iteration
    /// convergence, feedback, `on_max`, and intra-iteration concurrency.
    #[tokio::test]
    async fn run_workflow_runs_a_loop_workflow_now_that_the_gate_is_removed() {
        let yaml = r#"
name: has-loop
steps:
  - id: gen
    agent: x
    prompt: p
  - id: test
    agent: x
    prompt: p
    depends_on: [gen]
loops:
  refine:
    nodes: [gen, test]
    until: "{{ steps.test.output }}"
    max_iterations: 3
"#;
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::new());
        let wf = Workflow::parse(yaml).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);
        opts.factory = Arc::new(PerCallMockFactory);

        let result = run_workflow(opts).await.expect(
            "a loop workflow must now run to completion through run_workflow, not be rejected",
        );
        assert!(result.awaiting.is_none());
        let gen = result
            .step_results
            .iter()
            .find(|sr| sr.step_id == "gen")
            .expect("gen ran");
        let test = result
            .step_results
            .iter()
            .find(|sr| sr.step_id == "test")
            .expect("test ran");
        assert!(gen.success && !gen.skipped);
        assert!(test.success && !test.skipped);
        assert_eq!(test.output, "done:p");

        // Phase 3: the loop super-node's own persisted `StepResult` must
        // carry `StepKind::Loop`, not the reused `StepKind::Split` — this
        // is the exact render-correctness defect this task fixes (mirrors
        // the split/join assertions in
        // `run_workflow_runs_a_split_join_workflow_live_through_the_scheduler`
        // / the join-gathering equivalent).
        let loop_node = result
            .step_results
            .iter()
            .find(|sr| sr.step_id == "loop:refine")
            .expect("the loop super-node itself has a persisted StepResult");
        assert_eq!(
            loop_node.kind,
            crate::runs::StepKind::Loop,
            "a live loop super-node must persist kind: Loop, not the reused Split"
        );
    }

    /// Loop execution persists real run-state exactly like any other
    /// workflow now that the gate (which used to fire before any run.json
    /// side effect) is removed: a fresh `RunRecord` is created and flips
    /// to `Completed`.
    #[tokio::test]
    async fn run_workflow_loop_persists_a_completed_run_record() {
        let yaml = r#"
name: has-loop
steps:
  - id: gen
    agent: x
    prompt: p
  - id: test
    agent: x
    prompt: p
    depends_on: [gen]
loops:
  refine:
    nodes: [gen, test]
    until: "{{ steps.test.output }}"
    max_iterations: 3
"#;
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::new());
        let wf = Workflow::parse(yaml).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);
        opts.factory = Arc::new(PerCallMockFactory);
        let runs_dir = dir.path().join("runs");
        let store = Arc::new(crate::runs::RunStore::new(runs_dir.clone()));
        opts.run_store = Some(store.clone());
        opts.workflow_yaml = Some(yaml.to_string());

        run_workflow(opts)
            .await
            .expect("loop workflow with a run-store must complete");
        let records = store.list().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, crate::runs::RunStatus::Completed);
    }
}

/// Phase-2 DAG-scheduler foundation (task 1 of the arc — see
/// `.superpowers/sdd/task-1-brief.md`): `run_scheduler` must reproduce
/// `run_steps_inner` byte-for-byte over every real `.rupu/workflows/*.yaml`
/// sample before anything downstream (concurrency, later tasks) builds on
/// it. A `#[cfg(test)]` module in THIS file, not `tests/`, specifically so
/// it can call `run_steps_inner` / `run_scheduler` / `InnerOutcome`
/// directly without making any of them part of the crate's public API —
/// both are private helpers `run_workflow`'s router (`is_nonlinear`)
/// chooses between (Task 5); this module keeps proving their equality at
/// that white-box level rather than only through `run_workflow`.
#[cfg(test)]
mod dag_scheduler_golden {
    use super::*;
    use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn, DEFAULT_MAX_TOKENS};
    use rupu_mcp::{McpPermission, ToolDispatcher};
    use rupu_providers::types::StopReason;
    use rupu_scm::{
        Branch, Comment, CreatePr, Diff, FileContent, Platform, Pr, PrFilter, PrRef, Registry,
        RepoConnector, RepoRef, ScmError,
    };
    use rupu_tools::{PermissionMode, ToolContext};

    /// Echoes `step {id} agent {agent} echo: {prompt}` for every agent
    /// dispatch, across every sample workflow — deterministic and
    /// content-agnostic (mirrors `FakeFactory` in `tests/linear_runner.rs`).
    /// A panel step's panelist output under this factory is plain prose,
    /// not `{"findings": [...]}` JSON — but `parse_findings` treats
    /// unparseable text as zero findings rather than an error (see that
    /// function above), so panel steps still complete (with an empty
    /// findings list, clearing any `gate:` threshold immediately) under
    /// this factory. No panel-specific scripting is needed for this
    /// golden set.
    struct EchoFactory;

    #[async_trait]
    impl StepFactory for EchoFactory {
        async fn build_opts_for_step(
            &self,
            step_id: &str,
            agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: PathBuf,
            transcript_path: PathBuf,
            on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> AgentRunOpts {
            let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
                text: format!("step {step_id} agent {agent_name} echo: {rendered_prompt}"),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]);
            AgentRunOpts {
                agent_name: format!("ag-{agent_name}"),
                agent_system_prompt: "echo".into(),
                agent_tools: None,
                provider: Box::new(provider),
                provider_name: "mock".into(),
                model: "mock-1".into(),
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                max_turns: 5,
                decider: Arc::new(BypassDecider),
                tool_context: ToolContext::default(),
                user_message: rendered_prompt,
                initial_messages: Vec::new(),
                turn_index_offset: 0,
                mode_str: "bypass".into(),
                no_stream: true,
                suppress_stream_stdout: true,
                mcp_registry: None,
                effort: None,
                context_window: None,
                output_format: None,
                output_schema: None,
                anthropic_task_budget: None,
                anthropic_context_management: None,
                anthropic_speed: None,
                parent_run_id: None,
                depth: 0,
                dispatchable_agents: None,
                step_id: step_id.to_string(),
                on_tool_call,
                on_stream_event: None,
                concerns: None,
                max_tokens: DEFAULT_MAX_TOKENS,
                scope_name: None,
                surface_tag: None,
                context_window_tokens: None,
                compact_at_percent: None,
                pause: None,
            }
        }
    }

    /// Backs the one `action:` kind our sample workflows use
    /// (`scm.prs.list` — a real top-level step in `action-demo.yaml`, and
    /// `gate-demo.yaml`'s best-effort `notify:` hook). Every other
    /// `RepoConnector` method is unreachable from these samples.
    struct FakeConnector;

    #[async_trait]
    impl RepoConnector for FakeConnector {
        fn platform(&self) -> Platform {
            Platform::Github
        }
        async fn list_repos(&self) -> Result<Vec<rupu_scm::Repo>, ScmError> {
            unimplemented!()
        }
        async fn get_repo(&self, _r: &RepoRef) -> Result<rupu_scm::Repo, ScmError> {
            unimplemented!()
        }
        async fn list_branches(&self, _r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
            unimplemented!()
        }
        async fn create_branch(
            &self,
            _r: &RepoRef,
            _name: &str,
            _from_sha: &str,
        ) -> Result<Branch, ScmError> {
            unimplemented!()
        }
        async fn read_file(
            &self,
            _r: &RepoRef,
            _path: &str,
            _ref_: Option<&str>,
        ) -> Result<FileContent, ScmError> {
            unimplemented!()
        }
        async fn list_prs(&self, _r: &RepoRef, _f: PrFilter) -> Result<Vec<Pr>, ScmError> {
            Ok(Vec::new())
        }
        async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> {
            unimplemented!()
        }
        async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> {
            unimplemented!()
        }
        async fn comment_pr(&self, _p: &PrRef, _body: &str) -> Result<Comment, ScmError> {
            unimplemented!()
        }
        async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> {
            unimplemented!()
        }
        async fn clone_to(&self, _r: &RepoRef, _dir: &Path) -> Result<(), ScmError> {
            unimplemented!()
        }
    }

    fn build_dispatcher() -> Arc<ToolDispatcher> {
        let mut reg = Registry::empty();
        reg.insert_repo_connector(Platform::Github, Arc::new(FakeConnector));
        Arc::new(ToolDispatcher::new(
            Arc::new(reg),
            McpPermission::new(PermissionMode::Bypass, vec!["*".into()]),
        ))
    }

    /// Minimal `(inputs, issue, event)` fixture each sample workflow needs
    /// to render without a `RenderError`/`UndeclaredInput` — derived by
    /// reading each file's `inputs:` schema and `issue.*`/`event.*`
    /// template references. Any file not listed needs none of the three.
    fn fixture_for(
        name: &str,
    ) -> (
        BTreeMap<String, String>,
        Option<serde_json::Value>,
        Option<serde_json::Value>,
    ) {
        match name {
            "code-review-panel.yaml" => (
                BTreeMap::from([("diff".to_string(), "+ x".to_string())]),
                None,
                None,
            ),
            "dispatch-demo.yaml" => (
                BTreeMap::from([("subject".to_string(), "src/main.rs".to_string())]),
                None,
                None,
            ),
            "investigate-then-fix.yaml" => (
                BTreeMap::from([("prompt".to_string(), "bug".to_string())]),
                None,
                None,
            ),
            "issue-supervisor-dispatch.yaml" => (
                BTreeMap::new(),
                Some(serde_json::json!({"number": 1, "ref": "github:acme/widget#1"})),
                None,
            ),
            "issue-to-spec-and-plan.yaml" => (
                BTreeMap::new(),
                Some(serde_json::json!({
                    "number": 1,
                    "r": {"project": "acme/widget"},
                    "title": "t",
                    "labels": ["bug"],
                    "body": "b",
                })),
                None,
            ),
            "issue-triage.yaml" => (
                BTreeMap::new(),
                Some(serde_json::json!({
                    "number": 1,
                    "ref": "github:acme/widget#1",
                    "title": "t",
                    "author": "alice",
                    "labels": ["bug"],
                    "body": "b",
                })),
                None,
            ),
            "phase-delivery-cycle.yaml" => (
                BTreeMap::from([("phase".to_string(), "phase-1".to_string())]),
                Some(serde_json::json!({"number": 1})),
                None,
            ),
            "pr-code-review.yaml" => (
                BTreeMap::new(),
                None,
                Some(serde_json::json!({
                    "pull_request": {"diff": "+x", "number": 1, "url": "u", "head_sha": "abc"},
                })),
            ),
            "quick-bugfix.yaml" => (
                BTreeMap::from([("prompt".to_string(), "bug".to_string())]),
                None,
                None,
            ),
            "review-changed-files.yaml" => (
                BTreeMap::from([("files".to_string(), "a.rs\nb.rs".to_string())]),
                None,
                None,
            ),
            _ => (BTreeMap::new(), None, None),
        }
    }

    /// One comparable row of a step's golden result: exactly the fields
    /// the task-1 brief calls for (id, output, success, skipped, kind), in
    /// declaration/dispatch order.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct GoldenStep {
        id: String,
        output: String,
        success: bool,
        skipped: bool,
        kind: crate::runs::StepKind,
    }

    impl From<&StepResult> for GoldenStep {
        fn from(sr: &StepResult) -> Self {
            GoldenStep {
                id: sr.step_id.clone(),
                output: sr.output.clone(),
                success: sr.success,
                skipped: sr.skipped,
                kind: sr.kind,
            }
        }
    }

    /// `InnerOutcome`'s discriminant plus the paused step id (when any) —
    /// enough to compare two runs' terminal state alongside their
    /// `step_results` snapshot, without needing the full pause payload
    /// (prompt / timeout / seed / fanout units) to be comparable.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum GoldenTerminal {
        Done,
        Paused(String),
    }

    /// Which of the two step-dispatch drivers `run_one` exercises.
    #[derive(Debug, Clone, Copy)]
    enum Driver {
        /// `run_steps_inner` — today's live path (plain declaration order).
        Loop,
        /// `run_scheduler` — the ready-set walk this task adds, proven
        /// (not yet wired as the live path).
        Scheduler,
    }

    /// Every sample workflow's yaml file, read once per call (files are
    /// tiny; simplicity over caching).
    fn sample_workflow_files() -> Vec<(String, String)> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.rupu/workflows");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let name = p.file_name().unwrap().to_str().unwrap().to_string();
            let raw = std::fs::read_to_string(&p).unwrap();
            out.push((name, raw));
        }
        out.sort();
        assert!(!out.is_empty(), "expected sample workflows under {dir}");
        out
    }

    /// Run one sample workflow through `driver`, returning its terminal
    /// state + `step_results` snapshot. In-memory only (`run_store: None`,
    /// `run_id: ""`) — persistence is irrelevant to this comparison and
    /// both helpers already no-op cleanly without a store.
    async fn run_one(driver: Driver, name: &str, raw: &str) -> (GoldenTerminal, Vec<GoldenStep>) {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(raw).unwrap_or_else(|e| panic!("{name}: parse failed: {e}"));
        let (inputs, issue, event) = fixture_for(name);
        let opts = OrchestratorRunOpts {
            workflow: wf,
            inputs,
            workspace_id: format!("ws_{name}"),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(EchoFactory),
            event,
            issue,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: Some(build_dispatcher()),
            pause: None,
        };
        let resolved_inputs = resolve_inputs(&opts.workflow, &opts.inputs)
            .unwrap_or_else(|e| panic!("{name}: resolve_inputs failed: {e}"));
        let workflow_default_continue = opts.workflow.defaults.continue_on_error.unwrap_or(false);
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = match driver {
            Driver::Loop => {
                run_steps_inner(
                    &opts,
                    "",
                    &resolved_inputs,
                    workflow_default_continue,
                    None,
                    &mut step_results,
                )
                .await
            }
            Driver::Scheduler => {
                run_scheduler(
                    &opts,
                    "",
                    &resolved_inputs,
                    workflow_default_continue,
                    None,
                    &mut step_results,
                    None,
                )
                .await
            }
        };

        let terminal = match outcome {
            Ok(InnerOutcome::Done) => GoldenTerminal::Done,
            Ok(InnerOutcome::Paused { step_id, .. }) => GoldenTerminal::Paused(step_id),
            Err(e) => panic!("{name}: run failed under {driver:?}: {e}"),
        };
        let snapshot = step_results.iter().map(GoldenStep::from).collect();
        (terminal, snapshot)
    }

    /// Task-1 brief, steps 1-2: capture `run_steps_inner`'s `step_results`
    /// (+ terminal state) for every sample workflow and prove the
    /// MockProvider harness is deterministic — the golden baseline the
    /// scheduler is proven against below stays meaningful only if running
    /// the SAME workflow twice yields the SAME snapshot.
    #[tokio::test]
    async fn loop_snapshot_is_deterministic_for_every_sample_workflow() {
        for (name, raw) in sample_workflow_files() {
            let a = run_one(Driver::Loop, &name, &raw).await;
            let b = run_one(Driver::Loop, &name, &raw).await;
            assert_eq!(a, b, "{name}: run_steps_inner must be deterministic");
        }
    }

    /// Task-1 brief, step 4: `run_scheduler`'s ready-set walk must
    /// reproduce `run_steps_inner`'s declaration-order walk byte-for-byte
    /// — same `step_results` (ids, outputs, success, skipped, kind, order)
    /// AND the same terminal state (`Done`, or `Paused` at the same step)
    /// — for every real `.rupu/workflows/*.yaml` sample.
    #[tokio::test]
    async fn scheduler_matches_loop_byte_for_byte_for_every_sample_workflow() {
        for (name, raw) in sample_workflow_files() {
            let golden = run_one(Driver::Loop, &name, &raw).await;
            let scheduled = run_one(Driver::Scheduler, &name, &raw).await;
            assert_eq!(
                golden, scheduled,
                "{name}: run_scheduler must reproduce run_steps_inner exactly"
            );
        }
    }

    /// Regression test (found in review): the synthesized declaration-order
    /// chain in `run_scheduler` must be gated on `is_nonlinear`, NOT
    /// `workflow_has_explicit_edges`. Every sample above is edge-free, so
    /// none of them exercise the explicit-edges branch at all — this
    /// fixture is `is_nonlinear == false` (the exact boundary
    /// `run_workflow`'s honesty gate uses) yet DOES carry a partial
    /// explicit edge (`a: next: [b]`) alongside an independent trailing
    /// step `c` with no edge to/from `a`/`b` whatsoever. Gating on
    /// `workflow_has_explicit_edges` (true here) would route this into the
    /// real-dependency-graph branch, where Kahn's algorithm sees `a` and
    /// `c` as simultaneously ready (`workflow_edges` only contains `a ->
    /// b`) and dispatches them concurrently — silently reordering
    /// `step_results` vs `run_steps_inner`'s deterministic `a, b, c` the
    /// moment a later task lifts the `is_nonlinear` gate and this becomes
    /// the live path. Confirmed to FAIL against the pre-fix
    /// `workflow_has_explicit_edges` gate (see this task's report) and
    /// PASS against the `is_nonlinear` gate below.
    #[tokio::test]
    async fn scheduler_matches_loop_for_partial_explicit_edges_with_independent_step() {
        const RAW: &str = r#"
name: partial-edges-plus-independent-step
steps:
  - id: a
    agent: ag
    prompt: "do a"
    next: [b]
  - id: b
    agent: ag
    prompt: "do b"
  - id: c
    agent: ag
    prompt: "do c"
"#;
        let wf = Workflow::parse(RAW).unwrap();
        assert!(
            !crate::workflow::is_nonlinear(&wf),
            "fixture must be is_nonlinear == false to exercise the gap this test guards"
        );
        assert!(
            crate::workflow::workflow_has_explicit_edges(&wf),
            "fixture must carry an explicit edge — the whole point is is_nonlinear and \
             workflow_has_explicit_edges disagree here"
        );

        let name = "partial-edges-plus-independent-step";
        let golden = run_one(Driver::Loop, name, RAW).await;
        let scheduled = run_one(Driver::Scheduler, name, RAW).await;
        assert_eq!(
            golden, scheduled,
            "run_scheduler must reproduce run_steps_inner exactly for a partial-explicit-edges \
             workflow that is_nonlinear still treats as linear"
        );
    }
}

/// Task 2: concurrent dispatch + `split` fan-out + implicit all-join +
/// `max_concurrency`. These exercise [`run_scheduler`] directly (same
/// reason `dag_scheduler_golden` does — it isn't `pub`). As of Task 5,
/// [`run_workflow`] also reaches it for any [`is_nonlinear`] workflow (see
/// `join_and_prune`'s trailing `..._live_through_run_workflow` tests for
/// coverage through that full public surface); these tests target the
/// scheduler's internals directly instead.
#[cfg(test)]
mod scheduler_concurrency {
    use super::*;
    use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn, DEFAULT_MAX_TOKENS};
    use rupu_providers::types::StopReason;
    use rupu_tools::ToolContext;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    const SPLIT_FANOUT_WF: &str = r#"
name: split-fanout
steps:
  - id: fanout
    split: [a, b]
  - id: a
    agent: worker
    prompt: "do a"
  - id: b
    agent: worker
    prompt: "do b"
  - id: d
    agent: worker
    prompt: "reconverge a={{ steps.a.output }} b={{ steps.b.output }}"
"#;

    /// One `StepFactory` covering every Task-2 test below:
    /// - `concurrency_tracked` steps increment/decrement `current` around an
    ///   `sleep_ms`-long `tokio::time::sleep`, with `max_seen` recording the
    ///   high-water mark — the concurrency-overlap proof (§ this task's
    ///   brief: "assert overlap ... via ... a shared timestamp"). A step
    ///   NOT in this list dispatches instantly.
    /// - `slow_step` independently makes ONE named step artificially slow
    ///   (used to keep a sibling reliably still in-flight while another
    ///   step fails, for the mid-graph-failure test).
    /// - `calls` records every step id actually dispatched through this
    ///   factory, in call order — proves a `split:` node (never dispatched
    ///   here) is a true orchestration no-op, and gives a coarse ordering
    ///   check (a reconverge step is always the LAST id appended).
    ///
    /// Deliberately has NO "make the agent turn itself fail" knob: `rupu-
    /// agent`'s `run_agent` retries a `ScriptedTurn::ProviderError` up to
    /// `MAX_HTTP_RETRIES` times with capped exponential backoff (it maps to
    /// `ProviderError::Http`, which `is_retryable_provider_error` always
    /// retries) — a real ~48s wall-clock cascade before giving up, the same
    /// cost `tests/linear_runner.rs`'s `FailingFactory`-based tests already
    /// pay. The mid-graph-failure test below instead fails via a template
    /// render error (`RunWorkflowError::Render`), which propagates
    /// immediately with no retry — same "hard failure, no
    /// `continue_on_error`" contract, without the unrelated retry-timing
    /// cost.
    struct SchedulerTestFactory {
        concurrency_tracked: &'static [&'static str],
        sleep_ms: u64,
        slow_step: Option<(&'static str, u64)>,
        current: Arc<AtomicUsize>,
        max_seen: Arc<AtomicUsize>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl SchedulerTestFactory {
        fn new() -> Self {
            Self {
                concurrency_tracked: &[],
                sleep_ms: 0,
                slow_step: None,
                current: Arc::new(AtomicUsize::new(0)),
                max_seen: Arc::new(AtomicUsize::new(0)),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn tracking(mut self, steps: &'static [&'static str], sleep_ms: u64) -> Self {
            self.concurrency_tracked = steps;
            self.sleep_ms = sleep_ms;
            self
        }

        fn slow(mut self, step: &'static str, ms: u64) -> Self {
            self.slow_step = Some((step, ms));
            self
        }
    }

    #[async_trait]
    impl StepFactory for SchedulerTestFactory {
        async fn build_opts_for_step(
            &self,
            step_id: &str,
            agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: PathBuf,
            transcript_path: PathBuf,
            on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> AgentRunOpts {
            self.calls.lock().unwrap().push(step_id.to_string());

            if self.concurrency_tracked.contains(&step_id) {
                let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_seen.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
                self.current.fetch_sub(1, Ordering::SeqCst);
            } else if let Some((slow_id, ms)) = self.slow_step {
                if slow_id == step_id {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
            }

            let turn = ScriptedTurn::AssistantText {
                text: format!("step {step_id} agent {agent_name} echo: {rendered_prompt}"),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            };
            let provider = MockProvider::new(vec![turn]);
            AgentRunOpts {
                agent_name: format!("ag-{agent_name}"),
                agent_system_prompt: "echo".into(),
                agent_tools: None,
                provider: Box::new(provider),
                provider_name: "mock".into(),
                model: "mock-1".into(),
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                max_turns: 5,
                decider: Arc::new(BypassDecider),
                tool_context: ToolContext::default(),
                user_message: rendered_prompt,
                initial_messages: Vec::new(),
                turn_index_offset: 0,
                mode_str: "bypass".into(),
                no_stream: true,
                suppress_stream_stdout: true,
                mcp_registry: None,
                effort: None,
                context_window: None,
                output_format: None,
                output_schema: None,
                anthropic_task_budget: None,
                anthropic_context_management: None,
                anthropic_speed: None,
                parent_run_id: None,
                depth: 0,
                dispatchable_agents: None,
                step_id: step_id.to_string(),
                on_tool_call,
                on_stream_event: None,
                concerns: None,
                max_tokens: DEFAULT_MAX_TOKENS,
                scope_name: None,
                surface_tag: None,
                context_window_tokens: None,
                compact_at_percent: None,
                pause: None,
            }
        }
    }

    fn opts_for(wf: Workflow, factory: SchedulerTestFactory, tmp: &tempfile::TempDir) -> OrchestratorRunOpts {
        OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_sched".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        }
    }

    /// (1) A `split:` node fans into two targets that dispatch and run
    /// CONCURRENTLY (both in flight at once — `max_seen` hits 2, which is
    /// only possible if `a`'s and `b`'s `sleep(50ms)` windows overlapped in
    /// real time), and the reconverge step (`d`, which reads BOTH
    /// `steps.a.output` and `steps.b.output`) runs exactly once, strictly
    /// after both are done — the implicit all-join. Wrapped in a
    /// `tokio::time::timeout` as a belt-and-suspenders: if the scheduler
    /// somehow serialized `a`/`b` there'd be no deadlock risk here (unlike
    /// a barrier-based proof), but a regression that stalls the scheduler
    /// entirely still fails loudly instead of hanging the test suite.
    #[tokio::test]
    async fn split_fans_into_concurrent_jobs_and_reconverge_runs_once_after_both() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(SPLIT_FANOUT_WF).unwrap();
        let factory = SchedulerTestFactory::new().tracking(&["a", "b"], 50);
        let max_seen = Arc::clone(&factory.max_seen);
        let calls = Arc::clone(&factory.calls);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");

        assert!(matches!(outcome, InnerOutcome::Done));
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            2,
            "a and b must have been in flight at the same time at least once"
        );

        let ids: std::collections::BTreeSet<&str> =
            step_results.iter().map(|sr| sr.step_id.as_str()).collect();
        assert_eq!(
            ids,
            std::collections::BTreeSet::from(["fanout", "a", "b", "d"]),
            "every node should have a StepResult"
        );
        for sr in &step_results {
            assert!(sr.success, "{}: expected success, got {sr:?}", sr.step_id);
        }

        // `d` (the reconverge) is dispatched strictly after both `a` and
        // `b` have fully completed — never merely after their `sleep`
        // resolves, but after `run_node` returns and the scheduler marks
        // them `done`.
        let calls = calls.lock().unwrap().clone();
        assert_eq!(
            calls.len(),
            3,
            "fanout must never be dispatched (orchestration no-op): {calls:?}"
        );
        assert_eq!(calls[2], "d", "reconverge must be the last dispatch: {calls:?}");
        let first_two: std::collections::BTreeSet<&str> =
            [calls[0].as_str(), calls[1].as_str()].into_iter().collect();
        assert_eq!(first_two, std::collections::BTreeSet::from(["a", "b"]));
    }

    /// (2) Split's `StepResult` itself: no agent dispatched, instant
    /// success, empty output — the orchestration-no-op contract.
    #[tokio::test]
    async fn split_node_is_orchestration_only_no_agent_dispatched() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(SPLIT_FANOUT_WF).unwrap();
        let factory = SchedulerTestFactory::new();
        let calls = Arc::clone(&factory.calls);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None)
            .await
            .expect("scheduler must not error");

        let fanout = step_results
            .iter()
            .find(|sr| sr.step_id == "fanout")
            .expect("split node should still have a StepResult");
        assert!(fanout.success);
        assert!(fanout.output.is_empty());
        assert!(
            !calls.lock().unwrap().contains(&"fanout".to_string()),
            "split must never reach the StepFactory (no agent to dispatch)"
        );
    }

    /// (3) `max_concurrency: 1` bounds the semaphore to one permit: `a` and
    /// `b` still both run (the run still completes, correctly), but never
    /// concurrently — `max_seen` never exceeds 1, even though both are
    /// simultaneously READY the instant the split completes.
    #[tokio::test]
    async fn max_concurrency_one_serializes_the_split_fanout_but_still_completes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut wf = Workflow::parse(SPLIT_FANOUT_WF).unwrap();
        wf.max_concurrency = Some(1);
        let factory = SchedulerTestFactory::new().tracking(&["a", "b"], 30);
        let max_seen = Arc::clone(&factory.max_seen);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");

        assert!(matches!(outcome, InnerOutcome::Done));
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            1,
            "max_concurrency: 1 must never let two nodes run at once"
        );
        let ids: std::collections::BTreeSet<&str> =
            step_results.iter().map(|sr| sr.step_id.as_str()).collect();
        assert_eq!(
            ids,
            std::collections::BTreeSet::from(["fanout", "a", "b", "d"]),
            "the run must still complete every node despite the cap"
        );
        for sr in &step_results {
            assert!(sr.success, "{}: expected success, got {sr:?}", sr.step_id);
        }
    }

    /// (4) `max_concurrency: 0` is rejected at parse time (mirrors
    /// `max_parallel`'s own >=1 validation) rather than silently meaning
    /// "unbounded" or "never run anything".
    #[test]
    fn max_concurrency_zero_is_rejected_at_parse_time() {
        let raw = SPLIT_FANOUT_WF.replacen(
            "name: split-fanout\n",
            "name: split-fanout\nmax_concurrency: 0\n",
            1,
        );
        let err = Workflow::parse(&raw).expect_err("max_concurrency: 0 must fail to parse");
        assert!(
            matches!(
                err,
                WorkflowParseError::InvalidMaxConcurrency { value: 0 }
            ),
            "unexpected error: {err:?}"
        );
    }

    /// Same shape as [`SPLIT_FANOUT_WF`], except `a`'s prompt references an
    /// input nobody ever provides (`inputs.missing`). Under
    /// `strict_templates: true` this is a `RunWorkflowError::Render` —
    /// synchronous, inside `run_node`, with no retry — the cleanest way to
    /// fail a node instantly without touching `rupu-agent`'s provider-
    /// error retry cascade (see [`SchedulerTestFactory`]'s doc comment).
    const SPLIT_FANOUT_WF_BAD_TEMPLATE: &str = r#"
name: split-fanout-bad-template
steps:
  - id: fanout
    split: [a, b]
  - id: a
    agent: worker
    prompt: "{{ inputs.missing }}"
  - id: b
    agent: worker
    prompt: "do b"
  - id: d
    agent: worker
    prompt: "reconverge a={{ steps.a.output }} b={{ steps.b.output }}"
"#;

    /// (5) A mid-graph failure (no `continue_on_error`) stops the run: the
    /// failing node's sibling (`b`, deliberately kept in flight via
    /// `slow(...)` well past `a`'s near-instant render failure) never
    /// reaches `step_results`, and the reconverge (`d`, which needs BOTH
    /// `a` and `b` done) never launches at all. Matches `run_steps_over`'s
    /// own contract: a hard failure propagates as `Err` before its own
    /// `StepResult` is ever pushed, so only the already-synchronously-
    /// resolved `split` node's result survives.
    #[tokio::test]
    async fn mid_graph_failure_without_continue_on_error_stops_the_run() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(SPLIT_FANOUT_WF_BAD_TEMPLATE).unwrap();
        let factory = SchedulerTestFactory::new().slow("b", 300);
        let mut opts = opts_for(wf, factory, &tmp);
        opts.strict_templates = true;
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let err = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect_err("a's render failure must abort the run");

        assert!(
            matches!(err, RunWorkflowError::Render { ref step, .. } if step == "a"),
            "unexpected error: {err}"
        );
        let ids: Vec<&str> = step_results.iter().map(|sr| sr.step_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["fanout"],
            "only the already-resolved split node should be persisted; got {ids:?}"
        );
    }

    /// `b depends_on: [a]; c depends_on: [a]` describes the IDENTICAL fork
    /// as `a next: [b, c]` (spec §1a: `depends_on` is `next`'s symmetric
    /// inverse) — both fixtures must route through `run_workflow` (the
    /// live router, not `run_scheduler` called directly — see the
    /// `..._live_through_run_workflow` tests below `join_and_prune` for the
    /// established pattern) to the concurrent scheduler and dispatch `b`/
    /// `c` CONCURRENTLY, exactly like the `next`-authored fork does. Before
    /// the fix, `is_nonlinear`'s fork check read only `s.next.len()` and
    /// never saw a fork authored purely via `depends_on`: the
    /// `depends_on`-fork fixture stayed `is_nonlinear == false` and ran
    /// through `run_steps_inner`, which dispatches strictly sequentially —
    /// a silent divergence from the promised symmetry.
    #[tokio::test]
    async fn depends_on_authored_fork_dispatches_concurrently_through_run_workflow_matching_next() {
        const DEPENDS_ON_FORK_WF: &str = r#"
name: depends-on-fork
steps:
  - id: a
    agent: worker
    prompt: "do a"
  - id: b
    agent: worker
    prompt: "do b"
    depends_on: [a]
  - id: c
    agent: worker
    prompt: "do c"
    depends_on: [a]
"#;
        const NEXT_FORK_WF: &str = r#"
name: next-fork
steps:
  - id: a
    agent: worker
    prompt: "do a"
    next: [b, c]
  - id: b
    agent: worker
    prompt: "do b"
  - id: c
    agent: worker
    prompt: "do c"
"#;

        let dep_wf = Workflow::parse(DEPENDS_ON_FORK_WF).unwrap();
        let next_wf = Workflow::parse(NEXT_FORK_WF).unwrap();
        assert!(
            is_nonlinear(&dep_wf),
            "depends_on-authored fork must be non-linear"
        );
        assert!(
            is_nonlinear(&next_wf),
            "next-authored fork must be non-linear"
        );

        for (label, wf) in [("depends_on", dep_wf), ("next", next_wf)] {
            let tmp = tempfile::tempdir().unwrap();
            let factory = SchedulerTestFactory::new().tracking(&["b", "c"], 50);
            let max_seen = Arc::clone(&factory.max_seen);
            let opts = opts_for(wf, factory, &tmp);

            let res = tokio::time::timeout(Duration::from_secs(5), run_workflow(opts))
                .await
                .unwrap_or_else(|_| panic!("[{label}] run_workflow must not hang"))
                .unwrap_or_else(|e| panic!("[{label}] run_workflow must not error: {e}"));

            assert!(
                res.awaiting.is_none(),
                "[{label}] no gate in this workflow — must reach Done, not park"
            );
            assert_eq!(
                max_seen.load(Ordering::SeqCst),
                2,
                "[{label}] b and c must have been in flight at the same time at least once"
            );
            let ids: std::collections::BTreeSet<&str> = res
                .step_results
                .iter()
                .map(|sr| sr.step_id.as_str())
                .collect();
            assert_eq!(
                ids,
                std::collections::BTreeSet::from(["a", "b", "c"]),
                "[{label}] every node should have run"
            );
            for sr in &res.step_results {
                assert!(
                    sr.success,
                    "[{label}] {}: expected success, got {sr:?}",
                    sr.step_id
                );
            }
        }
    }
}

/// Task 3: bounded subgraph loops (spec §2b-§2e) — the super-node +
/// recursive iteration driver. Exercises [`run_workflow`] directly (the
/// honesty gate is gone, so a loop workflow now runs for real through
/// this exact public surface).
#[cfg(test)]
mod bounded_loops {
    use super::*;
    use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn, DEFAULT_MAX_TOKENS};
    use rupu_providers::types::StopReason;
    use rupu_tools::ToolContext;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    const REFINE_WF: &str = r#"
name: refine
steps:
  - id: seed
    agent: seeder
    prompt: "seed"
  - id: gen
    agent: generator
    prompt: "attempt={{ loops.refine.iteration }} seed={{ steps.seed.output }} critique={{ steps.critique.output }}"
    depends_on: [seed]
  - id: test
    agent: tester
    prompt: "test {{ steps.gen.output }}"
    depends_on: [gen]
  - id: critique
    agent: critic
    prompt: "critique {{ steps.test.output }}"
    depends_on: [test]
  - id: ship
    agent: shipper
    prompt: "ship critique={{ steps.critique.output }} converged={{ loops.refine.converged }}"
loops:
  refine:
    nodes: [gen, test, critique]
    until: "{{ steps.critique.output }}"
    max_iterations: 5
"#;

    /// Drives every `bounded_loops` fixture. `critique` returns `"false"`
    /// for its first `false_calls` dispatches, then `"true"` on every
    /// dispatch after — the mock stand-in for "approved=false until
    /// iteration K, true at K" (spec's `until` is a plain minijinja
    /// expression over `steps.*`, so a literal `"true"`/`"false"` string
    /// output is the simplest truthy/falsy fixture). Every other step's
    /// output is a fixed, step-id-keyed marker so downstream prompts are
    /// deterministic. `calls` and `gen_prompts` capture full dispatch
    /// history for the convergence-count and feedback assertions.
    struct RefineFactory {
        false_calls: usize,
        critique_calls: Arc<AtomicUsize>,
        calls: Arc<Mutex<Vec<String>>>,
        gen_prompts: Arc<Mutex<Vec<String>>>,
    }

    impl RefineFactory {
        fn new(false_calls: usize) -> Self {
            Self {
                false_calls,
                critique_calls: Arc::new(AtomicUsize::new(0)),
                calls: Arc::new(Mutex::new(Vec::new())),
                gen_prompts: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl StepFactory for RefineFactory {
        async fn build_opts_for_step(
            &self,
            step_id: &str,
            agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: PathBuf,
            transcript_path: PathBuf,
            on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> AgentRunOpts {
            self.calls.lock().unwrap().push(step_id.to_string());
            if step_id == "gen" {
                self.gen_prompts
                    .lock()
                    .unwrap()
                    .push(rendered_prompt.clone());
            }
            let text = if step_id == "critique" {
                let n = self.critique_calls.fetch_add(1, Ordering::SeqCst);
                if n < self.false_calls {
                    "false".to_string()
                } else {
                    "true".to_string()
                }
            } else {
                format!("out-{step_id}")
            };
            let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
                text,
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]);
            AgentRunOpts {
                agent_name: format!("ag-{agent_name}"),
                agent_system_prompt: "echo".into(),
                agent_tools: None,
                provider: Box::new(provider),
                provider_name: "mock".into(),
                model: "mock-1".into(),
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                max_turns: 5,
                decider: Arc::new(BypassDecider),
                tool_context: ToolContext::default(),
                user_message: rendered_prompt,
                initial_messages: Vec::new(),
                turn_index_offset: 0,
                mode_str: "bypass".into(),
                no_stream: true,
                suppress_stream_stdout: true,
                mcp_registry: None,
                effort: None,
                context_window: None,
                output_format: None,
                output_schema: None,
                anthropic_task_budget: None,
                anthropic_context_management: None,
                anthropic_speed: None,
                parent_run_id: None,
                depth: 0,
                dispatchable_agents: None,
                step_id: step_id.to_string(),
                on_tool_call,
                on_stream_event: None,
                concerns: None,
                max_tokens: DEFAULT_MAX_TOKENS,
                scope_name: None,
                surface_tag: None,
                context_window_tokens: None,
                compact_at_percent: None,
                pause: None,
            }
        }
    }

    fn opts_for(
        wf: Workflow,
        factory: Arc<RefineFactory>,
        tmp: &tempfile::TempDir,
    ) -> OrchestratorRunOpts {
        OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_loop".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory,
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        }
    }

    /// `critique` is falsy for iterations 0 and 1, truthy at iteration 2
    /// (`false_calls: 2`) — exactly K+1 = 3 iterations. `ship` runs
    /// exactly once, after convergence, reading the converged `critique`
    /// output and `loops.refine.converged == true`.
    #[tokio::test]
    async fn refine_loop_converges_after_k_plus_one_iterations_then_ships_once() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(REFINE_WF).unwrap();
        let factory = Arc::new(RefineFactory::new(2));
        let calls = Arc::clone(&factory.calls);
        let opts = opts_for(wf, factory, &tmp);

        let result = tokio::time::timeout(Duration::from_secs(5), run_workflow(opts))
            .await
            .expect("run must not hang")
            .expect("refine loop must converge and complete");
        assert!(result.awaiting.is_none());

        let calls = calls.lock().unwrap().clone();
        let critique_dispatch_count = calls.iter().filter(|s| s.as_str() == "critique").count();
        assert_eq!(
            critique_dispatch_count, 3,
            "false,false,true = exactly 3 iterations, got {calls:?}"
        );
        let ship_count = calls.iter().filter(|s| s.as_str() == "ship").count();
        assert_eq!(ship_count, 1, "ship must run exactly once, got {calls:?}");
        // ship must come after every gen/test/critique dispatch (runs only
        // once the loop has fully converged, not mid-iteration).
        let ship_pos = calls.iter().position(|s| s == "ship").unwrap();
        assert_eq!(
            ship_pos,
            calls.len() - 1,
            "ship must be the last node dispatched: {calls:?}"
        );

        let ship_result = result
            .step_results
            .iter()
            .find(|sr| sr.step_id == "ship")
            .expect("ship ran");
        assert!(
            ship_result.rendered_prompt.contains("critique=true"),
            "ship must read the converged critique output: {}",
            ship_result.rendered_prompt
        );
        assert!(
            ship_result.rendered_prompt.contains("converged=true"),
            "ship must see loops.refine.converged == true: {}",
            ship_result.rendered_prompt
        );
    }

    const CONTROL_FREE_FORWARD_WF: &str = r#"
name: control-free-forward
steps:
  - id: gen
    agent: generator
    prompt: "gen"
  - id: test
    agent: tester
    prompt: "test sees {{ steps.gen.output }}"
loops:
  refine:
    nodes: [gen, test]
    until: "{{ steps.test.output }}"
    max_iterations: 2
"#;

    /// Post-review fix: `test` references `{{ steps.gen.output }}` with NO
    /// `depends_on`/`next` between `gen` and `test` — a pure DATA edge,
    /// no control edge at all. Spec §2d: a member->member data ref stays
    /// a real INTRA-ITERATION dependency unless it would close a cycle;
    /// `gen -> test` closes no cycle (there ISN'T even a control graph to
    /// close one against), so it must be scheduled exactly like
    /// `workflow_edges` schedules a data ref everywhere else in this
    /// crate — `test` must see THIS iteration's `gen` output, never an
    /// empty/prior-iteration read. Before the fix, `loop_forward_edges`
    /// only kept a data edge when its source was a CONTROL ancestor of
    /// the target; with zero control edges here that's vacuously false,
    /// so the edge was wrongly dropped and `gen`/`test` ran with no real
    /// ordering between them — `test`'s context snapshot (built
    /// synchronously the moment it's popped from `ready`, alongside
    /// `gen` rather than after it) would see `extra_context`'s
    /// prior-iteration value (empty on iteration 0) instead of `gen`'s
    /// real output. This test FAILS on that code (`test sees ` with an
    /// empty tail) and PASSES once the edge is kept forward.
    #[tokio::test]
    async fn control_free_forward_data_dependency_schedules_gen_before_test_same_iteration() {
        struct Factory {
            calls: Arc<Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl StepFactory for Factory {
            async fn build_opts_for_step(
                &self,
                step_id: &str,
                agent_name: &str,
                rendered_prompt: String,
                run_id: String,
                workspace_id: String,
                workspace_path: PathBuf,
                transcript_path: PathBuf,
                on_tool_call: Option<rupu_agent::OnToolCallCallback>,
            ) -> AgentRunOpts {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("{step_id}:{rendered_prompt}"));
                let text = if step_id == "gen" {
                    "genout".to_string()
                } else {
                    "true".to_string()
                };
                let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
                    text,
                    stop: StopReason::EndTurn,
                    input_tokens: 1,
                    output_tokens: 1,
                }]);
                AgentRunOpts {
                    agent_name: format!("ag-{agent_name}"),
                    agent_system_prompt: "echo".into(),
                    agent_tools: None,
                    provider: Box::new(provider),
                    provider_name: "mock".into(),
                    model: "mock-1".into(),
                    run_id,
                    workspace_id,
                    workspace_path,
                    transcript_path,
                    max_turns: 5,
                    decider: Arc::new(BypassDecider),
                    tool_context: ToolContext::default(),
                    user_message: rendered_prompt,
                    initial_messages: Vec::new(),
                    turn_index_offset: 0,
                    mode_str: "bypass".into(),
                    no_stream: true,
                    suppress_stream_stdout: true,
                    mcp_registry: None,
                    effort: None,
                    context_window: None,
                    output_format: None,
                    output_schema: None,
                    anthropic_task_budget: None,
                    anthropic_context_management: None,
                    anthropic_speed: None,
                    parent_run_id: None,
                    depth: 0,
                    dispatchable_agents: None,
                    step_id: step_id.to_string(),
                    on_tool_call,
                    on_stream_event: None,
                    concerns: None,
                    max_tokens: DEFAULT_MAX_TOKENS,
                    scope_name: None,
                    surface_tag: None,
                    context_window_tokens: None,
                    compact_at_percent: None,
                    pause: None,
                }
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(CONTROL_FREE_FORWARD_WF).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let factory = Factory {
            calls: Arc::clone(&calls),
        };
        let opts = OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_loop".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };

        tokio::time::timeout(Duration::from_secs(5), run_workflow(opts))
            .await
            .expect("run must not hang")
            .expect("a control-free forward data dependency must not deadlock or misorder");

        let calls = calls.lock().unwrap().clone();
        let test_call = calls
            .iter()
            .find(|c| c.starts_with("test:"))
            .expect("test dispatched");
        assert!(
            test_call.contains("test sees genout"),
            "test must read THIS iteration's real gen output via a genuine \
             scheduling edge (not an empty/prior-iteration feedback read): {test_call}"
        );
    }

    /// Unit-level proof of `loop_forward_edges`'s edge-selection rule
    /// directly (no MockProvider dispatch needed): among 3 members, a
    /// member->member data ref that does NOT close a cycle over the
    /// control+data DAG accepted so far stays forward; the one that DOES
    /// close a cycle is excluded as feedback.
    #[test]
    fn loop_forward_edges_keeps_non_cycle_closing_data_edges_and_drops_the_cycle_closer() {
        let yaml = r#"
name: three-member-mixed
steps:
  - id: gen
    agent: g
    prompt: gen
  - id: test
    agent: t
    prompt: "{{ steps.gen.output }}"
    depends_on: [gen]
  - id: critique
    agent: c
    prompt: "{{ steps.test.output }} {{ steps.gen.output }}"
    depends_on: [test]
loops:
  refine:
    nodes: [gen, test, critique]
    until: "{{ steps.critique.output }}"
    max_iterations: 2
"#;
        let wf = Workflow::parse(yaml).unwrap();
        let edges: std::collections::BTreeSet<(String, String)> =
            loop_forward_edges(&wf, "refine").into_iter().collect();
        assert!(edges.contains(&("gen".to_string(), "test".to_string())));
        assert!(edges.contains(&("test".to_string(), "critique".to_string())));
        // `critique` ALSO references `steps.gen` directly — a data edge
        // that does NOT close a cycle (`gen` is already an ancestor of
        // `critique` via `test`) — must stay forward, not be
        // misclassified as feedback.
        assert!(
            edges.contains(&("gen".to_string(), "critique".to_string())),
            "a non-cycle-closing data edge among 3+ members must stay forward: {edges:?}"
        );
    }

    /// The companion negative case: `gen` reads `{{ steps.critique.output
    /// }}` — adding `critique -> gen` to the control chain `gen -> test ->
    /// critique` would close a cycle, so it must be excluded (the loop's
    /// controlled feedback back-reference, spec §2d), while the two real
    /// control edges stay forward.
    #[test]
    fn loop_forward_edges_drops_the_cycle_closing_feedback_edge() {
        let yaml = r#"
name: three-member-feedback
steps:
  - id: gen
    agent: g
    prompt: "{{ steps.critique.output }}"
  - id: test
    agent: t
    prompt: test
    depends_on: [gen]
  - id: critique
    agent: c
    prompt: critique
    depends_on: [test]
loops:
  refine:
    nodes: [gen, test, critique]
    until: "{{ steps.critique.output }}"
    max_iterations: 2
"#;
        let wf = Workflow::parse(yaml).unwrap();
        let edges: std::collections::BTreeSet<(String, String)> =
            loop_forward_edges(&wf, "refine").into_iter().collect();
        assert!(edges.contains(&("gen".to_string(), "test".to_string())));
        assert!(edges.contains(&("test".to_string(), "critique".to_string())));
        assert!(
            !edges.contains(&("critique".to_string(), "gen".to_string())),
            "the cycle-closing data edge must be excluded as feedback: {edges:?}"
        );
    }

    /// `gen` references `{{ steps.critique.output }}` — NOT an ancestor of
    /// `gen` within the loop's control DAG (`gen -> test -> critique`), so
    /// it's the controlled feedback reference (spec §2d): iteration 0
    /// renders empty (nothing has run yet), and iteration N (N>=1) sees
    /// iteration (N-1)'s critique value — never a cycle, never the
    /// not-yet-run current-iteration value.
    #[tokio::test]
    async fn refine_loop_feedback_reads_prior_iteration_critique_not_current() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(REFINE_WF).unwrap();
        let factory = Arc::new(RefineFactory::new(2));
        let gen_prompts = Arc::clone(&factory.gen_prompts);
        let opts = opts_for(wf, factory, &tmp);

        tokio::time::timeout(Duration::from_secs(5), run_workflow(opts))
            .await
            .expect("run must not hang")
            .expect("refine loop must converge and complete");

        let prompts = gen_prompts.lock().unwrap().clone();
        assert_eq!(
            prompts.len(),
            3,
            "gen dispatched once per iteration: {prompts:?}"
        );
        assert!(
            prompts[0].contains("critique=")
                && !prompts[0].contains("critique=false")
                && !prompts[0].contains("critique=true"),
            "iteration 0's gen must see an EMPTY critique (nothing has run yet): {}",
            prompts[0]
        );
        assert!(
            prompts[1].contains("critique=false"),
            "iteration 1's gen must see iteration 0's critique value (false): {}",
            prompts[1]
        );
        assert!(
            prompts[2].contains("critique=false"),
            "iteration 2's gen must see iteration 1's critique value (false), not the not-yet-run current value: {}",
            prompts[2]
        );
        // The `loops.refine.iteration` surface (spec §2e), sanity-checked
        // alongside feedback since it's rendered in the same prompt.
        assert!(prompts[0].contains("attempt=0"));
        assert!(prompts[1].contains("attempt=1"));
        assert!(prompts[2].contains("attempt=2"));
    }

    const NEVER_CONVERGES_WF: &str = r#"
name: never-converges
steps:
  - id: gen
    agent: generator
    prompt: "gen"
  - id: test
    agent: tester
    prompt: "test"
    depends_on: [gen]
  - id: ship
    agent: shipper
    prompt: "ship converged={{ loops.refine.converged }}"
    depends_on: [test]
loops:
  refine:
    nodes: [gen, test]
    until: "{{ steps.test.output }}"
    max_iterations: 3
"#;

    const NEVER_CONVERGES_PROCEED_WF: &str = r#"
name: never-converges-proceed
steps:
  - id: gen
    agent: generator
    prompt: "gen"
  - id: test
    agent: tester
    prompt: "test"
    depends_on: [gen]
  - id: ship
    agent: shipper
    prompt: "ship converged={{ loops.refine.converged }}"
    depends_on: [test]
loops:
  refine:
    nodes: [gen, test]
    until: "{{ steps.test.output }}"
    max_iterations: 3
    on_max: proceed
"#;

    /// A factory whose `test` step ALWAYS renders falsy ("false") — the
    /// loop can never converge, so `on_max` (fail vs proceed) is what
    /// decides the run's fate.
    struct NeverConvergesFactory;

    #[async_trait]
    impl StepFactory for NeverConvergesFactory {
        async fn build_opts_for_step(
            &self,
            step_id: &str,
            agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: PathBuf,
            transcript_path: PathBuf,
            on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> AgentRunOpts {
            let text = if step_id == "test" { "false" } else { "out" };
            let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
                text: text.to_string(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]);
            AgentRunOpts {
                agent_name: format!("ag-{agent_name}"),
                agent_system_prompt: "echo".into(),
                agent_tools: None,
                provider: Box::new(provider),
                provider_name: "mock".into(),
                model: "mock-1".into(),
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                max_turns: 5,
                decider: Arc::new(BypassDecider),
                tool_context: ToolContext::default(),
                user_message: rendered_prompt,
                initial_messages: Vec::new(),
                turn_index_offset: 0,
                mode_str: "bypass".into(),
                no_stream: true,
                suppress_stream_stdout: true,
                mcp_registry: None,
                effort: None,
                context_window: None,
                output_format: None,
                output_schema: None,
                anthropic_task_budget: None,
                anthropic_context_management: None,
                anthropic_speed: None,
                parent_run_id: None,
                depth: 0,
                dispatchable_agents: None,
                step_id: step_id.to_string(),
                on_tool_call,
                on_stream_event: None,
                concerns: None,
                max_tokens: DEFAULT_MAX_TOKENS,
                scope_name: None,
                surface_tag: None,
                context_window_tokens: None,
                compact_at_percent: None,
                pause: None,
            }
        }
    }

    fn opts_for_never_converges(wf: Workflow, tmp: &tempfile::TempDir) -> OrchestratorRunOpts {
        OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_loop".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(NeverConvergesFactory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        }
    }

    /// `until` never holds; `on_max` defaults to `fail` — the run fails
    /// with `LoopExhausted { name, max_iterations }` and the documented
    /// message, and `ship` never runs.
    #[tokio::test]
    async fn on_max_fail_is_the_default_and_fails_the_run_loudly() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(NEVER_CONVERGES_WF).unwrap();
        let opts = opts_for_never_converges(wf, &tmp);

        let err = tokio::time::timeout(Duration::from_secs(5), run_workflow(opts))
            .await
            .expect("run must not hang")
            .expect_err("an unconverged loop with on_max: fail must fail the run");
        match &err {
            RunWorkflowError::LoopExhausted {
                name,
                max_iterations,
            } => {
                assert_eq!(name, "refine");
                assert_eq!(*max_iterations, 3);
            }
            other => panic!("expected LoopExhausted, got {other:?}"),
        }
        assert_eq!(
            err.to_string(),
            "loop 'refine' exhausted 3 iterations, until never held"
        );
    }

    /// `on_max: proceed` — the run continues to `ship` with the last
    /// iteration's outputs and `loops.refine.converged == false`.
    #[tokio::test]
    async fn on_max_proceed_continues_with_converged_false() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(NEVER_CONVERGES_PROCEED_WF).unwrap();
        let opts = opts_for_never_converges(wf, &tmp);

        let result = tokio::time::timeout(Duration::from_secs(5), run_workflow(opts))
            .await
            .expect("run must not hang")
            .expect("on_max: proceed must let the run complete");
        assert!(result.awaiting.is_none());
        let ship = result
            .step_results
            .iter()
            .find(|sr| sr.step_id == "ship")
            .expect("ship ran despite non-convergence");
        assert!(
            ship.rendered_prompt.contains("converged=false"),
            "ship must see loops.refine.converged == false: {}",
            ship.rendered_prompt
        );
    }

    const LOOP_WITH_SPLIT_JOIN_WF: &str = r#"
name: loop-split-join
steps:
  - id: fanout
    split: [a, b]
  - id: a
    agent: worker
    prompt: "do a"
    next: [j]
  - id: b
    agent: worker
    prompt: "do b"
    next: [j]
  - id: j
    join: { wait: all }
  - id: gate
    agent: gater
    prompt: "gate a={{ steps.a.output }} b={{ steps.b.output }}"
    depends_on: [j]
loops:
  refine:
    nodes: [fanout, a, b, j, gate]
    until: "{{ steps.gate.output }}"
    max_iterations: 2
"#;

    /// A loop containing `split -> [a,b] -> join` runs `a`/`b`
    /// concurrently WITHIN each iteration (the atomic high-water-mark
    /// proof from Phase-2's `scheduler_concurrency` module) — confirms
    /// `run_loop_node`'s recursive `run_scheduler_scoped` call keeps
    /// Phase-2 concurrency rather than degenerating to one-at-a-time.
    /// `gate` renders truthy on its first dispatch, so this converges
    /// after exactly one iteration — only intra-iteration concurrency is
    /// under test here.
    #[tokio::test]
    async fn loop_member_split_join_runs_concurrently_within_an_iteration() {
        struct ConcurrencyFactory {
            current: Arc<AtomicUsize>,
            max_seen: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl StepFactory for ConcurrencyFactory {
            async fn build_opts_for_step(
                &self,
                step_id: &str,
                agent_name: &str,
                rendered_prompt: String,
                run_id: String,
                workspace_id: String,
                workspace_path: PathBuf,
                transcript_path: PathBuf,
                on_tool_call: Option<rupu_agent::OnToolCallCallback>,
            ) -> AgentRunOpts {
                if step_id == "a" || step_id == "b" {
                    let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
                    self.max_seen.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    self.current.fetch_sub(1, Ordering::SeqCst);
                }
                let text = if step_id == "gate" {
                    "true".to_string()
                } else {
                    format!("out-{step_id}")
                };
                let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
                    text,
                    stop: StopReason::EndTurn,
                    input_tokens: 1,
                    output_tokens: 1,
                }]);
                AgentRunOpts {
                    agent_name: format!("ag-{agent_name}"),
                    agent_system_prompt: "echo".into(),
                    agent_tools: None,
                    provider: Box::new(provider),
                    provider_name: "mock".into(),
                    model: "mock-1".into(),
                    run_id,
                    workspace_id,
                    workspace_path,
                    transcript_path,
                    max_turns: 5,
                    decider: Arc::new(BypassDecider),
                    tool_context: ToolContext::default(),
                    user_message: rendered_prompt,
                    initial_messages: Vec::new(),
                    turn_index_offset: 0,
                    mode_str: "bypass".into(),
                    no_stream: true,
                    suppress_stream_stdout: true,
                    mcp_registry: None,
                    effort: None,
                    context_window: None,
                    output_format: None,
                    output_schema: None,
                    anthropic_task_budget: None,
                    anthropic_context_management: None,
                    anthropic_speed: None,
                    parent_run_id: None,
                    depth: 0,
                    dispatchable_agents: None,
                    step_id: step_id.to_string(),
                    on_tool_call,
                    on_stream_event: None,
                    concerns: None,
                    max_tokens: DEFAULT_MAX_TOKENS,
                    scope_name: None,
                    surface_tag: None,
                    context_window_tokens: None,
                    compact_at_percent: None,
                    pause: None,
                }
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(LOOP_WITH_SPLIT_JOIN_WF).unwrap();
        let factory = ConcurrencyFactory {
            current: Arc::new(AtomicUsize::new(0)),
            max_seen: Arc::new(AtomicUsize::new(0)),
        };
        let max_seen = Arc::clone(&factory.max_seen);
        let opts = OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_loop".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };

        let result = tokio::time::timeout(Duration::from_secs(5), run_workflow(opts))
            .await
            .expect("run must not hang")
            .expect("loop with an internal split/join must complete");
        assert!(result.awaiting.is_none());
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            2,
            "a and b must have overlapped in flight within the same iteration"
        );
    }
}

/// Task 4 (spec §3): loop resume / checkpoint. Builds on `bounded_loops`'
/// `REFINE_WF` fixture but drives real on-disk `RunRecord`/
/// `step_results.jsonl` persistence (`run_store: Some(..)`) since
/// `run_loop_node`'s re-entry point (`RunRecord.loop_progress`) is,
/// deliberately, only reconstructible from disk — see
/// `load_loop_start_iteration`'s doc.
#[cfg(test)]
mod loop_resume {
    use super::*;
    use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn, DEFAULT_MAX_TOKENS};
    use rupu_providers::types::StopReason;
    use rupu_tools::ToolContext;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    const REFINE_WF: &str = r#"
name: refine
steps:
  - id: seed
    agent: seeder
    prompt: "seed"
  - id: gen
    agent: generator
    prompt: "attempt={{ loops.refine.iteration }} seed={{ steps.seed.output }} critique={{ steps.critique.output }}"
    depends_on: [seed]
  - id: test
    agent: tester
    prompt: "test {{ steps.gen.output }}"
    depends_on: [gen]
  - id: critique
    agent: critic
    prompt: "critique {{ steps.test.output }}"
    depends_on: [test]
  - id: ship
    agent: shipper
    prompt: "ship critique={{ steps.critique.output }} converged={{ loops.refine.converged }}"
loops:
  refine:
    nodes: [gen, test, critique]
    until: "{{ steps.critique.output }}"
    max_iterations: 5
"#;

    /// Flexible refine-loop mock factory covering every test in this
    /// module: `critique` answers `"false"` for its first `false_calls`
    /// dispatches (counted from `critique_call_offset`, so a restart's
    /// factory can continue the SAME global call sequence a prior
    /// phase started), then `"true"`. Optionally sleeps before
    /// returning for `critique` (`critique_delay_ms`) so a hard
    /// whole-run cancel has a real in-flight task to abort.
    struct ResumableRefineFactory {
        false_calls: usize,
        critique_call_offset: usize,
        critique_delay_ms: Option<u64>,
        critique_calls: Arc<AtomicUsize>,
        test_calls: Arc<AtomicUsize>,
        calls: Arc<Mutex<Vec<String>>>,
        gen_prompts: Arc<Mutex<Vec<String>>>,
    }

    impl ResumableRefineFactory {
        fn new(false_calls: usize) -> Self {
            Self {
                false_calls,
                critique_call_offset: 0,
                critique_delay_ms: None,
                critique_calls: Arc::new(AtomicUsize::new(0)),
                test_calls: Arc::new(AtomicUsize::new(0)),
                calls: Arc::new(Mutex::new(Vec::new())),
                gen_prompts: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn critique_call_offset(mut self, n: usize) -> Self {
            self.critique_call_offset = n;
            self
        }

        fn critique_delay_ms(mut self, ms: u64) -> Self {
            self.critique_delay_ms = Some(ms);
            self
        }
    }

    #[async_trait]
    impl StepFactory for ResumableRefineFactory {
        async fn build_opts_for_step(
            &self,
            step_id: &str,
            agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: PathBuf,
            transcript_path: PathBuf,
            on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> AgentRunOpts {
            self.calls.lock().unwrap().push(step_id.to_string());
            if step_id == "gen" {
                self.gen_prompts
                    .lock()
                    .unwrap()
                    .push(rendered_prompt.clone());
            }
            if step_id == "test" {
                let _ = self.test_calls.fetch_add(1, Ordering::SeqCst);
            }
            let text = if step_id == "critique" {
                if let Some(ms) = self.critique_delay_ms {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
                let n = self.critique_call_offset
                    + self.critique_calls.fetch_add(1, Ordering::SeqCst);
                if n < self.false_calls {
                    "false".to_string()
                } else {
                    "true".to_string()
                }
            } else {
                format!("out-{step_id}")
            };
            let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
                text,
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]);
            AgentRunOpts {
                agent_name: format!("ag-{agent_name}"),
                agent_system_prompt: "echo".into(),
                agent_tools: None,
                provider: Box::new(provider),
                provider_name: "mock".into(),
                model: "mock-1".into(),
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                max_turns: 5,
                decider: Arc::new(BypassDecider),
                tool_context: ToolContext::default(),
                user_message: rendered_prompt,
                initial_messages: Vec::new(),
                turn_index_offset: 0,
                mode_str: "bypass".into(),
                no_stream: true,
                suppress_stream_stdout: true,
                mcp_registry: None,
                effort: None,
                context_window: None,
                output_format: None,
                output_schema: None,
                anthropic_task_budget: None,
                anthropic_context_management: None,
                anthropic_speed: None,
                parent_run_id: None,
                depth: 0,
                dispatchable_agents: None,
                step_id: step_id.to_string(),
                on_tool_call,
                on_stream_event: None,
                concerns: None,
                max_tokens: DEFAULT_MAX_TOKENS,
                scope_name: None,
                surface_tag: None,
                context_window_tokens: None,
                compact_at_percent: None,
                pause: None,
            }
        }
    }

    fn store_opts(
        wf: Workflow,
        factory: ResumableRefineFactory,
        tmp: &tempfile::TempDir,
        store: Arc<crate::runs::RunStore>,
        pause: Option<CancellationToken>,
        resume_from: Option<ResumeState>,
    ) -> OrchestratorRunOpts {
        OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_loop_resume".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(store),
            workflow_yaml: Some(REFINE_WF.to_string()),
            resume_from,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause,
        }
    }

    fn count(calls: &[String], id: &str) -> usize {
        calls.iter().filter(|s| s.as_str() == id).count()
    }

    /// Runs `REFINE_WF` to full natural convergence (no pause) and
    /// returns `(run_id, full step_results)`. Used as the deterministic
    /// starting point for both resume tests below: rather than racing a
    /// live cooperative pause against a fully-synchronous `MockProvider`
    /// (no real dispatch latency exists to land a pause boundary
    /// exactly between two specific member dispatches), each test
    /// TRUNCATES this known-good, fully-tagged history back to "as if
    /// paused at iteration N" and manually sets the `RunRecord.
    /// loop_progress` checkpoint a real pause would have left — exactly
    /// the plan's own Step 1 wording ("simulate a pause... persist
    /// those + set `loop_progress`"). This is Phase 2's own resume-test
    /// convention too (`resume_and_cancel`'s tests hand-seed
    /// `step_results` rather than drive a live pause for the same
    /// reason).
    async fn run_refine_to_convergence(
        wf: &Workflow,
        tmp: &tempfile::TempDir,
        store: &Arc<crate::runs::RunStore>,
        false_calls: usize,
    ) -> (String, Vec<StepResult>) {
        let factory = ResumableRefineFactory::new(false_calls);
        let opts = store_opts(wf.clone(), factory, tmp, Arc::clone(store), None, None);
        let res = tokio::time::timeout(Duration::from_secs(5), run_workflow(opts))
            .await
            .expect("full run must not hang")
            .expect("full run must converge and complete");
        assert!(res.awaiting.is_none());
        (res.run_id, res.step_results)
    }

    /// Plan Task 4 Step 1 / spec §3: pause mid-loop (iteration 2, `gen`
    /// + `test` done, `critique` not), resume with a fresh scheduler
    /// over the same run dir. Must re-enter iteration 2, re-dispatch
    /// ONLY `critique`, then — since the global critique sequence isn't
    /// exhausted yet — continue into iteration 3, whose `gen` must see
    /// iteration 2's critique via the feedback mechanic (not empty, not
    /// its own not-yet-run value), converge, and run `ship` exactly
    /// once.
    #[tokio::test]
    async fn resume_mid_loop_reenters_and_reruns_only_not_done_members() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::runs::RunStore::new(tmp.path().join("runs")));
        let wf = Workflow::parse(REFINE_WF).unwrap();

        // Converges at global critique call #3 (0-indexed) — false at
        // calls 0,1,2 (iterations 0,1,2), true at call 3 (iteration 3).
        let (run_id, full_results) = run_refine_to_convergence(&wf, &tmp, &store, 3).await;

        let gen_at_2 = full_results
            .iter()
            .find(|sr| sr.step_id == "gen" && sr.loop_iteration == Some(2))
            .cloned()
            .expect("gen@iteration2 must be persisted with its iteration tag");
        assert!(gen_at_2.rendered_prompt.contains("attempt=2") && gen_at_2.rendered_prompt.contains("critique=false"));
        // Every non-loop step (seed) keeps `loop_iteration` absent.
        let seed = full_results.iter().find(|sr| sr.step_id == "seed").unwrap();
        assert_eq!(seed.loop_iteration, None);

        // --- Simulate "paused mid-iteration-2, critique not yet run":
        // truncate to seed + iterations 0,1 (full) + iteration 2's
        // `gen`/`test` only (drop iteration 2's `critique` onward, the
        // loop's own synthetic result, and `ship`); checkpoint iteration
        // 2 as in-flight — exactly what `run_loop_node` would have
        // persisted the instant it started iteration 2, before
        // dispatching any of its members.
        let truncated: Vec<StepResult> = full_results
            .iter()
            .filter(|sr| match sr.loop_iteration {
                Some(it) => it < 2 || (it == 2 && (sr.step_id == "gen" || sr.step_id == "test")),
                None => sr.step_id != "loop:refine" && sr.step_id != "ship",
            })
            .cloned()
            .collect();
        assert_eq!(count_ids(&truncated, "gen"), 3, "seed's siblings gen@0,1,2");
        assert_eq!(count_ids(&truncated, "test"), 3);
        assert_eq!(count_ids(&truncated, "critique"), 2, "only iterations 0,1");
        let mut rec = store.load(&run_id).unwrap();
        rec.loop_progress.insert("refine".to_string(), 2);
        store.update(&rec).unwrap();

        // --- Resume. Only `critique` of iteration 2 must be
        // re-dispatched (not gen/test again); the global critique
        // sequence continues from call #2 (0-indexed) — still false —
        // so the loop proceeds into iteration 3, whose `gen` must see
        // iteration 2's critique ("false") via the feedback mechanic,
        // then converges (call #3 = true) and ships once.
        let factory2 = ResumableRefineFactory::new(3).critique_call_offset(2);
        let calls2 = Arc::clone(&factory2.calls);
        let gen_prompts2 = Arc::clone(&factory2.gen_prompts);
        let opts2 = store_opts(
            wf,
            factory2,
            &tmp,
            Arc::clone(&store),
            None,
            Some(ResumeState {
                run_id: run_id.clone(),
                prior_step_results: truncated,
                approved_step_id: String::new(),
                completed_units: std::collections::BTreeMap::new(),
                reason: PauseReason::Manual,
                paused_step: None,
                rejected_reason: None,
            }),
        );

        let res2 = tokio::time::timeout(Duration::from_secs(5), run_workflow(opts2))
            .await
            .expect("resume must not hang")
            .expect("resume must converge and complete — Ok, never LoopIterationPaused/any error");
        assert!(res2.awaiting.is_none(), "run must complete after resume");

        let calls2 = calls2.lock().unwrap().clone();
        assert_eq!(
            count(&calls2, "gen"),
            1,
            "only iteration 3's gen must run on resume (iteration 2's gen is done): {calls2:?}"
        );
        assert_eq!(
            count(&calls2, "test"),
            1,
            "only iteration 3's test must run on resume: {calls2:?}"
        );
        assert_eq!(
            count(&calls2, "critique"),
            2,
            "iteration 2's critique (re-dispatched) + iteration 3's critique: {calls2:?}"
        );
        assert_eq!(count(&calls2, "ship"), 1, "ship exactly once: {calls2:?}");

        // Feedback reconstruction across the resumed iteration boundary:
        // iteration 3's gen prompt must show iteration 2's real critique
        // value ("false"), not empty and not a live/cyclic read.
        let gen_prompts2 = gen_prompts2.lock().unwrap().clone();
        assert_eq!(gen_prompts2.len(), 1);
        assert!(
            gen_prompts2[0].contains("attempt=3") && gen_prompts2[0].contains("critique=false"),
            "iteration 3's gen must see iteration 2's critique output: {gen_prompts2:?}"
        );

        let ship_result = res2
            .step_results
            .iter()
            .find(|sr| sr.step_id == "ship")
            .expect("ship ran");
        assert!(ship_result.rendered_prompt.contains("critique=true"));
        assert!(ship_result.rendered_prompt.contains("converged=true"));
    }

    fn count_ids(results: &[StepResult], id: &str) -> usize {
        results.iter().filter(|sr| sr.step_id == id).count()
    }

    /// Plan Task 4 Step 3 / spec §3 requirement #4: a loop that already
    /// converged (its members + the loop's own synthetic result
    /// recorded, but its successor `ship` not yet run) must NOT be
    /// re-entered on resume — `ship` runs exactly once, and no loop
    /// member is dispatched again — REGARDLESS of what `loop_progress`
    /// still says (deliberately left pointing at iteration 0 here, to
    /// prove the converged super-node result is what actually gates
    /// re-entry, not the counter alone).
    #[tokio::test]
    async fn converged_loop_not_rerun_on_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::runs::RunStore::new(tmp.path().join("runs")));
        let wf = Workflow::parse(REFINE_WF).unwrap();

        // Converges at iteration 1 (false at call 0, true at call 1).
        let (run_id, full_results) = run_refine_to_convergence(&wf, &tmp, &store, 1).await;
        assert!(
            full_results.iter().any(|sr| sr.step_id == "loop:refine"),
            "the loop's own super-node result must be persisted once it converges"
        );
        assert_eq!(count_ids(&full_results, "ship"), 1);

        // --- Simulate "paused right before ship": truncate to
        // everything except `ship` (the loop's own converged result
        // stays).
        let truncated: Vec<StepResult> = full_results
            .iter()
            .filter(|sr| sr.step_id != "ship")
            .cloned()
            .collect();

        // --- Resume. A factory that panics if ANY loop member is
        // dispatched again — only `ship` may run.
        struct PanicIfLoopMemberDispatched;
        #[async_trait]
        impl StepFactory for PanicIfLoopMemberDispatched {
            async fn build_opts_for_step(
                &self,
                step_id: &str,
                agent_name: &str,
                rendered_prompt: String,
                run_id: String,
                workspace_id: String,
                workspace_path: PathBuf,
                transcript_path: PathBuf,
                on_tool_call: Option<rupu_agent::OnToolCallCallback>,
            ) -> AgentRunOpts {
                assert_ne!(step_id, "gen", "a converged loop must not re-dispatch gen");
                assert_ne!(step_id, "test", "a converged loop must not re-dispatch test");
                assert_ne!(
                    step_id, "critique",
                    "a converged loop must not re-dispatch critique"
                );
                let text = format!("out-{step_id}");
                let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
                    text,
                    stop: StopReason::EndTurn,
                    input_tokens: 1,
                    output_tokens: 1,
                }]);
                AgentRunOpts {
                    agent_name: format!("ag-{agent_name}"),
                    agent_system_prompt: "echo".into(),
                    agent_tools: None,
                    provider: Box::new(provider),
                    provider_name: "mock".into(),
                    model: "mock-1".into(),
                    run_id,
                    workspace_id,
                    workspace_path,
                    transcript_path,
                    max_turns: 5,
                    decider: Arc::new(BypassDecider),
                    tool_context: ToolContext::default(),
                    user_message: rendered_prompt,
                    initial_messages: Vec::new(),
                    turn_index_offset: 0,
                    mode_str: "bypass".into(),
                    no_stream: true,
                    suppress_stream_stdout: true,
                    mcp_registry: None,
                    effort: None,
                    context_window: None,
                    output_format: None,
                    output_schema: None,
                    anthropic_task_budget: None,
                    anthropic_context_management: None,
                    anthropic_speed: None,
                    parent_run_id: None,
                    depth: 0,
                    dispatchable_agents: None,
                    step_id: step_id.to_string(),
                    on_tool_call,
                    on_stream_event: None,
                    concerns: None,
                    max_tokens: DEFAULT_MAX_TOKENS,
                    scope_name: None,
                    surface_tag: None,
                    context_window_tokens: None,
                    compact_at_percent: None,
                    pause: None,
                }
            }
        }
        let opts2 = OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_loop_resume".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(PanicIfLoopMemberDispatched),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(REFINE_WF.to_string()),
            resume_from: Some(ResumeState {
                run_id: run_id.clone(),
                prior_step_results: truncated,
                approved_step_id: String::new(),
                completed_units: std::collections::BTreeMap::new(),
                reason: PauseReason::Manual,
                paused_step: None,
                rejected_reason: None,
            }),
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };

        let res2 = tokio::time::timeout(Duration::from_secs(5), run_workflow(opts2))
            .await
            .expect("resume must not hang")
            .expect("resume must complete — the loop is converged, not re-entered");
        assert!(res2.awaiting.is_none());
        let ship_count = res2
            .step_results
            .iter()
            .filter(|sr| sr.step_id == "ship")
            .count();
        assert_eq!(ship_count, 1, "ship must run exactly once");
    }

    const GATED_LOOP_WF: &str = r#"
name: gated-loop
steps:
  - id: seed
    agent: seeder
    prompt: "seed"
  - id: work
    agent: worker
    prompt: "work {{ loops.refine.iteration }}"
    depends_on: [seed]
  - id: review
    approval:
      prompt: "approve iteration {{ loops.refine.iteration }}?"
    depends_on: [work]
  - id: ship
    agent: shipper
    prompt: "ship"
loops:
  refine:
    nodes: [work, review]
    until: "{{ steps.review.output }}"
    max_iterations: 3
"#;

    /// Plan Task 4 Step 5 / spec §3 requirement #5: a REAL, live pause
    /// landing mid-iteration — here, a standalone `approval:` gate that
    /// is itself a loop member — must surface as a normal, resumable
    /// `InnerOutcome::Paused` (reason `Approval`, `gates` populated),
    /// NEVER as an error (the fail-loud `LoopIterationPaused`
    /// placeholder Task 3 shipped, now removed). Deterministic (no
    /// timing race): an approval gate always parks on its first reach.
    /// Approving + resuming must converge the loop (an approval gate's
    /// own output is a non-empty decision JSON, so `until` holds right
    /// after) and run `ship` exactly once.
    #[tokio::test]
    async fn live_approval_gate_pause_inside_loop_is_resumable_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::runs::RunStore::new(tmp.path().join("runs")));
        let wf = Workflow::parse(GATED_LOOP_WF).unwrap();

        struct SimpleFactory;
        #[async_trait]
        impl StepFactory for SimpleFactory {
            async fn build_opts_for_step(
                &self,
                step_id: &str,
                agent_name: &str,
                rendered_prompt: String,
                run_id: String,
                workspace_id: String,
                workspace_path: PathBuf,
                transcript_path: PathBuf,
                on_tool_call: Option<rupu_agent::OnToolCallCallback>,
            ) -> AgentRunOpts {
                let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
                    text: format!("out-{step_id}"),
                    stop: StopReason::EndTurn,
                    input_tokens: 1,
                    output_tokens: 1,
                }]);
                AgentRunOpts {
                    agent_name: format!("ag-{agent_name}"),
                    agent_system_prompt: "echo".into(),
                    agent_tools: None,
                    provider: Box::new(provider),
                    provider_name: "mock".into(),
                    model: "mock-1".into(),
                    run_id,
                    workspace_id,
                    workspace_path,
                    transcript_path,
                    max_turns: 5,
                    decider: Arc::new(BypassDecider),
                    tool_context: ToolContext::default(),
                    user_message: rendered_prompt,
                    initial_messages: Vec::new(),
                    turn_index_offset: 0,
                    mode_str: "bypass".into(),
                    no_stream: true,
                    suppress_stream_stdout: true,
                    mcp_registry: None,
                    effort: None,
                    context_window: None,
                    output_format: None,
                    output_schema: None,
                    anthropic_task_budget: None,
                    anthropic_context_management: None,
                    anthropic_speed: None,
                    parent_run_id: None,
                    depth: 0,
                    dispatchable_agents: None,
                    step_id: step_id.to_string(),
                    on_tool_call,
                    on_stream_event: None,
                    concerns: None,
                    max_tokens: DEFAULT_MAX_TOKENS,
                    scope_name: None,
                    surface_tag: None,
                    context_window_tokens: None,
                    compact_at_percent: None,
                    pause: None,
                }
            }
        }

        let opts1 = OrchestratorRunOpts {
            workflow: wf.clone(),
            inputs: BTreeMap::new(),
            workspace_id: "ws_loop_resume".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(SimpleFactory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(GATED_LOOP_WF.to_string()),
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let res1 = tokio::time::timeout(Duration::from_secs(5), run_workflow(opts1))
            .await
            .expect("phase 1 must not hang")
            .expect("a gate pausing mid-loop must be Ok(..), never Err(LoopIterationPaused)");
        let run_id = res1.run_id.clone();
        let awaiting1 = res1.awaiting.clone().expect("the gate must park the run");
        assert_eq!(awaiting1.reason, PauseReason::Approval);
        assert_eq!(awaiting1.step_id, "review");
        assert_eq!(awaiting1.gates.len(), 1);
        assert_eq!(awaiting1.gates[0].step_id, "review");
        assert_eq!(count_ids(&res1.step_results, "work"), 1);
        assert_eq!(count_ids(&res1.step_results, "ship"), 0);

        let rec1 = store.load(&run_id).unwrap();
        assert_eq!(rec1.loop_progress.get("refine"), Some(&0));

        // --- Approve + resume: the gate resolves, `until` holds
        // (non-empty decision JSON), the loop converges at iteration 0,
        // `ship` runs exactly once.
        store.approve_gate(&run_id, "matt", chrono::Utc::now(), None).unwrap();
        let opts2 = OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_loop_resume".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(SimpleFactory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(GATED_LOOP_WF.to_string()),
            resume_from: Some(ResumeState::from_approval(
                run_id.clone(),
                res1.step_results.clone(),
                "review".to_string(),
            )),
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let res2 = tokio::time::timeout(Duration::from_secs(5), run_workflow(opts2))
            .await
            .expect("resume must not hang")
            .expect("resume must converge and complete");
        assert!(res2.awaiting.is_none());
        assert_eq!(count_ids(&res2.step_results, "work"), 1, "work must not be re-dispatched");
        assert_eq!(count_ids(&res2.step_results, "ship"), 1, "ship exactly once");
    }

    /// Plan Task 4 Step 5 / spec §3: cancel mid-loop (Phase-2 hard
    /// cancel, distinct from the cooperative pause above) aborts the
    /// in-flight iteration's member; restart re-enters at the recorded
    /// iteration. Asserts no member of a not-yet-started iteration
    /// (iteration 1) ran before the cancel.
    #[tokio::test]
    async fn cancel_mid_loop_then_restart_reenters_at_recorded_iteration() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::runs::RunStore::new(tmp.path().join("runs")));
        let wf = Workflow::parse(REFINE_WF).unwrap();
        let run_id = "run_loop_cancel_test".to_string();
        store
            .create(
                crate::runs::RunRecord {
                    id: run_id.clone(),
                    workflow_name: wf.name.clone(),
                    status: crate::runs::RunStatus::Running,
                    inputs: BTreeMap::new(),
                    event: None,
                    workspace_id: "ws_loop_resume".into(),
                    workspace_path: tmp.path().to_path_buf(),
                    transcript_dir: tmp.path().to_path_buf(),
                    started_at: chrono::Utc::now(),
                    finished_at: None,
                    error_message: None,
                    awaiting: Vec::new(),
                    awaiting_step_id: None,
                    approval_prompt: None,
                    awaiting_since: None,
                    expires_at: None,
                    issue_ref: None,
                    issue: None,
                    parent_run_id: None,
                    backend_id: None,
                    worker_id: None,
                    artifact_manifest_path: None,
                    runner_pid: None,
                    source_wake_id: None,
                    active_step_id: None,
                    active_step_kind: None,
                    active_step_agent: None,
                    active_step_transcript_path: None,
                    resume_requested_at: None,
                    resume_claimed_at: None,
                    resume_claimed_by: None,
                    resume_mode: None,
                    resume_gate_id: None,
                    final_output: None,
                    loop_progress: BTreeMap::new(),
                },
                REFINE_WF,
            )
            .unwrap();

        // --- Phase 1: `critique` is slow (150ms); cancel the whole run
        // 50ms in — iteration 0's `gen`/`test` finish near-instantly,
        // then `critique` starts and is aborted mid-flight.
        let factory1 = ResumableRefineFactory::new(2).critique_delay_ms(150);
        let calls1 = Arc::clone(&factory1.calls);
        let opts1 = OrchestratorRunOpts {
            workflow: wf.clone(),
            inputs: BTreeMap::new(),
            workspace_id: "ws_loop_resume".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory1),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let cancel_token = CancellationToken::new();
        let trigger = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            trigger.cancel();
        });

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(
                &opts1,
                &run_id,
                &resolved_inputs,
                false,
                None,
                &mut step_results,
                Some(&cancel_token),
            ),
        )
        .await
        .expect("cancel must not hang");
        match outcome {
            Err(RunWorkflowError::RunCancelled { aborted }) => {
                assert_eq!(aborted, 1, "critique must have been in-flight and aborted");
            }
            other => panic!("expected Err(RunCancelled), got {other:?}"),
        }

        let calls1 = calls1.lock().unwrap().clone();
        assert_eq!(count(&calls1, "gen"), 1, "only iteration 0's gen: {calls1:?}");
        assert_eq!(count(&calls1, "test"), 1, "only iteration 0's test: {calls1:?}");
        assert_eq!(
            count(&calls1, "critique"),
            1,
            "iteration 0's critique attempted (aborted): {calls1:?}"
        );
        assert!(
            !step_results.iter().any(|sr| sr.step_id == "critique"),
            "the aborted critique must not be recorded: {step_results:?}"
        );
        assert!(step_results.iter().any(|sr| sr.step_id == "gen"));
        assert!(step_results.iter().any(|sr| sr.step_id == "test"));

        let rec1 = store.load(&run_id).unwrap();
        assert_eq!(
            rec1.loop_progress.get("refine"),
            Some(&0),
            "checkpoint must record iteration 0 as in-flight, not advanced"
        );

        // --- Phase 2: restart — same run id/store, the SAME
        // `step_results` vector (already reflects "gen/test@0 done,
        // critique@0 not done" exactly like a real on-disk resume
        // would). A fresh factory continues the global critique
        // sequence from call #0 (iteration 0's critique wasn't
        // recorded as having answered anything yet).
        let factory2 = ResumableRefineFactory::new(2);
        let calls2 = Arc::clone(&factory2.calls);
        let opts2 = OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_loop_resume".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory2),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let outcome2 = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(
                &opts2,
                &run_id,
                &resolved_inputs,
                false,
                None,
                &mut step_results,
                None,
            ),
        )
        .await
        .expect("restart must not hang")
        .expect("restart must converge and complete");
        assert!(matches!(outcome2, InnerOutcome::Done));

        let calls2 = calls2.lock().unwrap().clone();
        assert_eq!(
            count(&calls2, "gen"),
            2,
            "iteration 0's gen must NOT be re-dispatched — only iterations 1,2: {calls2:?}"
        );
        assert_eq!(
            count(&calls2, "test"),
            2,
            "iteration 0's test must NOT be re-dispatched — only iterations 1,2: {calls2:?}"
        );
        assert_eq!(
            count(&calls2, "critique"),
            3,
            "iteration 0's critique (re-dispatched) + iterations 1,2: {calls2:?}"
        );
        assert_eq!(count(&calls2, "ship"), 1, "ship exactly once: {calls2:?}");
    }
}

/// Task 3: branch pruning (spec §6) + explicit `join` (spec §5,
/// `all`/`first`/`count`) + loser-cancellation (spec §8, first use).
/// Exercises [`run_scheduler`] directly — same reason `scheduler_concurrency`
/// does (not `pub`). This module's trailing `..._live_through_run_workflow`
/// tests (added by Task 5) re-run a couple of these same fixtures through
/// the live [`run_workflow`] entry point instead, now that its
/// [`is_nonlinear`] router reaches the scheduler for real.
#[cfg(test)]
mod join_and_prune {
    use super::*;
    use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn, DEFAULT_MAX_TOKENS};
    use rupu_providers::types::StopReason;
    use rupu_tools::ToolContext;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Dispatches every step to an echoing `MockProvider` turn. Any step
    /// named in `slow` sleeps that many ms BEFORE the (near-instant)
    /// scripted turn — the delay stands in for "this path is still doing
    /// real work" so a `first`/`count` join's threshold is reliably met by
    /// the OTHER (fast) inbound path first, giving deterministic winners.
    ///
    /// `finished` is the key proof for cancellation: it's appended to only
    /// AFTER the sleep completes, right before the scripted turn would run.
    /// A step that appears in `calls` (dispatch started) but NOT in
    /// `finished` (never got past its sleep) was genuinely cancelled by an
    /// abort — not merely "the join proceeded without waiting for it" —
    /// since aborting a `tokio::spawn`ed task drops it at its very next
    /// `.await` point (here, the `tokio::time::sleep`).
    struct JoinTestFactory {
        slow: &'static [(&'static str, u64)],
        calls: Arc<Mutex<Vec<String>>>,
        finished: Arc<Mutex<Vec<String>>>,
    }

    impl JoinTestFactory {
        fn new(slow: &'static [(&'static str, u64)]) -> Self {
            Self {
                slow,
                calls: Arc::new(Mutex::new(Vec::new())),
                finished: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl StepFactory for JoinTestFactory {
        async fn build_opts_for_step(
            &self,
            step_id: &str,
            agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: PathBuf,
            transcript_path: PathBuf,
            on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> AgentRunOpts {
            self.calls.lock().unwrap().push(step_id.to_string());
            if let Some(&(_, ms)) = self.slow.iter().find(|&&(id, _)| id == step_id) {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            self.finished.lock().unwrap().push(step_id.to_string());

            let turn = ScriptedTurn::AssistantText {
                text: format!("step {step_id} agent {agent_name} echo: {rendered_prompt}"),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            };
            let provider = MockProvider::new(vec![turn]);
            AgentRunOpts {
                agent_name: format!("ag-{agent_name}"),
                agent_system_prompt: "echo".into(),
                agent_tools: None,
                provider: Box::new(provider),
                provider_name: "mock".into(),
                model: "mock-1".into(),
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                max_turns: 5,
                decider: Arc::new(BypassDecider),
                tool_context: ToolContext::default(),
                user_message: rendered_prompt,
                initial_messages: Vec::new(),
                turn_index_offset: 0,
                mode_str: "bypass".into(),
                no_stream: true,
                suppress_stream_stdout: true,
                mcp_registry: None,
                effort: None,
                context_window: None,
                output_format: None,
                output_schema: None,
                anthropic_task_budget: None,
                anthropic_context_management: None,
                anthropic_speed: None,
                parent_run_id: None,
                depth: 0,
                dispatchable_agents: None,
                step_id: step_id.to_string(),
                on_tool_call,
                on_stream_event: None,
                concerns: None,
                max_tokens: DEFAULT_MAX_TOKENS,
                scope_name: None,
                surface_tag: None,
                context_window_tokens: None,
                compact_at_percent: None,
                pause: None,
            }
        }
    }

    fn opts_for(
        wf: Workflow,
        factory: JoinTestFactory,
        tmp: &tempfile::TempDir,
    ) -> OrchestratorRunOpts {
        OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_join".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        }
    }

    fn result_for<'a>(step_results: &'a [StepResult], id: &str) -> &'a StepResult {
        step_results
            .iter()
            .find(|sr| sr.step_id == id)
            .unwrap_or_else(|| panic!("missing StepResult for `{id}`; got {step_results:?}"))
    }

    const JOIN_ALL_WF: &str = r#"
name: join-all
steps:
  - id: fanout
    split: [a, b]
  - id: a
    agent: worker
    prompt: "do a"
    next: [gathered]
  - id: b
    agent: worker
    prompt: "do b"
    next: [gathered]
  - id: gathered
    join: { wait: all }
  - id: after
    agent: worker
    prompt: "n={{ steps.gathered.results | length }} a={{ steps.gathered.sub_results.a.output }} b={{ steps.gathered.sub_results.b.output }}"
"#;

    /// `join: { wait: all }` over 2 inbound paths merges BOTH outputs into
    /// `steps.<join>.results` (an ordered list) AND
    /// `steps.<join>.sub_results.<source_id>` (keyed by source step id) —
    /// the exact shape documented in this task's report. A downstream step
    /// reads both forms.
    #[tokio::test]
    async fn join_wait_all_merges_both_inbound_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(JOIN_ALL_WF).unwrap();
        let factory = JoinTestFactory::new(&[]);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        let gathered = result_for(&step_results, "gathered");
        assert!(gathered.success);
        assert_eq!(gathered.items.len(), 2, "both inbound paths waited for");
        let sub_ids: std::collections::BTreeSet<&str> =
            gathered.items.iter().map(|it| it.sub_id.as_str()).collect();
        assert_eq!(sub_ids, std::collections::BTreeSet::from(["a", "b"]));

        let after = result_for(&step_results, "after");
        assert!(
            after.output.contains("n=2"),
            "downstream step should see steps.gathered.results | length == 2: {}",
            after.output
        );
        assert!(
            after.output.contains("a=step a agent worker echo: do a")
                && after.output.contains("b=step b agent worker echo: do b"),
            "downstream step should see both sub_results outputs: {}",
            after.output
        );
    }

    const JOIN_ANY_WF: &str = r#"
name: join-any
steps:
  - id: fanout
    split: [a, b]
  - id: a
    agent: worker
    prompt: "do a"
    next: [gathered]
  - id: b
    agent: worker
    prompt: "do b"
    next: [gathered]
  - id: gathered
    join: { wait: any }
  - id: after
    agent: worker
    prompt: "n={{ steps.gathered.results | length }}"
"#;

    /// `wait: any` (`first`): the join proceeds on the FIRST inbound path
    /// (`a`, dispatched with no delay) and CANCELS the other (`b`, made
    /// artificially slow so it's still in-flight when `a` wins). The
    /// assertion is that `b` was genuinely cancelled — dispatched (in
    /// `calls`) but never reached past its sleep (`finished` lacks it),
    /// and never got a `StepResult` — not merely that the join proceeded
    /// without it.
    #[tokio::test]
    async fn join_wait_any_proceeds_on_first_and_cancels_the_loser() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(JOIN_ANY_WF).unwrap();
        let factory = JoinTestFactory::new(&[("b", 300)]);
        let calls = Arc::clone(&factory.calls);
        let finished = Arc::clone(&factory.finished);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        let gathered = result_for(&step_results, "gathered");
        assert!(gathered.success);
        assert_eq!(gathered.items.len(), 1, "any: exactly one winner");
        assert_eq!(gathered.items[0].sub_id, "a");

        let after = result_for(&step_results, "after");
        assert!(after.output.contains("n=1"));

        assert!(
            !step_results.iter().any(|sr| sr.step_id == "b"),
            "the cancelled loser must never get a StepResult"
        );
        assert!(
            calls.lock().unwrap().contains(&"b".to_string()),
            "b must have actually been dispatched (started) before losing"
        );
        assert!(
            !finished.lock().unwrap().contains(&"b".to_string()),
            "b must have been cancelled mid-flight, not run to completion"
        );
    }

    const JOIN_COUNT_WF: &str = r#"
name: join-count
steps:
  - id: fanout
    split: [a, b, c]
  - id: a
    agent: worker
    prompt: "do a"
    next: [gathered]
  - id: b
    agent: worker
    prompt: "do b"
    next: [gathered]
  - id: c
    agent: worker
    prompt: "do c"
    next: [gathered]
  - id: gathered
    join: { wait: { count: 2 } }
  - id: after
    agent: worker
    prompt: "n={{ steps.gathered.results | length }}"
"#;

    /// `wait: { count: 2 }` over 3 inbound paths: proceeds once 2 land
    /// (`a`, `b` — both dispatched with no delay), merges those 2, and
    /// cancels the 3rd (`c`, made artificially slow).
    #[tokio::test]
    async fn join_wait_count_proceeds_on_two_of_three_and_cancels_the_third() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(JOIN_COUNT_WF).unwrap();
        let factory = JoinTestFactory::new(&[("c", 300)]);
        let calls = Arc::clone(&factory.calls);
        let finished = Arc::clone(&factory.finished);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        let gathered = result_for(&step_results, "gathered");
        assert!(gathered.success);
        assert_eq!(gathered.items.len(), 2, "count:2 waits for exactly 2");
        let sub_ids: std::collections::BTreeSet<&str> =
            gathered.items.iter().map(|it| it.sub_id.as_str()).collect();
        assert_eq!(sub_ids, std::collections::BTreeSet::from(["a", "b"]));

        assert!(
            !step_results.iter().any(|sr| sr.step_id == "c"),
            "the cancelled 3rd path must never get a StepResult"
        );
        assert!(calls.lock().unwrap().contains(&"c".to_string()));
        assert!(
            !finished.lock().unwrap().contains(&"c".to_string()),
            "c must have been cancelled mid-flight, not run to completion"
        );
    }

    const BRANCH_PRUNE_WF: &str = r#"
name: branch-prune
steps:
  - id: br
    branch:
      condition: "true"
      then: [x]
      else: [y]
  - id: x
    agent: worker
    prompt: "x"
    next: [reconverge]
  - id: y
    agent: worker
    prompt: "y"
    next: [reconverge, only_via_y]
  - id: only_via_y
    agent: worker
    prompt: "only reachable via y"
  - id: reconverge
    agent: worker
    prompt: "reconverge"
"#;

    /// Branch pruning (spec §6): the branch takes `then` (`x`). Real
    /// reachability over the graph must prune `y` (the untaken arm) AND
    /// `only_via_y` (reachable ONLY through `y`) — neither is ever
    /// dispatched — while `reconverge` (reachable through BOTH `x` and
    /// `y`) still runs exactly once, on the live `x` arm, once `y`'s
    /// pruned-resolution has unblocked its half of `reconverge`'s
    /// indegree.
    #[tokio::test]
    async fn branch_prunes_the_untaken_arm_but_not_a_shared_reconverge() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(BRANCH_PRUNE_WF).unwrap();
        assert!(
            crate::workflow::is_nonlinear(&wf),
            "fixture must be graph mode to exercise real-reachability pruning"
        );
        let factory = JoinTestFactory::new(&[]);
        let calls = Arc::clone(&factory.calls);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        let x = result_for(&step_results, "x");
        assert!(!x.skipped && x.success, "taken arm must run: {x:?}");
        let reconverge = result_for(&step_results, "reconverge");
        assert!(
            !reconverge.skipped && reconverge.success,
            "shared reconverge must still run: {reconverge:?}"
        );

        let y = result_for(&step_results, "y");
        assert!(y.skipped, "untaken arm must be pruned: {y:?}");
        assert_eq!(y.output, "pruned");
        let only_via_y = result_for(&step_results, "only_via_y");
        assert!(
            only_via_y.skipped,
            "node reachable only through the untaken arm must be pruned: {only_via_y:?}"
        );
        assert_eq!(only_via_y.output, "pruned");

        let calls = calls.lock().unwrap();
        assert!(
            !calls.contains(&"y".to_string()) && !calls.contains(&"only_via_y".to_string()),
            "pruned nodes must never reach the StepFactory: {calls:?}"
        );
        assert!(calls.contains(&"x".to_string()) && calls.contains(&"reconverge".to_string()));
    }

    const BRANCH_TO_JOIN_WF: &str = r#"
name: branch-to-join
steps:
  - id: br
    branch:
      condition: "true"
      then: [a]
      else: [b]
  - id: a
    agent: worker
    prompt: "do a"
    next: [gathered]
  - id: b
    agent: worker
    prompt: "do b"
    next: [gathered]
  - id: gathered
    join: { wait: all }
  - id: after
    agent: worker
    prompt: "n={{ steps.gathered.results | length }}"
"#;

    /// **Post-review CRITICAL-fix regression.** A branch's PRUNED arm must
    /// never be merged into a downstream `join`'s results, nor poison its
    /// `success`. Before the fix, `mark_done_and_track_joins` counted
    /// EVERY completing node — pruned or not — as a join arrival, so
    /// `gathered` fired with BOTH `a` (live, taken) and `b` (pruned,
    /// `output: "pruned"`, `success: false`) merged in:
    /// `results.len() == 2` and `gathered.success == false` even though
    /// the live arm succeeded cleanly. Confirmed to FAIL against the
    /// pre-fix code (`items.len() == 2`, `sub_results.b.output ==
    /// "pruned"`, `gathered.success == false`) and PASS after the `live`
    /// parameter was threaded through — see this task's report.
    #[tokio::test]
    async fn branch_pruned_arm_is_never_merged_into_a_downstream_join() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(BRANCH_TO_JOIN_WF).unwrap();
        let factory = JoinTestFactory::new(&[]);
        let calls = Arc::clone(&factory.calls);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        let b = result_for(&step_results, "b");
        assert!(
            b.skipped && b.output == "pruned",
            "b (untaken arm) must be pruned: {b:?}"
        );

        let gathered = result_for(&step_results, "gathered");
        assert!(
            gathered.success,
            "join must not be poisoned by the pruned arm: {gathered:?}"
        );
        assert_eq!(
            gathered.items.len(),
            1,
            "join must merge ONLY the taken arm, not the pruned one: {:?}",
            gathered.items
        );
        assert_eq!(gathered.items[0].sub_id, "a");
        assert!(
            !gathered.items.iter().any(|it| it.output == "pruned"),
            "the pruned arm's marker output must never appear in the merge: {:?}",
            gathered.items
        );

        let after = result_for(&step_results, "after");
        assert!(
            after.output.contains("n=1"),
            "downstream should see exactly 1 merged result: {}",
            after.output
        );

        assert!(
            !calls.lock().unwrap().contains(&"b".to_string()),
            "pruned arm must never reach the StepFactory"
        );
    }

    const WHEN_SKIP_TO_JOIN_WF: &str = r#"
name: when-skip-to-join
steps:
  - id: a
    agent: worker
    prompt: "do a"
    when: "false"
    next: [gathered]
  - id: b
    agent: worker
    prompt: "do b"
    next: [gathered]
  - id: gathered
    join: { wait: all }
  - id: after
    agent: worker
    prompt: "n={{ steps.gathered.results | length }}"
"#;

    /// **Post-review CRITICAL-fix regression, the `when:`-skip flavor.** A
    /// `when: false` predecessor must never be merged into a downstream
    /// `join`'s results either — same bug class as the branch case above,
    /// same fix (the `live` flag on `mark_done_and_track_joins`).
    /// Confirmed to FAIL against the pre-fix code and PASS after.
    #[tokio::test]
    async fn when_skipped_predecessor_is_never_merged_into_a_downstream_join() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(WHEN_SKIP_TO_JOIN_WF).unwrap();
        let factory = JoinTestFactory::new(&[]);
        let calls = Arc::clone(&factory.calls);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        let a = result_for(&step_results, "a");
        assert!(a.skipped, "a (when: false) must be skipped: {a:?}");

        let gathered = result_for(&step_results, "gathered");
        assert!(
            gathered.success,
            "join must not be poisoned by the skipped predecessor: {gathered:?}"
        );
        assert_eq!(
            gathered.items.len(),
            1,
            "join must merge ONLY the live predecessor: {:?}",
            gathered.items
        );
        assert_eq!(gathered.items[0].sub_id, "b");

        let after = result_for(&step_results, "after");
        assert!(
            after.output.contains("n=1"),
            "downstream should see exactly 1 merged result: {}",
            after.output
        );

        assert!(
            !calls.lock().unwrap().contains(&"a".to_string()),
            "when-skipped predecessor must never reach the StepFactory"
        );
    }

    const BRANCH_PRUNE_UNRELATED_LIVE_PREDECESSOR_WF: &str = r#"
name: branch-prune-unrelated-live-predecessor
steps:
  - id: br
    branch:
      condition: "true"
      then: [a]
      else: [b]
  - id: a
    agent: worker
    prompt: "a"
  - id: b
    agent: worker
    prompt: "b"
    next: [shared]
  - id: z
    agent: worker
    prompt: "z"
    next: [shared]
  - id: shared
    agent: worker
    prompt: "shared sees z={{ steps.z.output }}"
"#;

    /// **Post-review IMPORTANT-fix regression.** `shared` is reachable
    /// from the untaken arm (`b`) AND from `z` — an entirely independent,
    /// always-live entry step that has nothing to do with either branch
    /// arm. The naive "reachable-from-untaken minus reachable-from-taken"
    /// diff would prune `shared` anyway (it's not reachable via the TAKEN
    /// arm `a`), silently stranding `z`'s real output. The fix compares
    /// against reachability from every graph entry point with only the
    /// untaken edge cut, which correctly finds `shared` still reachable
    /// via `z` and leaves it un-pruned. `b` itself (which has NO
    /// alternate live path) must still be pruned. Confirmed to FAIL
    /// against the pre-fix `branch_prune_set` (see this task's report)
    /// and PASS after.
    #[tokio::test]
    async fn branch_prune_does_not_strand_a_node_fed_by_an_unrelated_live_predecessor() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(BRANCH_PRUNE_UNRELATED_LIVE_PREDECESSOR_WF).unwrap();
        assert!(crate::workflow::is_nonlinear(&wf), "fixture must be graph mode");
        let factory = JoinTestFactory::new(&[]);
        let calls = Arc::clone(&factory.calls);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        let b = result_for(&step_results, "b");
        assert!(b.skipped && b.output == "pruned", "b must still be pruned: {b:?}");

        let shared = result_for(&step_results, "shared");
        assert!(
            !shared.skipped && shared.success,
            "shared must NOT be pruned — z gives it a live path: {shared:?}"
        );
        assert!(
            shared.output.contains("step z agent worker echo: z"),
            "shared must have actually run and seen z's real output: {}",
            shared.output
        );

        assert!(calls.lock().unwrap().contains(&"shared".to_string()));
        assert!(!calls.lock().unwrap().contains(&"b".to_string()));
    }

    const JOIN_CANCEL_UNRELATED_CONSUMER_WF: &str = r#"
name: join-cancel-unrelated-consumer
steps:
  - id: fanout
    split: [w, fast]
  - id: w
    agent: worker
    prompt: "w"
    next: [loser, y]
  - id: loser
    agent: worker
    prompt: "loser"
    next: [gathered]
  - id: fast
    agent: worker
    prompt: "fast"
    next: [gathered]
  - id: gathered
    join: { wait: any }
  - id: y
    agent: worker
    prompt: "y sees w={{ steps.w.output }}"
"#;

    /// **Post-review IMPORTANT-fix regression, the symmetric loser-
    /// cancellation case.** `w` feeds BOTH `loser` (the join's losing
    /// inbound path) AND `y` — a live consumer that has nothing to do
    /// with the join at all. The naive ancestor-exclusive closure
    /// (ancestors of `loser` minus ancestors of `fast`) would include `w`
    /// and cancel/abort it, silently stranding `y`. The fix protects `w`
    /// (it has a successor — `y` — outside the cancellation closure and
    /// isn't the join itself), so `w` runs to completion normally; only
    /// `loser` itself (whose ONLY successor is the join) is actually
    /// cancelled. Confirmed to FAIL against the pre-fix cancellation
    /// closure (see this task's report) and PASS after.
    #[tokio::test]
    async fn join_loser_cancellation_does_not_strand_an_ancestors_unrelated_consumer() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(JOIN_CANCEL_UNRELATED_CONSUMER_WF).unwrap();
        let factory = JoinTestFactory::new(&[("w", 80)]);
        let calls = Arc::clone(&factory.calls);
        let finished = Arc::clone(&factory.finished);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        let gathered = result_for(&step_results, "gathered");
        assert_eq!(gathered.items.len(), 1);
        assert_eq!(gathered.items[0].sub_id, "fast", "fast must win the race");

        // `w` must survive (not aborted) — it's needed by `y`.
        assert!(
            finished.lock().unwrap().contains(&"w".to_string()),
            "w must have been allowed to run to completion, not cancelled"
        );
        let w = result_for(&step_results, "w");
        assert!(w.success, "w must have a real StepResult: {w:?}");

        // `y` must actually run and see `w`'s real output.
        let y = result_for(&step_results, "y");
        assert!(
            y.output.contains("step w agent worker echo: w"),
            "y must have run with w's real output: {}",
            y.output
        );

        // `loser` itself (no consumer besides the join) must still be
        // cancelled — it gets a `StepResult` (the same "resolved as a
        // no-op" marker a pruned/skipped node gets, per the ready-drain
        // loop's unified skip check), but it must be `skipped` with the
        // `"cancelled"` marker, and must NEVER actually reach the
        // `StepFactory`.
        let loser = result_for(&step_results, "loser");
        assert!(
            loser.skipped && loser.output == "cancelled",
            "loser must be resolved as cancelled, not dispatched: {loser:?}"
        );
        assert!(
            !calls.lock().unwrap().contains(&"loser".to_string()),
            "loser must never reach the StepFactory"
        );
    }

    const MULTI_HOP_LOSER_CHAIN_WF: &str = r#"
name: multi-hop-loser-chain
steps:
  - id: fanout
    split: [fast, a]
  - id: fast
    agent: worker
    prompt: "fast"
    next: [gathered]
  - id: a
    agent: worker
    prompt: "a"
    next: [b]
  - id: b
    agent: worker
    prompt: "b"
    next: [c]
  - id: c
    agent: worker
    prompt: "c"
    next: [gathered]
  - id: gathered
    join: { wait: any }
"#;

    /// **Post-review regression: the multi-hop losing-closure silent-vanish
    /// bug.** `fast` wins `gathered`'s `wait: any` immediately; the losing
    /// closure is `{a, b, c}` (none protected — none has a consumer outside
    /// the closure or the join itself). `a` is the ONLY one actually
    /// in-flight at that moment (`b`/`c` are still blocked behind it,
    /// indegree > 0) — it gets hard-aborted via `cancel.in_flight_abort`.
    ///
    /// Before the fix: the abort-swallow path (`Err(join_err)` +
    /// `cancelled_by_us.remove` in the main loop) only `continue`d — it
    /// never ran `unlock_successors`/`track_join_arrivals` for `a`, so
    /// `b`'s indegree never reached 0. `b`/`c` were marked
    /// `cancel_state.cancelled` (by `drain_joins`, when the closure was
    /// computed) but NEVER entered `ready`, so they never reached the
    /// skip-persist check — no `StepResult`, ever. The run still returned
    /// `Ok(InnerOutcome::Done)`, silently dropping two reachable nodes with
    /// zero terminal record. Confirmed to FAIL against the pre-fix code
    /// (this exact assertion block) and PASS after: `a`'s abort now runs
    /// that bookkeeping non-live, which unblocks `b` into `ready`, which
    /// resolves through the EXISTING skip-persist path and unblocks `c`
    /// the same way — the SAME terminal `"cancelled"` marker a
    /// never-dispatched loser already gets.
    #[tokio::test]
    async fn multi_hop_losing_closure_does_not_silently_drop_descendants() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(MULTI_HOP_LOSER_CHAIN_WF).unwrap();
        let factory = JoinTestFactory::new(&[("a", 300)]);
        let calls = Arc::clone(&factory.calls);
        let finished = Arc::clone(&factory.finished);
        let opts = opts_for(wf, factory, &tmp);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("scheduler must not hang")
        .expect("scheduler must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        let gathered = result_for(&step_results, "gathered");
        assert!(gathered.success);
        assert_eq!(gathered.items.len(), 1, "any: exactly one winner");
        assert_eq!(gathered.items[0].sub_id, "fast");

        // `a` — the one genuinely in-flight, hard-aborted node — gets NO
        // `StepResult` of its own, unchanged from the single-hop contract
        // above: it never ran, so there's nothing of ITS OWN to record.
        assert!(
            !step_results.iter().any(|sr| sr.step_id == "a"),
            "the directly-aborted node itself still gets no StepResult"
        );
        assert!(calls.lock().unwrap().contains(&"a".to_string()));
        assert!(
            !finished.lock().unwrap().contains(&"a".to_string()),
            "a must have been genuinely aborted mid-flight, not run to completion"
        );

        // `b` and `c` — blocked behind `a`, never dispatched — MUST each
        // still resolve to a terminal `"cancelled"` marker. This is the
        // core regression assertion: before the fix, both of these
        // `result_for` calls panic (no StepResult exists for either).
        let b = result_for(&step_results, "b");
        assert!(
            b.skipped && b.output == "cancelled",
            "b must resolve as cancelled, not silently vanish: {b:?}"
        );
        let c = result_for(&step_results, "c");
        assert!(
            c.skipped && c.output == "cancelled",
            "c must resolve as cancelled, not silently vanish: {c:?}"
        );
        assert!(
            !calls.lock().unwrap().contains(&"b".to_string())
                && !calls.lock().unwrap().contains(&"c".to_string()),
            "b and c must never reach the StepFactory"
        );

        // Every node in the workflow must be accounted for: either it has
        // a StepResult, or (only `a`, the one genuinely in-flight
        // ancestor) it drove its successors' bookkeeping without one of
        // its own. No step silently disappears from both.
        let recorded: std::collections::BTreeSet<&str> =
            step_results.iter().map(|sr| sr.step_id.as_str()).collect();
        assert_eq!(
            recorded,
            std::collections::BTreeSet::from(["fanout", "fast", "gathered", "b", "c"]),
            "exactly every reachable node except the one genuinely in-flight ancestor must have a terminal StepResult"
        );
    }

    // -----------------------------------------------------------------------
    // Task 5: the SAME fixtures above, now driven through the LIVE
    // `run_workflow` entry point rather than calling `run_scheduler`
    // directly. Every test above this point proves the scheduler's
    // internals; these prove `run_workflow`'s router actually reaches it
    // for a non-linear workflow (spec §9) instead of rejecting it.
    // -----------------------------------------------------------------------

    /// `split → [a, b] → join(wait: all) gathered → after`, run through
    /// `run_workflow` (not `run_scheduler`). Before this task this fixture
    /// was rejected outright with `NonlinearNotYetSupported`; now it must
    /// run to a real `Done` terminal, with `a`/`b` both dispatched and
    /// `gathered` merging both of their results — the flipped shape of the
    /// old honesty-gate regression test.
    #[tokio::test]
    async fn join_wait_all_completes_live_through_run_workflow() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(JOIN_ALL_WF).unwrap();
        assert!(
            is_nonlinear(&wf),
            "fixture must be non-linear to exercise the live scheduler router"
        );
        let factory = JoinTestFactory::new(&[]);
        let opts = opts_for(wf, factory, &tmp);

        let res = run_workflow(opts)
            .await
            .expect("a split/join workflow must now run to completion through run_workflow");
        assert!(
            res.awaiting.is_none(),
            "no gate in this workflow — must reach Done, not park"
        );

        let gathered = result_for(&res.step_results, "gathered");
        assert!(gathered.success);
        assert_eq!(gathered.items.len(), 2, "both inbound paths waited for");
        let sub_ids: std::collections::BTreeSet<&str> =
            gathered.items.iter().map(|it| it.sub_id.as_str()).collect();
        assert_eq!(sub_ids, std::collections::BTreeSet::from(["a", "b"]));

        // The join node's own persisted `StepResult` must carry
        // `StepKind::Join`, not the reused `StepKind::Branch` — the exact
        // render-correctness defect this task fixes (mirrors the split
        // assertion in `run_workflow_runs_a_split_join_workflow_live_through_the_scheduler`).
        assert_eq!(
            gathered.kind,
            crate::runs::StepKind::Join,
            "a live join node must persist kind: Join, not the reused Branch"
        );

        let after = result_for(&res.step_results, "after");
        assert!(
            after.output.contains("n=2"),
            "downstream step must see steps.gathered.results | length == 2: {}",
            after.output
        );
    }

    /// The branch-pruning fixture, run through `run_workflow`: the taken
    /// arm (`x`) and the shared reconverge run; the untaken arm (`y`) and
    /// the node reachable only through it (`only_via_y`) are pruned and
    /// never dispatched — same assertions as
    /// `branch_prunes_the_untaken_arm_but_not_a_shared_reconverge` above,
    /// now proving the LIVE router reaches the same real-reachability
    /// pruning logic, not just `run_scheduler` called directly.
    #[tokio::test]
    async fn branch_pruning_completes_live_through_run_workflow() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(BRANCH_PRUNE_WF).unwrap();
        assert!(is_nonlinear(&wf), "fixture must be graph mode");
        let factory = JoinTestFactory::new(&[]);
        let calls = Arc::clone(&factory.calls);
        let opts = opts_for(wf, factory, &tmp);

        let res = run_workflow(opts)
            .await
            .expect("branch pruning must run to completion through run_workflow");
        assert!(res.awaiting.is_none());

        let x = result_for(&res.step_results, "x");
        assert!(!x.skipped && x.success, "taken arm must run: {x:?}");
        let reconverge = result_for(&res.step_results, "reconverge");
        assert!(
            !reconverge.skipped && reconverge.success,
            "shared reconverge must still run: {reconverge:?}"
        );
        let y = result_for(&res.step_results, "y");
        assert!(y.skipped, "untaken arm must be pruned: {y:?}");
        let only_via_y = result_for(&res.step_results, "only_via_y");
        assert!(
            only_via_y.skipped,
            "node reachable only through the untaken arm must be pruned: {only_via_y:?}"
        );
        let calls = calls.lock().unwrap();
        assert!(
            !calls.contains(&"y".to_string()) && !calls.contains(&"only_via_y".to_string()),
            "pruned nodes must never reach the StepFactory: {calls:?}"
        );
    }

    /// A non-linear workflow (a fork out of the gate step itself — a
    /// non-branch step with 2 `next:` targets, `is_nonlinear`'s fork rule)
    /// whose SOLE approval gate is also its entry node: the gate is the
    /// only ready node at the start, so it parks before anything else is
    /// ever dispatched — a clean, deterministic instance of "ONE approval
    /// gate parks" (spec §7's simplest case; T5b's full concurrent
    /// awaiting-set is NOT exercised here — see this task's report for the
    /// boundary). Approve-resume (mirroring `tests/gate_node.rs`'s
    /// pattern) then completes the run, dispatching both fork targets.
    const NONLINEAR_GATE_WF: &str = r#"
name: nonlinear-gate
steps:
  - id: gate
    approval:
      prompt: "Approve the fan-out?"
    next: [a, b]
  - id: a
    agent: worker
    prompt: "do a"
  - id: b
    agent: worker
    prompt: "do b"
"#;

    #[tokio::test]
    async fn single_gate_in_nonlinear_workflow_parks_then_resume_completes_live_through_run_workflow(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(NONLINEAR_GATE_WF).unwrap();
        assert!(
            is_nonlinear(&wf),
            "fixture must be non-linear (fork out of the gate) to exercise the live router"
        );

        // --- Phase 1: the gate is the only ready node — it must park
        // before `a`/`b` are ever reachable. ---
        let factory1 = JoinTestFactory::new(&[]);
        let calls1 = Arc::clone(&factory1.calls);
        let opts1 = opts_for(wf.clone(), factory1, &tmp);
        let res1 = run_workflow(opts1)
            .await
            .expect("a pause is Ok, not Err");
        let awaiting = res1.awaiting.clone().expect("the gate must pause the run");
        assert_eq!(awaiting.step_id, "gate");
        assert!(
            res1.step_results.is_empty(),
            "a paused gate has no completed result yet"
        );
        assert!(
            calls1.lock().unwrap().is_empty(),
            "a/b must never reach the StepFactory before the gate is approved: {:?}",
            calls1.lock().unwrap()
        );

        // --- Phase 2: approve + resume — mirrors `tests/gate_node.rs`'s
        // approve-resume pattern, adapted for the in-memory (no run_store)
        // shape this module's `opts_for` uses. ---
        let factory2 = JoinTestFactory::new(&[]);
        let calls2 = Arc::clone(&factory2.calls);
        let mut opts2 = opts_for(wf, factory2, &tmp);
        opts2.resume_from = Some(ResumeState::from_approval(
            res1.run_id.clone(),
            res1.step_results.clone(),
            "gate".to_string(),
        ));

        let res2 = run_workflow(opts2)
            .await
            .expect("resume after approval must complete the run");
        assert!(res2.awaiting.is_none(), "resumed run must reach Done");

        let gate = result_for(&res2.step_results, "gate");
        assert!(gate.success);
        let a = result_for(&res2.step_results, "a");
        let b = result_for(&res2.step_results, "b");
        assert!(!a.skipped && a.success, "a must run after the gate resolves: {a:?}");
        assert!(!b.skipped && b.success, "b must run after the gate resolves: {b:?}");
        let dispatched = calls2.lock().unwrap();
        assert!(
            dispatched.contains(&"a".to_string()) && dispatched.contains(&"b".to_string()),
            "both fork targets must have been dispatched post-approval: {dispatched:?}"
        );
    }

    /// Task 5b-1 (spec §7): TWO independent concurrent paths each hitting
    /// their own gate — the scenario T5a's report named as the explicit
    /// boundary it did NOT cover. `fanout` forks into three: `gate_a` and
    /// `gate_b` (each gating its own downstream `a`/`b`), and `indep` — a
    /// REAL dispatch with no gate on its path at all, artificially slowed
    /// so it's still in-flight at the exact moment both gates park.
    ///
    /// This exercises the batch-parking contract end to end: both gates
    /// land in ONE `Paused` with `awaiting.len() == 2` (not two separate
    /// pause/resume round-trips), AND `indep` — the independent in-flight
    /// sibling — is drained to completion rather than aborted when the
    /// scheduler stops for the gates (the T5a sibling-stranding fix).
    /// Uses a real disk-backed `RunStore` (unlike this module's other
    /// tests) specifically to assert on the PERSISTED `RunRecord.awaiting`
    /// set and `RunStatus`, not just the in-memory `AwaitingInfo`.
    const NONLINEAR_TWO_GATE_WF: &str = r#"
name: nonlinear-two-gate
steps:
  - id: fanout
    split: [gate_a, gate_b, indep]
  - id: gate_a
    approval:
      prompt: "Approve A?"
    next: [a]
  - id: gate_b
    approval:
      prompt: "Approve B?"
    next: [b]
  - id: indep
    agent: worker
    prompt: "independent work, no gate on this path"
  - id: a
    agent: worker
    prompt: "do a"
  - id: b
    agent: worker
    prompt: "do b"
"#;

    #[tokio::test]
    async fn two_concurrent_gates_batch_park_in_one_pause_while_indep_sibling_completes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::runs::RunStore::new(tmp.path().join("runs")));
        let wf = Workflow::parse(NONLINEAR_TWO_GATE_WF).unwrap();
        assert!(is_nonlinear(&wf), "fixture must fork to exercise the DAG scheduler");

        // --- Phase 1: fanout unlocks gate_a + gate_b + indep together.
        // `indep` is slowed so it's still in-flight when both gates park in
        // the same drain wave. ---
        let factory1 = JoinTestFactory::new(&[("indep", 60)]);
        let calls1 = Arc::clone(&factory1.calls);
        let opts1 = OrchestratorRunOpts {
            workflow: wf.clone(),
            inputs: BTreeMap::new(),
            workspace_id: "ws_two_gate".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory1),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(NONLINEAR_TWO_GATE_WF.to_string()),
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let res1 = run_workflow(opts1).await.expect("a batch-park is Ok, not Err");
        let run_id = res1.run_id.clone();

        let awaiting = res1.awaiting.clone().expect("both gates must pause the run");
        assert_eq!(awaiting.gates.len(), 2, "both gates must land in ONE pause");
        let parked_ids: std::collections::BTreeSet<&str> =
            awaiting.gates.iter().map(|g| g.step_id.as_str()).collect();
        assert_eq!(
            parked_ids,
            std::collections::BTreeSet::from(["gate_a", "gate_b"])
        );

        // The independent sibling ran to completion — NOT aborted just
        // because the two gates parked in the same wave.
        let indep = result_for(&res1.step_results, "indep");
        assert!(indep.success && !indep.skipped, "indep: {indep:?}");
        assert!(
            calls1.lock().unwrap().contains(&"indep".to_string()),
            "indep must have reached the StepFactory"
        );
        // Neither gated path was ever reachable yet.
        assert!(res1.step_results.iter().all(|sr| sr.step_id != "a" && sr.step_id != "b"));

        // Persisted state: the RunRecord itself carries the full set.
        let rec1 = store.load(&run_id).unwrap();
        assert_eq!(rec1.status, crate::runs::RunStatus::AwaitingApproval);
        assert_eq!(rec1.awaiting.len(), 2);
        let rec1_ids: std::collections::BTreeSet<&str> =
            rec1.awaiting.iter().map(|g| g.step_id.as_str()).collect();
        assert_eq!(
            rec1_ids,
            std::collections::BTreeSet::from(["gate_a", "gate_b"])
        );
        // Derived-compat mirrors the FIRST gate in the set.
        assert_eq!(rec1.awaiting_step_id.as_deref(), Some(rec1.awaiting[0].step_id.as_str()));

        // --- Phase 2: approve gate_a only (by id, via the store) —
        // gate_b must stay parked; the run must stay AwaitingApproval, NOT
        // flip to Running. ---
        store
            .approve_gate(&run_id, "matt", chrono::Utc::now(), Some("gate_a"))
            .expect("approve gate_a");
        let rec2 = store.load(&run_id).unwrap();
        assert_eq!(rec2.status, crate::runs::RunStatus::AwaitingApproval);
        assert_eq!(rec2.awaiting.len(), 1);
        assert_eq!(rec2.awaiting[0].step_id, "gate_b");

        let factory2 = JoinTestFactory::new(&[]);
        let calls2 = Arc::clone(&factory2.calls);
        let opts2 = OrchestratorRunOpts {
            workflow: wf.clone(),
            inputs: BTreeMap::new(),
            workspace_id: "ws_two_gate".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory2),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(NONLINEAR_TWO_GATE_WF.to_string()),
            resume_from: Some(ResumeState::from_approval(
                run_id.clone(),
                res1.step_results.clone(),
                "gate_a".to_string(),
            )),
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let res2 = run_workflow(opts2).await.expect("resume of gate_a's path is Ok");
        // gate_b is still parked — the resumed run pauses AGAIN, it does
        // not run to completion.
        let awaiting2 = res2.awaiting.clone().expect("gate_b must still be parked");
        assert_eq!(awaiting2.gates.len(), 1);
        assert_eq!(awaiting2.gates[0].step_id, "gate_b");
        let a = result_for(&res2.step_results, "a");
        assert!(a.success && !a.skipped, "a must run once gate_a is approved: {a:?}");
        assert!(
            calls2.lock().unwrap().contains(&"a".to_string())
                && !calls2.lock().unwrap().contains(&"b".to_string()),
            "only a's path may run while gate_b is still parked: {:?}",
            calls2.lock().unwrap()
        );
        let rec3 = store.load(&run_id).unwrap();
        assert_eq!(rec3.status, crate::runs::RunStatus::AwaitingApproval);
        assert_eq!(rec3.awaiting.len(), 1);
        assert_eq!(rec3.awaiting[0].step_id, "gate_b");

        // --- Phase 3: approve gate_b — the set empties, the run completes. ---
        store
            .approve_gate(&run_id, "matt", chrono::Utc::now(), Some("gate_b"))
            .expect("approve gate_b");
        let rec4 = store.load(&run_id).unwrap();
        assert_eq!(rec4.status, crate::runs::RunStatus::Running);
        assert!(rec4.awaiting.is_empty());

        let factory3 = JoinTestFactory::new(&[]);
        let calls3 = Arc::clone(&factory3.calls);
        let opts3 = OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_two_gate".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory3),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(NONLINEAR_TWO_GATE_WF.to_string()),
            resume_from: Some(ResumeState::from_approval(
                run_id.clone(),
                res2.step_results.clone(),
                "gate_b".to_string(),
            )),
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let res3 = run_workflow(opts3).await.expect("resume of gate_b's path completes");
        assert!(res3.awaiting.is_none(), "the run must reach Done once both gates are resolved");
        let b = result_for(&res3.step_results, "b");
        assert!(b.success && !b.skipped, "b must run once gate_b is approved: {b:?}");
        assert!(calls3.lock().unwrap().contains(&"b".to_string()));

        let rec5 = store.load(&run_id).unwrap();
        assert_eq!(rec5.status, crate::runs::RunStatus::Completed);
        assert!(rec5.awaiting.is_empty());
        assert!(rec5.awaiting_step_id.is_none());
    }

    const NONLINEAR_TWO_GATE_TIMEOUT_WF: &str = r#"
name: nonlinear-two-gate-timeout
steps:
  - id: fanout
    split: [gate_a, gate_b]
  - id: gate_a
    approval:
      prompt: "Approve A?"
    next: [a]
  - id: gate_b
    approval:
      prompt: "Approve B?"
      timeout_seconds: 3600
    next: [b]
  - id: a
    agent: worker
    prompt: "do a"
  - id: b
    agent: worker
    prompt: "do b"
"#;

    /// Task 5b-2a (5b-1 Minor #3, deferred): a still-parked gate must keep
    /// its ORIGINAL `since`/`expires_at` across a resume cycle, not get a
    /// fresh clock every time `run_workflow` re-enters. Before the fix,
    /// `run_workflow`'s Approval-pause handling unconditionally recomputed
    /// `since: now()` / `expires_at: now() + timeout` for EVERY gate the
    /// scheduler re-parks on a resume pass — including a gate that was
    /// already parked before this resume and never touched by the
    /// operator. Approving gate_a resumes gate_a's path while gate_b (with
    /// a real `timeout_seconds`) is re-evaluated fresh by the scheduler and
    /// re-parks in THIS resume's `Paused` outcome; its clock must be
    /// carried forward from the FIRST pause, not restarted.
    #[tokio::test]
    async fn resume_preserves_still_parked_gates_original_clock_not_a_fresh_one() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::runs::RunStore::new(tmp.path().join("runs")));
        let wf = Workflow::parse(NONLINEAR_TWO_GATE_TIMEOUT_WF).unwrap();
        assert!(is_nonlinear(&wf), "fixture must fork to exercise the DAG scheduler");

        // --- Phase 1: fanout parks both gates in one batch. ---
        let factory1 = JoinTestFactory::new(&[]);
        let opts1 = OrchestratorRunOpts {
            workflow: wf.clone(),
            inputs: BTreeMap::new(),
            workspace_id: "ws_gate_clock".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory1),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(NONLINEAR_TWO_GATE_TIMEOUT_WF.to_string()),
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let res1 = run_workflow(opts1).await.expect("both gates batch-park");
        let run_id = res1.run_id.clone();

        let rec1 = store.load(&run_id).unwrap();
        assert_eq!(rec1.awaiting.len(), 2);
        let gate_b_before = rec1
            .awaiting
            .iter()
            .find(|g| g.step_id == "gate_b")
            .cloned()
            .expect("gate_b must be parked after phase 1");
        assert!(
            gate_b_before.expires_at.is_some(),
            "gate_b carries a real timeout_seconds — its clock must be checkable"
        );

        // A newly-parked gate always gets `since` from THIS pause instant —
        // sanity check the fixture actually exercises a non-trivial clock
        // before asserting it survives a resume unchanged.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        // --- Phase 2: approve gate_a only; resume. gate_b re-parks in the
        // SAME resume call — its since/expires_at must be UNCHANGED from
        // phase 1, not recomputed against phase 2's `now()`. ---
        store
            .approve_gate(&run_id, "matt", chrono::Utc::now(), Some("gate_a"))
            .expect("approve gate_a");

        let factory2 = JoinTestFactory::new(&[]);
        let opts2 = OrchestratorRunOpts {
            workflow: wf.clone(),
            inputs: BTreeMap::new(),
            workspace_id: "ws_gate_clock".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory2),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(NONLINEAR_TWO_GATE_TIMEOUT_WF.to_string()),
            resume_from: Some(ResumeState::from_approval(
                run_id.clone(),
                res1.step_results.clone(),
                "gate_a".to_string(),
            )),
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let res2 = run_workflow(opts2).await.expect("resume of gate_a's path re-parks on gate_b");
        let awaiting2 = res2.awaiting.clone().expect("gate_b must still be parked");
        assert_eq!(awaiting2.gates.len(), 1);
        assert_eq!(awaiting2.gates[0].step_id, "gate_b");

        let rec2 = store.load(&run_id).unwrap();
        assert_eq!(rec2.status, crate::runs::RunStatus::AwaitingApproval);
        assert_eq!(rec2.awaiting.len(), 1);
        let gate_b_after = &rec2.awaiting[0];
        assert_eq!(gate_b_after.step_id, "gate_b");

        // The bug: `since`/`expires_at` reset to a fresh `now()` on every
        // resume. The fix: both are carried forward byte-for-byte from
        // phase 1's pause instant.
        assert_eq!(
            gate_b_after.since, gate_b_before.since,
            "gate_b's park instant must survive the resume unchanged"
        );
        assert_eq!(
            gate_b_after.expires_at, gate_b_before.expires_at,
            "gate_b's timeout deadline must survive the resume unchanged"
        );
    }

    /// Counterpart to the clock-preservation test above: a gate that is
    /// NEWLY parked in this resume pass (not present in the prior
    /// `awaiting` set at all) must still get a FRESH `since`/`expires_at`
    /// computed at ITS OWN pause instant — the preservation logic must not
    /// leak into gates that have no prior record to preserve.
    #[tokio::test]
    async fn resume_gives_a_genuinely_new_gate_a_fresh_clock() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::runs::RunStore::new(tmp.path().join("runs")));
        // Linear-with-two-sequential-gates: gate_a first, gate_b only
        // becomes reachable once gate_a is approved — so gate_b is a
        // GENUINELY new gate on the resume that approves gate_a, not one
        // that was already parked before it.
        const WF: &str = r#"
name: sequential-two-gate
steps:
  - id: gate_a
    approval:
      prompt: "Approve A?"
  - id: gate_b
    approval:
      prompt: "Approve B?"
      timeout_seconds: 3600
"#;
        let wf = Workflow::parse(WF).unwrap();

        let opts1 = OrchestratorRunOpts {
            workflow: wf.clone(),
            inputs: BTreeMap::new(),
            workspace_id: "ws_new_gate_clock".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(JoinTestFactory::new(&[])),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(WF.to_string()),
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let res1 = run_workflow(opts1).await.expect("pauses at gate_a");
        let run_id = res1.run_id.clone();
        let rec1 = store.load(&run_id).unwrap();
        assert_eq!(rec1.awaiting.len(), 1);
        assert_eq!(rec1.awaiting[0].step_id, "gate_a");

        let before_resume = chrono::Utc::now();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        store
            .approve_gate(&run_id, "matt", chrono::Utc::now(), Some("gate_a"))
            .expect("approve gate_a");

        let opts2 = OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_new_gate_clock".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(JoinTestFactory::new(&[])),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&store)),
            workflow_yaml: Some(WF.to_string()),
            resume_from: Some(ResumeState::from_approval(
                run_id.clone(),
                res1.step_results.clone(),
                "gate_a".to_string(),
            )),
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        };
        let res2 = run_workflow(opts2).await.expect("resume reaches the newly-unlocked gate_b");
        let awaiting2 = res2.awaiting.expect("gate_b must park");
        assert_eq!(awaiting2.gates.len(), 1);
        assert_eq!(awaiting2.gates[0].step_id, "gate_b");

        let rec2 = store.load(&run_id).unwrap();
        let gate_b = &rec2.awaiting[0];
        assert_eq!(gate_b.step_id, "gate_b");
        assert!(
            gate_b.since > before_resume,
            "a genuinely new gate must get a FRESH since from THIS pause instant, not \
             something predating the resume that unlocked it"
        );
        assert!(gate_b.expires_at.is_some_and(|exp| exp > gate_b.since));
    }
}

/// Task 4: per-node resume (rebuilding done/pruned/cancelled + join
/// arrival bookkeeping from disk) and whole-run cancel/restart-from-
/// checkpoint. See this task's report for the full design writeup.
#[cfg(test)]
mod resume_and_cancel {
    use super::*;
    use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn, DEFAULT_MAX_TOKENS};
    use rupu_providers::types::StopReason;
    use rupu_tools::ToolContext;
    use std::sync::Mutex;
    use std::time::Duration;

    /// One `StepFactory` covering every test below: records every
    /// dispatched step id (`calls`, in call order) and, separately, every
    /// id that ran to completion PAST any configured `slow` delay
    /// (`finished`) — the same "started but never finished" cancellation
    /// proof `join_and_prune`'s `JoinTestFactory` uses.
    struct RecordingFactory {
        calls: Arc<Mutex<Vec<String>>>,
        finished: Arc<Mutex<Vec<String>>>,
        slow: &'static [(&'static str, u64)],
    }

    impl RecordingFactory {
        fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                finished: Arc::new(Mutex::new(Vec::new())),
                slow: &[],
            }
        }

        fn slow(mut self, steps: &'static [(&'static str, u64)]) -> Self {
            self.slow = steps;
            self
        }
    }

    #[async_trait]
    impl StepFactory for RecordingFactory {
        async fn build_opts_for_step(
            &self,
            step_id: &str,
            agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: PathBuf,
            transcript_path: PathBuf,
            on_tool_call: Option<rupu_agent::OnToolCallCallback>,
        ) -> AgentRunOpts {
            self.calls.lock().unwrap().push(step_id.to_string());
            if let Some(&(_, ms)) = self.slow.iter().find(|&&(id, _)| id == step_id) {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            self.finished.lock().unwrap().push(step_id.to_string());

            let turn = ScriptedTurn::AssistantText {
                text: format!("step {step_id} agent {agent_name} echo: {rendered_prompt}"),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            };
            let provider = MockProvider::new(vec![turn]);
            AgentRunOpts {
                agent_name: format!("ag-{agent_name}"),
                agent_system_prompt: "echo".into(),
                agent_tools: None,
                provider: Box::new(provider),
                provider_name: "mock".into(),
                model: "mock-1".into(),
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                max_turns: 5,
                decider: Arc::new(BypassDecider),
                tool_context: ToolContext::default(),
                user_message: rendered_prompt,
                initial_messages: Vec::new(),
                turn_index_offset: 0,
                mode_str: "bypass".into(),
                no_stream: true,
                suppress_stream_stdout: true,
                mcp_registry: None,
                effort: None,
                context_window: None,
                output_format: None,
                output_schema: None,
                anthropic_task_budget: None,
                anthropic_context_management: None,
                anthropic_speed: None,
                parent_run_id: None,
                depth: 0,
                dispatchable_agents: None,
                step_id: step_id.to_string(),
                on_tool_call,
                on_stream_event: None,
                concerns: None,
                max_tokens: DEFAULT_MAX_TOKENS,
                scope_name: None,
                surface_tag: None,
                context_window_tokens: None,
                compact_at_percent: None,
                pause: None,
            }
        }
    }

    fn opts_for(
        wf: Workflow,
        factory: RecordingFactory,
        tmp: &tempfile::TempDir,
        pause: Option<CancellationToken>,
        resume_from: Option<ResumeState>,
    ) -> OrchestratorRunOpts {
        OrchestratorRunOpts {
            workflow: wf,
            inputs: BTreeMap::new(),
            workspace_id: "ws_resume_cancel".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            factory: Arc::new(factory),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause,
        }
    }

    const SPLIT_JOIN_ALL_WF: &str = r#"
name: split-join-resume
steps:
  - id: fanout
    split: [a, b, c]
  - id: a
    agent: worker
    prompt: "do a"
    next: [gathered]
  - id: b
    agent: worker
    prompt: "do b"
    next: [gathered]
  - id: c
    agent: worker
    prompt: "do c"
    next: [gathered]
  - id: gathered
    join: { wait: all }
"#;

    /// **Mid-DAG resume (spec §3, the Task-3-report gap).** Simulates a
    /// pause after `fanout`/`a`/`b` completed but `c` was still in-flight:
    /// hand-construct the on-disk shape (`step_results` pre-seeded with
    /// exactly those 3 entries) and drive a FRESH `run_scheduler` call over
    /// it. Resume must re-dispatch ONLY `c` (never `a`/`b` again), and
    /// `gathered` (`wait: all` over 3 inbound) must fire EXACTLY ONCE,
    /// merging all 3 — including the two seeded from resume, which never
    /// went through a live `mark_done_and_track_joins` call in this
    /// process. Without the resume-seeding fix, `gathered.threshold` (3)
    /// is never reached (only `c`'s live arrival is ever recorded) and the
    /// join never fires at all — the run would silently finish `Done`
    /// with no `gathered` `StepResult`, not merely hang.
    #[tokio::test]
    async fn mid_dag_resume_reruns_only_the_in_flight_node_and_the_join_fires_once() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(SPLIT_JOIN_ALL_WF).unwrap();
        let factory = RecordingFactory::new();
        let calls = Arc::clone(&factory.calls);
        let opts = opts_for(wf, factory, &tmp, None, None);
        let resolved_inputs = BTreeMap::new();

        let mut step_results = vec![
            StepResult {
                step_id: "fanout".into(),
                success: true,
                kind: crate::runs::StepKind::Split,
                ..Default::default()
            },
            StepResult {
                step_id: "a".into(),
                output: "out-a".into(),
                success: true,
                kind: crate::runs::StepKind::Linear,
                ..Default::default()
            },
            StepResult {
                step_id: "b".into(),
                output: "out-b".into(),
                success: true,
                kind: crate::runs::StepKind::Linear,
                ..Default::default()
            },
        ];

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("resume must not hang")
        .expect("resume must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        assert_eq!(
            calls.lock().unwrap().clone(),
            vec!["c".to_string()],
            "resume must re-dispatch ONLY the in-flight node, never a/b"
        );

        let gathered = step_results
            .iter()
            .find(|sr| sr.step_id == "gathered")
            .expect("join must fire after the resumed node completes");
        assert!(gathered.success);
        assert_eq!(
            gathered.items.len(),
            3,
            "wait:all must merge all 3 inbound paths, including the two seeded from resume"
        );
        let sub_ids: std::collections::BTreeSet<&str> =
            gathered.items.iter().map(|it| it.sub_id.as_str()).collect();
        assert_eq!(sub_ids, std::collections::BTreeSet::from(["a", "b", "c"]));
    }

    const BRANCH_PRUNE_RESUME_WF: &str = r#"
name: branch-prune-resume
steps:
  - id: br
    branch:
      condition: "true"
      then: [x]
      else: [y]
  - id: x
    agent: worker
    prompt: "x"
    next: [reconverge]
  - id: y
    agent: worker
    prompt: "y"
    next: [reconverge, only_via_y]
  - id: only_via_y
    agent: worker
    prompt: "only reachable via y"
  - id: reconverge
    agent: worker
    prompt: "reconverge"
"#;

    /// **Pruned-node persistence across resume (spec §3).** On-disk shape
    /// after the branch resolved and its entire pruned closure (`y`,
    /// `only_via_y`) resolved — Task 3's own resolution is synchronous and
    /// always persists the whole closure in one pass, so this is the only
    /// shape a real pause can ever leave behind — but before the live
    /// remainder (`reconverge`) was reached. Resume must dispatch ONLY
    /// `reconverge`; the pruned nodes must never reach the `StepFactory`.
    #[tokio::test]
    async fn pruned_node_stays_pruned_across_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(BRANCH_PRUNE_RESUME_WF).unwrap();
        let factory = RecordingFactory::new();
        let calls = Arc::clone(&factory.calls);
        let opts = opts_for(wf, factory, &tmp, None, None);
        let resolved_inputs = BTreeMap::new();

        let mut step_results = vec![
            StepResult {
                step_id: "br".into(),
                output: "then".into(),
                success: true,
                kind: crate::runs::StepKind::Branch,
                ..Default::default()
            },
            StepResult {
                step_id: "x".into(),
                output: "out-x".into(),
                success: true,
                kind: crate::runs::StepKind::Linear,
                ..Default::default()
            },
            StepResult {
                step_id: "y".into(),
                output: "pruned".into(),
                success: false,
                skipped: true,
                kind: crate::runs::StepKind::Linear,
                ..Default::default()
            },
            StepResult {
                step_id: "only_via_y".into(),
                output: "pruned".into(),
                success: false,
                skipped: true,
                kind: crate::runs::StepKind::Linear,
                ..Default::default()
            },
        ];

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("resume must not hang")
        .expect("resume must not error");
        assert!(matches!(outcome, InnerOutcome::Done));

        assert_eq!(
            calls.lock().unwrap().clone(),
            vec!["reconverge".to_string()],
            "resume must dispatch ONLY the still-live remainder, never a pruned node"
        );
        let reconverge = step_results
            .iter()
            .find(|sr| sr.step_id == "reconverge")
            .expect("reconverge must run on resume");
        assert!(reconverge.success);
    }

    const JOIN_CANCEL_RESUME_WF: &str = r#"
name: join-cancel-resume
steps:
  - id: fanout
    split: [w, fast]
  - id: w
    agent: worker
    prompt: "w"
    next: [loser, y]
  - id: loser
    agent: worker
    prompt: "loser"
    next: [gathered]
  - id: fast
    agent: worker
    prompt: "fast"
    next: [gathered]
  - id: gathered
    join: { wait: any }
  - id: y
    agent: worker
    prompt: "y"
"#;

    /// **Cancelled join-loser persistence across resume (spec §3+§5) —
    /// "same for a loser-cancelled node" as the pruned test above.** `w` is
    /// slow so `fast` wins `gathered`'s `wait: any`; `loser` (whose only
    /// successor is the join) is cancelled, `w` survives (it also feeds
    /// `y`, an unrelated live consumer) — the exact shape
    /// `join_loser_cancellation_does_not_strand_an_ancestors_unrelated_consumer`
    /// proves already persists a `"cancelled"` `StepResult` for `loser`
    /// within a single run. This test's OWN job is the resume half: hand
    /// that persisted run's full `step_results` to a fresh scheduler call
    /// and confirm NOTHING gets re-dispatched — `loser` stays cancelled,
    /// permanently, exactly like a pruned node (a join loser is
    /// permanently moot; see `cancel_finalize`'s doc for why this is NOT
    /// the same contract as a whole-run cancel's restart-clean node).
    #[tokio::test]
    async fn cancelled_join_loser_persists_across_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(JOIN_CANCEL_RESUME_WF).unwrap();
        let resolved_inputs = BTreeMap::new();

        let factory1 = RecordingFactory::new().slow(&[("w", 80)]);
        let opts1 = opts_for(wf.clone(), factory1, &tmp, None, None);
        let mut step_results: Vec<StepResult> = Vec::new();
        let outcome1 = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts1, "", &resolved_inputs, false, None, &mut step_results, None),
        )
        .await
        .expect("run 1 must not hang")
        .expect("run 1 must not error");
        assert!(matches!(outcome1, InnerOutcome::Done));
        let loser = step_results
            .iter()
            .find(|sr| sr.step_id == "loser")
            .expect("loser must resolve as cancelled within run 1");
        assert!(loser.skipped && loser.output == "cancelled");

        // Resume: a FRESH scheduler call + a FRESH factory (its own empty
        // `calls`), seeded with run 1's full persisted `step_results`.
        let factory2 = RecordingFactory::new();
        let calls2 = Arc::clone(&factory2.calls);
        let opts2 = opts_for(wf, factory2, &tmp, None, None);
        let mut step_results2 = step_results.clone();
        let outcome2 = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts2, "", &resolved_inputs, false, None, &mut step_results2, None),
        )
        .await
        .expect("resume must not hang")
        .expect("resume must not error");
        assert!(matches!(outcome2, InnerOutcome::Done));
        assert!(
            calls2.lock().unwrap().is_empty(),
            "resume must not re-dispatch anything, including the already-cancelled loser: {:?}",
            calls2.lock().unwrap()
        );
        assert_eq!(
            step_results2.len(),
            step_results.len(),
            "no new StepResults on resume — everything was already resolved"
        );
    }

    const CANCEL_FANOUT_WF: &str = r#"
name: cancel-mid-flight
steps:
  - id: fanout
    split: [a, b]
  - id: a
    agent: worker
    prompt: "a"
  - id: b
    agent: worker
    prompt: "b"
"#;

    /// **Whole-run cancel stops every in-flight job (spec §8).** `a`/`b`
    /// both dispatch concurrently (`split`'s fan-out) and sleep 300ms
    /// before finishing; an external task fires the cancel token at 50ms —
    /// reliably well before either finishes. Proof of a GENUINE mid-flight
    /// abort (not merely "the run ended before they were scheduled"):
    /// both appear in `calls` (dispatch started) but neither in `finished`
    /// (never got past its sleep). Per `cancel_finalize`'s doc, neither
    /// gets a `StepResult` — spec §8's "a node with no checkpoint restarts
    /// clean" — so a resume would simply re-dispatch them, not skip them.
    #[tokio::test]
    async fn whole_run_cancel_aborts_in_flight_nodes_and_does_not_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(CANCEL_FANOUT_WF).unwrap();
        let factory = RecordingFactory::new().slow(&[("a", 300), ("b", 300)]);
        let calls = Arc::clone(&factory.calls);
        let finished = Arc::clone(&factory.finished);
        let opts = opts_for(wf, factory, &tmp, None, None);
        let resolved_inputs = BTreeMap::new();
        let mut step_results: Vec<StepResult> = Vec::new();

        let cancel_token = CancellationToken::new();
        let trigger = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            trigger.cancel();
        });

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(
                &opts,
                "",
                &resolved_inputs,
                false,
                None,
                &mut step_results,
                Some(&cancel_token),
            ),
        )
        .await
        .expect("cancel must not hang");

        match outcome {
            Err(RunWorkflowError::RunCancelled { aborted }) => {
                assert_eq!(aborted, 2, "both a and b must have been in-flight and aborted");
            }
            other => panic!("expected Err(RunCancelled), got {other:?}"),
        }

        let calls = calls.lock().unwrap().clone();
        assert_eq!(
            std::collections::BTreeSet::from_iter(calls.iter().cloned()),
            std::collections::BTreeSet::from(["a".to_string(), "b".to_string()]),
            "both must have been dispatched (started) before cancellation: {calls:?}"
        );
        let finished = finished.lock().unwrap().clone();
        assert!(
            finished.is_empty(),
            "neither must have run to completion — genuinely aborted mid-flight: {finished:?}"
        );
        assert!(
            !step_results.iter().any(|sr| sr.step_id == "a" || sr.step_id == "b"),
            "a genuinely cancelled node must NOT be recorded — spec §8: restart clean, not permanently done: {step_results:?}"
        );
    }

    const FANOUT_CHECKPOINT_WF: &str = r#"
name: fanout-checkpoint
steps:
  - id: process
    for_each: "a\nb\nc"
    agent: worker
    prompt: "Process {{ item }}"
    max_parallel: 1
    distribute:
      hosts: [h1]
"#;

    /// Cancels the pause token right AFTER its FIRST dispatch returns a
    /// real result — i.e. once unit 0's work is genuinely done, not from
    /// inside its own dispatch (which would race `dispatch_one`'s
    /// `agent_opts.pause` wiring for a LOCAL unit and risk flagging unit 0
    /// itself as paused-mid-turn instead of completed). Lifted from
    /// `tests/pause_resume_e2e.rs`'s `CancelFirstUnitDispatcher` — the
    /// proven-correct way to land a mid-fan-out pause deterministically.
    struct CancelFirstUnitDispatcher {
        token: CancellationToken,
        calls: Mutex<Vec<usize>>,
    }
    #[async_trait]
    impl UnitDispatcher for CancelFirstUnitDispatcher {
        async fn dispatch_unit(
            &self,
            unit: UnitDispatch,
            _host: &str,
        ) -> Result<UnitOutcome, RunError> {
            let is_first = self.calls.lock().unwrap().is_empty();
            self.calls.lock().unwrap().push(unit.index);
            let outcome = UnitOutcome {
                output: format!("out-{}", unit.index),
                success: true,
                error: None,
                workspace_delta: None,
            };
            if is_first {
                self.token.cancel();
            }
            Ok(outcome)
        }
    }

    /// Records every dispatched `(index, host)` pair. No cancellation —
    /// used for the resume pass.
    #[derive(Default)]
    struct RecordingUnitDispatcher {
        calls: Mutex<Vec<usize>>,
    }
    #[async_trait]
    impl UnitDispatcher for RecordingUnitDispatcher {
        async fn dispatch_unit(
            &self,
            unit: UnitDispatch,
            _host: &str,
        ) -> Result<UnitOutcome, RunError> {
            self.calls.lock().unwrap().push(unit.index);
            Ok(UnitOutcome {
                output: format!("out-{}", unit.index),
                success: true,
                error: None,
                workspace_delta: None,
            })
        }
    }

    /// **Restart-from-checkpoint reuses `completed_units` unchanged under
    /// the scheduler (spec §8).** Uses the EXISTING cooperative
    /// `opts.pause` mechanism (not the new hard-cancel token above) to
    /// land a pause after exactly the first unit — the only mechanism
    /// that can extract a graceful mid-fan-out checkpoint at all (a hard
    /// `.abort()` drops the future instantly with nothing to extract; see
    /// `cancel_finalize`'s doc). Confirms `run_fanout_step`'s
    /// `completed_units` replay — built for `run_steps_over` — works
    /// unchanged when the SAME node is dispatched via `run_scheduler`
    /// (`run_node` is shared by both drivers): resume replays unit 0 from
    /// its checkpoint and dispatches ONLY units 1 and 2 fresh.
    #[tokio::test]
    async fn fanout_restarts_from_completed_units_checkpoint_under_scheduler() {
        let tmp = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(FANOUT_CHECKPOINT_WF).unwrap();
        let resolved_inputs = BTreeMap::new();

        let token = CancellationToken::new();
        let dispatcher1 = Arc::new(CancelFirstUnitDispatcher {
            token: token.clone(),
            calls: Mutex::new(Vec::new()),
        });
        let mut opts1 = opts_for(
            wf.clone(),
            RecordingFactory::new(),
            &tmp,
            Some(token),
            None,
        );
        opts1.unit_dispatcher = Some(dispatcher1.clone());
        let mut step_results1: Vec<StepResult> = Vec::new();

        let outcome1 = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts1, "", &resolved_inputs, false, None, &mut step_results1, None),
        )
        .await
        .expect("phase 1 must not hang")
        .expect("phase 1 must not error");

        let fanout_completed_units = match outcome1 {
            InnerOutcome::Paused {
                step_id,
                fanout_completed_units,
                ..
            } => {
                assert_eq!(step_id, "process");
                fanout_completed_units
            }
            other => panic!("expected a mid-fan-out pause, got {other:?}"),
        };
        assert_eq!(
            fanout_completed_units.len(),
            1,
            "only the first unit should have completed before the cooperative pause landed"
        );
        assert!(fanout_completed_units.contains_key(&0));
        assert_eq!(
            dispatcher1.calls.lock().unwrap().clone(),
            vec![0],
            "only unit 0 should have reached the dispatcher"
        );

        let mut completed_units = std::collections::BTreeMap::new();
        completed_units.insert("process".to_string(), fanout_completed_units);
        let dispatcher2 = Arc::new(RecordingUnitDispatcher::default());
        let mut opts2 = opts_for(
            wf,
            RecordingFactory::new(),
            &tmp,
            None,
            Some(ResumeState {
                run_id: String::new(),
                prior_step_results: Vec::new(),
                approved_step_id: String::new(),
                completed_units,
                reason: PauseReason::Manual,
                paused_step: None,
                rejected_reason: None,
            }),
        );
        opts2.unit_dispatcher = Some(dispatcher2.clone());
        let mut step_results2: Vec<StepResult> = Vec::new();
        let outcome2 = tokio::time::timeout(
            Duration::from_secs(5),
            run_scheduler(&opts2, "", &resolved_inputs, false, None, &mut step_results2, None),
        )
        .await
        .expect("resume must not hang")
        .expect("resume must not error");
        assert!(matches!(outcome2, InnerOutcome::Done));

        let process = step_results2
            .iter()
            .find(|sr| sr.step_id == "process")
            .expect("process must complete on resume");
        assert!(process.success);
        assert_eq!(
            process.items.len(),
            3,
            "all 3 units accounted for: 1 replayed from checkpoint + 2 freshly dispatched"
        );

        let mut calls2 = dispatcher2.calls.lock().unwrap().clone();
        calls2.sort_unstable();
        assert_eq!(
            calls2,
            vec![1, 2],
            "only the 2 not-yet-completed units should be dispatched on resume: {calls2:?}"
        );
    }
}

/// Walk `s` and return the first balanced-brace JSON object substring.
/// Bare-bones: counts `{` / `}` while tracking string-escape state.
/// Good enough for the LLM-prose-wrapping case we actually hit.
fn scan_for_json_object(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut start = None;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s0) = start {
                        return Some(&s[s0..=i]);
                    }
                }
            }
            _ => {}
        }
    }
    None
}
