//! Linear workflow runner. Real impl in Task 11.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RunWorkflowError {
    #[error("parse: {0}")]
    Parse(#[from] crate::workflow::WorkflowParseError),
    #[error("render: {0}")]
    Render(#[from] crate::templates::RenderError),
    #[error("agent: {0}")]
    Agent(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub struct OrchestratorRunOpts;
pub struct OrchestratorRunResult;

pub async fn run_workflow(
    _opts: OrchestratorRunOpts,
) -> Result<OrchestratorRunResult, RunWorkflowError> {
    todo!("run_workflow lands in Task 11")
}
