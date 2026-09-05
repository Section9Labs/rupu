//! `HostConnector` port — the trait every host adapter (local or HTTP) must
//! implement, plus the shared types and free helper functions used by multiple
//! connector implementations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

/// Keeps a remote→local transcript feed alive for as long as it lives (spec
/// §5). Dropping it releases the holder's interest; a connector stops the
/// underlying feed once the last guard for a file is gone. Connectors whose
/// recorded paths are already local hand back [`FeedGuard::noop`].
pub struct FeedGuard {
    _release: Option<Box<dyn std::any::Any + Send + Sync>>,
}

impl FeedGuard {
    pub fn noop() -> Self {
        Self { _release: None }
    }

    /// Hold `inner` (typically an `Arc` refcount on a shared feed) until drop.
    pub fn holding(inner: Box<dyn std::any::Any + Send + Sync>) -> Self {
        Self {
            _release: Some(inner),
        }
    }
}

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

// ── Run-start evidence ──────────────────────────────────────────

/// What a transport can say about whether a launched run's remote process
/// actually STARTED — asked independently of, and answerable much earlier
/// than, [`HostConnector::get_run`].
///
/// The distinction exists because "the run is observable through `get_run`"
/// is NOT a startup signal for an agent run. A standalone `rupu run <agent>`
/// writes its `run.json` only after the agent has finished
/// (`cmd/run.rs`: `let run_result = agent_task.await` — then "Write run.json
/// so the run is observable via RunStore"), so "never observed" is the normal
/// state of a placed agent run for its ENTIRE duration, however long that is.
/// A caller that treats "not observed yet" as "never launched" abandons
/// healthy work; this is the signal it should key on instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStartEvidence {
    /// The host shows this run actually started: a process carrying its run
    /// id, or artifacts only a started run writes (a non-empty transcript,
    /// `run.json`, `events.jsonl`, `step_results.jsonl`).
    Started,
    /// The transport looked and found no trace of the run at all. Combined
    /// with a launch the host ACCEPTED, this is positive evidence that the
    /// remote process died before doing anything.
    NoTrace,
    /// The transport cannot answer — it has no probe for this (the default),
    /// or the probe itself failed (host unreachable, malformed answer).
    /// Callers must treat this as "no information", never as `NoTrace`.
    Unknown,
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

    /// Wait until any coordinator-side mirroring for `run_id` has finished
    /// its terminal work (final transcript catch-up, `run.json`, `finish`).
    ///
    /// Transports that mirror asynchronously (SSH's tail pump is a spawned
    /// task) MUST override this so a short-lived dispatching process — the
    /// `rupu workflow run` CLI exits the moment the workflow completes — can
    /// join the mirror before reporting a placed unit terminal. Without the
    /// join the process exits mid-flight and the mirrored transcript is
    /// silently truncated at whatever the tail had delivered (measured: the
    /// `assistant_message` / `turn_end` / `run_complete` tail lost on a real
    /// host). Transports whose `get_run` already reflects everything the
    /// coordinator will ever hold (Local, HttpCp, Tunnel, Bucket) keep the
    /// default no-op. Must never hang a caller indefinitely: implementations
    /// bound the wait and log if the bound is hit.
    async fn await_run_mirror(&self, _run_id: &str) {}

    /// Best-effort diagnostics for a launch this connector ACCEPTED but whose
    /// run never showed up through [`get_run`](Self::get_run): whatever the
    /// detached remote process wrote to stderr before dying (wrong cwd,
    /// missing agent, bad flag, unresolvable provider). `launch_*` returning
    /// `Ok` means the detach succeeded, not that the run started, so a
    /// caller that gives up polling asks here for the reason and folds it
    /// into its error instead of reporting a bare registration timeout.
    ///
    /// `None` means "nothing recorded" — the transport doesn't capture launch
    /// stderr (the default), the log is empty, or the host can't be reached.
    /// Implementations must bound the excerpt and must never fail loudly:
    /// this is consulted while a real error is already being reported.
    async fn launch_diagnostics(&self, _run_id: &str) -> Option<String> {
        None
    }

    /// Best-effort evidence that a launched run's remote process actually
    /// STARTED, for callers that must distinguish "the run has not become
    /// observable through [`get_run`](Self::get_run) YET" from "the launch
    /// never happened".
    ///
    /// [`get_run`](Self::get_run) cannot answer that question for an agent
    /// run: `run.json` is written when the agent FINISHES, so a placed agent
    /// run is unobservable for its whole lifetime and a caller that bounds
    /// "never observed" with a startup deadline kills healthy long runs. This
    /// method is the cheap, early-true signal that deadline should key on
    /// instead — see [`RunStartEvidence`].
    ///
    /// Implementations MUST be cheap (one round trip at most), MUST NOT fail
    /// loudly, and MUST return [`RunStartEvidence::Unknown`] rather than
    /// [`RunStartEvidence::NoTrace`] when they could not actually look: the
    /// difference is whether a caller may blame the launch. The default is
    /// `Unknown`, so a transport without a probe keeps whatever bound its
    /// caller already applied.
    async fn run_start_evidence(&self, _run_id: &str) -> RunStartEvidence {
        RunStartEvidence::Unknown
    }

    /// Cooperatively pause an in-flight (`Pending`/`Running`) run, leaving it
    /// non-terminal and resumable via [`resume_run`](Self::resume_run).
    ///
    /// Distinct from [`cancel_run`](Self::cancel_run) (terminal). The
    /// default impl returns [`HostConnectorError::Unsupported`] so
    /// transports that haven't wired pause reach (Bucket / Tunnel) compile
    /// unchanged; Local / SSH / HttpCp override it.
    async fn pause_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("pause".into()))
    }

    /// Resume a `Paused` run. Requires the full `cp serve` runtime (the
    /// background resume worker that re-enters `run_workflow` lives there —
    /// see `RunStore::list_pending_resume`); callers gate this on the host's
    /// launcher being configured. The default impl returns
    /// [`HostConnectorError::Unsupported`].
    async fn resume_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("resume".into()))
    }

    /// Move a terminal run into the archive scope (reversible). See
    /// `RunStore::archive`. Non-terminal → `HostConnectorError::Invalid`;
    /// missing → `NotFound`.
    ///
    /// The default impl returns [`HostConnectorError::Unsupported`] so
    /// transports without an addressable per-run store of their own
    /// (Bucket/Tunnel — those observe a central mirror scoped by
    /// `worker_id`, not an independently archivable store) compile
    /// unchanged. Local / SSH / HTTP override it.
    async fn archive_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("archive".into()))
    }

    /// Move an archived run back to the active scope. See `RunStore::restore`.
    /// Default: see [`archive_run`](Self::archive_run).
    async fn restore_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("restore".into()))
    }

    /// Permanently delete a run (either scope). See `RunStore::delete`.
    /// Default: see [`archive_run`](Self::archive_run).
    async fn delete_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("delete".into()))
    }

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

    /// Map a transcript path *as recorded by this host's run artifacts* to
    /// the coordinator-local file that serves it (spec §3.2). Identity for
    /// hosts whose recorded paths are already local (Local, HTTP — the
    /// latter forwards reads to the remote CP instead). Mirror-backed
    /// transports return their cache path.
    fn local_transcript_path(&self, recorded: &Path) -> PathBuf {
        recorded.to_path_buf()
    }

    /// Ensure `recorded` (a path this host wrote, claimed by `run_id`'s own
    /// artifacts) is being fed into [`Self::local_transcript_path`] for as
    /// long as the returned guard lives. Default: unsupported.
    async fn ensure_transcript_feed(
        &self,
        _run_id: &str,
        _recorded: &Path,
    ) -> Result<FeedGuard, HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "transcript feed is not supported for this host type".into(),
        ))
    }

    /// One-shot pull of `recorded` into its local counterpart. `terminal`
    /// marks the copy authoritative (spec §6.1). Default: unsupported.
    async fn pull_transcript(
        &self,
        _run_id: &str,
        _recorded: &Path,
        _terminal: bool,
    ) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "transcript pull is not supported for this host type".into(),
        ))
    }

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

    /// Whether this transport's runs are mirrored into the coordinator's own
    /// `RunStore` (by `NodeMirror`) rather than living only on the remote.
    ///
    /// `true` means run-scoped detail endpoints (`graph`, `usage-timeline`)
    /// must build from the local mirror: the artifacts are already here, and
    /// these transports have no generic-GET surface to proxy to anyway.
    /// `false` — the default, and the HTTP connector's answer — means the
    /// run's artifacts live on the remote and must be fetched over the wire.
    fn serves_runs_from_local_mirror(&self) -> bool {
        false
    }

    /// Whether this transport executes an [`AgentLaunchRequest`] under the
    /// `run_id` the CALLER supplied, rather than minting its own.
    ///
    /// A placed fan-out unit's coordinator mints the id up front so it can
    /// announce the unit's mirrored transcript path before the run exists
    /// (`UnitDispatcher::unit_transcript_path`). That announcement is only
    /// truthful for connectors that actually honour the supplied id — today
    /// SSH alone. It is deliberately NOT the same question as
    /// [`Self::serves_runs_from_local_mirror`], which is also `true` for the
    /// tunnel and bucket transports even though both mint their own ids.
    ///
    /// [`AgentLaunchRequest`]: crate::agent_launcher::AgentLaunchRequest
    fn honours_supplied_run_id(&self) -> bool {
        false
    }

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

    /// Fetch one session's detail record from this host, **in API shape**
    /// (the same field names `GET /api/sessions/:id` returns locally —
    /// `agent_name`, `provider_name`, ...). The structured counterpart to
    /// `proxy_get_json("/api/sessions/<id>")`, so non-HTTP transports can
    /// serve session detail too: HTTP proxies verbatim, while the SSH
    /// connector shells `rupu session show <id> --format json` and renames
    /// that report's human-table field labels to the API's.
    ///
    /// The returned body may omit `usage` — a transport that cannot price
    /// (SSH carries no pricing config by design; see `SshHostConnector::new`)
    /// leaves that to the caller. The default errors so transports without
    /// session enumeration compile unchanged.
    async fn get_session(&self, _id: &str) -> Result<serde_json::Value, HostConnectorError> {
        Err(HostConnectorError::Unsupported("session detail".into()))
    }

    /// The runs one session recorded, newest-last, as a JSON array — the
    /// structured counterpart to `proxy_get_json("/api/sessions/<id>/runs")`.
    ///
    /// Deliberately separate from [`get_session`](Self::get_session): the API
    /// session DTO carries no `runs` field, so an HTTP host must proxy the
    /// dedicated `/runs` endpoint rather than dig into the detail body. The
    /// SSH connector reads the `runs[]` out of the same `session show`
    /// report — one ssh round trip per call, not two.
    async fn session_runs(&self, _id: &str) -> Result<serde_json::Value, HostConnectorError> {
        Err(HostConnectorError::Unsupported("session runs".into()))
    }

    /// One run's raw netflow records from this host, as
    /// `{ "flows": [FlowRecord...], "dropped_total": u64 }`.
    ///
    /// Deliberately RAW rather than an aggregated response: the CP applies
    /// its own window, filters and ASN table to the records, so a remote
    /// cannot return something that looks filtered but is not (the reason
    /// the proxy path carries a defensive re-filtering pass), and every
    /// host's flows get enriched identically. The SSH connector shells
    /// `rupu netflow show <run_id> --format json`.
    async fn run_netflow(&self, _run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        Err(HostConnectorError::Unsupported("run netflow".into()))
    }

    /// Token/cost rollup for a time window on this host — the structured
    /// counterpart to `proxy_get_json("/api/usage?...")`.
    ///
    /// `since`/`until` are RFC-3339; `group_by` is the CP's own group name.
    /// Returns the host's report verbatim (shapes differ per transport), so
    /// the caller maps it. The SSH connector shells `rupu usage --since
    /// <s> --until <u> --group-by <g> --format json`.
    async fn usage_rollup(
        &self,
        _since: &str,
        _until: &str,
        _group_by: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        Err(HostConnectorError::Unsupported("usage rollup".into()))
    }

    /// Per-turn token series for one session on this host — the structured
    /// counterpart to `proxy_get_json("/api/sessions/<id>/usage-timeline")`.
    ///
    /// Returns a JSON array of the same points the local branch emits. HTTP
    /// proxies; the SSH connector shells `rupu session usage-timeline <id>
    /// --format json`, which computes the series remotely in ONE round trip
    /// (fetching each run's transcript separately would be N ssh
    /// connections per page load).
    async fn session_usage_timeline(
        &self,
        _id: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "session usage timeline".into(),
        ))
    }

    /// Archive an active session on this host. The default impl returns
    /// [`HostConnectorError::Unsupported`] so transports without session
    /// enumeration/mutation (Local — routed through the `SessionMutator`
    /// port instead, see `api/sessions.rs`; Bucket/Tunnel — no session
    /// mirror) compile unchanged. SSH / HTTP override it.
    async fn archive_session(&self, _id: &str) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("session archive".into()))
    }

    /// Restore a previously-archived session on this host.
    /// Default: see [`archive_session`](Self::archive_session).
    async fn restore_session(&self, _id: &str) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("session restore".into()))
    }

    /// Permanently delete a session (either scope) on this host.
    /// Default: see [`archive_session`](Self::archive_session).
    async fn delete_session(&self, _id: &str) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("session delete".into()))
    }

    /// Archive a standalone agent-run transcript on this host. The default
    /// impl returns [`HostConnectorError::Unsupported`] so transports without
    /// transcript mutation (Local — routed through the `TranscriptMutator`
    /// port instead, see `api/transcripts.rs`; Bucket/Tunnel — no transcript
    /// mirror) compile unchanged. SSH / HTTP override it. No `restore_transcript`
    /// exists: `rupu transcript restore` is not a real CLI verb.
    ///
    /// `ignore_liveness` is the PID-reuse escape hatch — see
    /// `TranscriptMutator::mutate`'s doc. Defaults to `false`.
    async fn archive_transcript(
        &self,
        _id: &str,
        _ignore_liveness: bool,
    ) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("transcript archive".into()))
    }

    /// Permanently delete a standalone agent-run transcript on this host.
    /// Default: see [`archive_transcript`](Self::archive_transcript).
    async fn delete_transcript(
        &self,
        _id: &str,
        _ignore_liveness: bool,
    ) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported("transcript delete".into()))
    }

    /// List standalone/agent runs on this host (`GET /api/runs/agents`).
    /// The SSH connector shells `rupu transcript list --format json`. Default
    /// errors so transports without agent-run enumeration compile unchanged.
    async fn list_agent_runs(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        Err(HostConnectorError::Unsupported("agent-run listing".into()))
    }

    /// List autoflow cycle summaries on this host (`GET /api/runs/autoflows`).
    async fn list_autoflow_runs(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "autoflow-run listing".into(),
        ))
    }

    /// List recent autoflow events on this host
    /// (`GET /api/runs/autoflows/events`).
    async fn list_autoflow_events(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "autoflow-event listing".into(),
        ))
    }

    /// Aggregate dashboard state for this host, in ONE round-trip.
    ///
    /// Deliberately coarse. SSH hosts pay a full ssh handshake per call — there
    /// is no ControlMaster multiplexing in `RemoteExec::run` — so this must not
    /// decompose into per-panel calls.
    ///
    /// The default is `Unsupported`, and callers MUST render that as
    /// "unavailable", never as zero: a host that cannot report is not a host
    /// with no runs.
    async fn dashboard_summary(
        &self,
        _range: crate::host::dashboard_summary::DashboardRange,
    ) -> Result<crate::host::dashboard_summary::DashboardSummary, HostConnectorError> {
        Err(HostConnectorError::Unsupported("dashboard summary".into()))
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
        // No since/until on `RunListQuery` yet — see `LocalHostConnector::
        // list_runs`'s matching call site for why this is deferred.
        &crate::pagination::DateRangeQuery::default(),
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

/// A configurable [`HostConnector`] for tests.
///
/// Every method not explicitly configured panics rather than returning a
/// plausible empty value: a test that reaches an unconfigured method has
/// exercised a path it did not mean to, and should say so loudly.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    #[derive(Default)]
    pub(crate) struct StubConnector {
        /// Canned `run_netflow` reply. `None` → `Unsupported`, modelling a
        /// remote whose `rupu` predates `netflow show`.
        pub run_netflow: Option<Result<serde_json::Value, HostConnectorError>>,
    }

    #[async_trait::async_trait]
    impl HostConnector for StubConnector {
        async fn run_netflow(
            &self,
            _run_id: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            match &self.run_netflow {
                Some(Ok(v)) => Ok(v.clone()),
                Some(Err(e)) => Err(match e {
                    HostConnectorError::Unreachable(m) => {
                        HostConnectorError::Unreachable(m.clone())
                    }
                    HostConnectorError::Unsupported(m) => {
                        HostConnectorError::Unsupported(m.clone())
                    }
                    HostConnectorError::Invalid(m) => HostConnectorError::Invalid(m.clone()),
                    HostConnectorError::NotFound(m) => HostConnectorError::NotFound(m.clone()),
                    HostConnectorError::Unauthorized => HostConnectorError::Unauthorized,
                    HostConnectorError::Remote(c, m) => HostConnectorError::Remote(*c, m.clone()),
                }),
                None => Err(HostConnectorError::Unsupported("run netflow".into())),
            }
        }

        async fn info(&self) -> Result<HostInfo, HostConnectorError> {
            unimplemented!("StubConnector: info not configured")
        }
        async fn launch_run(
            &self,
            _req: crate::launcher::LaunchRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("StubConnector: launch_run not configured")
        }
        async fn launch_agent(
            &self,
            _req: crate::agent_launcher::AgentLaunchRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("StubConnector: launch_agent not configured")
        }
        async fn start_session(
            &self,
            _req: crate::session_starter::SessionStartRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("StubConnector: start_session not configured")
        }
        async fn send_session_turn(
            &self,
            _req: crate::session_sender::SendMessageRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("StubConnector: send_session_turn not configured")
        }
        async fn list_runs(
            &self,
            _params: RunListQuery,
        ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            unimplemented!("StubConnector: list_runs not configured")
        }
        async fn get_run(&self, _run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!("StubConnector: get_run not configured")
        }
        async fn approve_run(&self, _run_id: &str, _mode: &str) -> Result<(), HostConnectorError> {
            unimplemented!("StubConnector: approve_run not configured")
        }
        async fn reject_run(
            &self,
            _run_id: &str,
            _reason: Option<&str>,
        ) -> Result<(), HostConnectorError> {
            unimplemented!("StubConnector: reject_run not configured")
        }
        async fn cancel_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
            unimplemented!("StubConnector: cancel_run not configured")
        }
        async fn stream_run_events(
            &self,
            _run_id: &str,
        ) -> Result<EventByteStream, HostConnectorError> {
            unimplemented!("StubConnector: stream_run_events not configured")
        }
        async fn get_transcript(
            &self,
            _path: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!("StubConnector: get_transcript not configured")
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!("StubConnector: proxy_get_json not configured")
        }
    }

    #[tokio::test]
    async fn transcript_hooks_default_to_identity_and_unsupported() {
        struct Bare;
        #[async_trait::async_trait]
        impl HostConnector for Bare {
            async fn info(&self) -> Result<HostInfo, HostConnectorError> {
                unimplemented!()
            }
            async fn launch_run(&self, _r: LaunchRequest) -> Result<String, HostConnectorError> {
                unimplemented!()
            }
            async fn launch_agent(
                &self,
                _r: AgentLaunchRequest,
            ) -> Result<String, HostConnectorError> {
                unimplemented!()
            }
            async fn start_session(
                &self,
                _r: SessionStartRequest,
            ) -> Result<String, HostConnectorError> {
                unimplemented!()
            }
            async fn send_session_turn(
                &self,
                _r: SendMessageRequest,
            ) -> Result<String, HostConnectorError> {
                unimplemented!()
            }
            async fn list_runs(
                &self,
                _q: RunListQuery,
            ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
                unimplemented!()
            }
            async fn get_run(&self, _id: &str) -> Result<serde_json::Value, HostConnectorError> {
                unimplemented!()
            }
            async fn approve_run(&self, _id: &str, _m: &str) -> Result<(), HostConnectorError> {
                unimplemented!()
            }
            async fn reject_run(
                &self,
                _run_id: &str,
                _reason: Option<&str>,
            ) -> Result<(), HostConnectorError> {
                unimplemented!()
            }
            async fn cancel_run(&self, _id: &str) -> Result<(), HostConnectorError> {
                unimplemented!()
            }
            async fn stream_run_events(
                &self,
                _id: &str,
            ) -> Result<EventByteStream, HostConnectorError> {
                unimplemented!()
            }
            async fn get_transcript(
                &self,
                _p: &str,
            ) -> Result<serde_json::Value, HostConnectorError> {
                unimplemented!()
            }
            async fn proxy_get_json(
                &self,
                _p: &str,
            ) -> Result<serde_json::Value, HostConnectorError> {
                unimplemented!()
            }
        }
        let c = Bare;
        let p = std::path::Path::new("/remote/.rupu/transcripts/run_01A.jsonl");
        assert_eq!(c.local_transcript_path(p), p.to_path_buf());
        assert!(matches!(
            c.ensure_transcript_feed("run_01R", p).await,
            Err(HostConnectorError::Unsupported(_))
        ));
        assert!(matches!(
            c.pull_transcript("run_01R", p, true).await,
            Err(HostConnectorError::Unsupported(_))
        ));
        let _ = FeedGuard::noop();
    }
}
