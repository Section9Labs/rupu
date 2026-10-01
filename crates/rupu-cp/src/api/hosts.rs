//! `GET /api/hosts` · `GET /api/hosts/registered` · `POST /api/hosts` ·
//! `DELETE /api/hosts/:id`
//!
//! Exposes the `HostRegistry` over HTTP. `GET /api/hosts` probes every remote
//! host concurrently and tolerates unreachable hosts: a failed `info()` call
//! produces `status: "offline"` rather than failing the whole list.
//! `GET /api/hosts/registered` is the probe-free counterpart: a pure store
//! read (`id` / `name` / `transport_kind` only) that returns promptly even
//! when every registered remote is dead — used by pages that need to know
//! which hosts exist before deciding which ones to probe.

#![deny(clippy::all)]

use std::{path::PathBuf, sync::Arc};

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
        .route("/api/hosts/registered", get(list_registered_hosts))
        .route("/api/hosts/node", post(enroll_node_handler))
        .route("/api/hosts/ssh", post(add_ssh_host_handler))
        .route("/api/hosts/bucket", post(add_bucket_host_handler))
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

/// JSON view of one registered host, WITHOUT any live health data.
///
/// This is the store-only shape: `id` / `name` / `transport_kind`, nothing a
/// probe (`connector.info()` / `active_run_count()`) could produce. Backs
/// `GET /api/hosts/registered` — see that handler's doc comment.
#[derive(Debug, Serialize)]
pub struct RegisteredHostView {
    pub id: String,
    pub name: String,
    /// `"local"`, `"http_cp"`, `"tunnel"`, `"ssh"`, or `"bucket"`.
    pub transport_kind: String,
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Decompose a `HostTransport` into the serialised kind string + optional URL.
///
/// `pub(crate)` so `api::dashboard`'s freshness strip can reuse the same
/// kind-string vocabulary instead of a second, potentially disagreeing match.
pub(crate) fn transport_fields(t: &HostTransport) -> (String, Option<String>) {
    match t {
        HostTransport::Local => ("local".to_string(), None),
        HostTransport::HttpCp { base_url } => ("http_cp".to_string(), Some(base_url.clone())),
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
async fn active_run_count(conn: &Arc<dyn crate::host::connector::HostConnector>) -> usize {
    let q = RunListQuery {
        kind: RunKind::All,
        offset: 0,
        limit: 200,
        lifecycle: Some("active".to_string()),
    };
    conn.list_runs(q).await.map(|v| v.len()).unwrap_or(0)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /api/hosts/registered` — list all known hosts, no probe.
///
/// A dedicated route rather than a `?probe=false` branch on [`list_hosts`]:
/// that handler already fans out per-host `info()` + `active_run_count()`
/// futures and is about to grow more probe-related shape as the dashboard
/// rework lands, so keeping this as its own small handler avoids threading a
/// bool through that fan-out and keeps the "never probes" contract visible at
/// the type level (`RegisteredHostView` simply has no field a probe could
/// populate). It follows the existing precedent of literal sibling routes
/// under `/api/hosts/*` (`/node`, `/ssh`, `/bucket`) living alongside the
/// dynamic `/api/hosts/:id` — axum's router already resolves that ambiguity
/// in favor of the literal segment.
///
/// Backed by [`HostRegistry::list_hosts`], a pure store read — no `info()`,
/// no `active_run_count()`, no network I/O. Returns promptly even when a
/// registered remote is completely unreachable. Local is always first.
async fn list_registered_hosts(
    State(s): State<AppState>,
) -> ApiResult<Json<Vec<RegisteredHostView>>> {
    let views = s
        .hosts
        .list_hosts()
        .into_iter()
        .map(|host| {
            let (transport_kind, _base_url) = transport_fields(&host.transport);
            RegisteredHostView {
                id: host.id,
                name: host.name,
                transport_kind,
            }
        })
        .collect();
    Ok(Json(views))
}

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

/// Split an HTTP `Host` header value into `(host, port)`.
///
/// Handles bracketed IPv6 (`[::1]:7878` → `("[::1]", Some(7878))`), plain
/// `host:port`, and bare hosts (port `None`). An unparseable port is treated
/// as absent.
fn split_host_port(header: &str) -> (String, Option<u16>) {
    // Bracketed IPv6: `[::1]` or `[::1]:7878`.
    if let Some(rest) = header.strip_prefix('[') {
        if let Some(close) = rest.find(']') {
            let host = format!("[{}]", &rest[..close]);
            let port = rest[close + 1..]
                .strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok());
            return (host, port);
        }
    }
    // `host:port` — only when the suffix parses as a port (guards against
    // unbracketed IPv6, which has multiple colons and is left intact).
    if let Some((host, port)) = header.rsplit_once(':') {
        if !host.contains(':') {
            if let Ok(p) = port.parse::<u16>() {
                return (host.to_string(), Some(p));
            }
            return (host.to_string(), None);
        }
    }
    (header.to_string(), None)
}

/// True when `host` (as produced by [`split_host_port`]) is a loopback
/// address a *remote* node could never reach: `localhost`, `127.x.x.x`,
/// `::1` / `[::1]`.
fn is_loopback_host(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if bare.eq_ignore_ascii_case("localhost") {
        return true;
    }
    bare.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Build the copy-paste `rupu node …` command for a freshly-enrolled tunnel
/// node.
///
/// Host derivation, in order:
/// 1. the request's `Host` header — the operator reached the CP at that
///    address, so it is a good default for the node too — unless it is a
///    loopback address a remote node cannot use;
/// 2. for loopback (or absent) Host headers: the machine's detected routable
///    IP, keeping the Host header's port (default 7878);
/// 3. when detection also fails: a `<your-cp-host>` placeholder the operator
///    must substitute by hand.
///
/// Scheme is always `ws://` — this build has no TLS support in the node
/// agent, and the CP itself serves plain HTTP.
fn build_node_command(
    host_header: Option<&str>,
    detected_ip: Option<std::net::IpAddr>,
    token: &str,
    node_id: &str,
) -> String {
    let (header_host, header_port) = match host_header {
        Some(h) => {
            let (host, port) = split_host_port(h);
            (Some(host), port)
        }
        None => (None, None),
    };
    let port = header_port.unwrap_or(7878);
    let host = match header_host {
        Some(h) if !is_loopback_host(&h) => h,
        // Loopback or absent: a remote node cannot dial it — use this
        // machine's routable IP, or a placeholder if detection failed.
        _ => match detected_ip {
            Some(std::net::IpAddr::V6(v6)) => format!("[{v6}]"),
            Some(ip) => ip.to_string(),
            None => "<your-cp-host>".to_string(),
        },
    };
    format!(
        "rupu node --cp-url ws://{host}:{port}/api/node/connect --token {token} --node-id {node_id}"
    )
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
/// host view, the runnable command (host derived from the request's `Host`
/// header, falling back to this machine's routable IP when the operator is on
/// localhost — see [`build_node_command`]), and the plaintext token.
///
/// Requires the `cp serve` launcher adapter; returns 501 on a read-only deploy.
async fn enroll_node_handler(
    State(s): State<AppState>,
    headers: axum::http::HeaderMap,
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

    // Derive the node's dial address from the request: the Host header the
    // operator used to reach the CP, falling back to this machine's routable
    // IP when that address is loopback (unreachable for a remote node).
    let host_header = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok());
    let command = build_node_command(
        host_header,
        crate::net::detect_routable_ip(),
        &token,
        &host.id,
    );

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

// ── SSH host ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AddSshHostBody {
    name: String,
    host: String,
    port: Option<u16>,
    /// Path to an SSH identity file on the local machine (optional).
    /// No credential is stored; the path is metadata only.
    identity_file: Option<String>,
}

/// `POST /api/hosts/ssh` — register a new SSH host.
///
/// Requires the `cp serve` launcher adapter (501 if absent — read-only deploy).
/// No secrets are accepted or stored; auth is delegated to the system `ssh`.
async fn add_ssh_host_handler(
    State(s): State<AppState>,
    Json(body): Json<AddSshHostBody>,
) -> ApiResult<Json<HostView>> {
    s.launcher
        .as_ref()
        .ok_or_else(|| ApiError::not_available("managing hosts requires `rupu cp serve`"))?;

    let host = s
        .hosts
        .add_ssh_host(
            &body.name,
            &body.host,
            body.port,
            body.identity_file.map(PathBuf::from),
        )
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

// ── Bucket host ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AddBucketHostBody {
    name: String,
    url: String,
    prefix: Option<String>,
}

/// `POST /api/hosts/bucket` — register a new bucket (dead-drop) host.
///
/// Requires the `cp serve` launcher adapter (501 if absent — read-only deploy).
/// No credentials are accepted or stored; auth comes from the environment /
/// cloud credential chain.
async fn add_bucket_host_handler(
    State(s): State<AppState>,
    Json(body): Json<AddBucketHostBody>,
) -> ApiResult<Json<HostView>> {
    s.launcher
        .as_ref()
        .ok_or_else(|| ApiError::not_available("managing hosts requires `rupu cp serve`"))?;

    let host = s
        .hosts
        .add_bucket_host(&body.name, &body.url, body.prefix)
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

/// `DELETE /api/hosts/:id` — remove a registered host.
///
/// - 204 on success.
/// - 400 when `id` is `"local"` (the local host cannot be removed).
async fn remove_host(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    // ------------------------------------------------------------------
    // split_host_port
    // ------------------------------------------------------------------

    #[test]
    fn split_host_port_variants() {
        assert_eq!(
            split_host_port("cp.example.com"),
            ("cp.example.com".to_string(), None)
        );
        assert_eq!(
            split_host_port("cp.example.com:7878"),
            ("cp.example.com".to_string(), Some(7878))
        );
        assert_eq!(
            split_host_port("[::1]:7878"),
            ("[::1]".to_string(), Some(7878))
        );
        assert_eq!(split_host_port("[::1]"), ("[::1]".to_string(), None));
        // Unparseable port → treated as absent.
        assert_eq!(split_host_port("h:notaport"), ("h".to_string(), None));
    }

    // ------------------------------------------------------------------
    // is_loopback_host
    // ------------------------------------------------------------------

    #[test]
    fn loopback_hosts_detected() {
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LOCALHOST"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("[::1]"));
        assert!(is_loopback_host("::1"));
        assert!(!is_loopback_host("cp.example.com"));
        assert!(!is_loopback_host("192.168.1.10"));
    }

    // ------------------------------------------------------------------
    // build_node_command
    // ------------------------------------------------------------------

    #[test]
    fn command_uses_host_header_verbatim() {
        let cmd = build_node_command(Some("cp.example.com:9999"), None, "tok", "node_1");
        assert_eq!(
            cmd,
            "rupu node --cp-url ws://cp.example.com:9999/api/node/connect --token tok --node-id node_1"
        );
    }

    #[test]
    fn command_defaults_port_7878_when_header_has_none() {
        let cmd = build_node_command(Some("cp.example.com"), None, "tok", "node_1");
        assert!(
            cmd.contains("ws://cp.example.com:7878/api/node/connect"),
            "cmd: {cmd}"
        );
    }

    #[test]
    fn localhost_header_falls_back_to_detected_ip_keeping_port() {
        let ip: IpAddr = "10.1.2.3".parse().unwrap();
        let cmd = build_node_command(Some("localhost:9999"), Some(ip), "tok", "node_1");
        assert!(
            cmd.contains("ws://10.1.2.3:9999/api/node/connect"),
            "cmd: {cmd}"
        );
    }

    #[test]
    fn localhost_header_no_detected_ip_uses_placeholder() {
        let cmd = build_node_command(Some("127.0.0.1:7878"), None, "tok", "node_1");
        assert!(
            cmd.contains("ws://<your-cp-host>:7878/api/node/connect"),
            "cmd: {cmd}"
        );
    }

    #[test]
    fn absent_header_uses_detected_ip_and_default_port() {
        let ip: IpAddr = "192.168.7.7".parse().unwrap();
        let cmd = build_node_command(None, Some(ip), "tok", "node_1");
        assert!(
            cmd.contains("ws://192.168.7.7:7878/api/node/connect"),
            "cmd: {cmd}"
        );
    }

    #[test]
    fn detected_ipv6_is_bracketed() {
        let ip: IpAddr = "fd00::1".parse().unwrap();
        let cmd = build_node_command(Some("localhost"), Some(ip), "tok", "node_1");
        assert!(
            cmd.contains("ws://[fd00::1]:7878/api/node/connect"),
            "cmd: {cmd}"
        );
    }

    #[test]
    fn command_always_includes_node_id_and_ws_scheme() {
        let cmd = build_node_command(None, None, "tok", "node_01XYZ");
        assert!(cmd.contains("--node-id node_01XYZ"), "cmd: {cmd}");
        assert!(cmd.contains("--cp-url ws://"), "cmd: {cmd}");
        assert!(!cmd.contains("wss://"), "cmd: {cmd}");
    }

    // ------------------------------------------------------------------
    // detect_routable_ip
    // ------------------------------------------------------------------

    #[test]
    fn detect_routable_ip_not_loopback_when_some() {
        // CI sandboxes may yield None — acceptable; but a detected IP must
        // be a real interface address, never loopback.
        if let Some(ip) = crate::net::detect_routable_ip() {
            assert!(!ip.is_loopback(), "detected loopback: {ip}");
        }
    }
}
