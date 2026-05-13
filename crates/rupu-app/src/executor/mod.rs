//! AppExecutor — singleton per app instance. Wraps an
//! Arc<InProcessExecutor>; routes attach() between in-process tail
//! and disk-tail; mirrors approve/reject/cancel to the right backend.

pub mod attach;

use std::path::PathBuf;
use std::sync::Arc;

use rupu_orchestrator::executor::{
    EventStream, InProcessExecutor, RunFilter, WorkflowExecutor, WorkflowRunOpts,
};
use rupu_orchestrator::runs::{RunRecord, RunStore};
use rupu_orchestrator::{DefaultStepFactory, StepFactory};

use crate::executor::attach::attach_stream;
use crate::workspace::Workspace;

/// Provider-wiring config threaded into every workflow run started by
/// this executor. Collected in one struct to keep `AppExecutor::new`
/// under clippy's argument-count limit.
pub struct WorkflowConfig {
    pub global: PathBuf,
    pub project_root: Option<PathBuf>,
    pub resolver: Arc<rupu_auth::KeychainResolver>,
    pub mcp_registry: Arc<rupu_scm::Registry>,
}

pub struct AppExecutor {
    inner: Arc<InProcessExecutor>,
    run_store: Arc<RunStore>,
    config: WorkflowConfig,
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
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_dir: PathBuf,
        config: WorkflowConfig,
    ) -> Self {
        let inner = Arc::new(InProcessExecutor::new(
            run_store.clone(),
            workspace_id,
            workspace_path,
            transcript_dir,
        ));
        Self {
            inner,
            run_store,
            config,
        }
    }

    pub fn run_store(&self) -> &Arc<RunStore> {
        &self.run_store
    }

    pub fn config_mcp_registry(&self) -> Arc<rupu_scm::Registry> {
        Arc::clone(&self.config.mcp_registry)
    }

    pub async fn start_workflow(
        &self,
        workflow_path: PathBuf,
    ) -> Result<String, rupu_orchestrator::executor::ExecutorError> {
        let workspace_path = self
            .config
            .project_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        self.start_workflow_with_opts(
            workflow_path,
            Default::default(),
            crate::launcher::LauncherMode::Ask,
            workspace_path,
        )
        .await
    }

    pub async fn start_workflow_with_opts(
        &self,
        workflow_path: PathBuf,
        inputs: std::collections::BTreeMap<String, String>,
        mode: crate::launcher::LauncherMode,
        target_dir: PathBuf,
    ) -> Result<String, rupu_orchestrator::executor::ExecutorError> {
        let yaml = std::fs::read_to_string(&workflow_path)?;
        let workflow = rupu_orchestrator::Workflow::parse(&yaml)?;

        let factory: Arc<dyn StepFactory> = Arc::new(DefaultStepFactory {
            workflow,
            global: self.config.global.clone(),
            project_root: Some(target_dir.clone()),
            resolver: Arc::clone(&self.config.resolver),
            mode_str: mode.as_str().into(),
            mcp_registry: Arc::clone(&self.config.mcp_registry),
            system_prompt_suffix: None,
            dispatcher: None,
        });

        let handle = self
            .inner
            .start(
                WorkflowRunOpts {
                    workflow_path,
                    vars: inputs,
                },
                factory,
            )
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

    // Global rupu dir (mirrors CLI's paths::global_dir). Honors $RUPU_HOME.
    let global = std::env::var("RUPU_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".rupu")))
        .unwrap_or_else(|| PathBuf::from("/tmp/.rupu"));

    // Project root: the workspace path itself (the open directory).
    let project_root = Some(workspace_path.clone());

    let resolver = Arc::new(rupu_auth::KeychainResolver::new());

    // Build the SCM registry. We use an empty config here because
    // the app doesn't yet surface a per-workspace config.toml picker;
    // credentials are sourced from the keychain via resolver. Missing
    // platform configs are silently skipped (same as CLI behaviour
    // when no [scm] section is present).
    //
    // Registry::discover is async; build_executor is called from the
    // synchronous GPUI app closure (no tokio context). Spin up a
    // single-thread tokio runtime just for this one-shot await.
    let mcp_registry = {
        let resolver_ref = Arc::clone(&resolver);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime for registry discovery");
        rt.block_on(async move {
            let cfg = rupu_config::Config::default();
            Arc::new(rupu_scm::Registry::discover(resolver_ref.as_ref(), &cfg).await)
        })
    };

    Arc::new(AppExecutor::new(
        run_store,
        workspace.manifest.id.clone(),
        workspace_path,
        transcript_dir,
        WorkflowConfig {
            global,
            project_root,
            resolver,
            mcp_registry,
        },
    ))
}
