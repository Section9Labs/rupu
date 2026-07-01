//! `LocalHostConnector` — local (host[0]) implementation of [`HostConnector`].
//!
//! Delegates control operations to the per-capability port traits
//! (`RunLauncher`, `AgentLauncher`, `SessionStarter`, `SessionSender`) and
//! reads run state from the in-process `RunStore`. List / detail methods call
//! the shared builders in `crate::api::runs` so the JSON shape is identical to
//! what the HTTP API serves.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rupu_orchestrator::{
    runs::{CancelError, RunStore},
    ApprovalError, RunStoreError,
};
use ulid::Ulid;

use crate::{
    agent_launcher::{AgentLaunchRequest, AgentLauncher},
    api::runs::{query_run_detail, query_run_rows},
    host::connector::{
        decode_payload, deserialize_baseline, encode_delta, open_run_events_tail,
        read_transcript_file, serialize_baseline, EventByteStream, HostCapabilities, HostConnector,
        HostConnectorError, HostInfo, RunKind, RunListQuery, MAX_WORKSPACE_BYTES,
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
    /// Global rupu directory (e.g. `~/.rupu` or the project-level dir). Workspace
    /// staging scratch dirs live under `<global_dir>/workspace-sync/`.
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

    async fn start_session(&self, req: SessionStartRequest) -> Result<String, HostConnectorError> {
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
                serde_json::to_value(r).map_err(|e| HostConnectorError::Invalid(e.to_string()))
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

    async fn stream_run_events(&self, run_id: &str) -> Result<EventByteStream, HostConnectorError> {
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
    async fn get_transcript(&self, path: &str) -> Result<serde_json::Value, HostConnectorError> {
        read_transcript_file(path)
    }

    /// Stage a packed workspace into a fresh scratch dir under the CP cache.
    ///
    /// Layout: `<global_dir>/workspace-sync/<ulid>/work` is the working dir; the
    /// stage-time baseline is persisted to a `baseline.json` sidecar one level up
    /// (OUTSIDE `work`, so `collect_workspace_delta`'s tree hash never sees it).
    async fn stage_workspace(&self, payload: Vec<u8>) -> Result<String, HostConnectorError> {
        if payload.len() > MAX_WORKSPACE_BYTES {
            return Err(HostConnectorError::Invalid(format!(
                "workspace payload {} bytes exceeds limit {MAX_WORKSPACE_BYTES}",
                payload.len()
            )));
        }
        let decoded = decode_payload(&payload)?;
        let base = self
            .global_dir
            .join("workspace-sync")
            .join(Ulid::new().to_string());
        let work = base.join("work");
        let baseline = rupu_workspace::stage(&decoded, &work)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
        std::fs::write(base.join("baseline.json"), serialize_baseline(&baseline)?)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
        Ok(work.to_string_lossy().into_owned())
    }

    /// Reload the staged baseline, diff the working dir, and return the encoded
    /// delta. The scratch tree is removed (best-effort) before returning.
    async fn collect_workspace_delta(
        &self,
        working_dir: &str,
    ) -> Result<Vec<u8>, HostConnectorError> {
        let work = Path::new(working_dir);
        let base = work
            .parent()
            .ok_or_else(|| HostConnectorError::Invalid("invalid working dir".into()))?;
        let baseline_bytes = std::fs::read(base.join("baseline.json"))
            .map_err(|e| HostConnectorError::Invalid(format!("baseline missing: {e}")))?;
        let baseline = deserialize_baseline(&baseline_bytes)?;
        let delta = rupu_workspace::collect_delta(work, &baseline)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
        let bytes = encode_delta(&delta);
        let _ = std::fs::remove_dir_all(base);
        Ok(bytes)
    }
}

#[cfg(test)]
mod workspace_sync_tests {
    use super::*;
    use crate::host::connector::{decode_delta, encode_payload};

    fn local(global_dir: PathBuf) -> LocalHostConnector {
        let run_store = Arc::new(RunStore::new(global_dir.join("runs")));
        LocalHostConnector::new(None, None, None, None, run_store, global_dir)
    }

    /// The Local override stages a packed payload, lets the "agent" mutate the
    /// staged tree, then collects a delta that round-trips through the wire
    /// codec — and cleans up its scratch dir afterwards.
    #[tokio::test]
    async fn local_stage_collect_round_trip() {
        // Coordinator workspace (non-git temp dir → tar mode).
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("keep.txt"), "keep").unwrap();
        std::fs::write(ws.path().join("mod.txt"), "before").unwrap();

        let payload = rupu_workspace::pack(ws.path()).unwrap();
        assert_eq!(payload.mode, rupu_workspace::SyncMode::Tar);

        let global = tempfile::tempdir().unwrap();
        let conn = local(global.path().to_path_buf());

        let working_dir = conn
            .stage_workspace(encode_payload(&payload))
            .await
            .unwrap();
        let work = Path::new(&working_dir);
        assert!(work.join("keep.txt").exists());

        // "remote agent" edits + creates files in the staged tree.
        std::fs::write(work.join("mod.txt"), "after").unwrap();
        std::fs::write(work.join("new.txt"), "created").unwrap();

        let base = work.parent().unwrap().to_path_buf();
        let delta_bytes = conn.collect_workspace_delta(&working_dir).await.unwrap();
        let delta = decode_delta(&delta_bytes).unwrap();
        assert!(delta.changed.contains(&"mod.txt".to_string()));
        assert!(delta.changed.contains(&"new.txt".to_string()));
        assert!(!delta.changed.contains(&"keep.txt".to_string()));

        // Scratch base dir is removed after collect.
        assert!(!base.exists(), "scratch dir should be cleaned up");
    }
}
