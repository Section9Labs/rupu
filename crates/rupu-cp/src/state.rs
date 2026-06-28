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
    /// Optional repo-lister port. Defaults to `None`; rupu-cli's `cp serve`
    /// installs the registry-backed adapter via [`AppState::with_repos`].
    pub repos: Option<Arc<dyn crate::repos::RepoLister>>,
    /// Optional agent-launcher port. Defaults to `None`; rupu-cli's `cp serve`
    /// installs a subprocess-spawning adapter via [`AppState::with_agent_launcher`].
    pub agent_launcher: Option<Arc<dyn crate::agent_launcher::AgentLauncher>>,
    /// Optional session-starter port. Defaults to `None`; rupu-cli's `cp serve`
    /// installs a subprocess-spawning adapter via [`AppState::with_session_starter`].
    pub session_starter: Option<Arc<dyn crate::session_starter::SessionStarter>>,
    /// Host registry. Defaults to a local-only registry (no launchers) so that
    /// read-only `rupu cp` works without a running daemon. `cp serve` replaces
    /// this with a fully-wired registry via [`AppState::with_hosts`].
    pub hosts: Arc<crate::host::registry::HostRegistry>,
}

impl AppState {
    pub fn new(global_dir: PathBuf, pricing: rupu_config::PricingConfig) -> Self {
        let run_store = Arc::new(RunStore::new(global_dir.join("runs")));
        let workspace_dir =
            std::env::current_dir().unwrap_or_else(|_| global_dir.clone());

        // Build a read-only local-only registry. All launchers are `None` so
        // write-path operations return `HostConnectorError::Invalid`; list/get
        // run queries work fine because they only need `run_store`.
        let local = crate::host::local::LocalHostConnector::new(
            None,
            None,
            None,
            None,
            Arc::clone(&run_store),
            global_dir.clone(),
        )
        .with_pricing(pricing.clone());
        let store = rupu_workspace::HostStore { root: global_dir.join("hosts") };
        let hosts = Arc::new(crate::host::registry::HostRegistry::new(
            store,
            Arc::new(local),
        ));

        Self {
            global_dir,
            workspace_dir,
            run_store,
            pricing,
            launcher: None,
            session_sender: None,
            repos: None,
            agent_launcher: None,
            session_starter: None,
            hosts,
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

    /// Install a repo-lister adapter (or clear it with `None`).
    pub fn with_repos(
        mut self,
        repos: Option<Arc<dyn crate::repos::RepoLister>>,
    ) -> Self {
        self.repos = repos;
        self
    }

    /// Install an agent-launcher adapter (or clear it with `None`).
    pub fn with_agent_launcher(
        mut self,
        launcher: Option<Arc<dyn crate::agent_launcher::AgentLauncher>>,
    ) -> Self {
        self.agent_launcher = launcher;
        self
    }

    /// Install a session-starter adapter (or clear it with `None`).
    pub fn with_session_starter(
        mut self,
        starter: Option<Arc<dyn crate::session_starter::SessionStarter>>,
    ) -> Self {
        self.session_starter = starter;
        self
    }

    /// Replace the host registry. Used by `cp serve` to install a fully-wired
    /// registry (with real launchers) after the default read-only one built in
    /// [`AppState::new`].
    pub fn with_hosts(mut self, hosts: Arc<crate::host::registry::HostRegistry>) -> Self {
        self.hosts = hosts;
        self
    }

    /// Override the workspace dir. Useful in tests and when the CP is
    /// launched with an explicit `--workspace` argument.
    pub fn with_workspace_dir(mut self, p: PathBuf) -> Self {
        self.workspace_dir = p;
        self
    }
}
