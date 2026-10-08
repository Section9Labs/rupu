use std::collections::BTreeMap;

/// A request to start a fresh workflow run.
#[derive(Debug, Clone, Default)]
pub struct LaunchRequest {
    pub workflow: String,
    pub inputs: BTreeMap<String, String>,
    pub mode: Option<String>,
    pub target: Option<String>,
    /// Working directory for the run (project/dir target). When `None` the
    /// run executes in the cp-serve process's cwd.
    pub working_dir: Option<String>,
}

impl LaunchRequest {
    /// The detached `rupu workflow run --plain` this request asks for, under
    /// `run_id`. A mode that isn't `ask` / `bypass` / `readonly` is refused
    /// here, not by the child.
    pub fn workflow_run(&self, run_id: &str) -> Result<rupu_runtime::argv::WorkflowRun, String> {
        use rupu_runtime::argv::{WorkflowRef, WorkflowRun};
        let mut run = WorkflowRun::new(WorkflowRef::Name(self.workflow.clone()), run_id);
        run.target = self.target.clone();
        run.mode = crate::agent_launcher::parse_mode(self.mode.as_deref())?;
        run.inputs = self.inputs.clone();
        run.plain = true;
        Ok(run)
    }
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
