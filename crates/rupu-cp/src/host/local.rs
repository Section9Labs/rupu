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
    runs::{CancelError, PauseError, RunStore},
    ApprovalError, RunStoreError,
};
use rupu_runtime::{AutoflowHistoryStore, AutoflowHistoryStoreError};

use crate::{
    agent_launcher::{AgentLaunchRequest, AgentLauncher},
    api::runs::{query_run_detail, query_run_rows},
    host::connector::{
        open_run_events_tail, read_transcript_file, EventByteStream, HostCapabilities,
        HostConnector, HostConnectorError, HostInfo, RunKind, RunListQuery,
    },
    host::workspace_stage::{collect_from_dir, discard_from_dir, stage_to_dir},
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

fn map_pause_err(run_id: &str, e: PauseError) -> HostConnectorError {
    match e {
        PauseError::NotFound(_) => HostConnectorError::NotFound(run_id.to_string()),
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

    /// Cooperatively pause a `Pending`/`Running` run.
    ///
    /// Flips the persisted status to `Paused` AND writes the pause marker so
    /// a *detached* `rupu workflow run <id>` subprocess (the shape `cp serve`
    /// launches) genuinely pauses: its marker poller trips the run's pause
    /// token, and the T2/T3 machinery stops at the next safe boundary. The
    /// status flip is done first (it validates the transition — a terminal or
    /// already-paused run is refused); the marker is only written once the
    /// flip succeeds.
    async fn pause_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        let now = chrono::Utc::now();
        self.run_store
            .pause(run_id, now)
            .map_err(|e| map_pause_err(run_id, e))?;
        // Deliver the pause to a detached run process via the marker.
        self.run_store
            .set_pause_marker(run_id)
            .map_err(|e| map_store_err(run_id, e))
    }

    /// Marker-only resume request for a `Paused` run — mirrors
    /// `approve_run`'s marker-only design. The background resume worker
    /// (spawned by `rupu cp serve`) picks it up via
    /// `RunStore::list_pending_resume` and re-enters `run_workflow` via a
    /// detached `rupu workflow resume <id>` subprocess. Callers (the CP API
    /// handler) gate this on `AppState.launcher` being configured, since
    /// without `cp serve` running there is no worker to consume the marker.
    async fn resume_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        let now = chrono::Utc::now();
        // Marker-only request: the background resume worker consumes it and
        // spawns a detached `rupu workflow resume <id>`. That subprocess is
        // the *race-free* place to clear the pause marker — it does so only
        // AFTER its duplicate-execution guard confirms the original process
        // has exited (`runner_pid` no longer live), so clearing the marker
        // can't un-pause an original that hasn't yet honored the pause.
        self.run_store
            .request_resume_approval(run_id, "connector", None, now)
            .map(|_| ())
            .map_err(|e| map_approval_err(run_id, e))
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
        stage_to_dir(&payload, &self.global_dir)
    }

    /// Reload the staged baseline, diff the working dir, and return the encoded
    /// delta. The scratch tree is removed (best-effort) before returning.
    async fn collect_workspace_delta(
        &self,
        working_dir: &str,
    ) -> Result<Vec<u8>, HostConnectorError> {
        collect_from_dir(working_dir, &self.global_dir)
    }

    /// Best-effort discard of a staged workspace scratch dir — called when the
    /// unit that consumed it failed between stage and collect (launch failure
    /// or poll timeout), so `collect_workspace_delta` never ran.
    async fn discard_workspace(&self, working_dir: &str) -> Result<(), HostConnectorError> {
        discard_from_dir(working_dir, &self.global_dir)
    }

    /// Build this host's dashboard contribution from the local `RunStore`,
    /// the autoflow cycle history, and the findings ledger — one round-trip,
    /// no per-panel calls (see `dashboard_summary.rs` module docs).
    async fn dashboard_summary(
        &self,
        range: crate::host::dashboard_summary::DashboardRange,
    ) -> Result<crate::host::dashboard_summary::DashboardSummary, HostConnectorError> {
        let runs = self
            .run_store
            .list()
            .map_err(|e| HostConnectorError::Invalid(format!("run store list failed: {e}")))?;
        let cycles = collect_cycle_rollups(&self.global_dir).unwrap_or_default();
        let findings_open = count_open_findings(&self.global_dir).unwrap_or(0);
        Ok(crate::host::summary_build::build_summary(
            &runs,
            &cycles,
            findings_open,
            range,
            chrono::Utc::now(),
        ))
    }
}

// ── Dashboard summary helpers ────────────────────────────────────────────────

/// Read this host's autoflow cycle history and roll each cycle into a
/// [`CycleRollup`](crate::host::dashboard_summary::CycleRollup).
///
/// Reads through `AutoflowHistoryStore` exactly as
/// `list_autoflow_runs` (`api/run_streams.rs`) does, and reuses
/// `run_streams::harvest_run_ids` for the run-id extraction, so there is
/// exactly one place that parses `AutoflowCycleRecord` and one place that
/// harvests run ids from its events — no second history-reading path.
///
/// Each `CycleRun.status` is left `"unknown"` here: `build_summary` fills it
/// in from the runs it already holds, so this helper never does a per-run
/// store read (expanding a cycle costs zero extra reads).
fn collect_cycle_rollups(
    global_dir: &std::path::Path,
) -> Result<Vec<crate::host::dashboard_summary::CycleRollup>, HostConnectorError> {
    use crate::host::dashboard_summary::{CycleRollup, CycleRun};

    let store_root = global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);
    let records = match store.list_recent(100) {
        Ok(r) => r,
        Err(AutoflowHistoryStoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Vec::new()
        }
        Err(e) => return Err(HostConnectorError::Invalid(e.to_string())),
    };

    Ok(records
        .iter()
        .map(|r| {
            let started_at = parse_rfc3339_or_now(&r.started_at);
            // `AutoflowCycleRecord::new` initializes `finished_at` equal to
            // `started_at`; a worker overwrites it only once the cycle truly
            // finishes. There is no separate "unfinished" sentinel field, so
            // string equality is the only signal available here.
            let finished_at = if r.finished_at == r.started_at {
                None
            } else {
                Some(parse_rfc3339_or_now(&r.finished_at))
            };
            CycleRollup {
                cycle_id: r.cycle_id.clone(),
                worker_name: r.worker_name.clone(),
                started_at,
                finished_at,
                ran: r.ran_cycles as u64,
                skipped: r.skipped_cycles as u64,
                failed: r.failed_cycles as u64,
                runs: crate::api::run_streams::harvest_run_ids(r)
                    .into_iter()
                    .map(|run_id| CycleRun {
                        run_id,
                        status: "unknown".to_string(),
                    })
                    .collect(),
            }
        })
        .collect())
}

/// Best-effort RFC-3339 parse; falls back to `now()` on a malformed
/// timestamp rather than failing the whole dashboard summary over one bad
/// history record.
fn parse_rfc3339_or_now(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|e| {
            tracing::warn!(value = %s, error = %e, "failed to parse cycle timestamp; using now()");
            chrono::Utc::now()
        })
}

/// Total open findings across every registered workspace, via the same
/// collect-then-summarize path `GET /api/findings` uses (`api::findings`) —
/// never a second workspace/target walk.
fn count_open_findings(global_dir: &std::path::Path) -> Result<u64, HostConnectorError> {
    let findings = crate::api::findings::collect_all_findings(global_dir);
    Ok(crate::api::findings::build_response(findings).summary.total as u64)
}

#[cfg(test)]
mod workspace_sync_tests {
    use super::*;
    use crate::host::connector::{decode_delta, encode_payload};
    use std::path::Path;

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

#[cfg(test)]
mod pause_resume_tests {
    use super::*;
    use rupu_orchestrator::{RunRecord, RunStatus};
    use std::collections::BTreeMap;

    fn local(global_dir: PathBuf) -> (LocalHostConnector, Arc<RunStore>) {
        let run_store = Arc::new(RunStore::new(global_dir.join("runs")));
        let conn =
            LocalHostConnector::new(None, None, None, None, Arc::clone(&run_store), global_dir);
        (conn, run_store)
    }

    fn record(id: &str, status: RunStatus) -> RunRecord {
        RunRecord {
            id: id.into(),
            workflow_name: "wf".into(),
            status,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: PathBuf::from("/tmp/proj"),
            transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            final_output: None,
        }
    }

    #[tokio::test]
    async fn pause_run_marks_running_as_paused() {
        let tmp = tempfile::tempdir().unwrap();
        let (conn, store) = local(tmp.path().to_path_buf());
        store
            .create(record("run_local_pause", RunStatus::Running), "name: x\n")
            .unwrap();

        conn.pause_run("run_local_pause").await.unwrap();

        let loaded = store.load("run_local_pause").unwrap();
        assert_eq!(loaded.status, RunStatus::Paused);
    }

    #[tokio::test]
    async fn pause_run_rejects_terminal_run() {
        let tmp = tempfile::tempdir().unwrap();
        let (conn, store) = local(tmp.path().to_path_buf());
        store
            .create(
                record("run_local_pause_done", RunStatus::Completed),
                "name: x\n",
            )
            .unwrap();

        let err = conn
            .pause_run("run_local_pause_done")
            .await
            .expect_err("pausing a completed run should fail");
        assert!(matches!(err, HostConnectorError::Invalid(_)));
    }

    #[tokio::test]
    async fn resume_run_sets_pending_resume_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let (conn, store) = local(tmp.path().to_path_buf());
        store
            .create(record("run_local_resume", RunStatus::Paused), "name: x\n")
            .unwrap();

        conn.resume_run("run_local_resume").await.unwrap();

        let loaded = store.load("run_local_resume").unwrap();
        // Marker-only, same shape as `approve_run`: status is unchanged
        // (still `Paused`) and a background worker consumes the marker.
        assert_eq!(loaded.status, RunStatus::Paused);
        assert!(loaded.resume_requested_at.is_some());
    }

    #[tokio::test]
    async fn resume_run_rejects_non_paused_run() {
        let tmp = tempfile::tempdir().unwrap();
        let (conn, store) = local(tmp.path().to_path_buf());
        store
            .create(
                record("run_local_resume_running", RunStatus::Running),
                "name: x\n",
            )
            .unwrap();

        let err = conn
            .resume_run("run_local_resume_running")
            .await
            .expect_err("resuming a running (non-paused) run should fail");
        assert!(matches!(err, HostConnectorError::Invalid(_)));
    }
}
