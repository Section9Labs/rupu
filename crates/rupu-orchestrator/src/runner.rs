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
    render_step_prompt, render_when_expression, LoopInfo, RenderError, StepContext, StepOutput,
};
use crate::workflow::{yaml_scalar_to_string, InputType, Step, Workflow, WorkflowParseError};
use async_trait::async_trait;
use rupu_agent::{run_agent, AgentRunOpts, RunError};
use rupu_transcript::{Event, JsonlReader};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::{info, warn};
use ulid::Ulid;

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
}

pub async fn run_workflow(
    opts: OrchestratorRunOpts,
) -> Result<OrchestratorRunResult, RunWorkflowError> {
    std::fs::create_dir_all(&opts.transcript_dir)?;
    let resolved_inputs = resolve_inputs(&opts.workflow, &opts.inputs)?;
    let workflow_default_continue = opts.workflow.defaults.continue_on_error.unwrap_or(false);

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
            };
            Some(store.create(record, yaml).map_err(map_run_store_err)?)
        } else {
            None
        }
    } else if let Some(store) = &opts.run_store {
        // Resume path: load the existing record so the terminal-flip
        // block at the bottom of the function can update it.
        match store.load(&run_id) {
            Ok(rec) => Some(rec),
            Err(e) => {
                warn!(error = %e, "failed to load resumed run record");
                None
            }
        }
    } else {
        None
    };

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
            }
            Ok(InnerOutcome::Paused {
                step_id,
                prompt,
                timeout_seconds,
            }) => {
                let now = chrono::Utc::now();
                record.status = crate::runs::RunStatus::AwaitingApproval;
                record.awaiting_step_id = Some(step_id.clone());
                record.approval_prompt = Some(prompt.clone());
                record.awaiting_since = Some(now);
                record.expires_at =
                    timeout_seconds.map(|secs| now + chrono::Duration::seconds(secs as i64));
                // Don't set finished_at — the run hasn't ended.
                awaiting = Some(AwaitingInfo {
                    step_id: step_id.clone(),
                    prompt: prompt.clone(),
                    expires_at: record.expires_at,
                });
            }
            Err(e) => {
                record.status = crate::runs::RunStatus::Failed;
                record.finished_at = Some(chrono::Utc::now());
                record.error_message = Some(e.to_string());
            }
        }
        if let Err(persist_err) = store.update(record) {
            warn!(error = %persist_err, "failed to persist terminal run state");
        }
    } else if let Ok(InnerOutcome::Paused {
        step_id,
        prompt,
        timeout_seconds,
    }) = &outcome
    {
        // No store but the workflow asked for approval — surface
        // the paused state to the caller anyway.
        let expires_at =
            timeout_seconds.map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs as i64));
        awaiting = Some(AwaitingInfo {
            step_id: step_id.clone(),
            prompt: prompt.clone(),
            expires_at,
        });
    }

    outcome?;
    Ok(OrchestratorRunResult {
        step_results,
        run_id,
        awaiting,
    })
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

    for step in &opts.workflow.steps {
        // Resume: skip steps that already ran in the prior process.
        if already_done.contains(&step.id) {
            info!(step = %step.id, "resume: skipping already-completed step");
            continue;
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
            let take =
                render_when_expression(when_expr, &ctx).map_err(|e| RunWorkflowError::Render {
                    step: step.id.clone(),
                    source: e,
                })?;
            if !take {
                info!(step = %step.id, "skipping (when: expression is falsy)");
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

        // Approval gate: pause BEFORE dispatching the step. The
        // outer `run_workflow` flips the persisted RunRecord to
        // AwaitingApproval and exits cleanly. On resume the step's
        // id matches `approved_step_id`, so we skip the gate this
        // pass.
        if let Some(approval) = &step.approval {
            if approval.required && approved_step_id != Some(step.id.as_str()) {
                let prompt = match &approval.prompt {
                    Some(template) => render_step_prompt(template, &ctx).map_err(|e| {
                        RunWorkflowError::Render {
                            step: step.id.clone(),
                            source: e,
                        }
                    })?,
                    None => format!(
                        "Approve step `{}` of workflow `{}`?",
                        step.id, opts.workflow.name
                    ),
                };
                info!(step = %step.id, "pausing for approval");
                return Ok(InnerOutcome::Paused {
                    step_id: step.id.clone(),
                    prompt,
                    timeout_seconds: approval.timeout_seconds,
                });
            }
        }

        let effective_continue_on_error =
            step.continue_on_error.unwrap_or(workflow_default_continue);

        let result = if step.panel.is_some() {
            run_panel_step(step, &ctx, opts, effective_continue_on_error).await?
        } else if step.parallel.is_some() {
            run_parallel_step(step, &ctx, opts, effective_continue_on_error).await?
        } else if step.for_each.is_some() {
            run_fanout_step(step, &ctx, opts, effective_continue_on_error).await?
        } else {
            run_linear_step(step, &ctx, opts, effective_continue_on_error).await?
        };
        persist_step_result(opts, run_id, &result);
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
            },
        );
    }
    ctx
}

/// Single-shot linear step: render the prompt, build agent opts via
/// the factory, run the agent, capture final assistant text, return
/// a `StepResult`.
async fn run_linear_step(
    step: &Step,
    ctx: &StepContext,
    opts: &OrchestratorRunOpts,
    continue_on_error: bool,
) -> Result<StepResult, RunWorkflowError> {
    let prompt = step
        .prompt
        .as_deref()
        .expect("validate_step_shape guarantees prompt for linear steps");
    let agent_name = step
        .agent
        .as_deref()
        .expect("validate_step_shape guarantees agent for linear steps");
    let rendered = render_step_prompt(prompt, ctx).map_err(|e| RunWorkflowError::Render {
        step: step.id.clone(),
        source: e,
    })?;
    let run_id = format!("run_{}", Ulid::new());
    let transcript_path = opts.transcript_dir.join(format!("{run_id}.jsonl"));

    let outcome = dispatch_one(
        &opts.factory,
        &step.id,
        agent_name,
        rendered.clone(),
        run_id.clone(),
        opts.workspace_id.clone(),
        opts.workspace_path.clone(),
        transcript_path.clone(),
    )
    .await;

    let success = match outcome {
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
    Ok(StepResult {
        step_id: step.id.clone(),
        rendered_prompt: rendered,
        run_id,
        transcript_path,
        output,
        success,
        skipped: false,
        items: Vec::new(),
        ..Default::default()
    })
}

/// Fan-out step: render `for_each:` to a list, then dispatch the
/// step's agent + prompt template per item. Items run with up to
/// `max_parallel` concurrency (default 1). Per-item failures honor
/// `continue_on_error`: when set, failed items are recorded with
/// `success=false` and the rest still run; otherwise the first
/// failed item aborts the workflow.
async fn run_fanout_step(
    step: &Step,
    ctx: &StepContext,
    opts: &OrchestratorRunOpts,
    continue_on_error: bool,
) -> Result<StepResult, RunWorkflowError> {
    let for_each_expr = step
        .for_each
        .as_ref()
        .expect("run_fanout_step called for a non-fan-out step");
    let rendered_list =
        render_step_prompt(for_each_expr, ctx).map_err(|e| RunWorkflowError::Render {
            step: step.id.clone(),
            source: e,
        })?;
    let items = parse_fanout_items(&rendered_list);

    if items.is_empty() {
        info!(step = %step.id, "for_each rendered to an empty list; recording as success with no items");
        return Ok(StepResult {
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
        });
    }

    let max_parallel = step.max_parallel.unwrap_or(1).max(1) as usize;
    let semaphore = Arc::new(Semaphore::new(max_parallel));
    let total = items.len();

    // Render each item's prompt up front so a per-item template
    // error is reported before any agent dispatches. Each item gets
    // its own clone of the parent context with `item` + `loop` bound.
    let mut prepared: Vec<(usize, serde_json::Value, String, String, PathBuf)> =
        Vec::with_capacity(total);
    for (idx, item) in items.iter().enumerate() {
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
            render_step_prompt(item_prompt, &item_ctx).map_err(|e| RunWorkflowError::Render {
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
    let mut handles = Vec::with_capacity(total);
    for (idx, item_value, rendered, run_id, transcript_path) in prepared {
        let permit_sem = semaphore.clone();
        let factory = Arc::clone(&opts.factory);
        let step_id = step.id.clone();
        let agent_name = agent_name_root.clone();
        let workspace_id = opts.workspace_id.clone();
        let workspace_path = opts.workspace_path.clone();
        let rendered_clone = rendered.clone();
        let run_id_clone = run_id.clone();
        let transcript_clone = transcript_path.clone();

        handles.push(tokio::spawn(async move {
            // Held for the duration of this item's run; dropping it
            // releases a slot back to the pool.
            let _permit = permit_sem
                .acquire_owned()
                .await
                .expect("semaphore not closed");
            let outcome = dispatch_one(
                &factory,
                &step_id,
                &agent_name,
                rendered_clone.clone(),
                run_id_clone.clone(),
                workspace_id,
                workspace_path,
                transcript_clone.clone(),
            )
            .await;
            let (success, error_str, raw_error) = match outcome {
                Ok(()) => (true, None, None),
                Err(e) => (false, Some(e.to_string()), Some(e)),
            };
            let output =
                read_final_assistant_text(&transcript_clone, success, &run_id_clone, &step_id);
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

    // Apply `continue_on_error`: if not set, the first failed item
    // aborts the workflow. We surface the original RunError.
    if !continue_on_error {
        if let Some(failed) = item_outcomes.iter_mut().find(|o| !o.success) {
            if let Some(err) = failed.raw_error.take() {
                return Err(RunWorkflowError::Agent {
                    step: format!("{}[{}]", step.id, failed.idx),
                    source: err,
                });
            }
        }
    }

    let items_vec: Vec<ItemResult> = item_outcomes
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
    let outputs: Vec<String> = items_vec.iter().map(|i| i.output.clone()).collect();
    let aggregate_output = serde_json::to_string(&outputs).unwrap_or_else(|_| "[]".into());
    let success = items_vec.iter().all(|i| i.success);

    if !success {
        warn!(
            step = %step.id,
            failed = items_vec.iter().filter(|i| !i.success).count(),
            total,
            "fan-out completed with failed items (continue_on_error tolerated)"
        );
    }

    Ok(StepResult {
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
    })
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
        let rendered =
            render_step_prompt(&sub.prompt, ctx).map_err(|e| RunWorkflowError::Render {
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
            )
            .await;
            let (success, error_str, raw_error) = match outcome {
                Ok(()) => (true, None, None),
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
}

/// Build the agent opts via the factory and dispatch one agent run.
/// Shared by the linear and fan-out paths.
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
) -> Result<(), RunError> {
    let agent_opts = factory
        .build_opts_for_step(
            step_id,
            agent_name,
            rendered_prompt,
            run_id,
            workspace_id,
            workspace_path,
            transcript_path,
        )
        .await;
    run_agent(agent_opts).await.map(|_| ())
}

/// Read the just-finished transcript to extract the final assistant
/// text. The JSONL reader silently skips truncated lines, so this is
/// robust against half-written transcripts. We do this even on
/// failure so partial output is observable to downstream `when:`
/// gates.
fn read_final_assistant_text(
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

/// Validate user-provided `inputs` against the workflow's declared
/// `inputs:` block: required-ness, enum membership, and per-type
/// coercion. Returns the effective input map (declared defaults
/// applied for missing entries) used by every step's template
/// context.
fn resolve_inputs(
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
    step: &Step,
    ctx: &StepContext,
    opts: &OrchestratorRunOpts,
    continue_on_error: bool,
) -> Result<StepResult, RunWorkflowError> {
    let panel = step
        .panel
        .as_ref()
        .expect("run_panel_step called for a non-panel step");

    // Render the initial subject once against the parent context.
    // When a `gate:` loop is configured, subsequent iterations
    // re-bind the subject to the fixer agent's output.
    let initial_subject =
        render_step_prompt(&panel.subject, ctx).map_err(|e| RunWorkflowError::Render {
            step: format!("{}.subject", step.id),
            source: e,
        })?;

    // No gate → run a single panel pass and return.
    let Some(gate) = &panel.gate else {
        return run_panel_iteration(step, panel, ctx, opts, continue_on_error, &initial_subject)
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
        let pass = run_panel_iteration(step, panel, ctx, opts, continue_on_error, &subject).await?;
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
        let fixer_outcome = dispatch_fixer(step, &gate.fix_with, &fixer_subject, opts).await?;
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

async fn dispatch_fixer(
    step: &Step,
    fixer_agent: &str,
    rendered_prompt: &str,
    opts: &OrchestratorRunOpts,
) -> Result<FixerOutcome, RunWorkflowError> {
    let run_id = format!("run_{}", Ulid::new());
    let transcript_path = opts.transcript_dir.join(format!("{run_id}.jsonl"));
    let outcome = dispatch_one(
        &opts.factory,
        &step.id,
        fixer_agent,
        rendered_prompt.to_string(),
        run_id.clone(),
        opts.workspace_id.clone(),
        opts.workspace_path.clone(),
        transcript_path.clone(),
    )
    .await;
    match outcome {
        Ok(()) => {
            let output = read_final_assistant_text(&transcript_path, true, &run_id, &step.id);
            Ok(FixerOutcome::Ok { output })
        }
        Err(e) => Ok(FixerOutcome::Failed(e)),
    }
}

/// Run one panel iteration: dispatch all panelists in parallel
/// against `current_subject` and aggregate findings. Used by both
/// the single-pass and gate-loop paths.
async fn run_panel_iteration(
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
    // binding) or — when omitted — the current subject verbatim.
    let mut prepared: Vec<(usize, String, String, String, PathBuf)> = Vec::with_capacity(total);
    for (idx, panelist) in panel.panelists.iter().enumerate() {
        let mut item_ctx = ctx.clone();
        item_ctx
            .inputs
            .insert("subject".to_string(), current_subject.to_string());
        let rendered = match &panel.prompt {
            Some(template) => {
                render_step_prompt(template, &item_ctx).map_err(|e| RunWorkflowError::Render {
                    step: format!("{}.{}", step.id, panelist),
                    source: e,
                })?
            }
            None => current_subject.to_string(),
        };
        let run_id = format!("run_{}", Ulid::new());
        let transcript_path = opts.transcript_dir.join(format!("{run_id}.jsonl"));
        prepared.push((idx, panelist.clone(), rendered, run_id, transcript_path));
    }

    // Spawn each panelist with the concurrency cap.
    let semaphore = Arc::new(Semaphore::new(max_parallel));
    let mut handles = Vec::with_capacity(total);
    for (idx, agent_name, rendered, run_id, transcript_path) in prepared {
        let permit_sem = semaphore.clone();
        let factory = Arc::clone(&opts.factory);
        let parent_step_id = step.id.clone();
        let workspace_id = opts.workspace_id.clone();
        let workspace_path = opts.workspace_path.clone();
        let rendered_clone = rendered.clone();
        let run_id_clone = run_id.clone();
        let transcript_clone = transcript_path.clone();
        let agent_name_clone = agent_name.clone();

        handles.push(tokio::spawn(async move {
            let _permit = permit_sem
                .acquire_owned()
                .await
                .expect("semaphore not closed");
            let outcome = dispatch_one(
                &factory,
                &parent_step_id,
                &agent_name_clone,
                rendered_clone.clone(),
                run_id_clone.clone(),
                workspace_id,
                workspace_path,
                transcript_clone.clone(),
            )
            .await;
            let (success, _err_str, raw_error) = match outcome {
                Ok(()) => (true, None, None),
                Err(e) => (false, Some(e.to_string()), Some(e)),
            };
            let output = read_final_assistant_text(
                &transcript_clone,
                success,
                &run_id_clone,
                &parent_step_id,
            );
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
