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
    /// Engagement profile ids — `rupu workflow run --engagement-profile`,
    /// one flag per id; empty = the native `code` path. A local launch
    /// only: the API refuses it on a remote one, since no host transport
    /// carries it.
    pub engagement_profiles: Vec<String>,
    /// Run the workflow in this file instead of the catalog entry
    /// `workflow` names — `rupu workflow run --file`; the run is named by
    /// the file's `name:`. A local launch only, as `engagement_profiles`.
    pub file: Option<std::path::PathBuf>,
}

impl LaunchRequest {
    /// The detached `rupu workflow run --plain` this request asks for, under
    /// `run_id`. A mode that isn't `ask` / `bypass` / `readonly` is refused
    /// here, not by the child.
    pub fn workflow_run(&self, run_id: &str) -> Result<rupu_runtime::argv::WorkflowRun, String> {
        use rupu_runtime::argv::{WorkflowRef, WorkflowRun};
        let workflow = match &self.file {
            Some(file) => WorkflowRef::File(file.clone()),
            None => WorkflowRef::Name(self.workflow.clone()),
        };
        let mut run = WorkflowRun::new(workflow, run_id);
        run.target = self.target.clone();
        run.mode = crate::agent_launcher::parse_mode(self.mode.as_deref())?;
        run.inputs = self.inputs.clone();
        run.plain = true;
        run.engagement_profiles = self.engagement_profiles.clone();
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

    /// Retry a failed, rejected or cancelled run: a detached `rupu workflow
    /// resume <run_id> [--restart-interrupted]`, which flips the run back to
    /// `Running` under its own lock before it runs. Returns once the child
    /// is spawned; the run's status is the confirmation. A launcher that
    /// can't spawn one refuses.
    async fn retry(&self, req: RetryRequest) -> Result<(), LaunchError> {
        Err(LaunchError::Spawn(format!(
            "this launcher cannot retry run {}",
            req.run_id
        )))
    }
}

/// A request to retry a finished workflow run ([`RunLauncher::retry`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryRequest {
    pub run_id: String,
    /// Start every interrupted step and `for_each` unit over from its
    /// prompt instead of continuing it from its transcript.
    pub restart_interrupted: bool,
    /// The permission mode to resume in (`--mode`); `None` resumes in the
    /// run's own launch mode (`RunRecord.permission_mode`).
    pub mode: Option<rupu_tools::PermissionMode>,
    /// Where the child's stderr goes (the run dir's `retry.log`), so a
    /// resume that refuses before it takes the run leaves its reason.
    pub log: std::path::PathBuf,
}

impl RetryRequest {
    /// The `rupu workflow resume` this request asks for: an operator's
    /// retry, so never `--if-unfinished` (that flag refuses exactly the
    /// finished runs a retry is for).
    pub fn argv(&self) -> rupu_runtime::argv::RunArgv {
        use rupu_runtime::argv::{RunArgv, WorkflowControl};
        RunArgv::WorkflowControl(WorkflowControl::Resume {
            run_id: self.run_id.clone(),
            mode: self.mode,
            if_unfinished: false,
            restart_interrupted: self.restart_interrupted,
        })
    }
}
