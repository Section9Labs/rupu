//! `HttpHostConnector` — proxies every [`HostConnector`] call over HTTP to a
//! remote `rupu cp serve` instance.
//!
//! One private [`HttpHostConnector::send`] helper attaches the bearer token
//! and maps transport / status errors so every method stays DRY.

#![deny(clippy::all)]

use futures_util::StreamExt as _;
use std::time::Duration;

use crate::{
    agent_launcher::AgentLaunchRequest,
    host::connector::{
        EventByteStream, HostCapabilities, HostConnector, HostConnectorError, HostInfo, RunKind,
        RunListQuery, MAX_WORKSPACE_BYTES,
    },
    launcher::LaunchRequest,
    session_sender::SendMessageRequest,
    session_starter::SessionStartRequest,
};

// ── Struct ────────────────────────────────────────────────────────────────────

/// Remote-host connector: forwards every [`HostConnector`] call as an HTTP
/// request to a running `rupu cp serve`.
pub struct HttpHostConnector {
    client: reqwest::Client,
    base_url: String,
    token: Option<String>,
}

/// Private response struct for deserializing the `/api/host/info` endpoint.
#[derive(serde::Deserialize)]
struct HostInfoBody {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    capabilities: HostCapabilities,
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
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            base_url,
            token,
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
    /// Falls back to an unbounded client if the `reqwest::ClientBuilder`
    /// itself fails to build (e.g. an invalid TLS config) — best-effort, not
    /// a hard requirement for probing to function.
    pub fn new_with_timeout(base_url: String, token: Option<String>, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(timeout)
            .timeout(timeout)
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url,
            token,
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
        req: reqwest::RequestBuilder,
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
                let body: HostInfoBody = resp
                    .json()
                    .await
                    .map_err(|e| HostConnectorError::Remote(0, e.to_string()))?;
                Ok(HostInfo {
                    reachable: true,
                    version: body.version,
                    capabilities: body.capabilities,
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
        let body = serde_json::json!({
            "prompt": req.prompt,
            "mode": req.mode,
            "target": req.target,
            "working_dir": req.working_dir,
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
        resp.json()
            .await
            .map_err(|e| HostConnectorError::Remote(0, e.to_string()))
    }

    async fn list_sessions(
        &self,
        scope: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let mut path = "/api/sessions?host=local".to_string();
        if let Some(sc) = scope {
            path.push_str("&scope=");
            path.push_str(sc);
        }
        let v = self.proxy_get_json(&path).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
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
}
