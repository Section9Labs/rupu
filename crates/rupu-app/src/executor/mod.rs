//! AppExecutor — singleton per app instance. Wraps an
//! Arc<InProcessExecutor>; routes attach() between in-process tail
//! and disk-tail; mirrors approve/reject/cancel to the right backend.

pub mod attach;
pub mod step_factory;

use std::path::PathBuf;
use std::sync::Arc;

use rupu_orchestrator::executor::{
    EventStream, InProcessExecutor, RunFilter, WorkflowExecutor, WorkflowRunOpts,
};
use rupu_orchestrator::runner::StepFactory;
use rupu_orchestrator::runs::{RunRecord, RunStore};

use crate::executor::attach::attach_stream;
use crate::workspace::Workspace;

pub struct AppExecutor {
    inner: Arc<InProcessExecutor>,
    run_store: Arc<RunStore>,
}

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("run not found: {0}")]
    RunNotFound(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

impl AppExecutor {
    pub fn new(
        run_store: Arc<RunStore>,
        factory: Arc<dyn StepFactory>,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_dir: PathBuf,
    ) -> Self {
        let inner = Arc::new(InProcessExecutor::new(
            run_store.clone(),
            factory,
            workspace_id,
            workspace_path,
            transcript_dir,
        ));
        Self { inner, run_store }
    }

    pub fn run_store(&self) -> &Arc<RunStore> {
        &self.run_store
    }

    pub async fn start_workflow(
        &self,
        workflow_path: PathBuf,
    ) -> Result<String, rupu_orchestrator::executor::ExecutorError> {
        let handle = self
            .inner
            .start(WorkflowRunOpts {
                workflow_path,
                vars: Default::default(),
            })
            .await?;
        Ok(handle.run_id)
    }

    pub fn list_active_runs(&self, workflow_path: Option<PathBuf>) -> Vec<RunRecord> {
        match workflow_path {
            Some(p) => self.inner.list_runs(RunFilter::ByWorkflowPath(p)),
            None => self.inner.list_runs(RunFilter::Active),
        }
    }

    pub async fn attach(&self, run_id: &str) -> Result<EventStream, AttachError> {
        attach_stream(&self.inner, &self.run_store, run_id).await
    }

    pub async fn approve(
        &self,
        run_id: &str,
        approver: &str,
    ) -> Result<(), rupu_orchestrator::executor::ExecutorError> {
        self.inner.approve(run_id, approver).await
    }

    pub async fn reject(
        &self,
        run_id: &str,
        reason: &str,
    ) -> Result<(), rupu_orchestrator::executor::ExecutorError> {
        self.inner.reject(run_id, reason).await
    }

    pub async fn cancel(
        &self,
        run_id: &str,
    ) -> Result<(), rupu_orchestrator::executor::ExecutorError> {
        self.inner.cancel(run_id).await
    }
}

/// Construct the per-workspace `AppExecutor`. The `RunStore` root
/// follows the same convention as the orchestrator CLI:
/// `<data_local_dir>/rupu/runs`. If `dirs::data_local_dir()` fails
/// (unlikely outside unit tests) we fall back to `/tmp/rupu/runs` so
/// the app still launches.
pub fn build_executor(workspace: &Workspace) -> Arc<AppExecutor> {
    let runs_root = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("rupu")
        .join("runs");

    let run_store = Arc::new(RunStore::new(runs_root.clone()));
    let workspace_path = std::path::PathBuf::from(&workspace.manifest.path);
    let transcript_dir = runs_root.join("transcripts");

    let factory: Arc<dyn StepFactory> =
        Arc::new(step_factory::AppStepFactory { workspace_path: workspace_path.clone() });

    Arc::new(AppExecutor::new(
        run_store,
        factory,
        workspace.manifest.id.clone(),
        workspace_path,
        transcript_dir,
    ))
}
