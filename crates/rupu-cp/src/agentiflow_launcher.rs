//! Port: starts an agentiflow detached (`POST /api/agentiflows`). rupu-cp
//! defines it; rupu-cli's `cp serve` provides the adapter, which runs the
//! same detached start `rupu agentiflow run --detach` does.

use std::path::PathBuf;

/// A request to start an agentiflow run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentiflowLaunchRequest {
    /// The definition's name (`agentiflows/<definition>.yaml`).
    pub definition: String,
    /// The run id, minted by the caller (`rupu_agentiflow::new_run_id`) so it
    /// can answer with it; the run records itself under it.
    pub run_id: String,
    /// The directory the run starts in: its project (definition shadowing,
    /// layered config, profiles) and the lead's workspace.
    pub working_dir: PathBuf,
}

/// How the run was doing when the launcher stopped waiting for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentiflowStart {
    /// Its record exists: the run started.
    Started,
    /// Neither started nor exited within the launcher's wait; it is still
    /// going in the background and writes its output to `log`.
    Pending { log: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum AgentiflowLaunchError {
    /// The child exited before the run started (no credential, a lead agent
    /// that does not load, ...): its log is the reason. Nothing is left on
    /// disk.
    #[error("the agentiflow did not start: {0}")]
    NotStarted(String),
    /// The child could not be spawned at all.
    #[error("failed to start the agentiflow: {0}")]
    Spawn(String),
}

#[async_trait::async_trait]
pub trait AgentiflowLauncher: Send + Sync {
    async fn launch(
        &self,
        req: AgentiflowLaunchRequest,
    ) -> Result<AgentiflowStart, AgentiflowLaunchError>;
}
