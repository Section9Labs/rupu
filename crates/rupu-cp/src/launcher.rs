use std::collections::BTreeMap;

/// A request to start a fresh workflow run.
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub workflow: String,
    pub inputs: BTreeMap<String, String>,
    pub mode: Option<String>,
    pub target: Option<String>,
    /// Working directory for the run (project/dir target). When `None` the
    /// run executes in the cp-serve process's cwd.
    pub working_dir: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("invalid launch request: {0}")]
    Invalid(String),
    #[error("failed to start run: {0}")]
    Spawn(String),
}

/// Port: starts runs. rupu-cp defines it; rupu-cli's `cp serve` provides the
/// subprocess-spawning adapter. Returns the new run id.
#[async_trait::async_trait]
pub trait RunLauncher: Send + Sync {
    async fn launch(&self, req: LaunchRequest) -> Result<String, LaunchError>;
}
