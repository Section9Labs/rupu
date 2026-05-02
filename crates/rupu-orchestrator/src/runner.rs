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

use crate::templates::{render_step_prompt, RenderError, StepContext, StepOutput};
use crate::workflow::{Workflow, WorkflowParseError};
use async_trait::async_trait;
use rupu_agent::{run_agent, AgentRunOpts, RunError};
use rupu_transcript::{Event, JsonlReader};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tracing::warn;
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
}

#[derive(Debug, Clone)]
pub struct StepResult {
    pub step_id: String,
    pub rendered_prompt: String,
    pub run_id: String,
    pub transcript_path: PathBuf,
    /// Final assistant text from this step (used as input for the
    /// next step's template).
    pub output: String,
}

pub struct OrchestratorRunResult {
    pub step_results: Vec<StepResult>,
}

pub async fn run_workflow(
    opts: OrchestratorRunOpts,
) -> Result<OrchestratorRunResult, RunWorkflowError> {
    std::fs::create_dir_all(&opts.transcript_dir)?;
    let mut step_results: Vec<StepResult> = Vec::new();

    for step in &opts.workflow.steps {
        // Build template context from inputs + prior step outputs.
        let mut ctx = StepContext::new();
        ctx.inputs = opts.inputs.clone();
        for prior in &step_results {
            ctx.steps.insert(
                prior.step_id.clone(),
                StepOutput {
                    output: prior.output.clone(),
                },
            );
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

        run_agent(agent_opts)
            .await
            .map_err(|e| RunWorkflowError::Agent {
                step: step.id.clone(),
                source: e,
            })?;

        // Read the just-finished transcript to extract the final
        // assistant text. The reader silently skips truncated lines,
        // so this is robust against half-written transcripts.
        let mut output = String::new();
        if let Ok(iter) = JsonlReader::iter(&transcript_path) {
            for ev in iter.flatten() {
                if let Event::AssistantMessage { content, .. } = ev {
                    output = content;
                }
            }
        } else {
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
        });
    }

    Ok(OrchestratorRunResult { step_results })
}
