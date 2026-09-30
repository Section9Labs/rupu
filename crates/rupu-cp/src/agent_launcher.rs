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
    /// placed unit's coordinator, see `UnitDispatch::run_id`). This arc, SSH
    /// is the ONLY connector that honours it — it passes the id through as
    /// `rupu run --run-id <id>`. Every other connector (local, HTTP, tunnel,
    /// bucket) ignores it and mints its own, which is what
    /// `HostConnector::honours_supplied_run_id` reports. `None` → the
    /// connector mints.
    pub run_id: Option<String>,
    /// Findings contract override for this run — `rupu run --findings-profile`,
    /// the highest-precedence input to the run's profile resolution. A placed
    /// unit's coordinator sets it from the step's `findings_profile` /
    /// the workflow's `defaults.findings_profile` (the part of the precedence
    /// chain only the coordinator knows); `None` ⇒ the executing host resolves
    /// from the agent file's `findingsProfile`, else `full`.
    ///
    /// Unlike `run_id`, EVERY connector honours this — local, HTTP, tunnel,
    /// bucket and SSH all put it on the `rupu run` argv (directly or via the
    /// peer that builds it). A connector that cannot deliver it to the host
    /// (e.g. a tunnel node too old to advertise support) must refuse the
    /// launch rather than run the agent under a different profile.
    pub findings_profile: Option<rupu_coverage::FindingProfile>,
    /// Codename minted by a placed unit's coordinator. Forwarded to the
    /// remote `rupu run` as `RUPU_CODENAME` (an env var, so an older remote
    /// binary ignores it instead of rejecting an unknown flag). SSH only this
    /// arc; other connectors ignore it and the coordinator's records still
    /// carry the name.
    pub codename: Option<String>,
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
