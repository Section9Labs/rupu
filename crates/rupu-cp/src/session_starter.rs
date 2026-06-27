/// A request to start a fresh agent session.
#[derive(Debug, Clone)]
pub struct SessionStartRequest {
    pub agent: String,
    pub prompt: Option<String>,
    pub mode: Option<String>,
    pub target: Option<String>,
    /// Working directory for the session (project/dir target). When `None` the
    /// session executes in the cp-serve process's cwd.
    pub working_dir: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionStartError {
    #[error("invalid session start request: {0}")]
    Invalid(String),
    #[error("failed to start session: {0}")]
    Spawn(String),
}

/// Port: starts agent sessions. rupu-cp defines it; rupu-cli's `cp serve`
/// provides the subprocess-spawning adapter. Returns the new session id.
#[async_trait::async_trait]
pub trait SessionStarter: Send + Sync {
    async fn start(&self, req: SessionStartRequest) -> Result<String, SessionStartError>;
}
