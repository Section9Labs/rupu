use rupu_orchestrator::runs::RunStore;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub global_dir: PathBuf,
    /// Workspace dir — used for coverage data that lives under
    /// `<workspace>/.rupu/coverage/`. Defaults to `std::env::current_dir()`
    /// at construction time (Phase-1: single-project scope).
    pub workspace_dir: PathBuf,
    pub run_store: Arc<RunStore>,
    pub pricing: rupu_config::PricingConfig,
    /// Optional run-launcher port. Defaults to `None`; rupu-cli's `cp serve`
    /// installs a subprocess-spawning adapter via [`AppState::with_launcher`].
    pub launcher: Option<Arc<dyn crate::launcher::RunLauncher>>,
    /// Optional session-sender port. Defaults to `None`; rupu-cli's `cp serve`
    /// installs a subprocess-spawning adapter via
    /// [`AppState::with_session_sender`].
    pub session_sender: Option<Arc<dyn crate::session_sender::SessionSender>>,
}

impl AppState {
    pub fn new(global_dir: PathBuf, pricing: rupu_config::PricingConfig) -> Self {
        let run_store = Arc::new(RunStore::new(global_dir.join("runs")));
        let workspace_dir =
            std::env::current_dir().unwrap_or_else(|_| global_dir.clone());
        Self {
            global_dir,
            workspace_dir,
            run_store,
            pricing,
            launcher: None,
            session_sender: None,
        }
    }

    /// Install a run-launcher adapter (or clear it with `None`).
    pub fn with_launcher(
        mut self,
        launcher: Option<Arc<dyn crate::launcher::RunLauncher>>,
    ) -> Self {
        self.launcher = launcher;
        self
    }

    /// Install a session-sender adapter (or clear it with `None`).
    pub fn with_session_sender(
        mut self,
        sender: Option<Arc<dyn crate::session_sender::SessionSender>>,
    ) -> Self {
        self.session_sender = sender;
        self
    }

    /// Override the workspace dir. Useful in tests and when the CP is
    /// launched with an explicit `--workspace` argument.
    pub fn with_workspace_dir(mut self, p: PathBuf) -> Self {
        self.workspace_dir = p;
        self
    }
}
