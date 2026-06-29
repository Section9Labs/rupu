//! `TunnelHostConnector` — [`HostConnector`] implementation for dial-home
//! (tunnel) nodes.
//!
//! Tunnel nodes cannot be reached by HTTP (they're behind NAT); instead:
//! - **Observation** reads from the central [`RunStore`] mirror where all
//!   node artifacts are written as they arrive via the WS tunnel.  Runs are
//!   scoped to this node by filtering on `worker_id == node_id`.
//! - **Control** sends typed [`Frame`]s over the node's live [`NodeConn`]
//!   via the [`NodeRegistry`].  If the node is not currently connected,
//!   control operations return [`HostConnectorError::Unreachable`].

#![deny(clippy::all)]

use std::sync::Arc;

use rupu_orchestrator::runs::RunStore;
use ulid::Ulid;

use crate::{
    agent_launcher::AgentLaunchRequest,
    api::runs::{query_run_detail, query_run_rows},
    host::connector::{
        open_run_events_tail, read_transcript_file, EventByteStream, HostCapabilities,
        HostConnector, HostConnectorError, HostInfo, RunKind, RunListQuery,
    },
    launcher::LaunchRequest,
    node::{
        protocol::{Frame, RunSpec, RunSpecKind},
        NodeMirror, NodeRegistry,
    },
    session_sender::SendMessageRequest,
    session_starter::SessionStartRequest,
};

// ── Struct ────────────────────────────────────────────────────────────────────

/// [`HostConnector`] backed by a tunnel (dial-home) node.
///
/// Observation methods read the central [`RunStore`] mirror filtered to this
/// node's runs (`worker_id == node_id`).  Control methods send [`Frame`]s over
/// the node's live WebSocket connection via the [`NodeRegistry`].
pub struct TunnelHostConnector {
    /// The node identifier.  Matches `worker_id` on mirrored [`RunRecord`]s
    /// and the key used in [`NodeRegistry`].
    pub node_id: String,
    /// Live tunnel connection registry — used to look up the node's sender
    /// and to report reachability.
    pub registry: Arc<NodeRegistry>,
    /// Mirror writer — used to record new runs before dispatching them to
    /// the node so they appear in the central run list immediately.
    pub mirror: Arc<NodeMirror>,
    /// Central run store — used for all observation queries.
    pub run_store: Arc<RunStore>,
    /// Pricing configuration for usage calculations in list / detail responses.
    pub pricing: rupu_config::PricingConfig,
}

impl TunnelHostConnector {
    /// Construct a new connector.
    pub fn new(
        node_id: impl Into<String>,
        registry: Arc<NodeRegistry>,
        mirror: Arc<NodeMirror>,
        run_store: Arc<RunStore>,
        pricing: rupu_config::PricingConfig,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            registry,
            mirror,
            run_store,
            pricing,
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Get the live connection for this node, or return
    /// [`HostConnectorError::Unreachable`] with a descriptive message.
    fn live_conn(&self) -> Result<Arc<crate::node::NodeConn>, HostConnectorError> {
        self.registry.get(&self.node_id).ok_or_else(|| {
            HostConnectorError::Unreachable(format!(
                "node {} is not connected",
                self.node_id
            ))
        })
    }
}

// ── Trait impl ────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl HostConnector for TunnelHostConnector {
    async fn info(&self) -> Result<HostInfo, HostConnectorError> {
        Ok(HostInfo {
            reachable: self.registry.is_online(&self.node_id),
            version: None,
            capabilities: HostCapabilities::default(),
        })
    }

    async fn launch_run(&self, req: LaunchRequest) -> Result<String, HostConnectorError> {
        let run_id = format!("run_{}", Ulid::new());

        let spec = RunSpec {
            kind: RunSpecKind::Workflow,
            name: req.workflow.clone(),
            inputs: req.inputs.clone(),
            prompt: None,
            mode: req.mode.clone(),
            target: req.target.clone(),
        };

        // Verify the node is reachable BEFORE creating the mirror run.
        // This prevents an offline node from leaving an uncancellable Running
        // record with no executor attached.
        let conn = self.live_conn()?;

        self.mirror
            .create_run(&run_id, &self.node_id, &spec)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        if conn
            .send(Frame::Run {
                run_id: run_id.clone(),
                spec,
            })
            .await
            .is_err()
        {
            // Node disconnected in the narrow window between live_conn() and
            // send().  Best-effort: mark the orphaned mirror run cancelled so
            // it doesn't remain stuck in Running.
            let _ = self.mirror.finish(&run_id, &self.node_id, "cancelled");
            return Err(HostConnectorError::Unreachable(format!(
                "node {} disconnected before Run frame could be sent",
                self.node_id
            )));
        }

        Ok(run_id)
    }

    async fn launch_agent(&self, req: AgentLaunchRequest) -> Result<String, HostConnectorError> {
        let run_id = format!("run_{}", Ulid::new());

        let spec = RunSpec {
            kind: RunSpecKind::Agent,
            name: req.agent.clone(),
            inputs: std::collections::BTreeMap::new(),
            prompt: req.prompt.clone(),
            mode: req.mode.clone(),
            target: req.target.clone(),
        };

        // Verify the node is reachable BEFORE creating the mirror run.
        // This prevents an offline node from leaving an uncancellable Running
        // record with no executor attached.
        let conn = self.live_conn()?;

        self.mirror
            .create_run(&run_id, &self.node_id, &spec)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        if conn
            .send(Frame::Run {
                run_id: run_id.clone(),
                spec,
            })
            .await
            .is_err()
        {
            // Node disconnected in the narrow window between live_conn() and
            // send().  Best-effort: mark the orphaned mirror run cancelled so
            // it doesn't remain stuck in Running.
            let _ = self.mirror.finish(&run_id, &self.node_id, "cancelled");
            return Err(HostConnectorError::Unreachable(format!(
                "node {} disconnected before Run frame could be sent",
                self.node_id
            )));
        }

        Ok(run_id)
    }

    async fn start_session(
        &self,
        _req: SessionStartRequest,
    ) -> Result<String, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "sessions not supported over tunnel (slice 2)".into(),
        ))
    }

    async fn send_session_turn(
        &self,
        _req: SendMessageRequest,
    ) -> Result<String, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "session turns not supported over tunnel (slice 2)".into(),
        ))
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
            Some(self.node_id.as_str()),
            &self.pricing,
        )
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        rows.iter()
            .map(|r| {
                serde_json::to_value(r)
                    .map_err(|e| HostConnectorError::Invalid(e.to_string()))
            })
            .collect()
    }

    async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        // Verify the run exists and belongs to this node before returning detail.
        let record = self
            .run_store
            .load(run_id)
            .map_err(|e| match e {
                rupu_orchestrator::RunStoreError::NotFound(_) => {
                    HostConnectorError::NotFound(run_id.to_string())
                }
                other => HostConnectorError::Invalid(other.to_string()),
            })?;

        if record.worker_id.as_deref() != Some(self.node_id.as_str()) {
            return Err(HostConnectorError::NotFound(run_id.to_string()));
        }

        query_run_detail(&self.run_store, run_id, &self.pricing).map_err(|e| {
            HostConnectorError::Invalid(e.to_string())
        })
    }

    async fn approve_run(&self, _run_id: &str, _mode: &str) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "approval over tunnel not supported (slice 2)".into(),
        ))
    }

    async fn reject_run(
        &self,
        _run_id: &str,
        _reason: Option<&str>,
    ) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "rejection over tunnel not supported (slice 2)".into(),
        ))
    }

    async fn cancel_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        let conn = self.live_conn()?;
        conn.send(Frame::Cancel {
            run_id: run_id.to_string(),
        })
        .await
        .map_err(|_| {
            HostConnectorError::Unreachable(format!(
                "node {} disconnected before Cancel frame could be sent",
                self.node_id
            ))
        })
    }

    async fn stream_run_events(
        &self,
        run_id: &str,
    ) -> Result<EventByteStream, HostConnectorError> {
        // Verify the run exists and belongs to this node.
        let record = self
            .run_store
            .load(run_id)
            .map_err(|e| match e {
                rupu_orchestrator::RunStoreError::NotFound(_) => {
                    HostConnectorError::NotFound(run_id.to_string())
                }
                other => HostConnectorError::Invalid(other.to_string()),
            })?;

        if record.worker_id.as_deref() != Some(self.node_id.as_str()) {
            return Err(HostConnectorError::NotFound(run_id.to_string()));
        }

        open_run_events_tail(&self.run_store, run_id).await
    }

    async fn get_transcript(
        &self,
        path: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        read_transcript_file(path)
    }

    async fn proxy_get_json(
        &self,
        _path_and_query: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "proxy_get_json is not supported for tunnel hosts".into(),
        ))
    }
}
