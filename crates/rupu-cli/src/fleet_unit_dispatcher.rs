//! `FleetUnitDispatcher` — dispatches remote fan-out units through the
//! [`rupu_cp::host::registry::HostRegistry`], polling the mirrored run until
//! a terminal status is reached.

#![deny(clippy::all)]

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use rupu_agent::RunError;
use rupu_cp::{
    agent_launcher::AgentLaunchRequest,
    host::{
        connector::{
            decode_delta, encode_delta, encode_payload, HostConnector, HostConnectorError,
        },
        registry::HostRegistry,
    },
};
use rupu_orchestrator::runner::{
    UnitDispatch, UnitDispatcher, UnitOutcome, WorkspaceConflict, WorkspaceDelta,
};

// ── Poll constants ─────────────────────────────────────────────────────────────

/// Maximum number of `get_run` polls before giving up (120 × 500 ms = 60 s).
const POLL_MAX_ATTEMPTS: u32 = 120;
/// Interval between polls.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

// ── Helpers ───────────────────────────────────────────────────────────────────

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "rejected")
}

fn host_err_to_run_err(e: HostConnectorError) -> RunError {
    RunError::Provider(e.to_string())
}

// ── Workspace-delta bridge ──────────────────────────────────────────────────
//
// The orchestrator's `WorkspaceDelta` is opaque: `payload` is whatever the
// dispatcher chooses, and `changed`/`deleted` are mirrored for observability.
// We define the bridge here so the orchestrator never sees the codec types.
// The chosen payload encoding IS the connector wire encoding (`encode_delta` /
// `decode_delta`), so the same self-describing bytes flow coordinator→host and
// host→coordinator without a second format.

/// Convert a codec [`rupu_workspace::Delta`] into the orchestrator's opaque
/// [`WorkspaceDelta`]: mirror `changed`/`deleted` for logging, and stash the
/// wire-encoded delta as the opaque `payload`.
fn to_orchestrator_delta(d: &rupu_workspace::Delta) -> WorkspaceDelta {
    WorkspaceDelta {
        changed: d.changed.clone(),
        deleted: d.deleted.clone(),
        payload: encode_delta(d),
    }
}

/// Decode an orchestrator [`WorkspaceDelta`] back into a codec
/// [`rupu_workspace::Delta`] for `apply_deltas`.
fn from_orchestrator_delta(
    wd: &WorkspaceDelta,
) -> Result<rupu_workspace::Delta, HostConnectorError> {
    decode_delta(&wd.payload)
}

// ── Resolver — production vs. test seam ───────────────────────────────────────

/// Internal enum so tests can inject a fixed connector without a registry.
enum Resolver {
    Registry(Arc<HostRegistry>),
    Fixed(Arc<dyn HostConnector>),
}

impl Resolver {
    fn resolve(&self, host: &str) -> Result<Arc<dyn HostConnector>, RunError> {
        match self {
            Resolver::Registry(reg) => reg
                .resolve(host)
                .map_err(|e| RunError::Provider(e.to_string())),
            Resolver::Fixed(conn) => Ok(Arc::clone(conn)),
        }
    }
}

// ── FleetUnitDispatcher ───────────────────────────────────────────────────────

/// Dispatches remote fan-out units through the [`HostRegistry`].
///
/// Production path: `new(registry)` — resolves the connector from the registry
/// on every `dispatch_unit` call.
/// Test/seam path: `from_connector(conn)` — bypasses registry resolution.
pub struct FleetUnitDispatcher {
    resolver: Resolver,
}

impl FleetUnitDispatcher {
    /// Production constructor: resolves the connector via `registry` per call.
    pub fn new(registry: Arc<HostRegistry>) -> Self {
        Self {
            resolver: Resolver::Registry(registry),
        }
    }

    /// Seam constructor for tests: always uses `conn`, skipping registry lookup.
    pub fn from_connector(conn: Arc<dyn HostConnector>) -> Self {
        Self {
            resolver: Resolver::Fixed(conn),
        }
    }
}

#[async_trait]
impl UnitDispatcher for FleetUnitDispatcher {
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError> {
        let conn = self.resolver.resolve(host)?;

        // When the unit runs in `Sync` mode, pack the coordinator workspace and
        // stage it on the host BEFORE launching, so the agent runs against the
        // staged tree. `None` ⇒ self-contained: byte-for-byte the prior path.
        let working_dir = match &unit.workspace_path {
            Some(ws) => {
                let payload =
                    rupu_workspace::pack(ws).map_err(|e| RunError::Provider(e.to_string()))?;
                let encoded = encode_payload(&payload);
                let dir = conn
                    .stage_workspace(encoded)
                    .await
                    .map_err(host_err_to_run_err)?;
                Some(dir)
            }
            None => None,
        };

        // Launch the agent run on the remote host (against the staged dir, when
        // workspace sync is active).
        let run_id = conn
            .launch_agent(AgentLaunchRequest {
                agent: unit.agent,
                prompt: Some(unit.rendered_prompt),
                mode: None,
                target: None,
                working_dir: working_dir.clone(),
            })
            .await
            .map_err(host_err_to_run_err)?;

        // Poll the mirrored run until a terminal status is reached.
        for attempt in 0..POLL_MAX_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(POLL_INTERVAL).await;
            }

            let rec = conn.get_run(&run_id).await.map_err(host_err_to_run_err)?;

            // All HostConnector::get_run impls return the query_run_detail
            // envelope: {"run": <RunRecord>, "steps": [...], "usage": {...}}.
            // Read from the nested "run" object, not the top-level envelope.
            let run = &rec["run"];
            let status = run["status"].as_str().unwrap_or("").to_string();

            if is_terminal_status(&status) {
                let output = run["final_output"].as_str().unwrap_or("").to_string();
                let success = status == "completed";
                let error = (!success).then(|| {
                    run["error_message"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| status.clone())
                });

                // Collect the workspace delta when staging was active. On a
                // successful unit, surface collect/decode failures (losing the
                // delta would silently drop the unit's work). On a failed unit,
                // still collect best-effort so the host scratch is cleaned up,
                // but carry no delta.
                let workspace_delta = match (&working_dir, success) {
                    (Some(dir), true) => {
                        let bytes = conn
                            .collect_workspace_delta(dir)
                            .await
                            .map_err(host_err_to_run_err)?;
                        let delta = decode_delta(&bytes).map_err(host_err_to_run_err)?;
                        Some(to_orchestrator_delta(&delta))
                    }
                    (Some(dir), false) => {
                        let _ = conn.collect_workspace_delta(dir).await;
                        None
                    }
                    (None, _) => None,
                };

                return Ok(UnitOutcome {
                    output,
                    success,
                    error,
                    workspace_delta,
                });
            }
        }

        Err(RunError::Provider(format!(
            "timed out waiting for remote unit run {run_id} on host {host} \
             after {POLL_MAX_ATTEMPTS} polls"
        )))
    }

    /// Bridge the orchestrator's opaque deltas to the `rupu-workspace` codec and
    /// apply them to the coordinator workspace. Conflicts (overlapping tar files
    /// or conflicting git hunks) become [`WorkspaceConflict`]; any other codec
    /// failure is surfaced as a conflict-class step failure too.
    async fn apply_workspace_deltas(
        &self,
        workspace_path: &Path,
        deltas: &[WorkspaceDelta],
    ) -> Result<(), WorkspaceConflict> {
        let mut codec = Vec::with_capacity(deltas.len());
        for wd in deltas {
            match from_orchestrator_delta(wd) {
                Ok(d) => codec.push(d),
                Err(e) => return Err(WorkspaceConflict(vec![e.to_string()])),
            }
        }
        match rupu_workspace::apply_deltas(workspace_path, &codec) {
            Ok(()) => Ok(()),
            Err(rupu_workspace::SyncError::Conflict(paths)) => Err(WorkspaceConflict(paths)),
            Err(e) => Err(WorkspaceConflict(vec![e.to_string()])),
        }
    }
}

// ── Registry builder ──────────────────────────────────────────────────────────

/// Build a `FleetUnitDispatcher` only when the workflow needs one.
///
/// Returns `None` when the workflow has no `distribute:` or `host:` step
/// (fast path — avoids constructing the registry).  When a dispatcher is
/// returned, it is wired to `run_store` so mirrored unit runs appear in the
/// same store the coordinator reads.
pub fn build_dispatcher_if_needed(
    workflow: &rupu_orchestrator::Workflow,
    global: &Path,
    run_store: Arc<rupu_orchestrator::runs::RunStore>,
    pricing: rupu_config::PricingConfig,
) -> Option<Arc<dyn UnitDispatcher>> {
    if !workflow
        .steps
        .iter()
        .any(|s| s.distribute.is_some() || s.host.is_some())
    {
        return None;
    }

    let node_registry = Arc::new(rupu_cp::node::NodeRegistry::new());
    let node_mirror = Arc::new(rupu_cp::node::NodeMirror::new(Arc::clone(&run_store)));
    let local = rupu_cp::host::local::LocalHostConnector::new(
        None,
        None,
        None,
        None,
        Arc::clone(&run_store),
        global.to_path_buf(),
    )
    .with_pricing(pricing.clone());
    let host_store = rupu_workspace::HostStore {
        root: global.join("hosts"),
    };
    let registry = HostRegistry::new(host_store, Arc::new(local)).with_tunnel_deps(
        node_registry,
        node_mirror,
        run_store,
        pricing,
    );

    Some(Arc::new(FleetUnitDispatcher::new(Arc::new(registry))))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use rupu_cp::{
        host::connector::{
            EventByteStream, HostConnector, HostConnectorError, HostInfo, RunListQuery,
        },
        launcher::LaunchRequest,
        session_sender::SendMessageRequest,
        session_starter::SessionStartRequest,
    };

    // ── Fake connector — success path ─────────────────────────────────────────

    struct FakeConnector {
        run_id: &'static str,
        get_run_response: serde_json::Value,
    }

    impl FakeConnector {
        fn completed() -> Self {
            Self {
                run_id: "run_x",
                // Real envelope shape: {"run": <RunRecord>, "steps": [...], "usage": {...}}
                get_run_response: serde_json::json!({
                    "run": {
                        "status": "completed",
                        "final_output": "fake-out"
                    }
                }),
            }
        }

        fn failed() -> Self {
            Self {
                run_id: "run_y",
                get_run_response: serde_json::json!({
                    "run": {
                        "status": "failed",
                        "error_message": "boom"
                    }
                }),
            }
        }
    }

    #[async_trait]
    impl HostConnector for FakeConnector {
        async fn info(&self) -> Result<HostInfo, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_run(&self, _req: LaunchRequest) -> Result<String, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_agent(
            &self,
            _req: AgentLaunchRequest,
        ) -> Result<String, HostConnectorError> {
            Ok(self.run_id.to_string())
        }
        async fn start_session(
            &self,
            _req: SessionStartRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!()
        }
        async fn send_session_turn(
            &self,
            _req: SendMessageRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!()
        }
        async fn list_runs(
            &self,
            _params: RunListQuery,
        ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            unimplemented!()
        }
        async fn get_run(&self, _run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
            Ok(self.get_run_response.clone())
        }
        async fn approve_run(&self, _run_id: &str, _mode: &str) -> Result<(), HostConnectorError> {
            unimplemented!()
        }
        async fn reject_run(
            &self,
            _run_id: &str,
            _reason: Option<&str>,
        ) -> Result<(), HostConnectorError> {
            unimplemented!()
        }
        async fn cancel_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
            unimplemented!()
        }
        async fn stream_run_events(
            &self,
            _run_id: &str,
        ) -> Result<EventByteStream, HostConnectorError> {
            unimplemented!()
        }
        async fn get_transcript(
            &self,
            _path: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
    }

    // ── Fake connector — Unreachable on launch ────────────────────────────────

    struct UnreachableConnector;

    #[async_trait]
    impl HostConnector for UnreachableConnector {
        async fn info(&self) -> Result<HostInfo, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_run(&self, _req: LaunchRequest) -> Result<String, HostConnectorError> {
            unimplemented!()
        }
        async fn launch_agent(
            &self,
            _req: AgentLaunchRequest,
        ) -> Result<String, HostConnectorError> {
            Err(HostConnectorError::Unreachable("h1 is down".to_string()))
        }
        async fn start_session(
            &self,
            _req: SessionStartRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!()
        }
        async fn send_session_turn(
            &self,
            _req: SendMessageRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!()
        }
        async fn list_runs(
            &self,
            _params: RunListQuery,
        ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            unimplemented!()
        }
        async fn get_run(&self, _run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
        async fn approve_run(&self, _run_id: &str, _mode: &str) -> Result<(), HostConnectorError> {
            unimplemented!()
        }
        async fn reject_run(
            &self,
            _run_id: &str,
            _reason: Option<&str>,
        ) -> Result<(), HostConnectorError> {
            unimplemented!()
        }
        async fn cancel_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
            unimplemented!()
        }
        async fn stream_run_events(
            &self,
            _run_id: &str,
        ) -> Result<EventByteStream, HostConnectorError> {
            unimplemented!()
        }
        async fn get_transcript(
            &self,
            _path: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!()
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_unit() -> UnitDispatch {
        UnitDispatch {
            step_id: "s".to_string(),
            agent: "a".to_string(),
            rendered_prompt: "p".to_string(),
            index: 0,
            run_id: "r".to_string(),
            workspace_path: None,
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    /// `from_connector` with a fake that returns the real envelope
    /// `{"run":{"status":"completed","final_output":"fake-out"}}` on the
    /// first `get_run` poll → output "fake-out", success true.
    #[tokio::test]
    async fn fleet_dispatch_reads_final_output_from_mirror() {
        let conn = Arc::new(FakeConnector::completed());
        let d = FleetUnitDispatcher::from_connector(conn);
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        assert_eq!(out.output, "fake-out");
        assert!(out.success);
        assert!(out.error.is_none());
    }

    /// `from_connector` with a fake that returns a failed envelope
    /// `{"run":{"status":"failed","error_message":"boom"}}` → success false,
    /// error contains "boom" (prefers error_message over status literal).
    #[tokio::test]
    async fn fleet_dispatch_failed_run_surfaces_error_message() {
        let conn = Arc::new(FakeConnector::failed());
        let d = FleetUnitDispatcher::from_connector(conn);
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        assert!(!out.success);
        let err = out.error.expect("failed run must have an error field");
        assert!(err.contains("boom"), "expected 'boom' in error, got: {err}");
    }

    /// `from_connector` with a fake whose `launch_agent` returns `Unreachable`
    /// → `dispatch_unit` returns `Err`.
    #[tokio::test]
    async fn fleet_dispatch_unreachable_host_errors() {
        let conn = Arc::new(UnreachableConnector);
        let d = FleetUnitDispatcher::from_connector(conn);
        let result = d.dispatch_unit(make_unit(), "h1").await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unreachable") || msg.contains("h1 is down"));
    }

    /// A plain workflow (no `distribute:` / `host:` step) needs no fleet
    /// dispatcher — the fast path returns `None` without building a registry.
    #[test]
    fn build_dispatcher_none_for_plain_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let wf = rupu_orchestrator::Workflow::parse(
            r#"
name: plain
steps:
  - id: a
    agent: x
    prompt: "p"
"#,
        )
        .unwrap();
        let store = Arc::new(rupu_orchestrator::runs::RunStore::new(
            dir.path().join("runs"),
        ));
        let got = build_dispatcher_if_needed(
            &wf,
            dir.path(),
            store,
            rupu_config::PricingConfig::default(),
        );
        assert!(got.is_none(), "plain workflow must not get a dispatcher");
    }

    /// A workflow with a host-placed linear step needs a fleet dispatcher.
    #[test]
    fn build_dispatcher_some_for_host_placed_step() {
        let dir = tempfile::tempdir().unwrap();
        let wf = rupu_orchestrator::Workflow::parse(
            r#"
name: placed
steps:
  - id: a
    agent: x
    prompt: "p"
    host: worker-1
"#,
        )
        .unwrap();
        let store = Arc::new(rupu_orchestrator::runs::RunStore::new(
            dir.path().join("runs"),
        ));
        let got = build_dispatcher_if_needed(
            &wf,
            dir.path(),
            store,
            rupu_config::PricingConfig::default(),
        );
        assert!(got.is_some(), "host-placed workflow must get a dispatcher");
    }

    // ── Workspace-sync tests ──────────────────────────────────────────────────

    /// Build a one-file tar and wrap it in the dispatcher's wire encoding — the
    /// SAME `encode_delta` path `apply_workspace_deltas` decodes. The resulting
    /// bytes go into the orchestrator `WorkspaceDelta.payload`.
    fn tar_one(path: &str, body: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut b = tar::Builder::new(&mut buf);
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            b.append_data(&mut header, path, body.as_bytes()).unwrap();
            b.finish().unwrap();
        }
        let delta = rupu_workspace::Delta {
            mode: rupu_workspace::SyncMode::Tar,
            changed: vec![path.to_string()],
            deleted: vec![],
            bytes: buf,
        };
        rupu_cp::host::connector::encode_delta(&delta)
    }

    /// A transport that does not support workspace sync (default trait impls)
    /// surfaces a clear Unsupported error through the dispatcher.
    #[tokio::test]
    async fn workspace_sync_on_unsupported_transport_errors() {
        // UnreachableConnector inherits the default stage_workspace = Unsupported.
        let conn = Arc::new(UnreachableConnector);
        let d = FleetUnitDispatcher::from_connector(conn);
        let mut unit = make_unit();
        // Use a real dir so `pack` succeeds and `stage_workspace` (the default
        // Unsupported impl) is the genuine failure point.
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("f.txt"), "hi").unwrap();
        unit.workspace_path = Some(ws.path().to_path_buf());
        let err = d.dispatch_unit(unit, "h1").await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("workspace sync")
                || msg.contains("unsupported")
                || msg.contains("unreachable"),
            "got: {msg}"
        );
    }

    /// apply_workspace_deltas bridges to the rupu-workspace tar codec: two
    /// disjoint tar deltas apply cleanly.
    #[tokio::test]
    async fn apply_bridges_to_workspace_codec() {
        let conn = Arc::new(FakeConnector::completed());
        let d = FleetUnitDispatcher::from_connector(conn);
        let ws = tempfile::tempdir().unwrap();
        // Build two disjoint tar-mode orchestrator deltas via the same encode
        // path the dispatcher uses (payload = wire-encoded one-file tar delta).
        let a = rupu_orchestrator::runner::WorkspaceDelta {
            changed: vec!["a.txt".into()],
            deleted: vec![],
            payload: tar_one("a.txt", "A"),
        };
        let b = rupu_orchestrator::runner::WorkspaceDelta {
            changed: vec!["b.txt".into()],
            deleted: vec![],
            payload: tar_one("b.txt", "B"),
        };
        d.apply_workspace_deltas(ws.path(), &[a, b]).await.unwrap();
        assert!(ws.path().join("a.txt").exists());
        assert!(ws.path().join("b.txt").exists());
    }
}
