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
        connector::{HostConnector, HostConnectorError},
        registry::HostRegistry,
    },
};
use rupu_orchestrator::runner::{UnitDispatch, UnitDispatcher, UnitOutcome};

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
    async fn dispatch_unit(
        &self,
        unit: UnitDispatch,
        host: &str,
    ) -> Result<UnitOutcome, RunError> {
        let conn = self.resolver.resolve(host)?;

        // Launch the agent run on the remote host.
        let run_id = conn
            .launch_agent(AgentLaunchRequest {
                agent: unit.agent,
                prompt: Some(unit.rendered_prompt),
                mode: None,
                target: None,
                working_dir: None,
            })
            .await
            .map_err(host_err_to_run_err)?;

        // Poll the mirrored run until a terminal status is reached.
        for attempt in 0..POLL_MAX_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(POLL_INTERVAL).await;
            }

            let rec = conn
                .get_run(&run_id)
                .await
                .map_err(host_err_to_run_err)?;

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
                return Ok(UnitOutcome {
                    output,
                    success,
                    error,
                });
            }
        }

        Err(RunError::Provider(format!(
            "timed out waiting for remote unit run {run_id} on host {host} \
             after {POLL_MAX_ATTEMPTS} polls"
        )))
    }
}

// ── Registry builder ──────────────────────────────────────────────────────────

/// Build a `FleetUnitDispatcher` only when the workflow needs one.
///
/// Returns `None` when the workflow has no `distribute:` step (fast path —
/// avoids constructing the registry).  When a dispatcher is returned, it is
/// wired to `run_store` so mirrored unit runs appear in the same store the
/// coordinator reads.
pub fn build_dispatcher_if_needed(
    workflow: &rupu_orchestrator::Workflow,
    global: &Path,
    run_store: Arc<rupu_orchestrator::runs::RunStore>,
    pricing: rupu_config::PricingConfig,
) -> Option<Arc<dyn UnitDispatcher>> {
    if !workflow.steps.iter().any(|s| s.distribute.is_some()) {
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
        async fn approve_run(
            &self,
            _run_id: &str,
            _mode: &str,
        ) -> Result<(), HostConnectorError> {
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
        async fn approve_run(
            &self,
            _run_id: &str,
            _mode: &str,
        ) -> Result<(), HostConnectorError> {
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
}
