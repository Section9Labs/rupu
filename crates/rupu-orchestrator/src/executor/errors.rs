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

    /// The requested operation is not implementable in this executor
    /// context. Used by [`crate::executor::WorkflowExecutor::resume`]
    /// on `InProcessExecutor`: resuming needs the original
    /// `StepFactory` the run was started with, which isn't retained
    /// past `start()` returning. A launcher-gated caller (e.g. `rupu
    /// workflow resume` / the CP resume worker) re-enters
    /// `run_workflow` directly with a freshly built factory instead.
    #[error("unsupported: {0}")]
    Unsupported(String),
}
