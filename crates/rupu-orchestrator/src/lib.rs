//! rupu-orchestrator — workflow YAML parser + linear runner +
//! action-protocol validator.
//!
//! A workflow is a YAML file declaring a list of `steps:`, each
//! pointing at an agent with a prompt template and an `actions:`
//! allowlist. The runner executes steps in order; the previous
//! step's output is available as `{{ steps.<id>.output }}` in the
//! next step's prompt template (rendered with minijinja).

pub mod action_protocol;
pub mod cron_schedule;
pub mod runner;
pub mod templates;
pub mod workflow;

pub use action_protocol::{validate_actions, ActionValidationResult};
pub use runner::{
    run_workflow, OrchestratorRunOpts, OrchestratorRunResult, RunWorkflowError, StepFactory,
    StepResult,
};
pub use templates::{
    render_step_prompt, render_when_expression, RenderError, StepContext, StepOutput,
};
pub use workflow::{
    InputDef, InputType, Step, Trigger, TriggerKind, Workflow, WorkflowDefaults, WorkflowParseError,
};
