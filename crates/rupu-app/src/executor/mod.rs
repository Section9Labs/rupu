//! AppExecutor — singleton per app instance. Wraps an
//! Arc<InProcessExecutor>; routes attach() between in-process tail
//! and disk-tail; mirrors approve/reject/cancel to the right backend.

pub mod attach;

use std::path::{Path, PathBuf};
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
    /// OpenAI-compatible provider params resolved from the layered
    /// config, keyed by provider name (ISSUES.md I-19). Empty when no
    /// `[providers.<name>] kind = "openai-compatible"` is declared.
    pub openai_compatible:
        std::collections::HashMap<String, rupu_runtime::provider_factory::OpenAiCompatibleParams>,
    /// Resolved `[providers.<name>]` runtime knobs, keyed by provider name
    /// (ISSUES.md I-19, mirrors `rupu run` / `rupu workflow run`).
    pub provider_tuning: std::collections::HashMap<String, rupu_providers::ProviderTuning>,
    /// `default_provider` from the layered config.
    pub default_provider: Option<String>,
    /// `default_model` from the layered config.
    pub default_model: Option<String>,
    /// `[bash].timeout_secs` from the layered config.
    pub bash_timeout_secs: u64,
    /// `[bash].env_allowlist` from the layered config.
    pub bash_env_allowlist: Vec<String>,
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
            // Threaded from the layered `rupu_config::Config` loaded in
            // `build_executor` (ISSUES.md I-19, fixed) — global
            // `~/.rupu/config.toml` merged with this workspace's
            // `.rupu/config.toml`, same as `rupu run` / `rupu workflow run`.
            openai_compatible: self.config.openai_compatible.clone(),
            provider_tuning: self.config.provider_tuning.clone(),
            default_provider: self.config.default_provider.clone(),
            default_model: self.config.default_model.clone(),
            bash_timeout_secs: self.config.bash_timeout_secs,
            bash_env_allowlist: self.config.bash_env_allowlist.clone(),
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

/// Load the layered config that wires every workflow this workspace's
/// `AppExecutor` runs: global `<global>/config.toml` merged with the
/// opened workspace's own `.rupu/config.toml` (project overrides global,
/// key-by-key). Extracted as a pure helper so it's unit-testable without
/// standing up GPUI or a `Workspace` (ISSUES.md I-19).
///
/// Unlike `rupu run`'s `paths::project_root_for`, there is no upward walk
/// from a cwd here — the desktop app already knows its project root: it's
/// the workspace directory the user opened. `.rupu/config.toml` not
/// existing under it is normal (not every workspace has one) and resolves
/// to the global-only config, matching `rupu run`'s "missing project file"
/// behavior.
///
/// This is policy-bearing (`[scm]`, `default_provider`/`default_model`,
/// `[bash]`, `[providers.*]` tuning) so it goes through
/// `rupu_config::layer_files_locked`, never plain `layer_files` — see that
/// function's doc comment / ISSUES.md I-7. A malformed config.toml is
/// logged and treated as absent rather than blocking workspace-open
/// outright: `build_executor` is called from an infallible GPUI action
/// handler / startup path (see `main.rs`, `menu/app_menu.rs`) that already
/// treats "workspace opened" as the point of no return, and a broken
/// desktop config shouldn't make the app unusable.
pub(crate) fn load_workflow_config(global: &Path, workspace_path: &Path) -> rupu_config::Config {
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = workspace_path.join(".rupu/config.toml");
    rupu_config::layer_files_locked(Some(&global_cfg_path), Some(&project_cfg_path)).unwrap_or_else(
        |e| {
            tracing::warn!(
                global = %global_cfg_path.display(),
                project = %project_cfg_path.display(),
                error = %e,
                "config layering failed; falling back to defaults"
            );
            rupu_config::Config::default()
        },
    )
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

    // Load the real layered config (global + workspace `.rupu/config.toml`)
    // instead of `Config::default()` (ISSUES.md I-19, fixed). `[scm]`
    // credentials still come from the keychain via `resolver`; this also
    // feeds config-declared platform settings (e.g. base URLs) to
    // `Registry::discover`. Missing platform configs are silently skipped
    // (same as CLI behaviour when no `[scm]` section is present).
    let cfg = load_workflow_config(&global, &workspace_path);

    // Registry::discover is async; build_executor is called from the
    // synchronous GPUI app closure. The ambient Tokio runtime (installed
    // in main before GPUI starts) provides a Handle we can block on via
    // block_in_place so we don't block the runtime's async executor.
    let mcp_registry = {
        let resolver_ref = Arc::clone(&resolver);
        let cfg_ref = &cfg;
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                Arc::new(rupu_scm::Registry::discover(resolver_ref.as_ref(), cfg_ref).await)
            })
        })
    };

    let openai_compatible = rupu_runtime::provider_factory::openai_compatible_map(&cfg.providers);
    let provider_tuning = rupu_runtime::provider_factory::provider_tuning_map(&cfg.providers);

    Arc::new(AppExecutor::new(
        run_store,
        workspace.manifest.id.clone(),
        workspace_path,
        transcript_dir,
        WorkflowConfig {
            global,
            project_root,
            resolver,
            openai_compatible,
            provider_tuning,
            default_provider: cfg.default_provider.clone(),
            default_model: cfg.default_model.clone(),
            bash_timeout_secs: cfg.bash.timeout_secs.unwrap_or(120),
            bash_env_allowlist: cfg.bash.env_allowlist.clone().unwrap_or_default(),
            mcp_registry,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// I-19: the desktop executor's config-loading step must resolve real
    /// values from disk (global + project layering), not the hardcoded
    /// `Config::default()` `build_executor` used to pass to
    /// `Registry::discover`/the step factory. `build_executor` itself isn't
    /// directly unit-testable (it needs a `Workspace` + ambient Tokio
    /// runtime + GPUI closure context), so this exercises the extracted
    /// `load_workflow_config` helper it calls.
    #[test]
    fn load_workflow_config_reads_the_layered_config() {
        let global_dir = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();

        std::fs::write(
            global_dir.path().join("config.toml"),
            "default_provider = \"anthropic\"\ndefault_model = \"claude-opus-4-6\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(workspace_dir.path().join(".rupu")).unwrap();
        std::fs::write(
            workspace_dir.path().join(".rupu/config.toml"),
            "default_model = \"claude-haiku-4-6\"\n",
        )
        .unwrap();

        let cfg = load_workflow_config(global_dir.path(), workspace_dir.path());

        // Project overrides global key-by-key: default_model comes from the
        // workspace file, default_provider falls through from global —
        // both `Some`, not the `None`/`None` `Config::default()` produced.
        assert_eq!(cfg.default_provider.as_deref(), Some("anthropic"));
        assert_eq!(cfg.default_model.as_deref(), Some("claude-haiku-4-6"));
    }

    #[test]
    fn load_workflow_config_treats_missing_project_file_as_absent() {
        let global_dir = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();

        std::fs::write(
            global_dir.path().join("config.toml"),
            "default_provider = \"anthropic\"\n",
        )
        .unwrap();
        // No workspace_dir/.rupu/config.toml at all.

        let cfg = load_workflow_config(global_dir.path(), workspace_dir.path());
        assert_eq!(cfg.default_provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn load_workflow_config_falls_back_to_defaults_on_malformed_toml() {
        let global_dir = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();

        std::fs::write(global_dir.path().join("config.toml"), "not valid toml [[[").unwrap();

        // Must not panic and must not silently succeed with garbage —
        // falls back to the documented-default Config rather than
        // propagating (build_executor has no Result to return through).
        let cfg = load_workflow_config(global_dir.path(), workspace_dir.path());
        assert_eq!(cfg.default_provider, None);
    }
}
