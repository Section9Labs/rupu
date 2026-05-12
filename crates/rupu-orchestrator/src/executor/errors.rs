//! Errors surfaced by the `WorkflowExecutor` trait and its impls.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("workflow parse error: {0}")]
    WorkflowParse(#[from] crate::WorkflowParseError),

    #[error("run not found: {0}")]
    RunNotFound(String),

    #[error("run already active for workflow: {0}")]
    RunAlreadyActive(PathBuf),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("cancelled")]
    Cancelled,

    #[error("internal executor error: {0}")]
    Internal(String),
}
