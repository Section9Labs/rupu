/// A request to start a fresh agent run.
#[derive(Debug, Clone)]
pub struct AgentLaunchRequest {
    pub agent: String,
    pub prompt: Option<String>,
    pub mode: Option<String>,
    pub target: Option<String>,
    /// Working directory for the run (project/dir target). When `None` the
    /// run executes in the cp-serve process's cwd.
    pub working_dir: Option<String>,
    /// Run id to execute under, when the caller already minted one (a
    /// placed unit's coordinator, see `UnitDispatch::run_id`). Honoured by
    /// connectors that mint ids themselves (SSH); the local and HTTP
    /// connectors ignore it this arc. `None` → the connector mints.
    pub run_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentLaunchError {
    #[error("invalid launch request: {0}")]
    Invalid(String),
    #[error("failed to start run: {0}")]
    Spawn(String),
}

/// Port: starts agent runs. rupu-cp defines it; rupu-cli's `cp serve` provides
/// the subprocess-spawning adapter. Returns the new run id.
#[async_trait::async_trait]
pub trait AgentLauncher: Send + Sync {
    async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError>;
}
