use rupu_orchestrator::runs::RunStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

#[derive(Clone)]
pub struct AppState {
    pub global_dir: PathBuf,
    /// Workspace dir — used for coverage data that lives under
    /// `<workspace>/.rupu/coverage/`. Defaults to `std::env::current_dir()`
    /// at construction time (Phase-1: single-project scope).
    pub workspace_dir: PathBuf,
    pub run_store: Arc<RunStore>,
    pub pricing: rupu_config::PricingConfig,
    /// The resolved global config snapshot, reloaded after a config write so
    /// newly-started runs see updated values. Read via `config.read()`.
    pub config: Arc<RwLock<rupu_config::Config>>,
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
    /// Optional definition generator; `rupu cp serve` installs the
    /// orchestrator-backed adapter via [`AppState::with_generator`].
    pub generator: Option<Arc<dyn crate::definition_generator::DefinitionGenerator>>,
    /// Optional session-mutator port. Defaults to `None`; rupu-cli's `cp serve`
    /// installs a subprocess adapter via [`AppState::with_session_mutator`].
    pub session_mutator: Option<Arc<dyn crate::session_mutator::SessionMutator>>,
    /// Host registry. Defaults to a local-only registry (no launchers) so that
    /// read-only `rupu cp` works without a running daemon. `cp serve` replaces
    /// this with a fully-wired registry via [`AppState::with_hosts`].
    pub hosts: Arc<crate::host::registry::HostRegistry>,
    /// Per-run memoization for [`crate::api::run_resolve::resolve_run_location`]
    /// — see that function's doc comment for the TTL rationale. Keyed by
    /// `run_id`; value is `(resolved_at, location)`. Shared (not per-request)
    /// so repeated resolving-endpoint calls within one RunDetail page load
    /// reuse the first resolution instead of re-walking every store (and,
    /// worst case, re-probing every registered host) up to 4x.
    pub run_location_cache:
        Arc<Mutex<HashMap<String, (Instant, crate::api::run_resolve::RunLocation)>>>,
    /// Live tunnel connection registry. Shared across all WS handler tasks.
    pub node_registry: Arc<crate::node::NodeRegistry>,
    /// Mirror writer: streams artifact frames from tunnel nodes into the
    /// central [`RunStore`] so node runs appear as first-class runs.
    pub node_mirror: Arc<crate::node::NodeMirror>,
    /// The `--bind` address `rupu cp serve` was started with, as a display
    /// string (e.g. `127.0.0.1:7878`). Surfaced read-only via
    /// `GET /api/config`'s `status.bind` so the settings UI can show it next
    /// to the `restart_required` keys that change requires a restart to
    /// apply. Defaults to the CLI's documented default bind for tests / a
    /// bare `AppState::new`.
    pub bind: String,
    /// Whether `rupu cp serve` was started with a bearer token configured.
    /// A bool ONLY — the token value itself is never stored on `AppState`
    /// (the bearer-check middleware in `server::router` closes over the raw
    /// token directly), so it can never be echoed back through the config
    /// API.
    pub token_set: bool,
}

impl AppState {
    pub fn new(global_dir: PathBuf, pricing: rupu_config::PricingConfig) -> Self {
        let run_store = Arc::new(RunStore::new(global_dir.join("runs")));
        let workspace_dir = std::env::current_dir().unwrap_or_else(|_| global_dir.clone());

        // Build tunnel deps first so they can be wired into the host registry.
        let node_registry = Arc::new(crate::node::NodeRegistry::new());
        let node_mirror = Arc::new(crate::node::NodeMirror::new(Arc::clone(&run_store)));

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
        let store = rupu_workspace::HostStore {
            root: global_dir.join("hosts"),
        };
        let hosts = Arc::new(
            crate::host::registry::HostRegistry::new(store, Arc::new(local)).with_tunnel_deps(
                Arc::clone(&node_registry),
                Arc::clone(&node_mirror),
                Arc::clone(&run_store),
                pricing.clone(),
            ),
        );

        let config = Arc::new(RwLock::new(Self::resolve_global_config(&global_dir)));

        Self {
            global_dir,
            workspace_dir,
            run_store,
            pricing,
            config,
            launcher: None,
            session_sender: None,
            repos: None,
            agent_launcher: None,
            session_starter: None,
            generator: None,
            session_mutator: None,
            hosts,
            run_location_cache: Arc::new(Mutex::new(HashMap::new())),
            node_registry,
            node_mirror,
            bind: "127.0.0.1:7878".to_string(),
            token_set: false,
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
    pub fn with_repos(mut self, repos: Option<Arc<dyn crate::repos::RepoLister>>) -> Self {
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

    /// Install a definition-generator adapter (or clear it with `None`).
    pub fn with_generator(
        mut self,
        generator: Option<Arc<dyn crate::definition_generator::DefinitionGenerator>>,
    ) -> Self {
        self.generator = generator;
        self
    }

    /// Install a session-mutator adapter (or clear it with `None`).
    pub fn with_session_mutator(
        mut self,
        m: Option<Arc<dyn crate::session_mutator::SessionMutator>>,
    ) -> Self {
        self.session_mutator = m;
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

    /// Record the bind address `rupu cp serve` was started with, as a
    /// display string. Purely informational (`GET /api/config`'s
    /// `status.bind`) — changing it here does not rebind the listener.
    pub fn with_bind(mut self, bind: String) -> Self {
        self.bind = bind;
        self
    }

    /// Record whether a bearer token was configured at `cp serve` startup.
    /// The token value itself is never threaded through `AppState`.
    pub fn with_token_set(mut self, token_set: bool) -> Self {
        self.token_set = token_set;
        self
    }

    /// Resolve the global config from `<global_dir>/config.toml` (no project
    /// layer — the CP is a global-scope process). Falls back to
    /// `Config::default()` if the file is absent, unparseable, or invalid, so
    /// a broken config never blocks CP startup — it just serves defaults
    /// until fixed.
    fn resolve_global_config(global_dir: &std::path::Path) -> rupu_config::Config {
        let path = global_dir.join("config.toml");
        match rupu_config::resolve(Some(&path), None, &std::collections::BTreeMap::new()) {
            Ok(r) => r.config,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to resolve global config; using defaults");
                rupu_config::Config::default()
            }
        }
    }

    /// Re-resolve the global config from disk and swap it into the snapshot.
    /// Called after a successful global-config write so already-running
    /// handlers (and newly-started runs) observe the update without a
    /// process restart.
    pub fn reload_config(&self) {
        let resolved = Self::resolve_global_config(&self.global_dir);
        if let Ok(mut w) = self.config.write() {
            *w = resolved;
        }
    }
}
