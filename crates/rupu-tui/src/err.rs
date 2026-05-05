use std::path::PathBuf;
use thiserror::Error;

/// All errors surfaced by the TUI. Library callers (rupu-cli) map
/// these to anyhow::Error at the boundary.
#[derive(Debug, Error)]
pub enum TuiError {
    #[error("run `{0}` not found in {1}")]
    RunNotFound(String, PathBuf),

    #[error("workflow spec not found for run `{0}` (degraded mode)")]
    WorkflowSpecMissing(String),

    #[error("terminal too narrow (need at least {min} cols, have {have})")]
    TerminalTooNarrow { min: u16, have: u16 },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("transcript: {0}")]
    Transcript(#[from] serde_json::Error),

    #[error("orchestrator: {0}")]
    Orchestrator(#[from] rupu_orchestrator::RunStoreError),

    #[error("notify: {0}")]
    Notify(#[from] notify::Error),
}

pub type TuiResult<T> = Result<T, TuiError>;
