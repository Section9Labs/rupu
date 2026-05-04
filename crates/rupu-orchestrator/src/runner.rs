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

use crate::templates::{
    render_step_prompt, render_when_expression, RenderError, StepContext, StepOutput,
};
use crate::workflow::{yaml_scalar_to_string, InputType, Workflow, WorkflowParseError};
use async_trait::async_trait;
use rupu_agent::{run_agent, AgentRunOpts, RunError};
use rupu_transcript::{Event, JsonlReader};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
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
}

/// Trait the orchestrator uses to construct per-step [`AgentRunOpts`].
/// Production impl wires real providers + the default tool registry;
/// tests inject mock providers.
#[async_trait]
pub trait StepFactory: Send + Sync {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
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
}

#[derive(Debug, Clone)]
pub struct StepResult {
    pub step_id: String,
    pub rendered_prompt: String,
    pub run_id: String,
    pub transcript_path: PathBuf,
    /// Final assistant text from this step (used as input for the
    /// next step's template). Empty for skipped steps and for steps
    /// that errored before producing output.
    pub output: String,
    /// True when the step ran and finished without an agent error.
    pub success: bool,
    /// True when the step was skipped because its `when:` expression
    /// evaluated falsy. `success` is false in that case.
    pub skipped: bool,
}

pub struct OrchestratorRunResult {
    pub step_results: Vec<StepResult>,
}

pub async fn run_workflow(
    opts: OrchestratorRunOpts,
) -> Result<OrchestratorRunResult, RunWorkflowError> {
    std::fs::create_dir_all(&opts.transcript_dir)?;
    let resolved_inputs = resolve_inputs(&opts.workflow, &opts.inputs)?;
    let workflow_default_continue = opts.workflow.defaults.continue_on_error.unwrap_or(false);

    let mut step_results: Vec<StepResult> = Vec::new();

    for step in &opts.workflow.steps {
        // Build template context from inputs + prior step outputs.
        let mut ctx = StepContext::new();
        ctx.inputs = resolved_inputs.clone();
        ctx.event = opts.event.clone();
        for prior in &step_results {
            ctx.steps.insert(
                prior.step_id.clone(),
                StepOutput {
                    output: prior.output.clone(),
                    success: prior.success,
                    skipped: prior.skipped,
                },
            );
        }

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
                step_results.push(StepResult {
                    step_id: step.id.clone(),
                    rendered_prompt: String::new(),
                    run_id: String::new(),
                    transcript_path: PathBuf::new(),
                    output: String::new(),
                    success: false,
                    skipped: true,
                });
                continue;
            }
        }

        let rendered =
            render_step_prompt(&step.prompt, &ctx).map_err(|e| RunWorkflowError::Render {
                step: step.id.clone(),
                source: e,
            })?;

        let run_id = format!("run_{}", Ulid::new());
        let transcript_path = opts.transcript_dir.join(format!("{run_id}.jsonl"));
        let agent_opts = opts
            .factory
            .build_opts_for_step(
                &step.id,
                rendered.clone(),
                run_id.clone(),
                opts.workspace_id.clone(),
                opts.workspace_path.clone(),
                transcript_path.clone(),
            )
            .await;

        let effective_continue_on_error =
            step.continue_on_error.unwrap_or(workflow_default_continue);

        let run_outcome = run_agent(agent_opts).await;
        let success = match run_outcome {
            Ok(_) => true,
            Err(source) => {
                if effective_continue_on_error {
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

        // Read the just-finished transcript to extract the final
        // assistant text. The reader silently skips truncated lines,
        // so this is robust against half-written transcripts. We do
        // this even on failure so partial output is observable to
        // downstream `when:` gates.
        let mut output = String::new();
        if let Ok(iter) = JsonlReader::iter(&transcript_path) {
            for ev in iter.flatten() {
                if let Event::AssistantMessage { content, .. } = ev {
                    output = content;
                }
            }
        } else if success {
            warn!(
                run_id = %run_id,
                "transcript missing after step {}; using empty output",
                step.id
            );
        }

        step_results.push(StepResult {
            step_id: step.id.clone(),
            rendered_prompt: rendered,
            run_id,
            transcript_path,
            output,
            success,
            skipped: false,
        });
    }

    Ok(OrchestratorRunResult { step_results })
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
