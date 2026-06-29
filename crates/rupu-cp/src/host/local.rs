//! `LocalHostConnector` — local (host[0]) implementation of [`HostConnector`].
//!
//! Delegates control operations to the per-capability port traits
//! (`RunLauncher`, `AgentLauncher`, `SessionStarter`, `SessionSender`) and
//! reads run state from the in-process `RunStore`. List / detail methods call
//! the shared builders in `crate::api::runs` so the JSON shape is identical to
//! what the HTTP API serves.

use std::path::PathBuf;
use std::sync::Arc;

use rupu_orchestrator::{
    runs::{CancelError, RunStore},
    ApprovalError, RunStoreError,
};

use crate::{
    agent_launcher::{AgentLaunchRequest, AgentLauncher},
    api::runs::{query_run_detail, query_run_rows},
    host::connector::{
        open_run_events_tail, read_transcript_file, EventByteStream, HostCapabilities,
        HostConnector, HostConnectorError, HostInfo, RunKind, RunListQuery,
    },
    launcher::{LaunchRequest, RunLauncher},
    session_sender::{SendMessageRequest, SessionSender},
    session_starter::{SessionStartRequest, SessionStarter},
};

// ── Struct ────────────────────────────────────────────────────────────────────

/// Host[0] connector — backed entirely by in-process ports and the local
/// `RunStore`. No network calls; always reachable.
pub struct LocalHostConnector {
    launcher: Option<Arc<dyn RunLauncher>>,
    agent_launcher: Option<Arc<dyn AgentLauncher>>,
    session_starter: Option<Arc<dyn SessionStarter>>,
    session_sender: Option<Arc<dyn SessionSender>>,
    run_store: Arc<RunStore>,
    /// Global rupu directory (e.g. `~/.rupu` or the project-level dir).
    /// Not yet consumed in this slice; retained for Task 5 (AppState wiring)
    /// and Task 9 (host-aware launch).
    #[allow(dead_code)]
    global_dir: PathBuf,
    pricing: rupu_config::PricingConfig,
}

impl LocalHostConnector {
    /// Create a new local connector. Pass `None` for any capability that is not
    /// available; the corresponding `HostConnector` methods will return
    /// `HostConnectorError::Invalid` when called.
    pub fn new(
        launcher: Option<Arc<dyn RunLauncher>>,
        agent_launcher: Option<Arc<dyn AgentLauncher>>,
        session_starter: Option<Arc<dyn SessionStarter>>,
        session_sender: Option<Arc<dyn SessionSender>>,
        run_store: Arc<RunStore>,
        global_dir: PathBuf,
    ) -> Self {
        Self {
            launcher,
            agent_launcher,
            session_starter,
            session_sender,
            run_store,
            global_dir,
            pricing: rupu_config::PricingConfig::default(),
        }
    }

    /// Override the pricing configuration used for usage summaries.
    pub fn with_pricing(mut self, pricing: rupu_config::PricingConfig) -> Self {
        self.pricing = pricing;
        self
    }
}

// ── Error mapping helpers ─────────────────────────────────────────────────────

fn map_approval_err(run_id: &str, e: ApprovalError) -> HostConnectorError {
    match e {
        ApprovalError::NotFound(_) => HostConnectorError::NotFound(run_id.to_string()),
        other => HostConnectorError::Invalid(other.to_string()),
    }
}

fn map_store_err(run_id: &str, e: RunStoreError) -> HostConnectorError {
    match e {
        RunStoreError::NotFound(_) => HostConnectorError::NotFound(run_id.to_string()),
        other => HostConnectorError::Invalid(other.to_string()),
    }
}

fn map_cancel_err(run_id: &str, e: CancelError) -> HostConnectorError {
    match e {
        CancelError::NotFound(_) => HostConnectorError::NotFound(run_id.to_string()),
        other => HostConnectorError::Invalid(other.to_string()),
    }
}

// ── Trait impl ────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl HostConnector for LocalHostConnector {
    async fn info(&self) -> Result<HostInfo, HostConnectorError> {
        Ok(HostInfo {
            reachable: true,
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            capabilities: HostCapabilities::default(),
        })
    }

    async fn launch_run(&self, req: LaunchRequest) -> Result<String, HostConnectorError> {
        let launcher = self.launcher.as_ref().ok_or_else(|| {
            HostConnectorError::Invalid("no run launcher configured for this host".to_string())
        })?;
        launcher
            .launch(req)
            .await
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))
    }

    async fn launch_agent(&self, req: AgentLaunchRequest) -> Result<String, HostConnectorError> {
        let launcher = self.agent_launcher.as_ref().ok_or_else(|| {
            HostConnectorError::Invalid("no agent launcher configured for this host".to_string())
        })?;
        launcher
            .launch(req)
            .await
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))
    }

    async fn start_session(
        &self,
        req: SessionStartRequest,
    ) -> Result<String, HostConnectorError> {
        let starter = self.session_starter.as_ref().ok_or_else(|| {
            HostConnectorError::Invalid("no session starter configured for this host".to_string())
        })?;
        starter
            .start(req)
            .await
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))
    }

    async fn send_session_turn(
        &self,
        req: SendMessageRequest,
    ) -> Result<String, HostConnectorError> {
        let sender = self.session_sender.as_ref().ok_or_else(|| {
            HostConnectorError::Invalid("no session sender configured for this host".to_string())
        })?;
        sender
            .send(req)
            .await
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))
    }

    async fn list_runs(
        &self,
        params: RunListQuery,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let workflow_only = params.kind == RunKind::Workflow;
        let rows = query_run_rows(
            &self.run_store,
            params.offset,
            params.limit,
            params.lifecycle.as_deref(),
            workflow_only,
            None, // local host shows all runs regardless of worker_id
            &self.pricing,
        )
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
        // Convert typed rows to Value so the trait's return type is uniform
        // across local and HTTP connectors.
        rows.iter()
            .map(|r| {
                serde_json::to_value(r)
                    .map_err(|e| HostConnectorError::Invalid(e.to_string()))
            })
            .collect()
    }

    async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        query_run_detail(&self.run_store, run_id, &self.pricing)
            .map_err(|e| map_store_err(run_id, e))
    }

    async fn approve_run(&self, run_id: &str, mode: &str) -> Result<(), HostConnectorError> {
        let mode_opt = if mode.is_empty() { None } else { Some(mode) };
        let now = chrono::Utc::now();
        // TODO(task-5): replace hardcoded "connector" actor with identity from AppState
        self.run_store
            .request_resume_approval(run_id, "connector", mode_opt, now)
            .map(|_| ())
            .map_err(|e| map_approval_err(run_id, e))
    }

    async fn reject_run(
        &self,
        run_id: &str,
        reason: Option<&str>,
    ) -> Result<(), HostConnectorError> {
        let now = chrono::Utc::now();
        self.run_store
            .reject(run_id, "connector", reason.unwrap_or(""), now)
            .map(|_| ())
            .map_err(|e| map_approval_err(run_id, e))
    }

    async fn cancel_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        let now = chrono::Utc::now();
        self.run_store
            .cancel(run_id, "connector", "Cancelled via connector", now)
            .map(|_| ())
            .map_err(|e| map_cancel_err(run_id, e))
    }

    async fn stream_run_events(
        &self,
        run_id: &str,
    ) -> Result<EventByteStream, HostConnectorError> {
        // Verify the run exists before opening the tail.
        self.run_store
            .load(run_id)
            .map_err(|e| map_store_err(run_id, e))?;

        open_run_events_tail(&self.run_store, run_id).await
    }

    async fn proxy_get_json(
        &self,
        _path_and_query: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "local host is served in-process".into(),
        ))
    }

    // SAFETY/CAVEAT: must not be called with untrusted paths — the HTTP
    // transcript handler enforces allowed_roots before reaching a connector;
    // this method does NOT. Do not resolve("local") + get_transcript with
    // user input.
    async fn get_transcript(
        &self,
        path: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        read_transcript_file(path)
    }
}
