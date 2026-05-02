//! The agent loop. Real impl lands in Task 7.

use thiserror::Error;

/// Errors that can occur during an agent run.
#[derive(Debug, Error)]
pub enum RunError {
    #[error("provider: {0}")]
    Provider(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("context overflow at turn {turn}")]
    ContextOverflow { turn: u32 },
    #[error("max turns ({max}) reached")]
    MaxTurns { max: u32 },
    #[error("non-tty + ask mode aborted before first prompt")]
    NonTtyAskAbort,
}

/// Options controlling a single agent run.
pub struct AgentRunOpts;

/// Summary of a completed agent run.
pub struct RunResult;

/// Execute an agent run end-to-end: send messages to the provider, dispatch
/// tool calls, apply permission gating, and stream events into the transcript.
pub async fn run_agent(_opts: AgentRunOpts) -> Result<RunResult, RunError> {
    todo!("run_agent lands in Task 7")
}
