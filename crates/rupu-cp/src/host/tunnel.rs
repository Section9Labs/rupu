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
    host::connector::{
        blocking_host, mirror_get_run, mirror_list_runs, mirror_stream_run_events,
        read_transcript_file, EventByteStream, HostCapabilities, HostConnector, HostConnectorError,
        HostInfo, RunListQuery,
    },
    launcher::LaunchRequest,
    node::{
        protocol::{Frame, RunSpec, RunSpecKind, CAP_AGENT_FINDINGS_PROFILE},
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
            findings_profile: None,
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
            findings_profile: req.findings_profile,
        };

        // Verify the node is reachable BEFORE creating the mirror run.
        // This prevents an offline node from leaving an uncancellable Running
        // record with no executor attached.
        let conn = self.live_conn()?;

        // A node that predates `RunSpec.findings_profile` would deserialize
        // the frame, drop the field, and run the agent under its own
        // frontmatter profile. Refuse before creating the mirror run instead.
        if let Some(profile) = req.findings_profile {
            if !conn.supports(CAP_AGENT_FINDINGS_PROFILE) {
                return Err(HostConnectorError::Unsupported(format!(
                    "node {} (rupu {}) does not support findings_profile on agent \
                     launches, so this run cannot be held to the `{profile}` profile; \
                     upgrade rupu on that node",
                    self.node_id,
                    conn.rupu_version().unwrap_or("unknown version"),
                )));
            }
        }

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
        let (store, id, pricing) = (
            Arc::clone(&self.run_store),
            self.node_id.clone(),
            self.pricing.clone(),
        );
        blocking_host(move || mirror_list_runs(&store, &id, &params, &pricing)).await
    }

    async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        let (store, id, pricing) = (
            Arc::clone(&self.run_store),
            self.node_id.clone(),
            self.pricing.clone(),
        );
        let run_id = run_id.to_string();
        blocking_host(move || mirror_get_run(&store, &id, &run_id, &pricing)).await
    }

    async fn approve_run(&self, run_id: &str, mode: &str) -> Result<(), HostConnectorError> {
        let conn = self.live_conn()?;
        conn.send(Frame::Approve {
            run_id: run_id.to_string(),
            mode: mode.to_string(),
        })
        .await
        .map_err(|_| {
            HostConnectorError::Unreachable(format!(
                "node {} disconnected before Approve frame could be sent",
                self.node_id
            ))
        })
    }

    async fn reject_run(
        &self,
        run_id: &str,
        reason: Option<&str>,
    ) -> Result<(), HostConnectorError> {
        let conn = self.live_conn()?;
        conn.send(Frame::Reject {
            run_id: run_id.to_string(),
            reason: reason.map(str::to_string),
        })
        .await
        .map_err(|_| {
            HostConnectorError::Unreachable(format!(
                "node {} disconnected before Reject frame could be sent",
                self.node_id
            ))
        })
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

    /// The mirrored stream. Complete once the run is terminal: the node sends
    /// its coverage frames, final drain included, before `RunFinished` on the
    /// same socket, so they are mirrored before the run turns terminal here.
    async fn unit_coverage(
        &self,
        run_id: &str,
    ) -> Result<crate::host::connector::CoverageRead, HostConnectorError> {
        let bytes = crate::host::connector::mirror_unit_coverage(&self.run_store, run_id).await?;
        Ok(crate::host::connector::CoverageRead {
            bytes,
            complete: true,
        })
    }

    async fn stream_run_events(
        &self,
        run_id: &str,
    ) -> Result<EventByteStream, HostConnectorError> {
        mirror_stream_run_events(&self.run_store, &self.node_id, run_id).await
    }

    async fn get_transcript(
        &self,
        path: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        read_transcript_file(path)
    }

    /// SSH/Tunnel/Bucket runs are created in, and tailed into, the
    /// coordinator's own `RunStore` by `NodeMirror`, so run-scoped detail
    /// endpoints read that mirror instead of the wire.
    fn serves_runs_from_local_mirror(&self) -> bool {
        true
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
