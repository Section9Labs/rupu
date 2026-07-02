//! `HttpHostConnector` — proxies every [`HostConnector`] call over HTTP to a
//! remote `rupu cp serve` instance.
//!
//! One private [`HttpHostConnector::send`] helper attaches the bearer token
//! and maps transport / status errors so every method stays DRY.

#![deny(clippy::all)]

use futures_util::StreamExt as _;

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
        Self {
            client: reqwest::Client::new(),
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
