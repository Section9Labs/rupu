use std::path::PathBuf;
use thiserror::Error;

/// All errors surfaced by the TUI. Library callers (rupu-cli) map
/// these to anyhow::Error at the boundary.
#[derive(Debug, Error)]
pub enum TuiError {
    #[error("run `{0}` not found in {1}")]
    RunNotFound(String, PathBuf),

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
