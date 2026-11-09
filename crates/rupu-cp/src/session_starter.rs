/// A request to start a fresh agent session.
#[derive(Debug, Clone)]
pub struct SessionStartRequest {
    /// Immutable own-turn tool cap, distinct from declared tools.
    pub tool_ceiling: Option<Vec<String>>,
    pub effort: Option<String>,
    pub agent: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// First-turn declared tools; None inherits the agent, Some([]) removes
    /// declared tools while ambient/origin grants remain.
    pub tools: Option<Vec<String>>,
    pub mcp_tools: Option<Vec<rupu_runtime::session_mcp::ToolRef>>,
    pub mcp_catalog_revision: Option<String>,
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
    /// Builtin-only skill preset (`rupu-session-v1`, `rupu-session-full-v1` or `none`).
    pub skill_preset: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionStartError {
    #[error("invalid session start request: {0}")]
    Invalid(String),
    #[error("session selection changed: {0}")]
    Conflict(String),
    #[error("failed to start session: {0}")]
    Spawn(String),
}

/// Port: starts agent sessions. rupu-cp defines it; rupu-cli's `cp serve`
/// provides the subprocess-spawning adapter. Returns the new session id.
#[async_trait::async_trait]
pub trait SessionStarter: Send + Sync {
    fn supports_session_tool_ceiling(&self) -> bool {
        false
    }
    fn supports_exact_global_conversation(&self) -> bool {
        false
    }
    #[cfg(unix)]
    async fn start_exact_global(
        &self,
        _req: SessionStartRequest,
        _binding: rupu_runtime::exact_agent::Binding,
    ) -> Result<String, SessionStartError> {
        Err(SessionStartError::Invalid(
            "exact global conversations unsupported by this starter".into(),
        ))
    }
    /// The worker provides owner-bound, bounded builtin session read bindings.
    fn supports_session_scoped_reads(&self) -> bool {
        false
    }
    /// True only for an authenticated, live, private-channel approval adapter.
    fn supports_session_approvals(&self) -> bool {
        false
    }
    #[cfg(unix)]
    fn approval_broker(
        &self,
    ) -> Option<std::sync::Arc<rupu_runtime::session_approvals::broker::Broker>> {
        None
    }
    /// Both creation and the worker enforce frozen configured MCP selections.
    fn supports_session_mcp(&self) -> bool {
        false
    }
    fn supports_reasoning_effort(&self) -> bool {
        false
    }
    /// Exact pinned full-suite skill preset, independently gated from legacy v1.
    fn supports_full_skill_preset(&self) -> bool {
        false
    }
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
