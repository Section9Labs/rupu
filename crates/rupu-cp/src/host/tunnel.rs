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
//! - **Finding artifacts** are pulled over the same tunnel: `ArtifactPull`
//!   down, ordered `ArtifactChunk`s + `ArtifactPullDone` up, routed to the
//!   waiting pull by [`crate::node::NodeConn::route_pull`].
//! - **Shipped skills** go the other way on the same tunnel: `SkillsOffer`
//!   down, the node's `SkillsNeed` up, the needed archives as `SkillChunk`s
//!   down, `SkillsStored` up — answers routed by
//!   [`crate::node::NodeConn::route_offer`].

#![deny(clippy::all)]

use std::sync::Arc;

use rupu_orchestrator::runs::RunStore;
use rupu_runtime::services::operator::{MessagesView, OperatorSendReply, ViewQuery};
use ulid::Ulid;

use crate::{
    agent_launcher::AgentLaunchRequest,
    host::connector::{
        blocking_host, mirror_get_run, mirror_list_runs, mirror_stream_run_events,
        read_transcript_file, spec_peer_codename, EventByteStream, HostCapabilities, HostConnector,
        HostConnectorError, HostInfo, OperatorMessageRequest, RunListQuery,
    },
    host::skill_ship::{self, ShipMemo, ShipOutcome, SkillBundle, Transfer},
    launcher::LaunchRequest,
    node::{
        protocol::{
            Frame, RunSpec, RunSpecKind, ARTIFACT_CHUNK_BYTES, CAP_AGENT_ENGAGEMENT_PROFILE,
            CAP_AGENT_FINDINGS_PROFILE, CAP_AGENT_SKILLS, CAP_AGENT_SKILL_BUNDLE,
            CAP_FINDINGS_ARTIFACT_PULL,
        },
        NodeMirror, NodeRegistry,
    },
    session_sender::SendMessageRequest,
    session_starter::SessionStartRequest,
};

/// How long a tunnel artifact pull waits for the node's next frame before
/// giving up on it.
const ARTIFACT_PULL_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Forgets an in-flight pull's routing entry on drop — every return path,
/// and the pull future being dropped mid-transfer — so no sender outlives
/// its waiter in the connection's table.
struct PullRegistration<'a> {
    conn: &'a crate::node::NodeConn,
    req: &'a str,
}

impl Drop for PullRegistration<'_> {
    fn drop(&mut self) {
        self.conn.end_pull(self.req);
    }
}

/// [`PullRegistration`] for a skill offer.
struct OfferRegistration<'a> {
    conn: &'a crate::node::NodeConn,
    id: &'a str,
}

impl Drop for OfferRegistration<'_> {
    fn drop(&mut self) {
        self.conn.end_offer(self.id);
    }
}

/// `layer:name` of each ref, for messages.
fn named(refs: &[rupu_skills::ship::ShippedSkillRef]) -> String {
    let names: Vec<String> = refs
        .iter()
        .map(|r| format!("{}:{}", r.layer, r.name))
        .collect();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

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
    /// The coordinator's per-customer pricing, for usage in list / detail
    /// responses: a mirrored run is priced like the same run on every other
    /// surface (its recorded customer's layer; a legacy one as unknown).
    pub pricing: Arc<crate::customers::CustomerPricing>,
    /// The skills this node confirmed it holds, and the lock that makes
    /// concurrent ships to it one offer.
    ship_memo: ShipMemo,
}

impl TunnelHostConnector {
    /// Construct a new connector.
    pub fn new(
        node_id: impl Into<String>,
        registry: Arc<NodeRegistry>,
        mirror: Arc<NodeMirror>,
        run_store: Arc<RunStore>,
        pricing: Arc<crate::customers::CustomerPricing>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            registry,
            mirror,
            run_store,
            pricing,
            ship_memo: ShipMemo::default(),
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Get the live connection for this node, or return
    /// [`HostConnectorError::Unreachable`] with a descriptive message.
    fn live_conn(&self) -> Result<Arc<crate::node::NodeConn>, HostConnectorError> {
        self.registry.get(&self.node_id).ok_or_else(|| {
            HostConnectorError::Unreachable(format!("node {} is not connected", self.node_id))
        })
    }

    /// Why `conn` can't take shipped skills.
    fn no_skill_bundle(&self, conn: &crate::node::NodeConn) -> String {
        format!(
            "host {id}: node {id} (rupu {}) can't receive shipped skills; upgrade rupu there",
            conn.rupu_version().unwrap_or("unknown version"),
            id = self.node_id,
        )
    }

    /// Send `frame` (a `what`) down the tunnel, held to the idle bound: the
    /// tunnel's write channel is bounded, and a node that stopped reading
    /// leaves it full.
    async fn send_bounded(
        &self,
        conn: &crate::node::NodeConn,
        frame: Frame,
        what: &str,
    ) -> Result<(), HostConnectorError> {
        tokio::time::timeout(ARTIFACT_PULL_IDLE_TIMEOUT, conn.send(frame))
            .await
            .map_err(|_| {
                HostConnectorError::Unreachable(format!(
                    "host {id}: node {id} took no {what} frame for {}s (its tunnel is not draining)",
                    ARTIFACT_PULL_IDLE_TIMEOUT.as_secs(),
                    id = self.node_id,
                ))
            })?
            .map_err(|_| {
                HostConnectorError::Unreachable(format!(
                    "host {id}: node {id} disconnected before a {what} frame could be sent",
                    id = self.node_id,
                ))
            })
    }

    /// The node's next answer to a skill offer, within the idle bound.
    async fn next_offer_msg(
        &self,
        rx: &mut tokio::sync::mpsc::Receiver<crate::node::OfferMsg>,
    ) -> Result<crate::node::OfferMsg, HostConnectorError> {
        tokio::time::timeout(ARTIFACT_PULL_IDLE_TIMEOUT, rx.recv())
            .await
            .map_err(|_| {
                HostConnectorError::Unreachable(format!(
                    "host {id}: node {id} did not answer a skill offer for {}s",
                    ARTIFACT_PULL_IDLE_TIMEOUT.as_secs(),
                    id = self.node_id,
                ))
            })?
            .ok_or_else(|| self.offer_disconnected())
    }

    fn offer_disconnected(&self) -> HostConnectorError {
        HostConnectorError::Unreachable(format!(
            "host {id}: node {id} disconnected during a skill offer",
            id = self.node_id,
        ))
    }

    /// Ship `pending` over the tunnel: `SkillsOffer`, the node's
    /// `SkillsNeed`, each needed archive as ordered `SkillChunk`s, then its
    /// `SkillsStored`, which must name exactly the skills it was sent.
    async fn offer(
        &self,
        pending: Vec<Arc<rupu_skills::ship::PackedSkill>>,
    ) -> Result<Transfer, HostConnectorError> {
        use crate::node::OfferMsg;
        use base64::Engine as _;
        let who = format!("host {}", self.node_id);
        // Looked up again: the ship lock may have been waited on across a
        // reconnect, and an older node's frame parse is fatal.
        let conn = self.live_conn()?;
        if !conn.supports(CAP_AGENT_SKILL_BUNDLE) {
            return Err(HostConnectorError::Unsupported(self.no_skill_bundle(&conn)));
        }
        let id = format!("skills_{}", Ulid::new());
        let mut rx = conn.begin_offer(&id);
        let _registered = OfferRegistration {
            conn: &conn,
            id: &id,
        };
        let offered: Vec<_> = pending.iter().map(|p| p.reference()).collect();
        self.send_bounded(
            &conn,
            Frame::SkillsOffer {
                id: id.clone(),
                skills: offered.clone(),
            },
            "SkillsOffer",
        )
        .await?;

        let needed = match self.next_offer_msg(&mut rx).await? {
            OfferMsg::Need { error: Some(e), .. } => {
                return Err(HostConnectorError::Invalid(format!("{who}: {e}")))
            }
            OfferMsg::Need {
                skills,
                error: None,
            } => skills,
            OfferMsg::Stored { .. } => {
                return Err(HostConnectorError::Invalid(format!(
                    "{who}: answered a skill offer with SkillsStored before SkillsNeed"
                )))
            }
        };
        let mut sending: Vec<usize> = Vec::with_capacity(needed.len());
        for r in &needed {
            let Some(i) = offered.iter().position(|o| o == r) else {
                return Err(HostConnectorError::Invalid(format!(
                    "{who}: the node asked for skill {}:{} ({}), which was not offered",
                    r.layer, r.name, r.content_hash
                )));
            };
            if sending.contains(&i) {
                return Err(HostConnectorError::Invalid(format!(
                    "{who}: the node asked for skill {}:{} twice",
                    r.layer, r.name
                )));
            }
            sending.push(i);
        }
        if needed.is_empty() {
            return Ok(Transfer::Done);
        }

        // A node that fails the offer mid-transfer says so at once; stop
        // sending it chunks it would drop.
        let mut early = None;
        'send: for &i in &sending {
            let p = &pending[i];
            // An empty archive is one empty `last` chunk.
            let parts = p.bytes.len().div_ceil(ARTIFACT_CHUNK_BYTES).max(1);
            for seq in 0..parts {
                let start = seq * ARTIFACT_CHUNK_BYTES;
                let end = (start + ARTIFACT_CHUNK_BYTES).min(p.bytes.len());
                let frame = Frame::SkillChunk {
                    id: id.clone(),
                    layer: p.layer,
                    name: p.name.clone(),
                    hash: p.hash.hex(),
                    seq: seq as u64,
                    data_b64: base64::engine::general_purpose::STANDARD
                        .encode(&p.bytes[start..end]),
                    last: seq + 1 == parts,
                };
                self.send_bounded(&conn, frame, "SkillChunk").await?;
                match rx.try_recv() {
                    Ok(msg) => {
                        early = Some(msg);
                        break 'send;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        return Err(self.offer_disconnected())
                    }
                }
            }
        }

        let answer = match early {
            Some(msg) => msg,
            None => self.next_offer_msg(&mut rx).await?,
        };
        match answer {
            OfferMsg::Stored { error: Some(e), .. } => {
                Err(HostConnectorError::Invalid(format!("{who}: {e}")))
            }
            OfferMsg::Stored {
                stored,
                error: None,
            } => {
                let key = |r: &rupu_skills::ship::ShippedSkillRef| {
                    (r.layer, r.name.clone(), r.content_hash.clone())
                };
                let mut want: Vec<_> = needed.iter().map(key).collect();
                let mut got: Vec<_> = stored.iter().map(key).collect();
                want.sort();
                got.sort();
                if want != got {
                    return Err(HostConnectorError::Invalid(format!(
                        "{who}: the node reported storing {} but was sent {}",
                        named(&stored),
                        named(&needed)
                    )));
                }
                Ok(Transfer::Done)
            }
            OfferMsg::Need { .. } => Err(HostConnectorError::Invalid(format!(
                "{who}: answered one skill offer with SkillsNeed twice"
            ))),
        }
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
            skill_bundle: None,
            kind: RunSpecKind::Workflow,
            name: req.workflow.clone(),
            inputs: req.inputs.clone(),
            prompt: None,
            mode: req.mode.clone(),
            target: req.target.clone(),
            findings_profile: None,
            engagement_profiles: Vec::new(),
            skills: Vec::new(),
            preload_skills: Vec::new(),
            codename: None,
        };

        // Verify the node is reachable BEFORE creating the mirror run.
        // This prevents an offline node from leaving an uncancellable Running
        // record with no executor attached.
        let conn = self.live_conn()?;

        self.mirror
            .create_run(&run_id, &self.node_id, &spec)
            .await
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
            let _ = self
                .mirror
                .finish(&run_id, &self.node_id, "cancelled")
                .await;
            return Err(HostConnectorError::Unreachable(format!(
                "node {} disconnected before Run frame could be sent",
                self.node_id
            )));
        }

        Ok(run_id)
    }

    async fn launch_agent(&self, req: AgentLaunchRequest) -> Result<String, HostConnectorError> {
        // A coordinator-minted id (a placed unit's) is the id the node runs
        // under: it travels in the `Run` frame, which every node passes on as
        // `rupu run --run-id`. Validated before anything is created.
        let run_id = req.run_id_or_mint().map_err(HostConnectorError::Invalid)?;
        let run = req
            .agent_run(&run_id)
            .map_err(HostConnectorError::Invalid)?;

        // Verify the node is reachable BEFORE creating the mirror run.
        // This prevents an offline node from leaving an uncancellable Running
        // record with no executor attached.
        let conn = self.live_conn()?;

        let spec = RunSpec {
            skill_bundle: req.skill_bundle.clone(),
            kind: RunSpecKind::Agent,
            name: req.agent.clone(),
            inputs: std::collections::BTreeMap::new(),
            prompt: req.prompt.clone(),
            mode: req.mode.clone(),
            target: req.target.clone(),
            findings_profile: req.findings_profile,
            engagement_profiles: req.engagement_profiles.clone(),
            skills: req.skills.clone(),
            preload_skills: req.preload_skills.clone(),
            codename: spec_peer_codename(
                &run,
                conn.capabilities(),
                &self.node_id,
                conn.rupu_version(),
            ),
        };

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
        // Same for `RunSpec.engagement_profiles`: an older node would run the
        // agent on the `code` path.
        if !req.engagement_profiles.is_empty() && !conn.supports(CAP_AGENT_ENGAGEMENT_PROFILE) {
            return Err(HostConnectorError::Unsupported(format!(
                "node {} (rupu {}) does not support engagement profiles on agent \
                 launches, so this run cannot run under the `{}` engagement \
                 profile(s); upgrade rupu on that node",
                self.node_id,
                conn.rupu_version().unwrap_or("unknown version"),
                req.engagement_profiles.join(","),
            )));
        }
        // And `RunSpec.skills`: an older node would run the unit without its
        // step's skills.
        if (!req.skills.is_empty() || !req.preload_skills.is_empty())
            && !conn.supports(CAP_AGENT_SKILLS)
        {
            return Err(HostConnectorError::Unsupported(format!(
                "node {} (rupu {}) does not support workflow-step skills on agent \
                 launches, so this run's skills cannot be delivered; upgrade rupu on \
                 that node",
                self.node_id,
                conn.rupu_version().unwrap_or("unknown version"),
            )));
        }
        // And `RunSpec.skill_bundle`: an older node would run the unit
        // without the skills shipped to it.
        if req.skill_bundle.is_some() && !conn.supports(CAP_AGENT_SKILL_BUNDLE) {
            return Err(HostConnectorError::Unsupported(format!(
                "node {} (rupu {}) does not support shipped skills on agent launches, so \
                 this run's shipped skills cannot be delivered; upgrade rupu on that node",
                self.node_id,
                conn.rupu_version().unwrap_or("unknown version"),
            )));
        }

        self.mirror
            .create_run(&run_id, &self.node_id, &spec)
            .await
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
            let _ = self
                .mirror
                .finish(&run_id, &self.node_id, "cancelled")
                .await;
            return Err(HostConnectorError::Unreachable(format!(
                "node {} disconnected before Run frame could be sent",
                self.node_id
            )));
        }

        Ok(run_id)
    }

    /// Offer the bundle over the tunnel. A node without
    /// [`CAP_AGENT_SKILL_BUNDLE`] is the P5 downgrade, decided per call
    /// (never memoised): the node may reconnect upgraded.
    async fn ship_skills(&self, bundle: &SkillBundle) -> Result<ShipOutcome, HostConnectorError> {
        if bundle.is_empty() {
            return Ok(ShipOutcome::NotNeeded);
        }
        let conn = self.live_conn()?;
        if !conn.supports(CAP_AGENT_SKILL_BUNDLE) {
            return Ok(ShipOutcome::Unsupported(self.no_skill_bundle(&conn)));
        }
        let who = format!("host {}", self.node_id);
        skill_ship::ship_through(&self.ship_memo, bundle, &who, |pending| self.offer(pending)).await
    }

    async fn start_session(&self, _req: SessionStartRequest) -> Result<String, HostConnectorError> {
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
            Arc::clone(&self.pricing),
        );
        blocking_host(move || mirror_list_runs(&store, &id, &params, &pricing)).await
    }

    async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        let (store, id, pricing) = (
            Arc::clone(&self.run_store),
            self.node_id.clone(),
            Arc::clone(&self.pricing),
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

    /// A tunnel carries no message frames.
    async fn list_messages(
        &self,
        _id: &str,
        _q: ViewQuery,
    ) -> Result<MessagesView, HostConnectorError> {
        Err(crate::host::messages::no_message_transport("tunnel"))
    }

    async fn send_operator_message(
        &self,
        _id: &str,
        _msg: OperatorMessageRequest,
    ) -> Result<OperatorSendReply, HostConnectorError> {
        Err(crate::host::messages::no_message_transport("tunnel"))
    }

    /// Ask the node for the blob over the tunnel (`Frame::ArtifactPull`) and
    /// write its ordered `ArtifactChunk`s to `dest` until `ArtifactPullDone`.
    async fn pull_finding_artifact(
        &self,
        sha256: &str,
        dest: &std::path::Path,
        max_bytes: u64,
    ) -> Result<(), HostConnectorError> {
        use crate::node::PullMsg;
        use tokio::io::AsyncWriteExt;
        crate::host::connector::validate_sha256(sha256)?;
        let conn = self.live_conn()?;
        // An older node fails the whole tunnel on a frame it can't parse, so
        // never send one to a node that didn't advertise it.
        if !conn.supports(CAP_FINDINGS_ARTIFACT_PULL) {
            return Err(HostConnectorError::Unsupported(format!(
                "node {} (rupu {}) cannot serve artifact pulls; upgrade rupu on that node",
                self.node_id,
                conn.rupu_version().unwrap_or("unknown version"),
            )));
        }
        let write_err =
            |e: std::io::Error| HostConnectorError::Invalid(format!("local write failed: {e}"));
        // On any failure past this point `dest` may be partially written; the
        // caller owns its cleanup and only a returned `Ok` means it is whole.
        let mut file = tokio::fs::File::create(dest).await.map_err(write_err)?;

        let req = format!("pull_{}", Ulid::new());
        let mut rx = conn.begin_pull(&req);
        let _registered = PullRegistration {
            conn: &conn,
            req: &req,
        };
        // The tunnel's write channel is bounded: a node that stopped reading
        // leaves it full, so even the request is held to the idle bound.
        let sent = conn.send(Frame::ArtifactPull {
            req: req.clone(),
            sha256: sha256.to_string(),
        });
        tokio::time::timeout(ARTIFACT_PULL_IDLE_TIMEOUT, sent)
            .await
            .map_err(|_| {
                HostConnectorError::Unreachable(format!(
                    "node {} took no ArtifactPull frame for {}s (its tunnel is not draining)",
                    self.node_id,
                    ARTIFACT_PULL_IDLE_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|_| {
                HostConnectorError::Unreachable(format!(
                    "node {} disconnected before ArtifactPull frame could be sent",
                    self.node_id
                ))
            })?;

        let (mut total, mut next_seq) = (0u64, 0u64);
        loop {
            let msg = tokio::time::timeout(ARTIFACT_PULL_IDLE_TIMEOUT, rx.recv())
                .await
                .map_err(|_| {
                    HostConnectorError::Unreachable(format!(
                        "node {} sent nothing of artifact {sha256} for {}s",
                        self.node_id,
                        ARTIFACT_PULL_IDLE_TIMEOUT.as_secs()
                    ))
                })?;
            match msg {
                // The tunnel closed (`NodeConn::close_pulls`).
                None => {
                    return Err(HostConnectorError::Unreachable(format!(
                        "node {} disconnected while sending artifact {sha256}",
                        self.node_id
                    )))
                }
                Some(PullMsg::Chunk { seq, data }) => {
                    if seq != next_seq {
                        return Err(HostConnectorError::Invalid(format!(
                            "artifact chunk {seq} arrived out of order (expected {next_seq})"
                        )));
                    }
                    next_seq += 1;
                    total += data.len() as u64;
                    if total > max_bytes {
                        return Err(HostConnectorError::Invalid(format!(
                            "artifact {sha256} exceeds its recorded {max_bytes} bytes"
                        )));
                    }
                    file.write_all(&data).await.map_err(write_err)?;
                }
                Some(PullMsg::Done(None)) => {
                    file.flush().await.map_err(write_err)?;
                    return Ok(());
                }
                Some(PullMsg::Done(Some(e))) => {
                    return Err(HostConnectorError::NotFound(format!(
                        "node {}: {e}",
                        self.node_id
                    )))
                }
            }
        }
    }

    async fn stream_run_events(&self, run_id: &str) -> Result<EventByteStream, HostConnectorError> {
        self.stream_run_events_after(run_id, 0).await
    }

    async fn stream_run_events_after(
        &self,
        run_id: &str,
        after: u64,
    ) -> Result<EventByteStream, HostConnectorError> {
        mirror_stream_run_events(&self.run_store, &self.node_id, run_id, after).await
    }

    async fn get_transcript(&self, path: &str) -> Result<serde_json::Value, HostConnectorError> {
        read_transcript_file(path).await
    }

    /// SSH/Tunnel/Bucket runs are created in, and tailed into, the
    /// coordinator's own `RunStore` by `NodeMirror`, so run-scoped detail
    /// endpoints read that mirror instead of the wire.
    fn serves_runs_from_local_mirror(&self) -> bool {
        true
    }

    /// `launch_agent` sends a supplied id as the job's run id, which every
    /// node has run under (`rupu run --run-id`) since the first tunnel slice.
    fn honours_supplied_run_id(&self) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pull_capable(
        dir: &std::path::Path,
    ) -> (
        TunnelHostConnector,
        tokio::sync::mpsc::Receiver<Frame>,
        Arc<crate::node::NodeConn>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let registry = Arc::new(NodeRegistry::new());
        let node_conn = registry.register_with_hello(
            "node-x",
            tx,
            crate::node::protocol::node_capabilities(),
            None,
        );
        let run_store = Arc::new(RunStore::new(dir.join("runs")));
        let connector = TunnelHostConnector::new(
            "node-x",
            registry,
            Arc::new(NodeMirror::new(Arc::clone(&run_store))),
            run_store,
            Arc::new(crate::customers::CustomerPricing::flat(
                rupu_config::PricingConfig::default(),
            )),
        );
        (connector, rx, node_conn)
    }

    fn one_skill(dir: &std::path::Path) -> SkillBundle {
        let skill = dir.join("pdf");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: pdf\ndescription: PDFs.\n---\n\nBody.\n",
        )
        .unwrap();
        SkillBundle::new(vec![Arc::new(
            rupu_skills::load_skill(&skill, rupu_skills::Layer::Global).unwrap(),
        )])
    }

    /// A finished offer leaves no routing entry behind.
    #[tokio::test]
    async fn a_finished_offer_forgets_its_routing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let (connector, mut rx, node_conn) = pull_capable(dir.path());
        let bundle = one_skill(dir.path());
        let task = tokio::spawn(async move { connector.ship_skills(&bundle).await });
        let Some(Frame::SkillsOffer { id, .. }) = rx.recv().await else {
            panic!("expected SkillsOffer")
        };
        assert_eq!(node_conn.offers_in_flight(), 1);
        node_conn
            .route_offer(
                &id,
                crate::node::OfferMsg::Need {
                    skills: Vec::new(),
                    error: None,
                },
            )
            .await;
        assert!(matches!(
            task.await.unwrap().unwrap(),
            ShipOutcome::Shipped(_)
        ));
        assert_eq!(node_conn.offers_in_flight(), 0);
    }

    /// A ship whose future is dropped mid-offer (its task aborted) still
    /// forgets its routing entry.
    #[tokio::test]
    async fn an_abandoned_offer_forgets_its_routing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let (connector, mut rx, node_conn) = pull_capable(dir.path());
        let bundle = one_skill(dir.path());
        let task = tokio::spawn(async move { connector.ship_skills(&bundle).await });
        assert!(matches!(rx.recv().await, Some(Frame::SkillsOffer { .. })));
        assert_eq!(node_conn.offers_in_flight(), 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(node_conn.offers_in_flight(), 0);
    }

    /// A node that never answers fails the offer on the idle bound rather
    /// than holding the ship lock forever. (Paused time.)
    #[tokio::test(start_paused = true)]
    async fn a_silent_node_fails_the_offer_on_the_idle_bound() {
        let dir = tempfile::tempdir().unwrap();
        let (connector, mut rx, node_conn) = pull_capable(dir.path());
        let bundle = one_skill(dir.path());
        let task = tokio::spawn(async move { connector.ship_skills(&bundle).await });
        assert!(matches!(rx.recv().await, Some(Frame::SkillsOffer { .. })));
        let err = tokio::time::timeout(ARTIFACT_PULL_IDLE_TIMEOUT * 2, task)
            .await
            .expect("the offer must give up on the idle bound")
            .unwrap()
            .unwrap_err();
        assert!(
            matches!(&err, HostConnectorError::Unreachable(m) if m.contains("node-x")),
            "{err:?}"
        );
        assert_eq!(node_conn.offers_in_flight(), 0);
    }

    /// A finished pull leaves no routing entry behind.
    #[tokio::test]
    async fn a_finished_pull_forgets_its_routing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let (connector, mut rx, node_conn) = pull_capable(dir.path());
        let dest = dir.path().join("pulled");
        let task = tokio::spawn(async move {
            connector
                .pull_finding_artifact(&"ab".repeat(32), &dest, 6)
                .await
        });
        let Some(Frame::ArtifactPull { req, .. }) = rx.recv().await else {
            panic!("expected ArtifactPull")
        };
        assert_eq!(node_conn.pulls_in_flight(), 1);
        node_conn
            .route_pull(&req, crate::node::PullMsg::Done(None))
            .await;
        task.await.unwrap().unwrap();
        assert_eq!(node_conn.pulls_in_flight(), 0);
    }

    /// A pull whose future is dropped mid-transfer (its task aborted) still
    /// forgets its routing entry — no path leaves a sender in the table.
    #[tokio::test]
    async fn an_abandoned_pull_forgets_its_routing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let (connector, mut rx, node_conn) = pull_capable(dir.path());
        let dest = dir.path().join("pulled");
        let task = tokio::spawn(async move {
            connector
                .pull_finding_artifact(&"ab".repeat(32), &dest, 6)
                .await
        });
        assert!(matches!(rx.recv().await, Some(Frame::ArtifactPull { .. })));
        assert_eq!(node_conn.pulls_in_flight(), 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(node_conn.pulls_in_flight(), 0);
    }

    /// A node that stopped reading leaves its tunnel's bounded write channel
    /// full. The pull's initial `ArtifactPull` send must not wait on it
    /// forever: it fails on the same idle bound as a silent node, and
    /// forgets its routing entry. (Paused time: the 60 s pass instantly.)
    #[tokio::test(start_paused = true)]
    async fn a_full_tunnel_fails_the_initial_send_on_the_idle_bound() {
        let dir = tempfile::tempdir().unwrap();
        let (connector, _rx, node_conn) = pull_capable(dir.path());
        for i in 0..4 {
            node_conn
                .send(Frame::Cancel {
                    run_id: format!("run_{i}"),
                })
                .await
                .unwrap();
        }
        let err = tokio::time::timeout(
            ARTIFACT_PULL_IDLE_TIMEOUT * 2,
            connector.pull_finding_artifact(&"ab".repeat(32), &dir.path().join("pulled"), 6),
        )
        .await
        .expect("the initial send must give up on the idle bound")
        .unwrap_err();
        assert!(
            matches!(&err, HostConnectorError::Unreachable(m) if m.contains("ArtifactPull")),
            "{err:?}"
        );
        assert_eq!(node_conn.pulls_in_flight(), 0);
    }
}
