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
    effective_workspace_mode, yaml_scalar_to_string, InputType, Step, Workflow, WorkflowParseError,
    WorkspaceMode,
};
use async_trait::async_trait;
use rupu_agent::{run_agent, AgentRunOpts, RunError, RunResult};
use rupu_providers::types::Message;
use rupu_transcript::{Event, JsonlReader};
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
    #[error("step `{step}`: `action:` steps are not supported by this rupu version yet (Plan 2)")]
    ActionStepsNotYetSupported { step: String },
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
                final_output: None,
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

    let outcome = run_steps_inner(
        &opts,
        &run_id,
        &resolved_inputs,
        workflow_default_continue,
        approved_step_id.as_deref(),
        &mut step_results,
    )
    .await;

    // Map the inner outcome onto the persisted terminal status.
    // Paused = `AwaitingApproval` and the record carries the
    // awaiting_step_id + approval_prompt; Done = `Completed`;
    // Error = `Failed`.
    let mut awaiting: Option<AwaitingInfo> = None;
    if let (Some(store), Some(record)) = (opts.run_store.as_ref(), run_record_opt.as_mut()) {
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
                timeout_seconds,
                reason,
                seed,
                fanout_completed_units,
            }) => {
                let now = chrono::Utc::now();
                // Approval → non-terminal `AwaitingApproval` (existing shape).
                // Manual   → non-terminal `Paused`.
                record.status = match reason {
                    PauseReason::Approval => crate::runs::RunStatus::AwaitingApproval,
                    PauseReason::Manual => crate::runs::RunStatus::Paused,
                };
                record.awaiting_step_id = Some(step_id.clone());
                record.approval_prompt = match reason {
                    PauseReason::Approval => Some(prompt.clone()),
                    PauseReason::Manual => None,
                };
                record.awaiting_since = Some(now);
                record.expires_at =
                    timeout_seconds.map(|secs| now + chrono::Duration::seconds(secs as i64));
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
    }) = &outcome
    {
        // No store but the run paused (approval gate or manual pause) — surface
        // the paused state to the caller anyway.
        let expires_at =
            timeout_seconds.map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs as i64));
        awaiting = Some(AwaitingInfo {
            step_id: step_id.clone(),
            prompt: prompt.clone(),
            expires_at,
            reason: *reason,
            resume_seed: seed.clone(),
            fanout_completed_units: fanout_completed_units.clone(),
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
    },
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

    for step in &opts.workflow.steps {
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
                emit_gate_result(opts, run_id, step, "approved", "human", None, step_results);
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
                    emit_gate_result(opts, run_id, step, "approved", "auto", None, step_results);
                    continue;
                }
            }
            info!(step = %step.id, "gate: pausing for approval");
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
                prompt,
                timeout_seconds: ap.timeout_seconds,
                reason: PauseReason::Approval,
                seed: Vec::new(),
                fanout_completed_units: std::collections::BTreeMap::new(),
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
                    prompt,
                    timeout_seconds: approval.timeout_seconds,
                    reason: PauseReason::Approval,
                    seed: Vec::new(),
                    fanout_completed_units: std::collections::BTreeMap::new(),
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

        // Action steps parse (Plan 1) but execute in Plan 2. Fail loudly
        // rather than silently no-op — a workflow that names one needs
        // the newer binary.
        if step.action.is_some() {
            return Err(RunWorkflowError::ActionStepsNotYetSupported {
                step: step.id.clone(),
            });
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
        let step_timer = std::time::Instant::now();

        let dispatch_result: Result<StepResult, RunWorkflowError> = if step.panel.is_some() {
            run_panel_step(run_id, step, &ctx, opts, effective_continue_on_error).await
        } else if step.parallel.is_some() {
            run_parallel_step(step, &ctx, opts, effective_continue_on_error).await
        } else if step.for_each.is_some() {
            // A distributed fan-out honors the cooperative pause token
            // MID-UNIT (not just at the step boundary): a paused-incomplete
            // fan-out unwinds here carrying the units that already succeeded,
            // so the resumed run re-dispatches only the paused / not-yet-
            // started ones.
            match run_fanout_step(run_id, step, &ctx, opts, effective_continue_on_error).await {
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
                    return Ok(InnerOutcome::Paused {
                        step_id,
                        prompt: String::new(),
                        timeout_seconds: None,
                        reason: PauseReason::Manual,
                        seed: Vec::new(),
                        fanout_completed_units: completed_units,
                    });
                }
                Ok(FanoutStepOutcome::Completed(sr)) => Ok(sr),
                Err(e) => Err(e),
            }
        } else {
            // The linear path is the only other shape that pauses mid-step
            // (its agent run carries the cooperative pause token). A
            // paused-incomplete step unwinds here into a manual-pause
            // checkpoint carrying the seed.
            match run_linear_step(run_id, step, &ctx, opts, effective_continue_on_error).await {
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
                    return Ok(InnerOutcome::Paused {
                        step_id,
                        prompt: String::new(),
                        timeout_seconds: None,
                        reason: PauseReason::Manual,
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

        let result = dispatch_result?;
        persist_step_result(opts, run_id, &result);
        clear_active_step(opts, run_id, &step.id);
        step_results.push(result);
    }
    Ok(InnerOutcome::Done)
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

/// Record a resolved gate node's result: `StepStarted` + `StepCompleted`
/// events, a `StepResult` whose `output` is the decision JSON (spec §3.1),
/// persisted like any other step. `decision` is `"approved"` or
/// `"rejected"`; `via` is `"human"` (approve-resume / operator reject) or
/// `"auto"` (auto_approve truthy — always paired with `decision:
/// "approved"`); `reason` is the operator's rejection reason (`Some` only
/// for a rejected decision, `None` otherwise, matching spec §3.1's
/// `"reason": null` for approvals).
fn emit_gate_result(
    opts: &OrchestratorRunOpts,
    run_id: &str,
    step: &Step,
    decision: &str,
    via: &str,
    reason: Option<&str>,
    step_results: &mut Vec<StepResult>,
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
            // Action steps parse but don't execute yet (Plan 2), same
            // limitation `run_workflow`'s main loop enforces — but a
            // cleanup chain never aborts the (already-terminal) run for
            // it; record the step as a logged failure and continue.
            warn!(
                step = %step.id,
                "on_reject: action steps are not runtime-supported yet (Plan 2); skipping"
            );
            let result = StepResult {
                step_id: step.id.clone(),
                success: false,
                skipped: false,
                kind: crate::runs::StepKind::Action,
                ..Default::default()
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

    // 4. Cleanup never changes the terminal status — `RunStore::reject`
    //    already finalized it before this function was called.

    // 5. `RunStore::reject` already appended a terminal `RunCompleted`
    //    event before this function ran (step 1 doc comment above), but
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
    //    that keeps the log's last line authoritative. Skipped silently
    //    only when `opts.run_store` is `None` (in-memory runs have no
    //    `events.jsonl` to close).
    if let Some(store) = &opts.run_store {
        store.append_terminal_event(
            &run_id,
            &crate::executor::Event::RunCompleted {
                run_id: run_id.clone(),
                status: crate::runs::RunStatus::Rejected,
                finished_at: chrono::Utc::now(),
            },
        );
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
                    std::sync::Arc::new(move |_caller_step_id: &str, tool_name: &str| {
                        sink.emit(
                            &wf_run_id,
                            &crate::executor::Event::StepWorking {
                                run_id: wf_run_id.clone(),
                                step_id: step_id.clone(),
                                note: Some(tool_name.to_string()),
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
            pause: None,
        }
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
