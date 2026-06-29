//! `GET /api/hosts` · `POST /api/hosts` · `DELETE /api/hosts/:id`
//!
//! Exposes the `HostRegistry` over HTTP. The list endpoint probes every remote
//! host concurrently and tolerates unreachable hosts: a failed `info()` call
//! produces `status: "offline"` rather than failing the whole list.

#![deny(clippy::all)]

use std::sync::Arc;

use crate::{
    error::{ApiError, ApiResult},
    host::connector::{HostCapabilities, RunKind, RunListQuery},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use futures_util::future::join_all;
use rupu_workspace::HostTransport;
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/hosts", get(list_hosts).post(add_host))
        .route("/api/hosts/node", post(enroll_node_handler))
        .route("/api/hosts/:id", delete(remove_host))
}

// ── View type ─────────────────────────────────────────────────────────────────

/// JSON view of one registered host, enriched with live health data.
#[derive(Debug, Serialize)]
pub struct HostView {
    pub id: String,
    pub name: String,
    /// `"local"` or `"http_cp"`.
    pub transport_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// `"online"`, `"offline"`, or `"stale"`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<HostCapabilities>,
    pub active_run_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<String>,
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Decompose a `HostTransport` into the serialised kind string + optional URL.
fn transport_fields(t: &HostTransport) -> (String, Option<String>) {
    match t {
        HostTransport::Local => ("local".to_string(), None),
        HostTransport::HttpCp { base_url } => {
            ("http_cp".to_string(), Some(base_url.clone()))
        }
        HostTransport::Tunnel { node_id } => ("tunnel".to_string(), Some(node_id.clone())),
        HostTransport::Ssh { host, port, .. } => {
            let addr = match port {
                Some(p) => format!("{host}:{p}"),
                None => host.clone(),
            };
            ("ssh".to_string(), Some(addr))
        }
        HostTransport::Bucket { url, .. } => ("bucket".to_string(), Some(url.clone())),
    }
}

/// Ask a connector for its active-run count. Returns 0 on any error (best-effort).
async fn active_run_count(
    conn: &Arc<dyn crate::host::connector::HostConnector>,
) -> usize {
    let q = RunListQuery {
        kind: RunKind::All,
        offset: 0,
        limit: 200,
        lifecycle: Some("active".to_string()),
    };
    conn.list_runs(q).await.map(|v| v.len()).unwrap_or(0)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /api/hosts` — list all known hosts with live health data.
///
/// The local host is always first and always `status: "online"`. Every other
/// host is probed concurrently via `connector.info()`; failure → `"offline"`.
async fn list_hosts(State(s): State<AppState>) -> ApiResult<Json<Vec<HostView>>> {
    let hosts = s.hosts.list_hosts();

    // Build one future per host; local gets a fast path, remotes call info().
    let futs: Vec<_> = hosts
        .into_iter()
        .map(|host| {
            let reg = Arc::clone(&s.hosts);
            async move {
                let (transport_kind, base_url) = transport_fields(&host.transport);

                // ── Local host ────────────────────────────────────────────────
                if host.id == "local" {
                    let count = match reg.resolve("local") {
                        Ok(conn) => active_run_count(&conn).await,
                        Err(_) => 0,
                    };
                    return HostView {
                        id: host.id,
                        name: host.name,
                        transport_kind,
                        base_url,
                        status: "online".to_string(),
                        version: Some(env!("CARGO_PKG_VERSION").to_string()),
                        capabilities: None,
                        active_run_count: count,
                        last_seen_at: host.last_seen_at,
                    };
                }

                // ── Remote host ───────────────────────────────────────────────
                let connector = match reg.resolve(&host.id) {
                    Ok(c) => c,
                    Err(_) => {
                        return HostView {
                            id: host.id,
                            name: host.name,
                            transport_kind,
                            base_url,
                            status: "offline".to_string(),
                            version: None,
                            capabilities: None,
                            active_run_count: 0,
                            last_seen_at: host.last_seen_at,
                        };
                    }
                };

                let info = match connector.info().await {
                    Ok(i) if i.reachable => i,
                    _ => {
                        return HostView {
                            id: host.id,
                            name: host.name,
                            transport_kind,
                            base_url,
                            status: "offline".to_string(),
                            version: None,
                            capabilities: None,
                            active_run_count: 0,
                            last_seen_at: host.last_seen_at,
                        };
                    }
                };

                // Online: derive active-run count from the connector.
                let count = active_run_count(&connector).await;

                HostView {
                    id: host.id,
                    name: host.name,
                    transport_kind,
                    base_url,
                    status: "online".to_string(),
                    version: info.version,
                    capabilities: Some(info.capabilities),
                    active_run_count: count,
                    last_seen_at: host.last_seen_at,
                }
            }
        })
        .collect();

    let views = join_all(futs).await;
    Ok(Json(views))
}

#[derive(Deserialize)]
struct AddHostBody {
    name: String,
    base_url: String,
    /// Bearer token for the remote CP (optional; stored in keychain when present).
    #[serde(default)]
    token: Option<String>,
}

/// `POST /api/hosts` — register a new remote host.
///
/// Requires the `cp serve` launcher adapter (501 if absent — read-only deploy).
/// Returns the newly created host as a [`HostView`] (status `"offline"` until
/// probed by a subsequent `GET /api/hosts`).
async fn add_host(
    State(s): State<AppState>,
    Json(body): Json<AddHostBody>,
) -> ApiResult<Json<HostView>> {
    // Read-only guard — mirrors the pattern in api::workflows::launch_run.
    s.launcher
        .as_ref()
        .ok_or_else(|| ApiError::not_available("managing hosts requires `rupu cp serve`"))?;

    let host = s
        .hosts
        .add_host(&body.name, &body.base_url, body.token.as_deref())
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let (transport_kind, base_url) = transport_fields(&host.transport);

    Ok(Json(HostView {
        id: host.id,
        name: host.name,
        transport_kind,
        base_url,
        // Newly added: not yet probed.
        status: "offline".to_string(),
        version: None,
        capabilities: None,
        active_run_count: 0,
        last_seen_at: host.last_seen_at,
    }))
}

// ── Enroll-node types + handler ───────────────────────────────────────────────

#[derive(Deserialize)]
struct EnrollNodeBody {
    name: String,
}

/// Response from `POST /api/hosts/node`.
///
/// `token` is the plaintext enrollment token — shown **once** here and never
/// persisted (the store holds only the SHA-256 hash).  Callers must surface
/// this to the operator and then discard it.  Do **not** log this response body.
#[derive(Serialize)]
pub struct EnrollNodeResponse {
    pub host: HostView,
    /// Full `rupu node` command the operator can paste on the target machine.
    pub command: String,
    /// One-time plaintext token.  Only present in this response.
    pub token: String,
}

/// `POST /api/hosts/node` — enroll a new tunnel node.
///
/// Mints a one-time enrollment token and a `Tunnel` host record.  Returns the
/// host view, the runnable command (with a placeholder for the CP's public WSS
/// URL — substitute the real address before running), and the plaintext token.
///
/// Requires the `cp serve` launcher adapter; returns 501 on a read-only deploy.
async fn enroll_node_handler(
    State(s): State<AppState>,
    Json(body): Json<EnrollNodeBody>,
) -> ApiResult<Json<EnrollNodeResponse>> {
    // Read-only guard — same pattern as add_host.
    s.launcher
        .as_ref()
        .ok_or_else(|| ApiError::not_available("enrolling a node requires `rupu cp serve`"))?;

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("name must not be empty"));
    }

    let (host, token) = s
        .hosts
        .enroll_node(&name)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let (transport_kind, base_url) = transport_fields(&host.transport);

    // AppState does not carry the CP's public URL; emit a clear placeholder so
    // the operator knows they must substitute the real hostname/port.
    let command = format!("rupu node --cp-url wss://<your-cp-host>:7878 --token {token}");

    Ok(Json(EnrollNodeResponse {
        host: HostView {
            id: host.id,
            name: host.name,
            transport_kind,
            base_url,
            // Newly enrolled: offline until the node connects.
            status: "offline".to_string(),
            version: None,
            capabilities: None,
            active_run_count: 0,
            last_seen_at: host.last_seen_at,
        },
        command,
        token,
    }))
}

/// `DELETE /api/hosts/:id` — remove a registered host.
///
/// - 204 on success.
/// - 400 when `id` is `"local"` (the local host cannot be removed).
async fn remove_host(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if id == "local" {
        return Err(ApiError::bad_request(
            "cannot remove the built-in local host",
        ));
    }
    s.hosts
        .remove_host(&id)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
