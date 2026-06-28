//! `HostConnector` port — the trait every host adapter (local or HTTP) must
//! implement, plus the shared types it works over.

use std::pin::Pin;

use bytes::Bytes;
use futures_util::Stream;
use serde::{Deserialize, Serialize};

use crate::{
    agent_launcher::AgentLaunchRequest,
    launcher::LaunchRequest,
    session_sender::SendMessageRequest,
    session_starter::SessionStartRequest,
};

// ── Byte-stream alias ─────────────────────────────────────────────────────────

/// A pinned, boxed byte stream of SSE-formatted event frames, returned by
/// `stream_run_events`. Each `Ok(Bytes)` item is a complete `data: …\n\n`
/// chunk. Used by both the local tail and the HTTP proxy pass-through.
pub type EventByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

// ── Info / capabilities ───────────────────────────────────────────────────────

/// Advertised capabilities of a remote rupu CP host. Task 6 `/api/host/info`
/// will return this shape; for local host[0] in this slice it is left empty
/// (defaults).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostCapabilities {
    pub backends: Vec<String>,
    pub scm_hosts: Vec<String>,
    pub permission_modes: Vec<String>,
}

/// Health + version snapshot for one host.
#[derive(Debug, Clone)]
pub struct HostInfo {
    pub reachable: bool,
    pub version: Option<String>,
    pub capabilities: HostCapabilities,
}

// ── Query types ───────────────────────────────────────────────────────────────

/// Selects which runs to enumerate. Maps to the existing API endpoints:
/// - `All` → `GET /api/runs` (all runs regardless of trigger)
/// - `Workflow` → `GET /api/runs/workflows` (manual/direct runs only)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    All,
    Workflow,
}

/// Pagination + filter parameters for `list_runs`.
#[derive(Debug, Clone)]
pub struct RunListQuery {
    pub kind: RunKind,
    pub offset: usize,
    pub limit: usize,
    /// Optional lifecycle group: `"active"` | `"completed"` | `"failed"`.
    pub lifecycle: Option<String>,
}

// ── Error ─────────────────────────────────────────────────────────────────────

/// Errors produced by a `HostConnector` method.
#[derive(Debug, thiserror::Error)]
pub enum HostConnectorError {
    /// The target host could not be reached (network failure, DNS, timeout).
    #[error("host unreachable: {0}")]
    Unreachable(String),
    /// The request was rejected with a 401/403.
    #[error("unauthorized")]
    Unauthorized,
    /// The requested resource does not exist on this host.
    #[error("not found: {0}")]
    NotFound(String),
    /// A non-2xx HTTP response from a remote host (status code, body).
    #[error("remote error {0}: {1}")]
    Remote(u16, String),
    /// A bad request or a local precondition failure (no launcher, wrong mode).
    #[error("invalid: {0}")]
    Invalid(String),
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Uniform interface over a rupu CP host — local (in-process) or remote (HTTP).
/// The local impl delegates to the per-capability port traits and the
/// `RunStore`; the HTTP impl proxies over the wire.
#[async_trait::async_trait]
pub trait HostConnector: Send + Sync {
    /// Fetch health + version info for this host.
    async fn info(&self) -> Result<HostInfo, HostConnectorError>;

    /// Start a new workflow run; returns the new run id.
    async fn launch_run(&self, req: LaunchRequest) -> Result<String, HostConnectorError>;

    /// Start a new agent run; returns the new run id.
    async fn launch_agent(&self, req: AgentLaunchRequest) -> Result<String, HostConnectorError>;

    /// Start a new agent session; returns the new session id.
    async fn start_session(
        &self,
        req: SessionStartRequest,
    ) -> Result<String, HostConnectorError>;

    /// Send a prompt turn to a live session; returns the resulting run id.
    async fn send_session_turn(
        &self,
        req: SendMessageRequest,
    ) -> Result<String, HostConnectorError>;

    /// List runs matching the given query; each element is a run-row `Value`
    /// in the same shape `GET /api/runs` produces.
    async fn list_runs(
        &self,
        params: RunListQuery,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError>;

    /// Fetch a single run's detail (run record + steps + usage) in the shape
    /// `GET /api/runs/:id` produces.
    async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError>;

    /// Record a web-approval decision for a paused run (`mode` is the resume
    /// permission mode; empty string → host default).
    async fn approve_run(&self, run_id: &str, mode: &str) -> Result<(), HostConnectorError>;

    /// Record a rejection decision for a paused run.
    async fn reject_run(
        &self,
        run_id: &str,
        reason: Option<&str>,
    ) -> Result<(), HostConnectorError>;

    /// Cancel an in-flight run.
    async fn cancel_run(&self, run_id: &str) -> Result<(), HostConnectorError>;

    /// Open a live SSE byte stream of `events.jsonl` for the given run. Each
    /// `Ok(Bytes)` item is a `data: {json}\n\n` SSE frame. See Task 8 for
    /// host-aware observation built on top of this.
    async fn stream_run_events(
        &self,
        run_id: &str,
    ) -> Result<EventByteStream, HostConnectorError>;

    /// Fetch the parsed events + summary for a transcript JSONL path.
    ///
    /// Returns the same `{ "events": [...], "summary": ... }` shape that
    /// `GET /api/transcript` produces. For the local connector, `path` must be
    /// a `.jsonl` file with no `..` components; for the HTTP connector the
    /// request is forwarded to the remote's `/api/transcript?path=<path>`.
    async fn get_transcript(
        &self,
        path: &str,
    ) -> Result<serde_json::Value, HostConnectorError>;

    /// Generic GET passthrough: issue `GET {base_url}{path_and_query}` (bearer
    /// token attached) and return the parsed JSON body.
    ///
    /// `path_and_query` is an absolute path including any query string,
    /// e.g. `/api/runs/agents?limit=5`. The local connector always returns
    /// `Err(HostConnectorError::Invalid("local host is served in-process"))`.
    async fn proxy_get_json(
        &self,
        path_and_query: &str,
    ) -> Result<serde_json::Value, HostConnectorError>;
}
