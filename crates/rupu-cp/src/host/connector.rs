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
#[derive(Debug, Clone, thiserror::Error)]
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
    /// A 2xx reply whose complete body is not JSON — e.g. an older rupu-cp
    /// whose SPA fallback answers an `/api/*` path it has no route for with
    /// its HTML index. Distinct from a body that could not be READ (a
    /// transport failure mid-response), which stays `Remote(0, _)`.
    #[error("remote reply is not JSON: {0}")]
    NotJson(String),
    /// A bad request or a local precondition failure (no launcher, wrong mode).
    #[error("invalid: {0}")]
    Invalid(String),
    /// A failure on THIS side that the caller did not cause and cannot fix by
    /// changing its request — an I/O error reading the coordinator's own run
    /// store, say. Distinct from [`Self::Invalid`] (the caller's mistake →
    /// HTTP 400) so a server-side fault is never reported as a bad request;
    /// the API layer maps this to 500.
    #[error("internal error: {0}")]
    Internal(String),
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

    /// The coverage stream (`runs/<run_id>/coverage.jsonl`) the executing
    /// host wrote for `run_id`, for the coordinator to merge (spec
    /// 2026-09-30-rupu-remote-findings-transport-design.md §A2), and whether
    /// the transport guarantees it is all of it (see [`CoverageRead`]).
    /// Empty bytes ⇒ no stream arrived. Deliberately no default: every
    /// transport must say how it delivers this, or refuse.
    async fn unit_coverage(&self, run_id: &str) -> Result<CoverageRead, HostConnectorError>;

    /// Stream the blob `sha256` from this host's finding-artifact store into
    /// `dest` (a temp file the caller created inside the coordinator's
    /// store), aborting once more than `max_bytes` (the recorded size) arrive.
    /// The caller verifies size + sha256 before using it (spec
    /// 2026-09-30-rupu-remote-findings-transport-design.md §B1). No default:
    /// every transport must say how, or refuse.
    async fn pull_finding_artifact(
        &self,
        sha256: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(), HostConnectorError>;

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
    /// `true` means run-scoped detail endpoints (`graph`, `usage-timeline`,
    /// `usage`) must build from the local mirror: the artifacts are already
    /// here, and these transports have no generic-GET surface to proxy to
    /// anyway.
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

    /// [`Self::dashboard_summary`] narrowed to one customer's work (plan
    /// ruling 5). Only the local host implements it: a remote host's summary
    /// arrives already summed, so the default — every remote transport — is
    /// `Unsupported`, which callers render as a 501 / "unavailable", never as
    /// zero.
    async fn dashboard_summary_for_customer(
        &self,
        _range: crate::host::dashboard_summary::DashboardRange,
        _customer: &crate::customers::CustomerFilter,
    ) -> Result<crate::host::dashboard_summary::DashboardSummary, HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "dashboard summary by customer".into(),
        ))
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

    // Legacy (pre-codename) runs' step/unit/dispatch events get a derived
    // name; every other event is serialized exactly as before.
    // Classified once at attach, off the executor (`None` for a codename-era
    // run); any per-event disk work runs on the blocking pool.
    let namers = crate::codename_legacy::namers_for_run(Arc::clone(run_store), run_id).await;
    let stream = source.then(move |ev| {
        let namers = namers.clone();
        async move {
            let row = match &namers {
                Some(n) => crate::codename_legacy::name_event(n, &ev).await,
                None => None,
            };
            let json = match row {
                Some(row) => serde_json::to_string(&row),
                None => serde_json::to_string(&ev),
            }
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let frame = format!("data: {json}\n\n");
            Ok::<Bytes, std::io::Error>(Bytes::from(frame.into_bytes()))
        }
    });

    Ok(Box::pin(stream))
}

// ── Mirror-backed observation helpers ────────────────────────────────────────

/// Run a synchronous connector body — run-store reads plus the usage fold,
/// which does file IO under a per-run `std::sync::Mutex` — on tokio's
/// blocking pool instead of an executor thread. Only a panicked body errors.
pub(crate) async fn blocking_host<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, HostConnectorError> + Send + 'static,
) -> Result<T, HostConnectorError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| HostConnectorError::Invalid(format!("connector task failed: {e}")))?
}

/// List runs from the central [`RunStore`] filtered to `worker_id`.
///
/// Shared by [`TunnelHostConnector`] and the upcoming `SshHostConnector` — both
/// read from the same mirror; only the `worker_id` they scope to differs.
///
/// Customers: a mirrored run's workspace lives on the worker, not in this
/// coordinator's store, so its customer cannot be derived here. A row whose
/// run RECORDED a customer carries it (`customer_derived: false`); a row whose
/// run recorded none OMITS the `customer`/`customer_derived` keys — the
/// coordinator cannot tell a legacy run from one with no customer, so under a
/// customer filter such a host answers 501 / is named in
/// `X-Rupu-Hosts-Without-Customer` rather than being counted as "none".
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
        &mut crate::customers::FlatPricing(pricing),
        // No since/until on `RunListQuery` yet — see `LocalHostConnector::
        // list_runs`'s matching call site for why this is deferred.
        &crate::pagination::DateRangeQuery::default(),
        // A mirrored remote run's workspace is not in this coordinator's
        // store, so only the customer the run recorded is reported.
        crate::api::runs::RowCustomers::default(),
    )
    .map_err(crate::api::runs::RunRowsError::into_host)?;

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
    check_mirror_run(run_store, worker_id, run_id)?;
    crate::api::runs::query_run_detail(run_store, run_id, pricing)
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))
}

/// `Ok` when `run_id` is in the mirror and `worker_id` ran it, else
/// [`HostConnectorError::NotFound`] for either miss (see [`mirror_get_run`]).
/// Reads `run.json`, so callers on the async runtime run it on the blocking
/// pool.
fn check_mirror_run(
    run_store: &RunStore,
    worker_id: &str,
    run_id: &str,
) -> Result<(), HostConnectorError> {
    let record = run_store.load(run_id).map_err(|e| match e {
        rupu_orchestrator::RunStoreError::NotFound(_) => {
            HostConnectorError::NotFound(run_id.to_string())
        }
        other => HostConnectorError::Invalid(other.to_string()),
    })?;
    if record.worker_id.as_deref() != Some(worker_id) {
        return Err(HostConnectorError::NotFound(run_id.to_string()));
    }
    Ok(())
}

/// Open a live SSE byte-stream for `run_id`, verifying it belongs to
/// `worker_id` first — a read of `run.json`, made in one hop to the
/// blocking pool.
///
/// Returns [`HostConnectorError::NotFound`] when the run does not exist or
/// belongs to a different node.
pub(crate) async fn mirror_stream_run_events(
    run_store: &Arc<RunStore>,
    worker_id: &str,
    run_id: &str,
) -> Result<EventByteStream, HostConnectorError> {
    let (store, worker, id) = (
        Arc::clone(run_store),
        worker_id.to_owned(),
        run_id.to_owned(),
    );
    blocking_host(move || check_mirror_run(&store, &worker, &id)).await?;
    open_run_events_tail(run_store, run_id).await
}

/// Whether `id` has the shape of a run id this system mints: `run_` followed
/// by ASCII alphanumerics and `_` only. The ONE definition of "safe to use as
/// a run-store path component" — no separators, no `.`, so it can neither
/// traverse out of the runs root nor smuggle shell metacharacters. Every
/// connector, the mirror and the coverage route validate an id they did not
/// mint with this.
pub(crate) fn valid_run_id(id: &str) -> bool {
    id.starts_with("run_") && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A unit's coverage stream as its host connector read it
/// ([`HostConnector::unit_coverage`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageRead {
    /// The stream; empty ⇒ no stream arrived.
    pub bytes: Vec<u8>,
    /// Whether the transport guarantees these are every byte the run wrote,
    /// PROVIDED the run was terminal when they were read: true for a local
    /// run (its own file), a tunnel node (its coverage frames precede
    /// `RunFinished` on one socket), a bucket worker (its finished marker
    /// follows its uploads) and an HTTP remote (its own local file); for SSH
    /// only once the tail pump's terminal pull replaced the mirrored copy
    /// with the host's file. A read taken while the run may still be running
    /// is a snapshot whatever this says — the caller knows which it took.
    pub complete: bool,
}

/// A run's coverage stream read from the coordinator's own run store: the
/// local host's file, or a mirror-backed transport's mirrored copy.
///
/// Async because the stream can be large and the callers are request
/// handlers and connector methods on the async runtime: the read goes through
/// `tokio::fs` (the blocking pool), never a bare `std::fs::read`. A run that
/// wrote no stream reads as empty; a malformed id is [`HostConnectorError::Invalid`]
/// (the caller's fault); any other I/O failure is
/// [`HostConnectorError::Internal`] (ours).
pub async fn mirror_unit_coverage(
    run_store: &RunStore,
    run_id: &str,
) -> Result<Vec<u8>, HostConnectorError> {
    if !valid_run_id(run_id) {
        return Err(HostConnectorError::Invalid(format!(
            "{run_id:?} is not a valid run id"
        )));
    }
    match tokio::fs::read(rupu_coverage::stream_path(&run_store.root, run_id)).await {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(HostConnectorError::Internal(format!(
            "read coverage stream for {run_id}: {e}"
        ))),
    }
}

/// `Invalid` unless `sha256` is a store key (64 lowercase hex).
pub fn validate_sha256(sha256: &str) -> Result<(), HostConnectorError> {
    if rupu_coverage::report::is_sha256_hex(sha256) {
        Ok(())
    } else {
        Err(HostConnectorError::Invalid(format!(
            "{sha256:?} is not a sha256 (64 lowercase hex characters)"
        )))
    }
}

/// Copy the blob at `src` to `dest`, refusing a source that is missing or not
/// a regular file (`NotFound`) or larger than `max_bytes` (`Invalid`).
///
/// `src` is opened once — non-blocking, so a FIFO swapped into the store
/// cannot park the thread — and the type and size are read off that handle,
/// then the bytes come from the same handle. A file that grows after the size
/// check still cannot overshoot: the copy stops one byte past `max_bytes` and
/// fails. On failure `dest` may be left partially written (or absent); the
/// caller owns its cleanup.
pub async fn copy_blob_capped(
    src: &Path,
    dest: &Path,
    max_bytes: u64,
) -> Result<(), HostConnectorError> {
    let not_in_store = || {
        HostConnectorError::NotFound(format!(
            "artifact is not in this host's store ({})",
            src.display()
        ))
    };
    let path = src.to_path_buf();
    let opened = tokio::task::spawn_blocking(move || crate::api::fs_open::open_regular_file(&path))
        .await
        .map_err(|e| HostConnectorError::Invalid(format!("opening the stored blob failed: {e}")))?;
    let file = match opened {
        Ok(f) => f,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
            ) =>
        {
            return Err(not_in_store())
        }
        Err(e) => {
            return Err(HostConnectorError::Invalid(format!(
                "opening the stored blob failed: {e}"
            )))
        }
    };
    let len = file
        .metadata()
        .map_err(|e| HostConnectorError::Invalid(format!("reading the stored blob failed: {e}")))?
        .len();
    if len > max_bytes {
        return Err(HostConnectorError::Invalid(format!(
            "stored blob is {len} bytes, more than its recorded {max_bytes}"
        )));
    }
    copy_reader_capped(tokio::fs::File::from_std(file), dest, max_bytes).await
}

/// Write at most `max_bytes` from `reader` to `dest` (created or truncated);
/// `Invalid` if the reader has more. Reads one byte past the cap to tell
/// "exactly `max_bytes`" from "more", never further, and never writes the
/// over-cap bytes. Each failure says which side failed: the coordinator's
/// `dest` ("local write failed"), the source ("reading the stored blob
/// failed"), or the size cap.
async fn copy_reader_capped<R>(
    reader: R,
    dest: &Path,
    max_bytes: u64,
) -> Result<(), HostConnectorError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let write_err =
        |e: std::io::Error| HostConnectorError::Invalid(format!("local write failed: {e}"));
    let read_err = |e: std::io::Error| {
        HostConnectorError::Invalid(format!("reading the stored blob failed: {e}"))
    };
    let mut out = tokio::fs::File::create(dest).await.map_err(write_err)?;
    let mut limited = reader.take(max_bytes.saturating_add(1));
    let mut buf = vec![0u8; 64 * 1024];
    let mut copied: u64 = 0;
    loop {
        let n = limited.read(&mut buf).await.map_err(read_err)?;
        if n == 0 {
            break;
        }
        copied += n as u64;
        if copied > max_bytes {
            return Err(HostConnectorError::Invalid(format!(
                "stored blob is larger than its recorded {max_bytes} bytes"
            )));
        }
        out.write_all(&buf[..n]).await.map_err(write_err)?;
    }
    out.flush().await.map_err(write_err)?;
    Ok(())
}

/// Read and parse a transcript `.jsonl` file into the standard
/// `{ "events": [...], "summary": … }` shape.
///
/// Returns the same value regardless of whether it is called from a local or
/// tunnel connector.  Basic path safety (no `..` components, must be `.jsonl`)
/// is enforced here; callers that accept user-supplied paths must also apply
/// their own `allowed_roots` checks before delegating. The path checks run
/// in place; the read is one hop to the blocking pool.
pub(crate) async fn read_transcript_file(
    path: &str,
) -> Result<serde_json::Value, HostConnectorError> {
    let p = Path::new(path);
    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err(HostConnectorError::Invalid("not a .jsonl file".into()));
    }
    if p.components().any(|c| c == std::path::Component::ParentDir) {
        return Err(HostConnectorError::Invalid(
            "path must not contain ..".into(),
        ));
    }
    let p = p.to_path_buf();
    blocking_host(move || read_transcript_blocking(&p)).await
}

/// The blocking body of [`read_transcript_file`], once its path checks
/// passed.
fn read_transcript_blocking(p: &Path) -> Result<serde_json::Value, HostConnectorError> {
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

#[cfg(test)]
mod off_runtime_tests {
    use super::*;
    use crate::host::runtime_liveness::{assert_runtime_live_while_parked_on, make_fifo};

    /// Every connector's `get_transcript` is `read_transcript_file`, and its
    /// read runs on the blocking pool: parked on a FIFO transcript, the
    /// runtime keeps ticking.
    #[tokio::test(flavor = "current_thread")]
    async fn read_transcript_file_reads_off_the_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("run_01FIFO.jsonl");
        if !make_fifo(&path) {
            eprintln!("mkfifo unavailable; skipping");
            return;
        }

        let read = read_transcript_file(path.to_str().unwrap());
        let got = assert_runtime_live_while_parked_on(read, &path, false)
            .await
            .unwrap();

        assert_eq!(got, serde_json::json!({ "events": [], "summary": null }));
    }

    /// A mirrored run that recorded a customer carries it; one that recorded
    /// none omits the key, so a customer filter reads the host as unable to
    /// say (501) instead of counting the run as "no customer".
    #[test]
    fn mirror_rows_omit_the_customer_a_worker_run_never_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let seed = |id: &str, customer: Option<&str>| {
            let mut v = serde_json::json!({
                "id": id,
                "workflow_name": "wf",
                "status": "completed",
                "inputs": {},
                "workspace_id": "ws_remote",
                "workspace_path": "/tmp/proj",
                "transcript_dir": "/tmp/proj/.rupu/transcripts",
                "started_at": "2026-10-06T00:00:00Z",
                "worker_id": "node_1",
            });
            if let Some(c) = customer {
                v["customer"] = serde_json::json!(c);
            }
            store
                .create(serde_json::from_value(v).unwrap(), "name: wf\n")
                .unwrap();
        };
        seed("run_acme", Some("acme"));
        let params = RunListQuery {
            kind: RunKind::All,
            offset: 0,
            limit: 50,
            lifecycle: None,
        };
        let pricing = rupu_config::PricingConfig::default();
        let none = crate::customers::CustomerFilter::Unassigned;
        let acme = crate::customers::CustomerFilter::Slug("acme".into());

        // Only recorded runs: filtered normally.
        let rows = mirror_list_runs(&store, "node_1", &params, &pricing).unwrap();
        assert_eq!(rows[0]["customer"], "acme");
        assert_eq!(rows[0]["customer_derived"], false);
        assert_eq!(
            crate::customers::filter_remote_rows(rows.clone(), &acme).map(|r| r.len()),
            Some(1)
        );
        assert_eq!(
            crate::customers::filter_remote_rows(rows, &none).map(|r| r.len()),
            Some(0)
        );

        // A legacy run: no key, so the host can't be filtered (501).
        seed("run_legacy", None);
        let rows = mirror_list_runs(&store, "node_1", &params, &pricing).unwrap();
        let legacy = rows.iter().find(|r| r["id"] == "run_legacy").unwrap();
        assert!(!legacy.as_object().unwrap().contains_key("customer"));
        assert!(!legacy.as_object().unwrap().contains_key("customer_derived"));
        assert!(crate::customers::filter_remote_rows(rows, &none).is_none());
    }

    /// `stream_run_events` on a mirror-backed connector (SSH, tunnel,
    /// bucket) is `mirror_stream_run_events`, and its ownership check — a
    /// read of `run.json`, which no FIFO can park (`RunStore::load` refuses a
    /// non-regular file) — waits for the blocking pool. The run belongs to
    /// another host, so the check is the whole call.
    #[test]
    fn mirror_stream_run_events_checks_ownership_off_the_runtime() {
        use crate::host::runtime_liveness::{
            assert_waits_for_the_blocking_pool, one_blocking_thread_runtime, HeldBlockingThread,
        };
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(RunStore::new(tmp.path().join("runs")));
        let record = serde_json::from_value(serde_json::json!({
            "id": "run_01ELSEWHERE",
            "workflow_name": "wf",
            "status": "running",
            "inputs": {},
            "workspace_id": "ws_1",
            "workspace_path": "/tmp/proj",
            "transcript_dir": "/tmp/proj/.rupu/transcripts",
            "started_at": "2026-10-06T00:00:00Z",
            "worker_id": "host_other",
        }))
        .unwrap();
        store.create(record, "name: wf\n").unwrap();

        one_blocking_thread_runtime().block_on(async {
            let held = HeldBlockingThread::hold();
            let open = mirror_stream_run_events(&store, "host_abc", "run_01ELSEWHERE");
            let got = assert_waits_for_the_blocking_pool(open, held).await;

            assert!(
                matches!(got, Err(HostConnectorError::NotFound(ref id)) if id == "run_01ELSEWHERE"),
                "another host's run is not found"
            );
        });
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
                // `HostConnectorError` is `Clone`; copying it keeps this stub
                // exhaustive-by-construction as variants are added.
                Some(Err(e)) => Err(e.clone()),
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
        async fn unit_coverage(&self, _run_id: &str) -> Result<CoverageRead, HostConnectorError> {
            Ok(CoverageRead {
                bytes: Vec::new(),
                complete: true,
            })
        }
        async fn pull_finding_artifact(
            &self,
            _sha256: &str,
            _dest: &std::path::Path,
            _max_bytes: u64,
        ) -> Result<(), HostConnectorError> {
            Err(HostConnectorError::Unsupported("test double".into()))
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
            async fn unit_coverage(
                &self,
                _run_id: &str,
            ) -> Result<CoverageRead, HostConnectorError> {
                Ok(CoverageRead {
                    bytes: Vec::new(),
                    complete: true,
                })
            }
            async fn pull_finding_artifact(
                &self,
                _sha256: &str,
                _dest: &std::path::Path,
                _max_bytes: u64,
            ) -> Result<(), HostConnectorError> {
                Err(HostConnectorError::Unsupported("test double".into()))
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

    #[tokio::test]
    async fn mirror_unit_coverage_reads_the_run_stream_or_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        assert_eq!(
            mirror_unit_coverage(&store, "run_X1").await.unwrap(),
            Vec::<u8>::new()
        );
        let p = rupu_coverage::stream_path(&store.root, "run_X1");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"line\n").unwrap();
        assert_eq!(
            mirror_unit_coverage(&store, "run_X1").await.unwrap(),
            b"line\n"
        );
        assert!(matches!(
            mirror_unit_coverage(&store, "../etc").await,
            Err(HostConnectorError::Invalid(_))
        ));
    }

    /// An I/O failure reading a stream that exists is OUR fault, not a bad
    /// request: it must not be classified `Invalid` (which the API turns into
    /// a 400).
    #[tokio::test]
    async fn mirror_unit_coverage_classifies_an_unreadable_stream_as_internal() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        // A directory where the stream file belongs: present, but unreadable.
        std::fs::create_dir_all(rupu_coverage::stream_path(&store.root, "run_X2")).unwrap();
        assert!(matches!(
            mirror_unit_coverage(&store, "run_X2").await,
            Err(HostConnectorError::Internal(_))
        ));
    }

    #[test]
    fn valid_run_id_accepts_minted_ids_and_rejects_everything_else() {
        assert!(valid_run_id("run_01HXYZ"));
        assert!(valid_run_id(&format!("run_{}", ulid::Ulid::new())));
        for bad in [
            "",
            "run",
            "01HXYZ",
            "Run_01X",
            "run_a.b",
            "run_../x",
            "run_a/b",
            "run_a-b",
            "run_a b",
            "run_a\n",
            "run_$HOME",
        ] {
            assert!(!valid_run_id(bad), "{bad:?} must be rejected");
        }
    }

    #[tokio::test]
    async fn copy_blob_capped_copies_refuses_missing_and_oversize() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        std::fs::write(&src, b"12345").unwrap();
        copy_blob_capped(&src, &dest, 5).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"12345");
        assert!(matches!(
            copy_blob_capped(&src, &dest, 4).await,
            Err(HostConnectorError::Invalid(_))
        ));
        assert!(matches!(
            copy_blob_capped(&tmp.path().join("absent"), &dest, 5).await,
            Err(HostConnectorError::NotFound(_))
        ));
        assert!(validate_sha256("../x").is_err());
        assert!(validate_sha256(&"ab".repeat(32)).is_ok());
    }

    #[tokio::test]
    async fn copy_blob_capped_refuses_an_oversize_source_without_creating_dest() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        std::fs::write(&src, b"12345").unwrap();
        assert!(copy_blob_capped(&src, &dest, 4).await.is_err());
        assert!(!dest.exists(), "an oversize source must not create dest");
    }

    #[tokio::test]
    async fn copy_blob_capped_refuses_a_non_regular_source_as_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        // A directory sitting where a blob should be.
        assert!(matches!(
            copy_blob_capped(tmp.path(), &dest, 100).await,
            Err(HostConnectorError::NotFound(_))
        ));
        // A FIFO has no writer: a blocking open would park here forever, so
        // the timeout is what this assertion really checks.
        let fifo = tmp.path().join("pipe");
        if crate::api::fs_open::test_support::mkfifo(&fifo) {
            let res = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                copy_blob_capped(&fifo, &dest, 100),
            )
            .await
            .expect("a FIFO source must be refused promptly, not waited on");
            assert!(
                matches!(res, Err(HostConnectorError::NotFound(_))),
                "{res:?}"
            );
        }
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn copy_reader_capped_stops_one_byte_past_the_cap() {
        // A source that keeps producing (a file that grew after the size
        // check): the copy must stop at cap + 1 and fail, never run on.
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let res = copy_reader_capped(tokio::io::repeat(7), &dest, 10).await;
        assert!(
            matches!(res, Err(HostConnectorError::Invalid(_))),
            "{res:?}"
        );
        // Only in-cap bytes ever reach the file.
        assert!(std::fs::metadata(&dest).unwrap().len() <= 10);
        // Exactly at the cap is fine.
        let ok = copy_reader_capped(&b"0123456789"[..], &dest, 10).await;
        assert!(ok.is_ok(), "{ok:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"0123456789");
    }

    /// A reader that fails on the first read.
    struct FailingReader;

    impl tokio::io::AsyncRead for FailingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("disk gone")))
        }
    }

    #[tokio::test]
    async fn copy_reader_capped_labels_which_side_failed() {
        let tmp = tempfile::tempdir().unwrap();
        // Source read failure.
        let err = copy_reader_capped(FailingReader, &tmp.path().join("dest"), 10)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, HostConnectorError::Invalid(m)
                if m == "reading the stored blob failed: disk gone"),
            "{err:?}"
        );
        // Coordinator-side destination failure (parent directory missing).
        let err = copy_reader_capped(&b"abc"[..], &tmp.path().join("no-dir").join("dest"), 10)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, HostConnectorError::Invalid(m) if m.starts_with("local write failed: ")),
            "{err:?}"
        );
        // Over the cap: its own message, neither of the above.
        let err = copy_reader_capped(&b"0123456789ab"[..], &tmp.path().join("dest"), 10)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, HostConnectorError::Invalid(m)
                if m == "stored blob is larger than its recorded 10 bytes"),
            "{err:?}"
        );
    }
}
