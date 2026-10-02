//! `HttpHostConnector` — proxies every [`HostConnector`] call over HTTP to a
//! remote `rupu cp serve` instance.
//!
//! One private [`HttpHostConnector::send`] helper attaches the bearer token
//! and maps transport / status errors so every method stays DRY.

#![deny(clippy::all)]

use futures_util::StreamExt as _;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    agent_launcher::AgentLaunchRequest,
    host::connector::{
        EventByteStream, HostCapabilities, HostConnector, HostConnectorError, HostInfo, RunKind,
        RunListQuery, MAX_WORKSPACE_BYTES,
    },
    launcher::LaunchRequest,
    node::protocol::CAP_AGENT_FINDINGS_PROFILE,
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

// ── Struct ────────────────────────────────────────────────────────────────────

/// Remote-host connector: forwards every [`HostConnector`] call as an HTTP
/// request to a running `rupu cp serve`.
pub struct HttpHostConnector {
    client: reqwest_middleware::ClientWithMiddleware,
    base_url: String,
    token: Option<String>,
    /// [`ARTIFACT_PULL_IDLE_TIMEOUT`]; a field so tests can shorten it.
    artifact_idle: Duration,
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
        }
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

    /// Refuse unless the remote's `/api/host/info` lists `feature`. A remote
    /// predating a request field ignores it (its body structs don't deny
    /// unknown fields), so an unadvertised feature — including a remote with
    /// no `/api/host/info` at all — is a refusal, not a best-effort send.
    async fn require_feature(&self, feature: &str, what: &str) -> Result<(), HostConnectorError> {
        let resp = match self.send(self.client.get(self.url("/api/host/info"))).await {
            Ok(resp) => resp,
            Err(HostConnectorError::NotFound(_)) => {
                return Err(HostConnectorError::Unsupported(format!(
                    "{what}: remote host {} predates /api/host/info, so it cannot \
                     advertise support; upgrade rupu there",
                    self.base_url
                )))
            }
            Err(e) => return Err(e),
        };
        // Read, then parse (as `proxy_get_json` does): a body that failed to
        // ARRIVE is a transport failure, but a 200 whose body is not the info
        // JSON is a remote that has no such route and answered with its SPA —
        // the signature of a rupu older than `/api/host/info`. That cannot
        // advertise a feature, so it is the same refusal as a 404.
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| HostConnectorError::Remote(0, e.to_string()))?;
        let body = match parse_host_info(&bytes) {
            Ok(body) => body,
            Err(e) => {
                return Err(HostConnectorError::Unsupported(format!(
                    "{what}: remote host {} did not answer /api/host/info with host \
                     info ({e}; an older rupu serves its web UI there), so it cannot \
                     advertise support; upgrade rupu there",
                    self.base_url
                )))
            }
        };
        if body.features.iter().any(|f| f == feature) {
            return Ok(());
        }
        Err(HostConnectorError::Unsupported(format!(
            "{what}: remote host {} (rupu {}) does not support it; upgrade rupu there",
            self.base_url,
            body.version.as_deref().unwrap_or("unknown version"),
        )))
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
                    Ok(body) => HostInfo {
                        reachable: true,
                        version: body.version,
                        capabilities: body.capabilities,
                    },
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
        if let Some(profile) = req.findings_profile {
            self.require_feature(
                CAP_AGENT_FINDINGS_PROFILE,
                &format!("this run cannot be held to the `{profile}` findings profile"),
            )
            .await?;
        }
        let body = serde_json::json!({
            "prompt": req.prompt,
            "mode": req.mode,
            "target": req.target,
            "working_dir": req.working_dir,
            "findings_profile": req.findings_profile,
        });
        let resp = self
            .send(
                self.client
                    .post(self.url(&format!("/api/agents/{}/run", req.agent)))
                    .json(&body),
            )
            .await?;
        extract_string_field(resp.json().await, "run_id")
    }

    async fn start_session(&self, req: SessionStartRequest) -> Result<String, HostConnectorError> {
        let body = serde_json::json!({
            "prompt": req.prompt,
            "mode": req.mode,
            "target": req.target,
            "working_dir": req.working_dir,
        });
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
        let body = serde_json::json!({ "prompt": req.prompt });
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
        let req = self
            .client
            .get(self.url("/api/events/stream"))
            .query(&[("run", run_id)])
            .header("Accept", "text/event-stream");

        let resp = self.send(req).await?;

        let stream = resp
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));

        Ok(Box::pin(stream))
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

    /// GET the remote CP's own local file. Complete once the run is terminal:
    /// the remote serves the file its `rupu run` wrote, not a copy in transit.
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
        let resp = tokio::time::timeout(
            idle,
            self.send(
                self.client
                    .get(self.url(&format!("/api/findings/artifacts/{sha256}")))
                    .timeout(ARTIFACT_PULL_TIMEOUT),
            ),
        )
        .await
        .map_err(|_| stalled())??;
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
