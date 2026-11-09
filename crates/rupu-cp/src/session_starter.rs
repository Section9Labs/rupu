/// A request to start a fresh agent session.
#[derive(Debug, Clone)]
pub struct SessionStartRequest {
    pub agent: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// First-turn declared tools; None inherits the agent, Some([]) removes
    /// declared tools while ambient/origin grants remain.
    pub tools: Option<Vec<String>>,
    /// Expected registered workspace identity; checked before creating the session.
    pub workspace_id: Option<String>,
    /// Persist an idle session without creating a turn or spawning a worker.
    pub idle: bool,
    pub prompt: Option<String>,
    pub mode: Option<String>,
    pub target: Option<String>,
    /// Working directory for the session (project/dir target). When `None` the
    /// session executes in the cp-serve process's cwd.
    pub working_dir: Option<String>,
    /// `rupu session start --skill`, one flag per entry: skills the session
    /// is given beside its agent's. Well-formed (`parse_grants`); whether
    /// each is installed is the starting `rupu`'s call.
    pub skills: Vec<String>,
    /// `rupu session start --preload-skill`, as `skills`.
    pub preload_skills: Vec<String>,
    /// Builtin-only skill preset (`rupu-session-v1` or `none`).
    pub skill_preset: Option<String>,
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
    fn supports_session_capabilities(&self) -> bool {
        false
    }
    /// The adapter freezes explicit tool selections before the first turn.
    fn supports_session_tool_selection(&self) -> bool {
        false
    }
    /// The adapter preserves project identity and first-turn model selection.
    fn supports_builtin_harness(&self) -> bool {
        false
    }

    /// The adapter can create a zero-turn session without implicit execution.
    fn supports_idle_harness(&self) -> bool {
        false
    }

    async fn start(&self, req: SessionStartRequest) -> Result<String, SessionStartError>;
}
