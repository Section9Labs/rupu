//! `HttpHostConnector` — proxies every [`HostConnector`] call over HTTP to a
//! remote `rupu cp serve` instance.
//!
//! One private [`HttpHostConnector::send`] helper attaches the bearer token
//! and maps transport / status errors so every method stays DRY.

#![deny(clippy::all)]

use futures_util::StreamExt as _;
use rupu_runtime::services::operator::{MessagesView, OperatorSendReply, ViewQuery};
use rupu_skills::ship::{PackedSkill, ShippedSkillRef};
use std::sync::Arc;
use std::time::Duration;

use crate::{
    agent_launcher::AgentLaunchRequest,
    host::connector::{
        spec_peer_codename, EventByteStream, HostCapabilities, HostConnector, HostConnectorError,
        HostInfo, NetflowLanding, OperatorMessageRequest, RunKind, RunListQuery,
        MAX_WORKSPACE_BYTES,
    },
    host::messages::MessageRoute,
    host::skill_ship::{prefixed, ship_through, ShipMemo, ShipOutcome, SkillBundle, Transfer},
    launcher::LaunchRequest,
    node::protocol::{
        messaging_capability, CAP_AGENT_ENGAGEMENT_PROFILE, CAP_AGENT_FINDINGS_PROFILE,
        CAP_AGENT_SKILLS, CAP_AGENT_SKILL_BUNDLE, CAP_RUN_MESSAGES, CAP_RUN_NETFLOW_STREAM,
        CAP_RUN_SUPPLIED_RUN_ID,
        CAP_SESSION_SKILLS,
    },
    session_sender::SendMessageRequest,
    session_starter::SessionStartRequest,
};

/// Whole-transfer bound for one artifact pull. Replaces the client's 30 s
/// total timeout (sized for JSON calls) on that request alone.
const ARTIFACT_PULL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long an artifact pull may receive nothing — the response head, then
/// each body chunk — before it is abandoned: a half-open connection must not
/// pin the coordinator's shared pull (and every viewer waiting on it) for the
/// whole [`ARTIFACT_PULL_TIMEOUT`]. The SSH and tunnel pulls use the same 60 s.
const ARTIFACT_PULL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Whole-life bound for one run event stream: in place of the client's 30 s
/// total timeout, which would cut a live stream (and a `wait` reading it)
/// every 30 s. The connect timeout still applies.
const EVENT_STREAM_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a run event stream's response head may take: the old total bound,
/// so a remote that accepts but never answers still fails fast.
const EVENT_STREAM_HEAD_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a run event stream may receive nothing, once its head arrived,
/// before it ends with an error: the peer sends a keep-alive every 15 s
/// (`tail_events_sse`), so a stream silent this long is a dead or half-open
/// peer, and its reader (the web's proxied stream, a `wait`) reconnects
/// rather than hang for the whole [`EVENT_STREAM_TIMEOUT`]. As
/// [`ARTIFACT_PULL_IDLE_TIMEOUT`].
const EVENT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Whole-transfer bound for one shipped skill's `PUT` (up to 32 MiB), in
/// place of the client's 30 s total timeout.
const SKILL_PUT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// `stream`, ended with a `TimedOut` error once `idle` passes with no chunk.
fn idle_bounded(
    stream: impl futures_util::Stream<Item = std::io::Result<bytes::Bytes>> + Send + 'static,
    idle: Duration,
    base_url: String,
) -> EventByteStream {
    let state = Some(Box::pin(stream));
    Box::pin(futures_util::stream::unfold(state, move |state| {
        let base_url = base_url.clone();
        async move {
            let mut stream = state?;
            match tokio::time::timeout(idle, stream.next()).await {
                Ok(Some(item)) => Some((item, Some(stream))),
                Ok(None) => None,
                // Said once; the stream ends after it.
                Err(_) => Some((
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("no data from {base_url} for {} s", idle.as_secs()),
                    )),
                    None,
                )),
            }
        }
    }))
}

// ── Struct ────────────────────────────────────────────────────────────────────

/// Remote-host connector: forwards every [`HostConnector`] call as an HTTP
/// request to a running `rupu cp serve`.
pub struct HttpHostConnector {
    client: reqwest_middleware::ClientWithMiddleware,
    base_url: String,
    token: Option<String>,
    /// [`ARTIFACT_PULL_IDLE_TIMEOUT`]; a field so tests can shorten it.
    artifact_idle: Duration,
    /// [`EVENT_STREAM_IDLE_TIMEOUT`]; a field so tests can shorten it.
    event_idle: Duration,
    /// The `features` the remote's `/api/host/info` last listed (empty for a
    /// remote predating it); `None` until it was first read. What the
    /// synchronous [`HostConnector::honours_supplied_run_id`] answers from.
    features_seen: std::sync::Mutex<Option<Vec<String>>>,
    /// The shipped skills this remote confirmed it holds — see
    /// [`HostConnector::ship_skills`].
    ship_memo: ShipMemo,
    /// The registry id this connector was built for ([`Self::with_host_id`]);
    /// what a netflow landing record names as its host. `None` for a bare
    /// connector, which names its base URL instead.
    host_id: Option<String>,
}

/// What `/api/host/info` said about the remote.
enum PeerInfo {
    Info(HostInfoBody),
    /// The remote predates `/api/host/info` (404, or its web UI's HTML):
    /// it advertises nothing. The text says which, for a refusal message.
    Predates(String),
}

/// Private response struct for deserializing the `/api/host/info` endpoint.
#[derive(serde::Deserialize)]
struct HostInfoBody {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    capabilities: HostCapabilities,
    /// What the remote's launch endpoints honour (e.g.
    /// [`CAP_AGENT_FINDINGS_PROFILE`]). Absent on a remote predating it.
    #[serde(default)]
    features: Vec<String>,
}

/// Parse a `/api/host/info` body. The ONE place that decides what "the remote
/// did not answer with host info" means, shared by [`HostConnector::info`]
/// (reachable, version unknown) and `require_feature` (`Unsupported`).
///
/// An `Err` is a 2xx whose complete body is not usable host info — in practice
/// the web UI's HTML from a rupu older than `/api/host/info` (whose SPA
/// fallback answers any `/api/*` path with 200). It is NOT a transport
/// failure: a body that failed to arrive never reaches this function.
fn parse_host_info(bytes: &[u8]) -> Result<HostInfoBody, serde_json::Error> {
    serde_json::from_slice(bytes)
}

impl HttpHostConnector {
    /// Create a new connector for the remote server at `base_url`.
    ///
    /// `token`, when `Some`, is sent as `Authorization: Bearer <token>` on
    /// every request.
    pub fn new(base_url: String, token: Option<String>) -> Self {
        // Bounded so one unreachable host cannot stall a fan-out on the OS TCP
        // connect timeout. Fan-out is concurrent (join_all), so wall-clock is
        // the slowest host — which must therefore be bounded.
        let ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Cp);
        let builder = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30));
        // `Arc::new(NullSink)`, deliberately: `cp serve`'s own fleet HTTP
        // traffic is daemon-lifetime, not run-scoped, and per the netflow
        // per-run plan the daemon's own traffic is no longer recorded (the
        // old shared `$RUPU_HOME/netflow/flows.jsonl` ledger this used to
        // land in — installed via the now-deleted `http::init` call in
        // `rupu-cli/src/cmd/cp.rs` — is gone). `.expect(...)` preserves the
        // deleted `http::client()` fallback's panic-on-failure behaviour.
        let client =
            rupu_netflow::http::client_with(ctx, builder, Arc::new(rupu_netflow::NullSink))
                .expect("cp host client build");
        Self {
            client,
            base_url,
            token,
            artifact_idle: ARTIFACT_PULL_IDLE_TIMEOUT,
            event_idle: EVENT_STREAM_IDLE_TIMEOUT,
            features_seen: std::sync::Mutex::new(None),
            ship_memo: ShipMemo::default(),
            host_id: None,
        }
    }

    /// Name the registry host this connector serves, so what it records (a
    /// netflow landing's `host`) carries the id every other view uses.
    pub fn with_host_id(mut self, host_id: impl Into<String>) -> Self {
        self.host_id = Some(host_id.into());
        self
    }

    /// Like [`Self::new`], but bounds every request's connect + total time to
    /// a caller-chosen `timeout` rather than [`Self::new`]'s 5s/30s. Used by
    /// the host-probe fallback (`api::run_resolve::probe_hosts`), which wants
    /// to fail much faster than a normal request should.
    ///
    /// Both constructors are now bounded. [`Self::new`] used to keep
    /// `reqwest`'s default (effectively unbounded) behavior, which made this
    /// method the only fast-failing path; that stopped being true once
    /// dashboard fan-out started calling every host concurrently, where
    /// wall-clock is the slowest host and one unreachable box could stall the
    /// whole page on the OS's TCP connect timeout.
    ///
    /// Panics (`.expect()`) if the `reqwest::ClientBuilder` itself fails
    /// to build (e.g. an invalid TLS config) — preserves the deleted
    /// `http::client()` fallback's panic-on-failure behaviour (netflow
    /// per-run plan, Task 67); there is no unbounded-client fallback any
    /// more, uninstrumented or otherwise.
    pub fn new_with_timeout(base_url: String, token: Option<String>, timeout: Duration) -> Self {
        let ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Cp);
        let builder = reqwest::Client::builder()
            .connect_timeout(timeout)
            .timeout(timeout);
        // See `Self::new` — `cp serve`'s own fleet traffic is deliberately
        // unrecorded (daemon-lifetime, not run-scoped).
        let client =
            rupu_netflow::http::client_with(ctx, builder, Arc::new(rupu_netflow::NullSink))
                .expect("cp host client build");
        Self {
            client,
            base_url,
            token,
            artifact_idle: ARTIFACT_PULL_IDLE_TIMEOUT,
            event_idle: EVENT_STREAM_IDLE_TIMEOUT,
            features_seen: std::sync::Mutex::new(None),
            ship_memo: ShipMemo::default(),
            host_id: None,
        }
    }

    /// Build an absolute URL by appending `path` (which must start with `/`)
    /// to the configured base URL.
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Attach the bearer token (if set), send the request, and map transport
    /// and HTTP status errors to [`HostConnectorError`].
    ///
    /// - Network/DNS/timeout → `Unreachable`
    /// - HTTP 401 → `Unauthorized`
    /// - HTTP 404 → `NotFound`
    /// - HTTP ≥ 400 (other) → `Remote(status, body)`
    /// - 2xx → `Ok(Response)`
    async fn send(
        &self,
        req: reqwest_middleware::RequestBuilder,
    ) -> Result<reqwest::Response, HostConnectorError> {
        let req = match &self.token {
            Some(tok) => req.header("Authorization", format!("Bearer {tok}")),
            None => req,
        };

        let resp = req
            .send()
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;

        match resp.status().as_u16() {
            200..=299 => Ok(resp),
            401 => Err(HostConnectorError::Unauthorized),
            404 => {
                let url = resp.url().to_string();
                Err(HostConnectorError::NotFound(url))
            }
            s => {
                let body = resp.text().await.unwrap_or_default();
                Err(HostConnectorError::Remote(s, body))
            }
        }
    }

    /// Read the remote's `/api/host/info`, remembering its `features` for
    /// [`HostConnector::honours_supplied_run_id`]. Read, then parse (as
    /// `proxy_get_json` does): a body that failed to ARRIVE is a transport
    /// failure, but a 200 whose body is not the info JSON is a remote that
    /// has no such route and answered with its SPA — the signature of a rupu
    /// older than `/api/host/info`, the same as its 404.
    async fn peer_info(&self) -> Result<PeerInfo, HostConnectorError> {
        let info = match self.send(self.client.get(self.url("/api/host/info"))).await {
            Ok(resp) => {
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| HostConnectorError::Remote(0, e.to_string()))?;
                match parse_host_info(&bytes) {
                    Ok(body) => PeerInfo::Info(body),
                    Err(e) => PeerInfo::Predates(format!(
                        "did not answer /api/host/info with host info ({e}; an older rupu \
                         serves its web UI there)"
                    )),
                }
            }
            Err(HostConnectorError::NotFound(_)) => {
                PeerInfo::Predates("predates /api/host/info".to_string())
            }
            Err(e) => return Err(e),
        };
        self.remember_features(match &info {
            PeerInfo::Info(body) => body.features.clone(),
            PeerInfo::Predates(_) => Vec::new(),
        });
        Ok(info)
    }

    fn remember_features(&self, features: Vec<String>) {
        *self.features_seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(features);
    }

    /// Refuse unless the remote's `/api/host/info` lists `feature`. A remote
    /// predating a request field ignores it (its body structs don't deny
    /// unknown fields), so an unadvertised feature — including a remote with
    /// no `/api/host/info` at all — is a refusal, not a best-effort send.
    async fn require_feature(&self, feature: &str, what: &str) -> Result<(), HostConnectorError> {
        let info = self.peer_info().await?;
        Self::require_in(&info, feature, what, &self.base_url)
    }

    /// [`Self::require_feature`], answered from the `features` this connector
    /// last read (the host probe re-reads them) and reading `/api/host/info`
    /// only when it never has: for a call a page may poll.
    async fn require_known_feature(
        &self,
        feature: &str,
        what: &str,
    ) -> Result<(), HostConnectorError> {
        let seen = self
            .features_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match seen {
            None => self.require_feature(feature, what).await,
            Some(features) if features.iter().any(|f| f == feature) => Ok(()),
            Some(_) => Err(HostConnectorError::Unsupported(format!(
                "{what}: remote host {} does not advertise `{feature}`; upgrade rupu there",
                self.base_url
            ))),
        }
    }

    /// `/api/<route>/<id>/messages` on the remote, once it advertises
    /// [`CAP_RUN_MESSAGES`]. Its 404 names the id, and its 400 is the
    /// caller's mistake.
    async fn messages_request(
        &self,
        id: &str,
        what: &str,
        build: impl FnOnce(String) -> reqwest_middleware::RequestBuilder,
    ) -> Result<reqwest::Response, HostConnectorError> {
        let route = MessageRoute::of_id(id)
            .ok_or_else(|| HostConnectorError::NotFound(format!("no run `{id}`")))?;
        self.require_known_feature(CAP_RUN_MESSAGES, what).await?;
        let url = self.url(&format!("/api/{}/{id}/messages", route.segment()));
        self.send(build(url)).await.map_err(|e| match e {
            HostConnectorError::NotFound(_) => HostConnectorError::NotFound(format!(
                "remote host {} has no {} `{id}`",
                self.base_url,
                route.noun()
            )),
            HostConnectorError::Remote(400, body) => {
                HostConnectorError::Invalid(crate::api::runs::remote_error_text(&body))
            }
            other => other,
        })
    }

    /// [`Self::require_feature`] against an already-read [`PeerInfo`].
    fn require_in(
        info: &PeerInfo,
        feature: &str,
        what: &str,
        base_url: &str,
    ) -> Result<(), HostConnectorError> {
        match info {
            PeerInfo::Predates(why) => Err(HostConnectorError::Unsupported(format!(
                "{what}: remote host {base_url} {why}, so it cannot advertise support; \
                 upgrade rupu there"
            ))),
            PeerInfo::Info(body) if body.features.iter().any(|f| f == feature) => Ok(()),
            PeerInfo::Info(body) => Err(HostConnectorError::Unsupported(format!(
                "{what}: remote host {base_url} (rupu {}) does not advertise `{feature}`; upgrade rupu there",
                body.version.as_deref().unwrap_or("unknown version"),
            ))),
        }
    }

    /// Send the skills of `pending` the remote lacks: `POST
    /// /api/skills/bundle/missing`, then a `PUT /api/skills/bundle/:hash` per
    /// missing one. A remote not advertising [`CAP_AGENT_SKILL_BUNDLE`] is
    /// a [`Transfer::Downgrade`], which the memo remembers for the
    /// connector's life.
    async fn ship_pending(
        &self,
        pending: Vec<Arc<PackedSkill>>,
    ) -> Result<Transfer, HostConnectorError> {
        let who = format!("remote host {}", self.base_url);
        let info = self.peer_info().await.map_err(|e| prefixed(e, &who))?;
        if let Err(e) = Self::require_in(
            &info,
            CAP_AGENT_SKILL_BUNDLE,
            "shipped skills",
            &self.base_url,
        ) {
            return match e {
                HostConnectorError::Unsupported(m) => Ok(Transfer::Downgrade(m)),
                e => Err(e),
            };
        }
        let refs: Vec<ShippedSkillRef> = pending.iter().map(|p| p.reference()).collect();
        let resp = self
            .send(
                self.client
                    .post(self.url("/api/skills/bundle/missing"))
                    .json(&serde_json::json!({ "skills": refs })),
            )
            .await
            .map_err(|e| prefixed(e, &who))?;
        #[derive(serde::Deserialize)]
        struct Missing {
            missing: Vec<ShippedSkillRef>,
        }
        let missing: Missing = resp.json().await.map_err(|e| {
            HostConnectorError::Remote(
                0,
                format!("{who}: bad /api/skills/bundle/missing reply: {e}"),
            )
        })?;
        for r in &missing.missing {
            let Some(p) = refs.iter().position(|x| x == r).map(|i| &pending[i]) else {
                return Err(HostConnectorError::Remote(
                    0,
                    format!(
                        "{who}: /api/skills/bundle/missing named {}:{}, which was not asked",
                        r.layer, r.name
                    ),
                ));
            };
            let resp = self
                .send(
                    self.client
                        .put(self.url(&format!("/api/skills/bundle/{}", r.content_hash)))
                        .query(&[("layer", r.layer.to_string()), ("name", r.name.clone())])
                        .header("Content-Type", "application/x-tar")
                        .timeout(SKILL_PUT_TIMEOUT)
                        .body(p.bytes.clone()),
                )
                .await
                .map_err(|e| prefixed(e, &who))?;
            let stored =
                extract_string_field(resp.json().await, "stored").map_err(|e| prefixed(e, &who))?;
            if stored != r.content_hash {
                return Err(HostConnectorError::Remote(
                    0,
                    format!(
                        "{who}: stored {stored} for {}:{} ({})",
                        r.layer, r.name, r.content_hash
                    ),
                ));
            }
        }
        Ok(Transfer::Done)
    }
}

// ── Trait impl ────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl HostConnector for HttpHostConnector {
    /// Fetch health + version info.
    ///
    /// Unlike every other method, an unreachable host is **not** an error here:
    /// it returns `HostInfo { reachable: false, .. }` instead.
    async fn info(&self) -> Result<HostInfo, HostConnectorError> {
        let req = self.client.get(self.url("/api/host/info"));
        match self.send(req).await {
            Ok(resp) => {
                // Read, then parse: a body that failed to ARRIVE is a
                // transport failure (`Remote(0, _)`), but a 200 that is not
                // host info is a reachable remote that predates the endpoint
                // — the same outcome as its 404 below, not "offline".
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| HostConnectorError::Remote(0, e.to_string()))?;
                Ok(match parse_host_info(&bytes) {
                    Ok(body) => {
                        self.remember_features(body.features);
                        HostInfo {
                            reachable: true,
                            version: body.version,
                            capabilities: body.capabilities,
                        }
                    }
                    Err(_) => HostInfo {
                        reachable: true,
                        version: None,
                        capabilities: HostCapabilities::default(),
                    },
                })
            }
            Err(HostConnectorError::Unreachable(_)) => Ok(HostInfo {
                reachable: false,
                version: None,
                capabilities: HostCapabilities::default(),
            }),
            Err(HostConnectorError::NotFound(_)) => Ok(HostInfo {
                reachable: true,
                version: None,
                capabilities: HostCapabilities::default(),
            }),
            Err(e) => Err(e),
        }
    }

    async fn launch_run(&self, req: LaunchRequest) -> Result<String, HostConnectorError> {
        let body = serde_json::json!({
            "inputs": req.inputs,
            "mode": req.mode,
            "target": req.target,
            "working_dir": req.working_dir,
        });
        let resp = self
            .send(
                self.client
                    .post(self.url(&format!("/api/workflows/{}/run", req.workflow)))
                    .json(&body),
            )
            .await?;
        extract_string_field(resp.json().await, "run_id")
    }

    async fn launch_agent(&self, req: AgentLaunchRequest) -> Result<String, HostConnectorError> {
        // A supplied id (and the mode) is validated whether or not the remote
        // will take it; `run` is what the codename decision below reads.
        if let Some(Err(e)) = req.run_id.as_ref().map(|_| req.run_id_or_mint()) {
            return Err(HostConnectorError::Invalid(e));
        }
        let run = req
            .agent_run(req.run_id.as_deref().unwrap_or("(minted by the remote)"))
            .map_err(HostConnectorError::Invalid)?;
        // One `/api/host/info` read covers every field that needs the
        // remote's say-so, and none is made for a launch that needs none.
        let has_skills = !req.skills.is_empty() || !req.preload_skills.is_empty();
        let needs_info = req.findings_profile.is_some()
            || !req.engagement_profiles.is_empty()
            || has_skills
            || req.skill_bundle.is_some()
            || !req.messaging.is_default()
            || req.run_id.is_some()
            || req.codename.is_some();
        let info = if needs_info {
            Some(self.peer_info().await?)
        } else {
            None
        };
        if let (Some(profile), Some(info)) = (req.findings_profile, &info) {
            Self::require_in(
                info,
                CAP_AGENT_FINDINGS_PROFILE,
                &format!("this run cannot be held to the `{profile}` findings profile"),
                &self.base_url,
            )?;
        }
        if let (false, Some(info)) = (req.engagement_profiles.is_empty(), &info) {
            Self::require_in(
                info,
                CAP_AGENT_ENGAGEMENT_PROFILE,
                &format!(
                    "this run cannot run under the `{}` engagement profile(s)",
                    req.engagement_profiles.join(",")
                ),
                &self.base_url,
            )?;
        }
        if let (true, Some(info)) = (has_skills, &info) {
            Self::require_in(
                info,
                CAP_AGENT_SKILLS,
                "this run's workflow-step skills cannot be delivered",
                &self.base_url,
            )?;
        }
        if let (true, Some(info)) = (req.skill_bundle.is_some(), &info) {
            Self::require_in(
                info,
                CAP_AGENT_SKILL_BUNDLE,
                "this run's shipped skills cannot be delivered",
                &self.base_url,
            )?;
        }
        if let (Some((feature, what)), Some(info)) = (messaging_capability(&req.messaging), &info) {
            Self::require_in(info, feature, what, &self.base_url)?;
        }
        let (features, version): (&[String], Option<&str>) = match &info {
            Some(PeerInfo::Info(body)) => (&body.features, body.version.as_deref()),
            _ => (&[], None),
        };
        let mut body = serde_json::json!({
            "prompt": req.prompt,
            "mode": req.mode,
            "target": req.target,
            "working_dir": req.working_dir,
            "findings_profile": req.findings_profile,
        });
        // Only when set: a body without an engagement stays what it was.
        if !req.engagement_profiles.is_empty() {
            body["engagement_profiles"] = serde_json::json!(req.engagement_profiles);
        }
        if !req.skills.is_empty() {
            body["skills"] = serde_json::json!(req.skills);
        }
        if !req.preload_skills.is_empty() {
            body["preload_skills"] = serde_json::json!(req.preload_skills);
        }
        if let Some(m) = &req.skill_bundle {
            body["skill_bundle"] = serde_json::to_value(m)
                .map_err(|e| HostConnectorError::Invalid(format!("skill bundle: {e}")))?;
        }
        if !req.messaging.is_default() {
            body["messaging"] = serde_json::to_value(&req.messaging)
                .map_err(|e| HostConnectorError::Invalid(format!("messaging: {e}")))?;
        }
        // A remote that doesn't advertise `run.supplied_run_id` mints its own
        // id, and the one it returns is the one used either way.
        if let Some(id) = &req.run_id {
            if features.iter().any(|f| f == CAP_RUN_SUPPLIED_RUN_ID) {
                body["run_id"] = serde_json::json!(id);
            }
        }
        if let Some(codename) = spec_peer_codename(&run, features, &self.base_url, version) {
            body["codename"] = serde_json::json!(codename);
        }
        let resp = self
            .send(
                self.client
                    .post(self.url(&format!("/api/agents/{}/run", req.agent)))
                    .json(&body),
            )
            .await?;
        extract_string_field(resp.json().await, "run_id")
    }

    async fn ship_skills(&self, bundle: &SkillBundle) -> Result<ShipOutcome, HostConnectorError> {
        let who = format!("remote host {}", self.base_url);
        ship_through(&self.ship_memo, bundle, &who, |pending| {
            self.ship_pending(pending)
        })
        .await
    }

    async fn start_session(&self, req: SessionStartRequest) -> Result<String, HostConnectorError> {
        if !req.skills.is_empty() || !req.preload_skills.is_empty() {
            self.require_feature(
                CAP_SESSION_SKILLS,
                "this session's skills cannot be delivered",
            )
            .await?;
        }
        let mut body = serde_json::json!({
            "prompt": req.prompt,
            "mode": req.mode,
            "target": req.target,
            "working_dir": req.working_dir,
        });
        // Only when set: a start without skills stays what it was.
        if !req.skills.is_empty() {
            body["skills"] = serde_json::json!(req.skills);
        }
        if !req.preload_skills.is_empty() {
            body["preload_skills"] = serde_json::json!(req.preload_skills);
        }
        let resp = self
            .send(
                self.client
                    .post(self.url(&format!("/api/agents/{}/session", req.agent)))
                    .json(&body),
            )
            .await?;
        extract_string_field(resp.json().await, "session_id")
    }

    async fn send_session_turn(
        &self,
        req: SendMessageRequest,
    ) -> Result<String, HostConnectorError> {
        let mut body = serde_json::json!({ "prompt": req.prompt });
        if !req.skills.is_empty() {
            self.require_feature(CAP_SESSION_SKILLS, "this turn's skills cannot be invoked")
                .await?;
            body["skills"] = serde_json::json!(req.skills);
        }
        let resp = self
            .send(
                self.client
                    .post(self.url(&format!("/api/sessions/{}/send", req.session_id)))
                    .json(&body),
            )
            .await?;
        extract_string_field(resp.json().await, "run_id")
    }

    async fn list_runs(
        &self,
        params: RunListQuery,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        // `All` → `/api/runs`; `Workflow` → `/api/runs/workflows`.
        // See connector.rs doc comments for the mapping rationale.
        let path = match params.kind {
            RunKind::All => "/api/runs",
            RunKind::Workflow => "/api/runs/workflows",
        };

        let mut req = self.client.get(self.url(path)).query(&[
            ("offset", params.offset.to_string()),
            ("limit", params.limit.to_string()),
            // Scope the remote CP to its own local runs so we don't get
            // recursive fan-out in multi-hop topologies (remote CPs are
            // host-aware and would otherwise fan out across *their* hosts).
            ("host", "local".to_string()),
        ]);

        if let Some(lc) = &params.lifecycle {
            req = req.query(&[("lifecycle", lc.as_str())]);
        }

        let resp = self.send(req).await?;
        resp.json()
            .await
            .map_err(|e| HostConnectorError::Remote(0, e.to_string()))
    }

    async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        let resp = self
            .send(self.client.get(self.url(&format!("/api/runs/{run_id}"))))
            .await?;
        resp.json()
            .await
            .map_err(|e| HostConnectorError::Remote(0, e.to_string()))
    }

    async fn approve_run(&self, run_id: &str, mode: &str) -> Result<(), HostConnectorError> {
        let body = serde_json::json!({
            "mode": if mode.is_empty() { None::<&str> } else { Some(mode) },
        });
        self.send(
            self.client
                .post(self.url(&format!("/api/runs/{run_id}/approve")))
                .json(&body),
        )
        .await
        .map(|_| ())
    }

    async fn reject_run(
        &self,
        run_id: &str,
        reason: Option<&str>,
    ) -> Result<(), HostConnectorError> {
        let body = serde_json::json!({ "reason": reason });
        self.send(
            self.client
                .post(self.url(&format!("/api/runs/{run_id}/reject")))
                .json(&body),
        )
        .await
        .map(|_| ())
    }

    async fn cancel_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.send(
            self.client
                .post(self.url(&format!("/api/runs/{run_id}/cancel")))
                .json(&serde_json::json!({})),
        )
        .await
        .map(|_| ())
    }

    /// POST to the remote CP's `POST /api/runs/:id/pause` — the remote,
    /// running this same feature, cooperatively pauses the run on its own
    /// in-process executor (or its own host-routing, for a further hop).
    async fn pause_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.send(
            self.client
                .post(self.url(&format!("/api/runs/{run_id}/pause")))
                .json(&serde_json::json!({})),
        )
        .await
        .map(|_| ())
    }

    /// POST to the remote CP's `POST /api/runs/:id/resume`. Launcher-gated
    /// on the remote (a read-only remote deploy surfaces a `Remote(501, _)`
    /// error, mapped through unchanged — never a silent no-op).
    async fn resume_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.send(
            self.client
                .post(self.url(&format!("/api/runs/{run_id}/resume")))
                .json(&serde_json::json!({})),
        )
        .await
        .map(|_| ())
    }

    /// POST to the remote CP's `POST /api/runs/:id/archive`.
    async fn archive_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.send(
            self.client
                .post(self.url(&format!("/api/runs/{run_id}/archive")))
                .json(&serde_json::json!({})),
        )
        .await
        .map(|_| ())
    }

    /// POST to the remote CP's `POST /api/runs/:id/restore`.
    async fn restore_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.send(
            self.client
                .post(self.url(&format!("/api/runs/{run_id}/restore")))
                .json(&serde_json::json!({})),
        )
        .await
        .map(|_| ())
    }

    /// DELETE to the remote CP's `DELETE /api/runs/:id`.
    async fn delete_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.send(self.client.delete(self.url(&format!("/api/runs/{run_id}"))))
            .await
            .map(|_| ())
    }

    async fn stream_run_events(&self, run_id: &str) -> Result<EventByteStream, HostConnectorError> {
        self.stream_run_events_after(run_id, 0).await
    }

    /// Forwards the resume point as the remote's own `Last-Event-ID`: the
    /// remote CP numbers the run's events the same way, so the ids line up.
    async fn stream_run_events_after(
        &self,
        run_id: &str,
        after: u64,
    ) -> Result<EventByteStream, HostConnectorError> {
        // A per-request timeout overrides the client's 30 s total one, which
        // would end the stream mid-run; the head is still bounded.
        let mut req = self
            .client
            .get(self.url("/api/events/stream"))
            .query(&[("run", run_id)])
            .header("Accept", "text/event-stream")
            .timeout(EVENT_STREAM_TIMEOUT);
        if after > 0 {
            req = req.header("Last-Event-ID", after.to_string());
        }

        let resp = tokio::time::timeout(EVENT_STREAM_HEAD_TIMEOUT, self.send(req))
            .await
            .map_err(|_| {
                HostConnectorError::Unreachable(format!(
                    "no answer from {} for {} s",
                    self.base_url,
                    EVENT_STREAM_HEAD_TIMEOUT.as_secs()
                ))
            })??;

        let stream = resp
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));

        Ok(idle_bounded(stream, self.event_idle, self.base_url.clone()))
    }

    async fn get_transcript(&self, path: &str) -> Result<serde_json::Value, HostConnectorError> {
        let resp = self
            .send(
                self.client
                    .get(self.url("/api/transcript"))
                    .query(&[("path", path)]),
            )
            .await?;
        resp.json()
            .await
            .map_err(|e| HostConnectorError::Remote(0, e.to_string()))
    }

    /// Proxy session detail verbatim: an HTTP remote's `/api/sessions/:id`
    /// already answers in API shape, `usage` included (priced by that
    /// remote's own config, which we do not second-guess).
    async fn get_session(&self, id: &str) -> Result<serde_json::Value, HostConnectorError> {
        self.proxy_get_json(&format!("/api/sessions/{id}")).await
    }

    /// Proxy the dedicated runs endpoint — the API session DTO carries no
    /// `runs` field, so this cannot be read out of `get_session`'s body.
    async fn session_runs(&self, id: &str) -> Result<serde_json::Value, HostConnectorError> {
        self.proxy_get_json(&format!("/api/sessions/{id}/runs"))
            .await
    }

    async fn session_usage_timeline(
        &self,
        id: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        self.proxy_get_json(&format!("/api/sessions/{id}/usage-timeline"))
            .await
    }

    /// GET the remote CP's own `runs/<id>/netflow.jsonl` and land it here
    /// (spec 2026-10-14 §N2/§N3). The remote serves the file its `rupu run`
    /// wrote, so a read of a terminal run is complete and a snapshot is
    /// partial. A peer without [`CAP_RUN_NETFLOW_STREAM`] (an older rupu
    /// answers the unknown path with its SPA) or a failed fetch is recorded
    /// `unavailable` with the reason: missing netflow must never read as
    /// "no flows". Only an invalid id or a landing I/O error is an `Err`.
    async fn land_run_netflow(
        &self,
        run_id: &str,
        terminal: bool,
        runs_root: &std::path::Path,
    ) -> Result<NetflowLanding, HostConnectorError> {
        if !crate::host::connector::valid_run_id(run_id) {
            return Err(HostConnectorError::Invalid(format!(
                "invalid run id {run_id:?}"
            )));
        }
        let fetched: Result<Vec<u8>, String> =
            match self
                .require_feature(
                    CAP_RUN_NETFLOW_STREAM,
                    "this unit's netflow cannot be collected",
                )
                .await
            {
                Err(e) => Err(e.to_string()),
                Ok(()) => {
                    let path = format!("/api/runs/{run_id}/netflow/stream");
                    match self.send(self.client.get(self.url(&path))).await {
                        Err(e) => Err(e.to_string()),
                        Ok(resp) => resp.bytes().await.map(|b| b.to_vec()).map_err(|e| {
                            HostConnectorError::Unreachable(e.to_string()).to_string()
                        }),
                    }
                }
            };
        let (runs_root, run_id, host) = (
            runs_root.to_path_buf(),
            run_id.to_string(),
            self.host_id.clone().unwrap_or_else(|| self.base_url.clone()),
        );
        let rec = tokio::task::spawn_blocking(move || match fetched {
            Ok(bytes) => rupu_netflow::land::land_and_record(
                &runs_root,
                &run_id,
                &host,
                Some(&bytes),
                terminal,
                (!terminal).then(|| "read before the unit finished".to_string()),
            ),
            Err(why) => rupu_netflow::land::land_and_record(
                &runs_root,
                &run_id,
                &host,
                None,
                false,
                Some(why),
            ),
        })
        .await
        .map_err(|e| HostConnectorError::Internal(e.to_string()))?
        .map_err(|e| HostConnectorError::Internal(e.to_string()))?;
        Ok(NetflowLanding::Landed(rec.status))
    }
    async fn unit_coverage(
        &self,
        run_id: &str,
    ) -> Result<crate::host::connector::CoverageRead, HostConnectorError> {
        if !crate::host::connector::valid_run_id(run_id) {
            return Err(HostConnectorError::Invalid(format!(
                "{run_id:?} is not a valid run id"
            )));
        }
        // An older remote answers an unknown /api path with the SPA and 200,
        // so the feature — not the status — says whether this is a stream.
        self.require_feature(
            crate::node::protocol::CAP_RUN_COVERAGE_STREAM,
            "this unit's coverage cannot be collected",
        )
        .await?;
        let resp = self
            .send(
                self.client
                    .get(self.url(&format!("/api/runs/{run_id}/coverage"))),
            )
            .await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
        Ok(crate::host::connector::CoverageRead {
            bytes: bytes.to_vec(),
            complete: true,
        })
    }
    async fn list_messages(
        &self,
        id: &str,
        q: ViewQuery,
    ) -> Result<MessagesView, HostConnectorError> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(after) = q.after {
            query.push(("after", after));
        }
        if q.limit != usize::MAX {
            query.push(("limit", q.limit.to_string()));
        }
        if let Some(kinds) = q.kinds {
            let kinds: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
            query.push(("kind", kinds.join(",")));
        }
        let resp = self
            .messages_request(id, "this run's messages cannot be read", |url| {
                self.client.get(url).query(&query)
            })
            .await?;
        json_body(resp).await
    }

    /// The remote CP records its own viewer, if it has one; `msg.viewer`
    /// has no field to travel in.
    async fn send_operator_message(
        &self,
        id: &str,
        msg: OperatorMessageRequest,
    ) -> Result<OperatorSendReply, HostConnectorError> {
        let body = serde_json::json!({ "to": msg.to, "body": msg.body, "urgent": msg.urgent });
        let resp = self
            .messages_request(id, "this run cannot be messaged", |url| {
                self.client.post(url).json(&body)
            })
            .await?;
        json_body(resp).await
    }

    async fn pull_finding_artifact(
        &self,
        sha256: &str,
        dest: &std::path::Path,
        max_bytes: u64,
    ) -> Result<(), HostConnectorError> {
        use tokio::io::AsyncWriteExt;
        crate::host::connector::validate_sha256(sha256)?;
        // An older remote answers an unknown /api path with the SPA and 200,
        // so the feature — not the status — says whether this is a blob.
        self.require_feature(
            crate::node::protocol::CAP_FINDINGS_ARTIFACT_BLOB,
            "this finding artifact cannot be pulled",
        )
        .await?;
        // The client's 30 s total timeout would cut a large blob short, and a
        // per-request timeout overrides it: an artifact pull gets its own
        // whole-transfer ceiling. Within it, the response head and each body
        // chunk must arrive within `artifact_idle` of the last, so a
        // half-open connection fails the pull instead of pinning it.
        let idle = self.artifact_idle;
        let stalled = || {
            HostConnectorError::Unreachable(format!(
                "no data from {} for {} s",
                self.base_url,
                idle.as_secs()
            ))
        };
        let sent = tokio::time::timeout(
            idle,
            self.send(
                self.client
                    .get(self.url(&format!("/api/findings/artifacts/{sha256}")))
                    .timeout(ARTIFACT_PULL_TIMEOUT),
            ),
        )
        .await
        .map_err(|_| stalled())?;
        let resp = match sent {
            Ok(resp) => resp,
            // `send`'s NotFound carries the bare request URL, which tells a
            // viewer nothing they can act on.
            Err(HostConnectorError::NotFound(_)) => {
                return Err(HostConnectorError::NotFound(format!(
                    "artifact {sha256} is not in host {}'s store",
                    self.base_url
                )))
            }
            Err(e) => return Err(e),
        };
        let too_big = || {
            HostConnectorError::Invalid(format!(
                "artifact {sha256} exceeds its recorded {max_bytes} bytes"
            ))
        };
        // A declared length over the cap is refused before anything is
        // written; a chunked body is held to the cap as it arrives.
        if resp.content_length().is_some_and(|n| n > max_bytes) {
            return Err(too_big());
        }
        let write_err =
            |e: std::io::Error| HostConnectorError::Invalid(format!("local write failed: {e}"));
        // On any failure past this point `dest` may be partially written; the
        // caller owns its cleanup and only a returned `Ok` means it is whole.
        let mut file = tokio::fs::File::create(dest).await.map_err(write_err)?;
        let mut stream = resp.bytes_stream();
        let mut total: u64 = 0;
        while let Some(chunk) = tokio::time::timeout(idle, stream.next())
            .await
            .map_err(|_| stalled())?
        {
            let chunk = chunk.map_err(|e| {
                HostConnectorError::Unreachable(format!(
                    "reading artifact {sha256} from the remote failed: {e}"
                ))
            })?;
            total += chunk.len() as u64;
            if total > max_bytes {
                return Err(too_big());
            }
            file.write_all(&chunk).await.map_err(write_err)?;
        }
        file.flush().await.map_err(write_err)?;
        Ok(())
    }

    /// From the `features` the remote's `/api/host/info` last listed
    /// (`run.supplied_run_id`); `false` before it was ever read. Whatever this
    /// says, `launch_agent` returns the id the remote reports it used.
    fn honours_supplied_run_id(&self) -> bool {
        self.features_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|f| f.iter().any(|f| f == CAP_RUN_SUPPLIED_RUN_ID))
    }

    async fn proxy_get_json(
        &self,
        path_and_query: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        let resp = self
            .send(
                self.client
                    .get(format!("{}{}", self.base_url, path_and_query)),
            )
            .await?;
        // Read, then parse: reqwest's `json()` reports a body that failed to
        // arrive and a body that arrived but isn't JSON alike (`is_decode`),
        // and callers must tell a transport failure (5xx) from a remote that
        // does not serve this path as JSON at all (an older CP's SPA fallback).
        let body = resp
            .bytes()
            .await
            .map_err(|e| HostConnectorError::Remote(0, e.to_string()))?;
        serde_json::from_slice(&body).map_err(|e| HostConnectorError::NotJson(e.to_string()))
    }

    async fn list_sessions(
        &self,
        scope: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        // The remote's own session list defaults to 20 rows and clamps any
        // `limit` at its `MAX_LIMIT` (200); ask for everything it will give,
        // as `list_agent_runs` / `list_autoflow_runs` do.
        let mut path = "/api/sessions?host=local&limit=10000".to_string();
        if let Some(sc) = scope {
            path.push_str("&scope=");
            path.push_str(sc);
        }
        let v = self.proxy_get_json(&path).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// POST to the remote CP's `POST /api/sessions/:id/archive`.
    async fn archive_session(&self, id: &str) -> Result<(), HostConnectorError> {
        self.send(
            self.client
                .post(self.url(&format!("/api/sessions/{id}/archive")))
                .json(&serde_json::json!({})),
        )
        .await
        .map(|_| ())
    }

    /// POST to the remote CP's `POST /api/sessions/:id/restore`.
    async fn restore_session(&self, id: &str) -> Result<(), HostConnectorError> {
        self.send(
            self.client
                .post(self.url(&format!("/api/sessions/{id}/restore")))
                .json(&serde_json::json!({})),
        )
        .await
        .map(|_| ())
    }

    /// DELETE to the remote CP's `DELETE /api/sessions/:id`.
    async fn delete_session(&self, id: &str) -> Result<(), HostConnectorError> {
        self.send(self.client.delete(self.url(&format!("/api/sessions/{id}"))))
            .await
            .map(|_| ())
    }

    /// POST to the remote CP's `POST /api/transcripts/:id/archive[?ignore_liveness=true]`.
    async fn archive_transcript(
        &self,
        id: &str,
        ignore_liveness: bool,
    ) -> Result<(), HostConnectorError> {
        let qs = if ignore_liveness {
            "?ignore_liveness=true"
        } else {
            ""
        };
        self.send(
            self.client
                .post(self.url(&format!("/api/transcripts/{id}/archive{qs}")))
                .json(&serde_json::json!({})),
        )
        .await
        .map(|_| ())
    }

    /// DELETE to the remote CP's `DELETE /api/transcripts/:id[?ignore_liveness=true]`.
    async fn delete_transcript(
        &self,
        id: &str,
        ignore_liveness: bool,
    ) -> Result<(), HostConnectorError> {
        let qs = if ignore_liveness {
            "?ignore_liveness=true"
        } else {
            ""
        };
        self.send(
            self.client
                .delete(self.url(&format!("/api/transcripts/{id}{qs}"))),
        )
        .await
        .map(|_| ())
    }

    async fn list_agent_runs(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let v = self
            .proxy_get_json("/api/runs/agents?host=local&limit=10000")
            .await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    async fn list_autoflow_runs(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let v = self
            .proxy_get_json("/api/runs/autoflows?host=local&limit=10000")
            .await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    async fn list_autoflow_events(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let v = self
            .proxy_get_json("/api/runs/autoflows/events?host=local&limit=10000")
            .await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// GET the remote CP's `/api/dashboard?host=local&range=<wire form>` and
    /// parse the response as a [`DashboardSummary`](crate::host::dashboard_summary::DashboardSummary).
    ///
    /// `host=local` scopes the remote CP to ITS OWN data — without it the
    /// remote would fan out to its own remotes and a host registered on both
    /// sides would be double-counted.
    ///
    /// The remote's `hosts[]` array (see `api::dashboard::DashboardResponse`)
    /// is the ONLY place a remote CP records that its own local connector
    /// failed to report — when that happens it still answers 200 with an
    /// all-zero `DashboardSummary` and `captured_at: now()` (the honest
    /// no-host-reported fallback `get_dashboard` falls back to). Parsing the
    /// flattened body alone would accept that as a genuine "ok, live, 0 runs"
    /// summary, indistinguishable from an idle host. So `hosts[]` is checked
    /// FIRST: if present and none of its entries report `state == "ok"`, this
    /// returns an error carrying the remote's own reason instead of the
    /// zeroed data. `hosts[]` absent (an older/bare body, as in some test
    /// fixtures) skips the check and parses the summary as before — the
    /// flatten contract stays intact either way.
    async fn dashboard_summary(
        &self,
        range: crate::host::dashboard_summary::DashboardRange,
    ) -> Result<crate::host::dashboard_summary::DashboardSummary, HostConnectorError> {
        let path = format!("/api/dashboard?host=local&range={}", range.as_str());
        let v = self.proxy_get_json(&path).await?;

        if let Some(hosts) = v.get("hosts").and_then(|h| h.as_array()) {
            let any_ok = hosts
                .iter()
                .any(|h| h.get("state").and_then(|s| s.as_str()) == Some("ok"));
            if !any_ok {
                let reason = hosts
                    .iter()
                    .find_map(|h| h.get("reason").and_then(|r| r.as_str()))
                    .unwrap_or("remote host did not report (no reason given)");
                return Err(HostConnectorError::Unreachable(format!(
                    "remote CP's local host did not report dashboard data: {reason}"
                )));
            }
        }

        // Deliberately parse `v` itself (not a `hosts`-stripped clone): the
        // `#[serde(flatten)]` on `DashboardResponse::summary` is what makes
        // this work by construction — serde ignores the extra `hosts` /
        // `findings_partial` keys rather than a mapper that can drift.
        serde_json::from_value(v)
            .map_err(|e| HostConnectorError::Invalid(format!("bad dashboard summary: {e}")))
    }

    /// POST the wire-encoded payload to the remote CP's `/api/workspace/stage`;
    /// the remote stages it under its own cache and returns `{working_dir}`.
    async fn stage_workspace(&self, payload: Vec<u8>) -> Result<String, HostConnectorError> {
        if payload.len() > MAX_WORKSPACE_BYTES {
            return Err(HostConnectorError::Invalid(format!(
                "workspace payload {} bytes exceeds limit {MAX_WORKSPACE_BYTES}",
                payload.len()
            )));
        }
        let resp = self
            .send(
                self.client
                    .post(self.url("/api/workspace/stage"))
                    .header("Content-Type", "application/octet-stream")
                    .body(payload),
            )
            .await?;
        extract_string_field(resp.json().await, "working_dir")
    }

    /// GET the wire-encoded delta from `/api/workspace/delta?dir=<working_dir>`.
    async fn collect_workspace_delta(
        &self,
        working_dir: &str,
    ) -> Result<Vec<u8>, HostConnectorError> {
        let resp = self
            .send(
                self.client
                    .get(self.url("/api/workspace/delta"))
                    .query(&[("dir", working_dir)]),
            )
            .await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| HostConnectorError::Remote(0, e.to_string()))?;
        // Cap the download symmetrically with the upload limit so a compromised
        // or misbehaving host cannot push an unbounded delta payload.
        if bytes.len() > MAX_WORKSPACE_BYTES {
            return Err(HostConnectorError::Invalid(format!(
                "collect-delta response {} bytes exceeds limit {MAX_WORKSPACE_BYTES}",
                bytes.len()
            )));
        }
        Ok(bytes.to_vec())
    }

    /// DELETE the staged scratch dir via `/api/workspace/discard?dir=<working_dir>`.
    ///
    /// Best-effort: called by a coordinator when it gave up on a unit between
    /// staging and collecting (launch failure, poll timeout) so the remote
    /// scratch is not left to leak until the next best-effort sweep.
    async fn discard_workspace(&self, working_dir: &str) -> Result<(), HostConnectorError> {
        self.send(
            self.client
                .delete(self.url("/api/workspace/discard"))
                .query(&[("dir", working_dir)]),
        )
        .await
        .map(|_| ())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Decode a JSON body (already `Result<Value, reqwest::Error>`) and extract a
/// named `String` field, mapping both decode and missing-field failures to
/// `HostConnectorError::Invalid`.
/// A 2xx reply's body as `T`: a body that failed to arrive is a transport
/// failure, one that arrived but is not `T` is `NotJson` (an older peer's web
/// UI answering a route it lacks).
async fn json_body<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, HostConnectorError> {
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| HostConnectorError::Remote(0, e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| HostConnectorError::NotJson(e.to_string()))
}

fn extract_string_field(
    result: Result<serde_json::Value, reqwest::Error>,
    field: &str,
) -> Result<String, HostConnectorError> {
    let val = result.map_err(|e| HostConnectorError::Remote(0, e.to_string()))?;
    val.get(field)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| HostConnectorError::Invalid(format!("missing `{field}` in response")))
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// `new_with_timeout` must bound a request even when the remote accepts
    /// the TCP connection but never responds — this is the fix for the
    /// host-probe fallback (`api::run_resolve::probe_hosts`) stalling on an
    /// unreachable-but-listening host. A bare `HttpHostConnector::new` (used
    /// for the normal, explicit `?host=` path) has no such bound and would
    /// hang here.
    #[tokio::test]
    async fn new_with_timeout_bounds_a_hanging_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Accept the connection and hold it open without ever writing a
        // response, so only the connector's own timeout (not a refusal or
        // EOF) can end the call.
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                tokio::time::sleep(Duration::from_secs(30)).await;
                drop(stream);
            }
        });

        let conn = HttpHostConnector::new_with_timeout(
            format!("http://{addr}"),
            None,
            Duration::from_millis(300),
        );

        let start = std::time::Instant::now();
        let result = conn.proxy_get_json("/api/runs/does-not-matter").await;
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "expected the bounded client to time out on a non-responding host, got {result:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "bounded probe took {elapsed:?}; expected well under the OS default connect timeout"
        );
    }

    /// A one-connection HTTP server that reads the request head, writes
    /// `response` verbatim, and closes the socket.
    async fn one_shot_server(response: Vec<u8>) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            }
        });
        addr
    }

    /// Without a `limit` the remote answered its default 20 sessions, so a
    /// host's list was silently cut at 20 on every path that reads it.
    #[tokio::test]
    async fn list_sessions_asks_the_remote_for_its_whole_list() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (head_tx, head_rx) = tokio::sync::oneshot::channel::<String>();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = head_tx.send(String::from_utf8_lossy(&head).into_owned());
                let body = "[]";
                let _ = stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
                let _ = stream.shutdown().await;
            }
        });
        let conn = HttpHostConnector::new(format!("http://{addr}"), None);
        conn.list_sessions(Some("archived")).await.unwrap();
        let head = head_rx.await.unwrap();
        let request_line = head.lines().next().unwrap();
        assert_eq!(
            request_line,
            "GET /api/sessions?host=local&limit=10000&scope=archived HTTP/1.1"
        );
    }

    /// A 2xx whose complete body is not JSON (an older CP's SPA fallback) is
    /// `NotJson` — what `/api/runs/:id/usage` degrades to a 404 on.
    #[tokio::test]
    async fn proxy_get_json_non_json_reply_is_not_json() {
        let body = "<!doctype html><html></html>";
        let addr = one_shot_server(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
        )
        .await;
        let conn = HttpHostConnector::new(format!("http://{addr}"), None);
        let err = conn.proxy_get_json("/api/runs/r/usage").await.unwrap_err();
        assert!(matches!(err, HostConnectorError::NotJson(_)), "{err:?}");
    }

    /// A body cut off mid-response is a transport failure, NOT `NotJson`:
    /// callers keep reporting it as a 5xx.
    #[tokio::test]
    async fn proxy_get_json_truncated_body_is_a_transport_failure() {
        let addr = one_shot_server(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"summ"
                .to_vec(),
        )
        .await;
        let conn = HttpHostConnector::new(format!("http://{addr}"), None);
        let err = conn.proxy_get_json("/api/runs/r/usage").await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Remote(0, _)), "{err:?}");
    }

    /// What [`artifact_remote`]'s blob route sends.
    enum BlobPlan {
        /// Nothing at all — not even the response head.
        Silent,
        /// A chunked body: each `(delay_ms, bytes)` chunk after its delay.
        /// With `stall`, the remote then goes silent with the connection
        /// open instead of ending the body.
        Chunks {
            chunks: Vec<(u64, &'static [u8])>,
            stall: bool,
        },
    }

    /// A remote that advertises `findings.artifact_blob` on `/api/host/info`
    /// and answers any other request (the blob GET) per `plan`. One request
    /// per connection (`connection: close`), so the client never reuses a
    /// socket this server is closing.
    async fn artifact_remote(plan: BlobPlan) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let plan = Arc::new(plan);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let plan = plan.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    if head.starts_with(b"GET /api/host/info ") {
                        let body = serde_json::json!({
                            "version": "0.81.0",
                            "features": [crate::node::protocol::CAP_FINDINGS_ARTIFACT_BLOB],
                        })
                        .to_string();
                        let _ = stream
                            .write_all(
                                format!(
                                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                                    body.len()
                                )
                                .as_bytes(),
                            )
                            .await;
                        let _ = stream.shutdown().await;
                        return;
                    }
                    let BlobPlan::Chunks { chunks, stall } = &*plan else {
                        tokio::time::sleep(Duration::from_secs(20)).await;
                        return;
                    };
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\n\
                              transfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                        )
                        .await;
                    for (delay_ms, data) in chunks {
                        tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
                        let mut frame = format!("{:x}\r\n", data.len()).into_bytes();
                        frame.extend_from_slice(data);
                        frame.extend_from_slice(b"\r\n");
                        if stream.write_all(&frame).await.is_err() {
                            return;
                        }
                    }
                    if *stall {
                        tokio::time::sleep(Duration::from_secs(20)).await;
                        return;
                    }
                    let _ = stream.write_all(b"0\r\n\r\n").await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        addr
    }

    /// A connector on `addr` whose artifact idle bound is 1 s.
    fn quick_idle_connector(addr: std::net::SocketAddr) -> HttpHostConnector {
        HttpHostConnector {
            artifact_idle: Duration::from_secs(1),
            ..HttpHostConnector::new(format!("http://{addr}"), None)
        }
    }

    /// Pull `"ab" * 32` into a fresh temp file; fails the test if the pull
    /// takes longer than 10 s (so a missing idle bound fails fast instead of
    /// hanging on the 30-minute ceiling).
    async fn bounded_pull(
        conn: &HttpHostConnector,
    ) -> (Result<(), HostConnectorError>, Vec<u8>, Duration) {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("pulled");
        let start = std::time::Instant::now();
        let res = tokio::time::timeout(
            Duration::from_secs(10),
            conn.pull_finding_artifact(&"ab".repeat(32), &dest, 100),
        )
        .await
        .expect("the pull must end on its idle bound, not run on to its 30-minute ceiling");
        let elapsed = start.elapsed();
        (res, std::fs::read(&dest).unwrap_or_default(), elapsed)
    }

    /// A half-open remote that sends one chunk and then nothing must not pin
    /// the pull: it fails once the idle bound passes with no data.
    #[tokio::test]
    async fn an_artifact_pull_that_stalls_mid_body_fails_on_the_idle_bound() {
        let addr = artifact_remote(BlobPlan::Chunks {
            chunks: vec![(0, b"abc")],
            stall: true,
        })
        .await;
        let (res, _, elapsed) = bounded_pull(&quick_idle_connector(addr)).await;
        let err = res.unwrap_err();
        assert!(
            matches!(&err, HostConnectorError::Unreachable(m) if m.contains("no data from")),
            "{err:?}"
        );
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }

    /// A remote that accepts the blob request and never answers it is bounded
    /// the same way (the response head counts as data).
    #[tokio::test]
    async fn an_artifact_pull_whose_remote_never_answers_fails_on_the_idle_bound() {
        let addr = artifact_remote(BlobPlan::Silent).await;
        let (res, _, elapsed) = bounded_pull(&quick_idle_connector(addr)).await;
        let err = res.unwrap_err();
        assert!(
            matches!(&err, HostConnectorError::Unreachable(m) if m.contains("no data from")),
            "{err:?}"
        );
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }

    /// A run event stream that goes silent (a dead or half-open peer, which
    /// would otherwise send a keep-alive every 15 s) ends with an error once
    /// the idle bound passes, rather than hang for its 24 h whole-life bound.
    #[tokio::test]
    async fn an_event_stream_that_goes_silent_ends_on_the_idle_bound() {
        let addr = artifact_remote(BlobPlan::Chunks {
            chunks: vec![(0, b"ab")],
            stall: true,
        })
        .await;
        let conn = HttpHostConnector {
            event_idle: Duration::from_secs(1),
            ..HttpHostConnector::new(format!("http://{addr}"), None)
        };
        let t = std::time::Instant::now();
        let mut stream = conn.stream_run_events_after("run_x", 0).await.unwrap();
        let read = tokio::time::timeout(Duration::from_secs(10), async {
            let mut body = Vec::new();
            let mut err = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(b) => body.extend_from_slice(&b),
                    Err(e) => err = Some(e),
                }
            }
            (body, err)
        })
        .await
        .expect("the stream must end on its idle bound");
        assert_eq!(read.0, b"ab");
        let err = read.1.expect("an idle end is an error, not a clean end");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{err}");
        assert!(err.to_string().contains("no data from"), "{err}");
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    /// A run event stream is long-lived: its own per-request timeout
    /// overrides the client's total one, so a client built with a 1 s total
    /// timeout still reads a stream that trickles over ~2 s.
    #[tokio::test]
    async fn an_event_stream_outlives_the_clients_total_timeout() {
        let addr = artifact_remote(BlobPlan::Chunks {
            chunks: vec![(0, b"ab"), (700, b"cd"), (700, b"ef"), (700, b"gh")],
            stall: false,
        })
        .await;
        let conn = HttpHostConnector::new_with_timeout(
            format!("http://{addr}"),
            None,
            Duration::from_secs(1),
        );
        let t = std::time::Instant::now();
        let mut stream = conn.stream_run_events_after("run_x", 0).await.unwrap();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(body, b"abcdefgh");
        assert!(t.elapsed() > Duration::from_millis(1500), "{:?}", t.elapsed());
    }

    /// The idle bound is per gap, not per transfer, and the pull's own
    /// per-request timeout overrides the client's total one: a client built
    /// with a 1 s total timeout still pulls a body trickled over ~2 s.
    #[tokio::test]
    async fn an_artifact_pull_outlives_the_clients_total_timeout_while_data_flows() {
        let addr = artifact_remote(BlobPlan::Chunks {
            chunks: vec![(0, b"ab"), (700, b"cd"), (700, b"ef"), (700, b"gh")],
            stall: false,
        })
        .await;
        let conn = HttpHostConnector::new_with_timeout(
            format!("http://{addr}"),
            None,
            Duration::from_secs(1),
        );
        let (res, body, elapsed) = bounded_pull(&conn).await;
        res.unwrap();
        assert_eq!(body, b"abcdefgh");
        assert!(
            elapsed > Duration::from_millis(1500),
            "the trickle must outlast the client's 1 s total timeout: {elapsed:?}"
        );
    }
}
