//! `HostConnector` port — the trait every host adapter (local or HTTP) must
//! implement, plus the shared types and free helper functions used by multiple
//! connector implementations.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use futures_util::{Stream, StreamExt as _};
use rupu_orchestrator::{executor::FileTailRunSource, runs::RunStore};
use serde::{Deserialize, Serialize};

use crate::{
    agent_launcher::AgentLaunchRequest, launcher::LaunchRequest,
    session_sender::SendMessageRequest, session_starter::SessionStartRequest,
};

// ── Byte-stream alias ─────────────────────────────────────────────────────────

/// A pinned, boxed byte stream of SSE-formatted event frames, returned by
/// `stream_run_events`. Each `Ok(Bytes)` item is a complete `data: …\n\n`
/// chunk. Used by both the local tail and the HTTP proxy pass-through.
pub type EventByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

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
    /// The operation is not supported on this transport (e.g. workspace sync
    /// over a Bucket/Tunnel host).
    #[error("unsupported on this transport: {0}")]
    Unsupported(String),
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
    async fn start_session(&self, req: SessionStartRequest) -> Result<String, HostConnectorError>;

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
    async fn stream_run_events(&self, run_id: &str) -> Result<EventByteStream, HostConnectorError>;

    /// Fetch the parsed events + summary for a transcript JSONL path.
    ///
    /// Returns the same `{ "events": [...], "summary": ... }` shape that
    /// `GET /api/transcript` produces. For the local connector, `path` must be
    /// a `.jsonl` file with no `..` components; for the HTTP connector the
    /// request is forwarded to the remote's `/api/transcript?path=<path>`.
    async fn get_transcript(&self, path: &str) -> Result<serde_json::Value, HostConnectorError>;

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

    /// List sessions on this host, optionally filtered by `scope`
    /// (`"active"` | `"archived"`). The structured counterpart to
    /// `proxy_get_json("/api/sessions")`, so non-HTTP transports (SSH) can
    /// enumerate sessions too — the SSH connector shells `rupu session list
    /// --format json` over `ssh`. The default errors so transports without
    /// session enumeration compile unchanged.
    async fn list_sessions(
        &self,
        _scope: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        Err(HostConnectorError::Unsupported("session listing".into()))
    }

    /// Stage a packed workspace on the host; returns the remote working dir.
    ///
    /// `payload` is a wire-encoded [`rupu_workspace::Payload`] (see
    /// [`encode_payload`]). The default impl returns [`HostConnectorError::Unsupported`]
    /// so transports without workspace sync (Bucket / Tunnel) compile unchanged.
    async fn stage_workspace(&self, _payload: Vec<u8>) -> Result<String, HostConnectorError> {
        Err(HostConnectorError::Unsupported("workspace sync".into()))
    }

    /// Collect the workspace change-delta from a staged working dir.
    ///
    /// Returns a wire-encoded [`rupu_workspace::Delta`] (see [`encode_delta`]).
    /// The default impl returns [`HostConnectorError::Unsupported`].
    async fn collect_workspace_delta(
        &self,
        _working_dir: &str,
    ) -> Result<Vec<u8>, HostConnectorError> {
        Err(HostConnectorError::Unsupported("workspace sync".into()))
    }

    /// Best-effort discard of a staged workspace scratch dir.
    ///
    /// Called by a coordinator when the unit that consumed the staged tree
    /// failed *between* `stage_workspace` and `collect_workspace_delta` (e.g.
    /// `launch_agent` errored, or the run poll timed out) — so
    /// `collect_workspace_delta` never ran and the scratch would otherwise
    /// leak indefinitely. The default no-op impl is correct for transports
    /// that don't support workspace sync at all; every transport that
    /// implements `stage_workspace` should also implement this.
    async fn discard_workspace(&self, _working_dir: &str) -> Result<(), HostConnectorError> {
        Ok(())
    }
}

// ── Workspace-sync wire codec ─────────────────────────────────────────────────
//
// The connector boundary moves opaque bytes: `stage_workspace` takes an encoded
// [`rupu_workspace::Payload`]; `collect_workspace_delta` returns an encoded
// [`rupu_workspace::Delta`]. These free functions define that self-describing
// wire format so both the coordinator (rupu-cli's dispatcher) and every
// transport impl agree on it.

/// Upper bound on a packed workspace payload accepted by `stage_workspace`.
/// Over-limit payloads are rejected with [`HostConnectorError::Invalid`] before
/// any disk work, guarding both the coordinator and the host.
pub const MAX_WORKSPACE_BYTES: usize = 256 * 1024 * 1024;

fn mode_to_u8(m: rupu_workspace::SyncMode) -> u8 {
    match m {
        rupu_workspace::SyncMode::Tar => 0,
        rupu_workspace::SyncMode::Git => 1,
    }
}

fn u8_to_mode(b: u8) -> Result<rupu_workspace::SyncMode, HostConnectorError> {
    match b {
        0 => Ok(rupu_workspace::SyncMode::Tar),
        1 => Ok(rupu_workspace::SyncMode::Git),
        other => Err(HostConnectorError::Invalid(format!(
            "unknown workspace sync mode tag {other}"
        ))),
    }
}

/// Encode a [`rupu_workspace::Payload`] as `[mode:1][raw bytes…]`.
pub fn encode_payload(p: &rupu_workspace::Payload) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.bytes.len() + 1);
    out.push(mode_to_u8(p.mode));
    out.extend_from_slice(&p.bytes);
    out
}

/// Decode a payload produced by [`encode_payload`].
pub fn decode_payload(bytes: &[u8]) -> Result<rupu_workspace::Payload, HostConnectorError> {
    let (&mode, rest) = bytes
        .split_first()
        .ok_or_else(|| HostConnectorError::Invalid("empty workspace payload".into()))?;
    Ok(rupu_workspace::Payload {
        mode: u8_to_mode(mode)?,
        bytes: rest.to_vec(),
    })
}

#[derive(Serialize, Deserialize)]
struct DeltaWireHeader {
    mode: u8,
    changed: Vec<String>,
    deleted: Vec<String>,
}

/// Encode a [`rupu_workspace::Delta`] as
/// `[hdr_len:4 LE][serde_json header][raw delta bytes]`. The header carries the
/// mode tag plus the changed/deleted path lists; the trailing bytes are the
/// codec's opaque tar/patch payload.
pub fn encode_delta(d: &rupu_workspace::Delta) -> Vec<u8> {
    let hdr = DeltaWireHeader {
        mode: mode_to_u8(d.mode),
        changed: d.changed.clone(),
        deleted: d.deleted.clone(),
    };
    let hdr_bytes = serde_json::to_vec(&hdr).unwrap_or_default();
    let mut out = Vec::with_capacity(4 + hdr_bytes.len() + d.bytes.len());
    out.extend_from_slice(&(hdr_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&hdr_bytes);
    out.extend_from_slice(&d.bytes);
    out
}

/// Decode a delta produced by [`encode_delta`].
pub fn decode_delta(bytes: &[u8]) -> Result<rupu_workspace::Delta, HostConnectorError> {
    if bytes.len() < 4 {
        return Err(HostConnectorError::Invalid(
            "workspace delta too short".into(),
        ));
    }
    let hdr_len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let rest = &bytes[4..];
    if rest.len() < hdr_len {
        return Err(HostConnectorError::Invalid(
            "workspace delta header truncated".into(),
        ));
    }
    let (hdr_bytes, payload) = rest.split_at(hdr_len);
    let hdr: DeltaWireHeader = serde_json::from_slice(hdr_bytes)
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
    Ok(rupu_workspace::Delta {
        mode: u8_to_mode(hdr.mode)?,
        changed: hdr.changed,
        deleted: hdr.deleted,
        bytes: payload.to_vec(),
    })
}

#[derive(Serialize, Deserialize)]
struct BaselineWire {
    mode: u8,
    manifest: BTreeMap<String, Vec<u8>>,
    git_commit: Option<String>,
}

/// Serialize a stage-time [`rupu_workspace::Baseline`] to JSON for the sidecar
/// file persisted between `stage_workspace` and `collect_workspace_delta`.
pub(crate) fn serialize_baseline(
    b: &rupu_workspace::Baseline,
) -> Result<Vec<u8>, HostConnectorError> {
    let wire = BaselineWire {
        mode: mode_to_u8(b.mode),
        manifest: b
            .tar_manifest
            .iter()
            .map(|(k, v)| (k.clone(), v.to_vec()))
            .collect(),
        git_commit: b.git_commit.clone(),
    };
    serde_json::to_vec(&wire).map_err(|e| HostConnectorError::Invalid(e.to_string()))
}

/// Reload a baseline written by [`serialize_baseline`].
pub(crate) fn deserialize_baseline(
    bytes: &[u8],
) -> Result<rupu_workspace::Baseline, HostConnectorError> {
    let wire: BaselineWire =
        serde_json::from_slice(bytes).map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
    let mut manifest = BTreeMap::new();
    for (k, v) in wire.manifest {
        let arr: [u8; 32] = v
            .try_into()
            .map_err(|_| HostConnectorError::Invalid("bad baseline hash length".into()))?;
        manifest.insert(k, arr);
    }
    Ok(rupu_workspace::Baseline {
        mode: u8_to_mode(wire.mode)?,
        tar_manifest: manifest,
        git_commit: wire.git_commit,
    })
}

// ── Shared read helpers ───────────────────────────────────────────────────────

/// Open a live SSE byte-stream for `run_id`'s `events.jsonl`.
///
/// The caller is responsible for verifying that the run exists (and optionally
/// that it belongs to the expected host/worker) **before** calling this
/// function. This helper only opens the file tail and maps it into the
/// `data: …\n\n` SSE frame format.
pub(crate) async fn open_run_events_tail(
    run_store: &Arc<RunStore>,
    run_id: &str,
) -> Result<EventByteStream, HostConnectorError> {
    let events_path = run_store.events_path(run_id);
    let source = FileTailRunSource::open(&events_path)
        .await
        .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;

    let stream = source.map(|ev| {
        let json = serde_json::to_string(&ev)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let frame = format!("data: {json}\n\n");
        Ok::<Bytes, std::io::Error>(Bytes::from(frame.into_bytes()))
    });

    Ok(Box::pin(stream))
}

// ── Mirror-backed observation helpers ────────────────────────────────────────

/// List runs from the central [`RunStore`] filtered to `worker_id`.
///
/// Shared by [`TunnelHostConnector`] and the upcoming `SshHostConnector` — both
/// read from the same mirror; only the `worker_id` they scope to differs.
pub(crate) fn mirror_list_runs(
    run_store: &RunStore,
    worker_id: &str,
    params: &RunListQuery,
    pricing: &rupu_config::PricingConfig,
) -> Result<Vec<serde_json::Value>, HostConnectorError> {
    let workflow_only = params.kind == RunKind::Workflow;
    let rows = crate::api::runs::query_run_rows(
        run_store,
        params.offset,
        params.limit,
        params.lifecycle.as_deref(),
        workflow_only,
        Some(worker_id),
        pricing,
    )
    .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

    rows.iter()
        .map(|r| serde_json::to_value(r).map_err(|e| HostConnectorError::Invalid(e.to_string())))
        .collect()
}

/// Fetch detail for a single run, verifying it belongs to `worker_id`.
///
/// Returns [`HostConnectorError::NotFound`] when the run does not exist or
/// belongs to a different node — callers should not distinguish these two cases
/// (leaking the existence of another node's run would be a data-scope violation).
pub(crate) fn mirror_get_run(
    run_store: &RunStore,
    worker_id: &str,
    run_id: &str,
    pricing: &rupu_config::PricingConfig,
) -> Result<serde_json::Value, HostConnectorError> {
    let record = run_store.load(run_id).map_err(|e| match e {
        rupu_orchestrator::RunStoreError::NotFound(_) => {
            HostConnectorError::NotFound(run_id.to_string())
        }
        other => HostConnectorError::Invalid(other.to_string()),
    })?;
    if record.worker_id.as_deref() != Some(worker_id) {
        return Err(HostConnectorError::NotFound(run_id.to_string()));
    }
    crate::api::runs::query_run_detail(run_store, run_id, pricing)
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))
}

/// Open a live SSE byte-stream for `run_id`, verifying it belongs to
/// `worker_id` first.
///
/// Returns [`HostConnectorError::NotFound`] when the run does not exist or
/// belongs to a different node.
pub(crate) async fn mirror_stream_run_events(
    run_store: &Arc<RunStore>,
    worker_id: &str,
    run_id: &str,
) -> Result<EventByteStream, HostConnectorError> {
    let record = run_store.load(run_id).map_err(|e| match e {
        rupu_orchestrator::RunStoreError::NotFound(_) => {
            HostConnectorError::NotFound(run_id.to_string())
        }
        other => HostConnectorError::Invalid(other.to_string()),
    })?;
    if record.worker_id.as_deref() != Some(worker_id) {
        return Err(HostConnectorError::NotFound(run_id.to_string()));
    }
    open_run_events_tail(run_store, run_id).await
}

/// Read and parse a transcript `.jsonl` file into the standard
/// `{ "events": [...], "summary": … }` shape.
///
/// Returns the same value regardless of whether it is called from a local or
/// tunnel connector.  Basic path safety (no `..` components, must be `.jsonl`)
/// is enforced here; callers that accept user-supplied paths must also apply
/// their own `allowed_roots` checks before delegating.
pub(crate) fn read_transcript_file(path: &str) -> Result<serde_json::Value, HostConnectorError> {
    use std::path::Path;
    let p = Path::new(path);
    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err(HostConnectorError::Invalid("not a .jsonl file".into()));
    }
    if p.components().any(|c| c == std::path::Component::ParentDir) {
        return Err(HostConnectorError::Invalid(
            "path must not contain ..".into(),
        ));
    }
    if !p.exists() {
        return Ok(serde_json::json!({ "events": [], "summary": null }));
    }
    let events: Vec<rupu_transcript::Event> = rupu_transcript::JsonlReader::iter(p)
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    let summary = rupu_transcript::JsonlReader::summary(p).ok();
    Ok(serde_json::json!({ "events": events, "summary": summary }))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod codec_tests {
    use super::*;

    #[test]
    fn payload_wire_round_trip() {
        let p = rupu_workspace::Payload {
            mode: rupu_workspace::SyncMode::Tar,
            bytes: b"hello payload".to_vec(),
        };
        let decoded = decode_payload(&encode_payload(&p)).unwrap();
        assert_eq!(decoded.mode, rupu_workspace::SyncMode::Tar);
        assert_eq!(decoded.bytes, p.bytes);
    }

    #[test]
    fn delta_wire_round_trip() {
        let d = rupu_workspace::Delta {
            mode: rupu_workspace::SyncMode::Git,
            changed: vec!["a.txt".into(), "dir/b.txt".into()],
            deleted: vec!["gone.txt".into()],
            bytes: b"raw patch bytes".to_vec(),
        };
        let decoded = decode_delta(&encode_delta(&d)).unwrap();
        assert_eq!(decoded.mode, rupu_workspace::SyncMode::Git);
        assert_eq!(decoded.changed, d.changed);
        assert_eq!(decoded.deleted, d.deleted);
        assert_eq!(decoded.bytes, d.bytes);
    }

    #[test]
    fn baseline_sidecar_round_trip() {
        let mut manifest = BTreeMap::new();
        manifest.insert("a.txt".to_string(), [7u8; 32]);
        let b = rupu_workspace::Baseline {
            mode: rupu_workspace::SyncMode::Tar,
            tar_manifest: manifest,
            git_commit: None,
        };
        let reloaded = deserialize_baseline(&serialize_baseline(&b).unwrap()).unwrap();
        assert_eq!(reloaded.mode, rupu_workspace::SyncMode::Tar);
        assert_eq!(reloaded.tar_manifest.get("a.txt"), Some(&[7u8; 32]));
        assert!(reloaded.git_commit.is_none());
    }

    #[test]
    fn decode_rejects_short_and_unknown_mode() {
        assert!(decode_payload(&[]).is_err());
        assert!(decode_payload(&[9]).is_err()); // unknown mode tag
        assert!(decode_delta(&[0, 0]).is_err()); // shorter than 4-byte header len
    }
}
