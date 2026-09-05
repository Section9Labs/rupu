//! Netflow API — run-scoped read of the run's own per-run ledger file(s)
//! — including every sub-agent it dispatched, at any depth, folded in
//! (see [`run_and_unit_ids`]'s doc for the attribution decision) — plus
//! its transcript, merged, with read-time ASN enrichment.
//!
//! ASN is resolved HERE, at render time, not stamped into the record (spec
//! §6.2). A table that arrives later therefore improves every historical
//! flow with no backfill.
//!
//! ## Why this still reads the ledger AND the transcript (do not "simplify")
//!
//! With one ledger file per run (`NetflowPaths::for_run`), a flow's
//! attribution to a run is the FILE it landed in, not the `ctx.run_id`
//! field — `run_id` is `None` on every production flow today (see
//! `rupu_netflow::ctx::FlowCtx`'s doc comment), so it was never usable
//! for attribution to begin with. `Origin::Scm` (SCM connector traffic,
//! e.g. `Registry::discover`) is the one *non-provider* origin that DOES
//! reach a real per-run sink and lands in THIS run's own ledger file,
//! same as `Origin::Provider` does. `Origin::System` (which covers
//! auth/oauth token exchange, the theme-URL fetch, and the ASN-table
//! refresh — there is no separate `Auth` variant), `Origin::Update` and
//! `Origin::Cp` are all wired to `NullSink` and reach no ledger at all
//! (see `ScopeDisclosure.tsx` for the full accounting). `filter_by_run`'s
//! old job — recovering `run_id: None` flows a shared ledger's field
//! filter would otherwise drop — is not what the transcript merge is for
//! anymore.
//!
//! What it's for instead: `netflow_sink::for_run` (rupu-cli) is
//! best-effort — capture must never break a run, so a ledger file that
//! fails to open is not fatal, and the run's sink degrades to
//! transcript-only capture for the rest of that run's life (see that
//! function's doc). For such a run the ledger file never exists at all, so
//! a ledger-only read reports nothing for it — the run's own transcript,
//! which the same sink always writes to independent of the ledger's fate,
//! is the ONLY surviving record of that run's network activity.
//! [`merge_with_transcript`] is what makes those flows reachable at all;
//! see its doc for the dedup rule.
//!
//! ## Blocking I/O never runs on the async task
//!
//! A ledger is per-run (`<netflow_dir>/<run_id>.jsonl`), so its life is
//! bounded by its own run, not by the `cp serve` daemon's — but nothing
//! deletes it automatically either: `rupu netflow prune --older-than
//! <duration>` (`rupu-cli`'s `cmd::netflow`) is the retention tool, and an
//! installation that never runs it keeps every ledger forever (see
//! `rupu_netflow`'s crate doc, `# Retention`, for the full accounting —
//! this paragraph used to claim there was no retention story at all,
//! which stopped being true once that command landed). Every
//! ledger/transcript/ASN-table read in THIS module is still a synchronous
//! `std::fs` call regardless of how many or how large the ledgers it
//! reads are, so running it directly on an async handler would block
//! whatever tokio worker thread picked up the request — stalling *every
//! other* concurrent CP request sharing that thread (approvals, the gate
//! sweep, unrelated API calls), not just the netflow caller. [`run_blocking`]
//! is the one place every such read goes through instead.

use crate::{
    api::run_resolve::{resolve_run_location, RunLocation},
    api::runs::{resolve_host, run_not_found_or_internal},
    error::{ApiError, ApiResult},
    host::connector::HostConnectorError,
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_netflow::{
    global_netflow_dir, is_per_run_ledger_path,
    ledger::explorer::{
        self, ExplorerFilters, ExplorerFlow, HistogramView, KpiView, RunSpan, SankeyView,
        TimelineView, EXPLORER_BUCKETS,
    },
    project_local_netflow_dir, AsnInfo, AsnTable, FlowId, FlowRecord, NetflowPaths,
};
use rupu_orchestrator::runs::{RunRecord, RunStore};
use rupu_transcript::{Event as TranscriptEvent, JsonlReader};
use rupu_workspace::WorkspaceStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path as StdPath, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs/:id/netflow", get(get_run_netflow))
        .route("/api/projects/:id/netflow", get(get_project_netflow))
        .route("/api/netflow", get(get_global_netflow))
        .route("/api/netflow/graph", get(get_netflow_graph))
        .route("/api/netflow/explorer", get(get_netflow_explorer))
}

/// Run `f` — a synchronous ledger/transcript/ASN-table read — on the tokio
/// blocking thread pool instead of inline on the async task. See the module
/// doc's "Blocking I/O" note for why this matters once a ledger has grown
/// past trivial size. A panic inside `f` becomes `ApiError::internal`
/// rather than propagating (there's nothing sensible for the caller to
/// degrade to here — unlike a missing/corrupt ledger file, which the
/// `rupu_netflow::ledger` readers already turn into an empty result).
async fn run_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(format!("netflow blocking read task failed: {e}")))
}

/// A flow plus its read-time enrichment.
///
/// `Deserialize` is derived (not just `Serialize`) so the `Host` branch of
/// `get_run_netflow` can parse a remote CP's JSON response straight off the
/// wire when proxying — the same struct is both the producer's and the
/// proxying consumer's type, so there is no separate wire DTO to drift out
/// of sync (mirrors `UsageSummary` in `usage.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowView {
    #[serde(flatten)]
    pub flow: FlowRecord,
    /// `None` when the peer IP is unknown (Coarse fidelity) or the ASN
    /// table has no entry. The UI distinguishes both from "AS0".
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub asn: Option<AsnInfo>,
    /// The TOP-LEVEL run this flow folds into — the ledger file's own id
    /// mapped back through step/fan-out/sub-agent records to its root run
    /// (see [`RunMetaIndex`]). Distinct from the flattened `ctx.run_id`,
    /// which is `None` on every production flow (attribution is by ledger
    /// FILE — module doc). `None` when the ledger id matched no run record
    /// anywhere (e.g. a standalone agent run, which never enters
    /// `RunStore`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub run_id: Option<String>,
    /// `RunRecord::workflow_name` of that root run; `None` when
    /// unresolvable (same cases as `run_id`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workflow: Option<String>,
}

impl FlowView {
    pub fn from_flow(flow: FlowRecord, table: Option<&AsnTable>) -> Self {
        let asn = flow.peer_ip.and_then(|ip| table.and_then(|t| t.lookup(ip)));
        Self {
            flow,
            asn,
            run_id: None,
            workflow: None,
        }
    }
}

/// The `?from=`/`?to=` window actually applied to produce `NetflowResponse::flows`
/// (and, transitively, `hosts`) — deliberately NOT `dropped_total`, which is
/// whole-file by definition (see that field's doc comment).
///
/// Present unconditionally on every response (both sides `null` when
/// unbounded, i.e. no filter was requested) rather than only when a filter
/// is active — Minor 4, Task 3 review round 1: this gives a caller positive
/// confirmation that whatever filter it sent was actually honoured, and
/// makes `dropped_total`'s whole-file scope legible BY CONTRAST (a reader
/// comparing a narrow `window` against an unrelated-looking `dropped_total`
/// has a visual cue that the two are not the same scope), rather than that
/// meaning being carried by the field name alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowEcho {
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<&rupu_netflow::ledger::TimeRange> for WindowEcho {
    fn from(range: &rupu_netflow::ledger::TimeRange) -> Self {
        Self {
            from: range.from,
            to: range.to,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NetflowResponse {
    pub flows: Vec<FlowView>,
    /// Per-host rollup, computed here rather than in the browser so the
    /// percentile and unknown-bytes logic has exactly ONE implementation
    /// (`rupu_netflow::ledger::host_rollup`).
    pub hosts: Vec<rupu_netflow::ledger::HostRollup>,
    /// The `?from=`/`?to=` window that was actually applied to `flows` —
    /// see [`WindowEcho`]. `#[serde(default)]` so a response proxied from
    /// an older remote that predates this field (but already has
    /// `dropped_total`, so it still deserializes at all) degrades to
    /// unbounded rather than failing closed the way a missing
    /// `dropped_total` does — `run_netflow_from_host` immediately
    /// overwrites this with the LOCALLY enforced range regardless, so a
    /// stale/absent value from the remote is never actually trusted.
    #[serde(default)]
    pub window: WindowEcho,
    /// Records lost to writer-channel overflow, for the WHOLE ledger
    /// file(s) this response reads — NEVER narrowed by `?from=`/`?to=`. A
    /// drop batch (`LedgerLine::Dropped`) carries no per-record timestamp,
    /// so it can never be tested against a window (see
    /// `rupu_netflow::ledger::TimeRange`'s and `read_flows_in_range`'s doc
    /// comments). Named `dropped_total` rather than `dropped` so a
    /// time-filtered response can never be misread as "nothing was lost in
    /// this window" — a bare `dropped` next to a filtered `flows` list
    /// would read that way even though doc comments explaining otherwise
    /// don't survive into JSON. Surfaced so the UI can say "N flows
    /// dropped" instead of quietly under-reporting.
    pub dropped_total: u64,
    /// Whether the ASN table was available for this request. `false` means
    /// the `asn` fields are absent because we could not look them up, NOT
    /// because the flows had no ASN.
    pub asn_loaded: bool,
    /// Sources that own part of this run's traffic but could not be read.
    ///
    /// Non-empty means the flows shown are INCOMPLETE, and by how much is
    /// unknown. Serving a short list silently would be the worse failure —
    /// a run placed on a remote host keeps its ledger there, so a local-only
    /// answer for one is not "no traffic", it is "we could not look".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incomplete: Vec<IncompleteSource>,
}

/// One source of this run's flows that could not be read, and why.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IncompleteSource {
    /// The registered host id whose ledger is missing from this response.
    pub host_id: String,
    /// Why it could not be read, in the words of whatever refused —
    /// an out-of-date remote `rupu`, an unreachable host, an unregistered
    /// host id. Rendered to an operator, so it must name the fix.
    pub reason: String,
}

/// Flows belonging to one run, filtered from an in-memory set by
/// `ctx.run_id`.
///
/// No production read path calls this anymore: a run-scoped read opens
/// that run's own per-run ledger FILE(s) directly (`resolve_ledger_paths` /
/// [`run_scoped_flows_and_dropped`]), so attribution comes from which file
/// a flow landed in, not from this field — see the module doc. Kept as a
/// pure, still-exported function because
/// `crates/rupu-cp/tests/netflow_api.rs`'s `run_scope_filters_to_that_run_only`
/// exercises it directly against a hand-mixed fixture; there is no other
/// caller.
pub fn filter_by_run(flows: &[FlowRecord], run_id: &str) -> Vec<FlowRecord> {
    flows
        .iter()
        .filter(|f| f.ctx.run_id.as_deref() == Some(run_id))
        .cloned()
        .collect()
}

/// Snapshot `[netflow]` out of `AppState`'s shared config lock, cloned so
/// the lock is not held across the `maybe_refresh_asn` call (which may
/// spawn a task) or any `.await` point. Mirrors `workspace.rs`'s
/// `s.config.read().map(|c| c.cp.clone()).unwrap_or_default()` — a
/// poisoned lock degrades to defaults rather than panicking the handler.
fn netflow_config(s: &AppState) -> rupu_config::NetflowConfig {
    s.config
        .read()
        .map(|c| c.netflow.clone())
        .unwrap_or_default()
}

/// Process-wide cache for the parsed ASN table, keyed on the on-disk file's
/// mtime (Fix 5, netflow Plan 3 review round 3).
///
/// Before this cache, [`load_asn_table`] did a bare `std::fs::read` +
/// `serde_json::from_slice` of the WHOLE table on every call, at four call
/// sites, with no cache and no mtime check. The real iptoasn dataset is
/// ~600k ranges — tens of megabytes of JSON — and all three UI surfaces
/// (global/project/run Network tabs) fire a netflow read on mount, so a
/// single page view could reparse the whole table multiple times. Lives on
/// [`AppState`] (one instance per process, shared across requests via
/// `Arc`), not as a `static` — mirrors `run_location_cache`'s placement for
/// the same reason: request handlers only ever see it through `AppState`,
/// and tests construct a fresh `AppState` per case rather than sharing
/// process-global state across them.
#[derive(Default)]
pub struct AsnCache {
    inner: std::sync::Mutex<Option<(std::time::SystemTime, Arc<AsnTable>)>>,
}

impl AsnCache {
    /// Load the table, reusing the cached parse when the on-disk file's
    /// mtime hasn't moved since the last load; re-reads and re-parses
    /// otherwise. `None` when the path can't be determined (no
    /// `$RUPU_HOME`) or the file is missing/unreadable/corrupt — the same
    /// "enrichment degrades, never errors" contract `load_asn_table` always
    /// had. `Arc<AsnTable>` so a cache hit is a refcount bump, not a clone
    /// of the parsed ranges.
    fn load(&self) -> Option<Arc<AsnTable>> {
        let path = rupu_netflow::asn::asn_db_path()?;
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;

        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cached_mtime, table)) = guard.as_ref() {
            if *cached_mtime == mtime {
                return Some(Arc::clone(table));
            }
        }
        let table = Arc::new(AsnTable::load(&path).ok()?);
        *guard = Some((mtime, Arc::clone(&table)));
        Some(table)
    }

    /// Drop the cached parse so the next [`Self::load`] re-reads from disk
    /// even if the mtime comparison would (rarely) miss a change — e.g. a
    /// refresh that completes within the same mtime-resolution window as
    /// the copy already cached on some filesystems. Called once a
    /// [`maybe_refresh_asn`] download completes successfully; a failed
    /// refresh leaves the existing table (and this cache) untouched, same
    /// as before this cache existed.
    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = None;
        }
    }
}

/// Load the ASN table if present, through `cache`. A missing table is not
/// an error — enrichment simply degrades. See [`AsnCache`] for why this
/// caches rather than re-reading the file on every call.
pub(crate) fn load_asn_table(cache: &AsnCache) -> Option<Arc<AsnTable>> {
    cache.load()
}

/// Process-wide single-flight guard: ensures at most one ASN refresh
/// spawned from a netflow read is ever in flight at a time.
///
/// `cp serve`'s gate-sweep tick (`rupu-cli/src/cmd/cp.rs`) has its own
/// process-wide `AtomicBool` guarding the SAME hazard for the SAME
/// resource (`AsnTable::write`'s fixed `<path>.db.tmp` intermediate file —
/// two concurrent writers race on it, a data race, not just wasted
/// bandwidth) — but that guard lives in `rupu-cli`, a downstream crate this
/// one (`rupu-cp`) cannot depend on, so it cannot be shared. A burst of CP
/// requests hitting a missing/stale table here must still collapse to one
/// download, hence this crate's own guard for its own trigger path.
#[derive(Debug, Default)]
pub struct RefreshGuard {
    running: AtomicBool,
}

impl RefreshGuard {
    /// `true` if the caller now owns the refresh and must call [`Self::finish`]
    /// when done — on BOTH the success and failure path, so a failed refresh
    /// never wedges the guard shut for the rest of the process's life.
    pub fn try_begin(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Release a claim taken by [`Self::try_begin`].
    pub fn finish(&self) {
        self.running.store(false, Ordering::Release);
    }
}

static ASN_REFRESH_GUARD: OnceLock<RefreshGuard> = OnceLock::new();

fn asn_refresh_guard() -> &'static RefreshGuard {
    ASN_REFRESH_GUARD.get_or_init(RefreshGuard::default)
}

/// Same generous bound as `cp serve`'s gate-sweep tick
/// (`rupu-cli/src/cmd/cp.rs`'s `ASN_REFRESH_TIMEOUT`): this is a
/// multi-megabyte download over an HTTP client that otherwise sets no
/// request timeout, so without a bound a stalled peer would hold the guard
/// (see [`RefreshGuard`]) for the life of the process — every future read
/// would see it held and silently stop triggering refreshes forever.
const ASN_REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// The backstop trigger: called from every netflow-read handler. If the
/// table is missing or stale AND `[netflow].asn_auto_refresh` is on, spawn
/// a refresh so the *next* request benefits — this one already answered
/// (or is about to) with whatever table it found, `asn_loaded: false` if
/// none. Never blocks the caller: the network request happens on a
/// detached `tokio::spawn`, not awaited here.
///
/// This is the backstop for operators who never run `cp serve` (whose
/// sweep loop is the primary trigger, per netflow Plan 2 Task 9) — a
/// read-triggered path exists precisely so enrichment can start working
/// without the operator running any command at all.
///
/// `cache` is invalidated (Fix 5) once a refresh completes successfully, so
/// the very next read observes the new table even if its mtime happened to
/// collide with the cached one's. A failed refresh does not invalidate —
/// the existing on-disk table (and cache) is still the best one available.
pub(crate) fn maybe_refresh_asn(cfg: &rupu_config::NetflowConfig, cache: &Arc<AsnCache>) {
    if !cfg.asn_auto_refresh {
        return;
    }
    let Some(db) = rupu_netflow::asn::asn_db_path() else {
        return;
    };
    if !rupu_netflow::asn::is_stale(&db, cfg.asn_refresh_interval_days) {
        return;
    }
    let guard = asn_refresh_guard();
    if !guard.try_begin() {
        return;
    }
    let url = cfg.asn_source_url.clone();
    let cache = Arc::clone(cache);
    tokio::spawn(async move {
        let ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::System);
        // `Arc::new(NullSink)`: this is `cp serve`'s own background ASN-table
        // download, not run-scoped traffic — the daemon's own egress is
        // deliberately not recorded (see `rupu-cli/src/cmd/cp.rs`'s deleted
        // `http::init` call).
        let client = match rupu_netflow::http::client_with(
            ctx,
            reqwest::Client::builder().timeout(ASN_REFRESH_TIMEOUT),
            Arc::new(rupu_netflow::NullSink),
        ) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "failed to build netflow ASN-refresh client");
                guard.finish();
                return;
            }
        };
        match rupu_netflow::asn::refresh(&url, &db, &client).await {
            Ok(()) => {
                cache.invalidate();
                tracing::info!(path = ?db, "netflow ASN table refreshed on demand");
            }
            Err(e) => tracing::warn!(
                error = %e,
                "on-demand netflow ASN refresh failed; keeping existing table"
            ),
        }
        guard.finish();
    });
}

/// Merge a run's ledger-scoped flows with the `Event::NetFlow` lines found
/// in the run's own transcript files, deduped by [`FlowId`].
///
/// This exists for exactly one reason now: `netflow_sink::for_run`
/// (rupu-cli) is a best-effort sink builder — a ledger file it cannot open
/// is not fatal ("capture must never break a run"), so that run's sink
/// degrades to transcript-only capture for the rest of its life. When that
/// happens, this run's ledger file was never created at all, so
/// `run_scoped_flows_and_dropped`'s ledger read for it returns nothing —
/// the run's own transcript, which the same sink always writes to
/// regardless of the ledger's fate, is the ONLY surviving record of that
/// run's network activity. This merge is what makes those flows reachable
/// at all. It is deliberately NOT here to recover `ctx.run_id: None` flows
/// from a shared ledger — with one ledger file per run, attribution is by
/// file, so those flows are already in the ledger read (see the module
/// doc).
///
/// On a normal run (ledger opened fine), every flow already exists in the
/// ledger; the merge is then a no-op union — nothing new is added, because
/// every transcript-side id the dedup below sees already has a ledger-side
/// entry.
///
/// On an id collision the LEDGER's copy wins: `ledger::read_flows` folds
/// any `LedgerLine::Complete` into it, so its `bytes_in`/`duration_ms` are
/// finalized, whereas the transcript's copy is only the snapshot taken at
/// the moment the flow was first recorded (a streaming flow is written to
/// the transcript before it completes). Transcript-only flows — every id
/// the ledger read never produced, whether because that run's ledger
/// degraded entirely or (rarer) one line failed to parse — are added
/// as-is; there is no more-authoritative copy of them anywhere.
///
/// A transcript file that fails to open/parse is skipped, not fatal — the
/// same "missing data degrades, never errors" contract as the ledger read.
fn merge_with_transcript(
    ledger_scoped: Vec<FlowRecord>,
    transcript_paths: &[PathBuf],
) -> Vec<FlowRecord> {
    let mut by_id: HashMap<FlowId, FlowRecord> = HashMap::new();
    for f in ledger_scoped {
        by_id.insert(f.id, f);
    }

    for path in transcript_paths {
        let Ok(events) = JsonlReader::iter(path) else {
            continue;
        };
        for event in events.flatten() {
            if let TranscriptEvent::NetFlow { flow } = event {
                by_id.entry(flow.id).or_insert(*flow);
            }
        }
    }

    by_id.into_values().collect()
}

// (`build_response` — the pre-explorer response builder — was deleted
// when `build_filtered_response` became the ONE builder every route uses,
// including the empty `Unpersisted` case. Two builders meant every
// contract change had to land twice, and the copy nothing served would be
// the one that got missed.)

/// Resolve every ledger file a given run/step id's flows could have
/// landed in: the workspace-local `<workspace>/.rupu/netflow/<id>.jsonl`
/// AND the global `<global_dir>/netflow/<id>.jsonl` fallback — BOTH, not
/// whichever exists first. Whole-branch review (Finding 5): `cp serve`
/// resumes a paused run by spawning a DETACHED command with the DAEMON's
/// own cwd, not the original invocation's; `resume.rs`/`workflow.rs`
/// derive `project_root` from that cwd via `project_root_for`. So the
/// SAME run id can legitimately end up with a ledger written to the
/// workspace-local root by its original dispatch and a SECOND, later
/// ledger written to the global root by a resumed step running under a
/// different cwd (or the reverse) — `<workspace>/.rupu/netflow/` existing
/// or not is a per-process observation, not a fixed property of the id.
/// Reading only "whichever exists" silently drops whichever wrote last.
/// Both reads already tolerate a missing file (`read_flows_and_dropped`
/// degrades to `([], 0)`) and dedupe by `FlowId` (`merge_with_transcript`),
/// so reading both unconditionally is safe — the only cost is one extra
/// open-attempt on a path that, in the common (non-split) case, doesn't
/// exist.
///
/// Deduped when both paths canonicalize to the SAME directory (the
/// `$HOME`/`~/.rupu` collision `read_all_workspaces_sync` also guards
/// against) so that case reads one file once, not twice — reading the
/// identical file under two different-looking paths would double-count
/// its flows and its `Dropped` line the same way the un-deduped global
/// union used to.
///
/// Reading both roots does widen an existing id-collision window: two
/// DIFFERENT projects given the same operator-supplied `--run-id` could
/// now have their same-named ledger files unioned together where before
/// only one would ever be read. Not a new hazard this change introduces
/// on its own — `read_all_workspaces_sync`'s global union already collides
/// operator-chosen ids across workspaces the same way — and impossible for
/// the generated `run_<ULID>` ids every real dispatch path mints.
fn resolve_ledger_paths(workspace: &StdPath, global_dir: &StdPath, id: &str) -> Vec<PathBuf> {
    let workspace_path = NetflowPaths::for_run(&project_local_netflow_dir(workspace), id).flows;
    let global_path = NetflowPaths::for_run(&global_netflow_dir(global_dir), id).flows;
    if canonicalize_or_self(&workspace_path) == canonicalize_or_self(&global_path) {
        vec![workspace_path]
    } else {
        vec![workspace_path, global_path]
    }
}

/// Every ledger-file id this run's own dispatch could have written to:
/// the run's own id (covers `Registry::discover`'s SCM sink and every
/// non-DAG-scheduled provider call made directly under this run's own
/// id, e.g. `rupu session`'s per-turn worker), PLUS every dispatched
/// step's — and fan-out item's — own freshly-minted id recorded in
/// `step_results.jsonl`, PLUS every sub-agent id this run (or one of its
/// sub-agents, transitively) dispatched via `dispatch_agent`. The
/// concurrent DAG scheduler mints a fresh run id per dispatched unit (see
/// `rupu-orchestrator`'s `runner.rs`, and `step_factory.rs`'s
/// `step_netflow_sink`, which is handed that id and builds THAT unit's
/// own `NetflowPaths::for_run`), so a step's provider flows AND its
/// ledger-only `Dropped` count live in their own file, never the parent
/// workflow run's. `dispatch_agent` (`rupu-cli`'s `cmd/dispatch.rs`) does
/// the identical thing by a different mechanism — CORRECTION: an earlier
/// version of this doc comment claimed sub-agent dispatch "reuses the
/// run's own id directly"; it does not and never has. `RunStore::
/// create_sub_run` mints its own fresh `sub_<ULID>` id, and `dispatch.rs`
/// hands THAT id (never the parent's) to `netflow_sink::for_run`, so a
/// sub-agent's flows and `Dropped` count land in their own ledger file
/// from the start — same as a dispatched step's. A `Dropped` line has no
/// transcript fallback (unlike a flow record, which the transcript merge
/// below can still recover under a different id) — skipping any of these
/// own-id ledger files here would silently under-report loss for any run
/// that dispatches more than one step or sub-agent, which is the common
/// case.
///
/// `StepResultRecord.run_id`/`ItemResultRecord.run_id` are `String::new()`
/// for `for_each`/`parallel`/panel STEP records themselves (only their
/// per-unit `items` carry a real id) and for a skipped item — filtered
/// out before dedup so `resolve_ledger_paths` is never asked to stat
/// `<dir>/.jsonl` (harmless today — that file never exists — but an empty
/// id in this list is never a valid ledger name and shouldn't linger).
///
/// Sub-agent ids come from [`RunStore::sub_run_ids_recursive`], which
/// walks the WHOLE dispatch tree (a sub-agent can itself dispatch
/// sub-agents up to `rupu-tools::dispatch_agent::MAX_DEPTH`), not just
/// the first level — before this, there was no way to discover a
/// sub-agent's id from its parent at all, so its ledger (and its
/// ledger-only `Dropped` count) was reachable only at global scope, with
/// no indication from the parent run's own netflow view that any
/// traffic — or any loss — was missing.
///
/// Critically, this recursion is applied to EVERY id already collected
/// above — the run's own id AND every step/item id — not just the run's
/// own id. `for_each`/`parallel`/panel each mint a fresh unit run id and
/// hand it down as `ToolContext.parent_run_id` for that unit's own agent
/// run (`step_factory.rs`), so a `dispatch_agent` call made from inside a
/// fan-out unit calls `create_sub_run(unit_id, ...)`, never
/// `create_sub_run(run_id, ...)` — its sub-run lives at
/// `<root>/<unit_id>/sub/`, a directory that recursing from `run_id`
/// alone would never visit. Anchoring the fold only at the top-level id
/// (an earlier version of this function did exactly that) would have
/// left a sub-agent dispatched from inside a fan-out unit exactly as
/// invisible as before this fix — the ids are snapshotted into a
/// `Vec` first specifically so the loop below can iterate them without
/// also iterating the ids `sub_run_ids_recursive` appends as it goes.
///
/// **Attribution decision**: sub-agent flows are FOLDED into the
/// dispatching run's view, the same way a dispatched step's already are
/// — not kept as a separate scope. Both are "this run's own dispatch,
/// each in its own file" (see `resolve_ledger_paths`'s doc), so both are
/// recovered the same way and neither can be told apart from a normal
/// direct provider call by inspecting the merged `flows` list alone
/// (`FlowCtx::agent`/`run_id` are unset on every production flow — see
/// `rupu_netflow::ctx::FlowCtx`'s doc). The operator-facing text in
/// `ScopeDisclosure.tsx` says so explicitly, so "this run's netflow"
/// is never silently read as "only what this run's own provider calls
/// did" when it also includes what every agent it dispatched did.
fn run_and_unit_ids(store: &RunStore, run_id: &str) -> Vec<String> {
    let mut ids = vec![run_id.to_string()];
    for record in store.read_step_results(run_id).unwrap_or_default() {
        ids.push(record.run_id);
        for item in record.items {
            ids.push(item.run_id);
        }
    }
    // Snapshot before extending: sub-agent recursion must run once PER id
    // already collected (the run itself and every step/fan-out-item id),
    // not just the run's own id — see the doc comment above.
    let base_ids = ids.clone();
    for id in &base_ids {
        if id.is_empty() {
            continue;
        }
        ids.extend(store.sub_run_ids_recursive(id));
    }
    ids.retain(|id| !id.is_empty());
    ids.sort();
    ids.dedup();
    ids
}

// ── Explorer attribution (run/workflow per ledger id) ───────────────────

/// Ledger-file id → (root run id, workflow name) for every run this
/// scope's stores know about, plus each run's lifetime span for the
/// timeline's active-runs strip.
///
/// This is how a flow read at project/global scope (tagged with the
/// ledger FILE's own id — a step/fan-out unit's or sub-agent's id at any
/// depth, not necessarily the root run's) folds back up to the run and
/// workflow the explorer displays. An id no store can account for maps to
/// `(None, None)` and the explorer groups it under the explicit `unknown`
/// workflow — never dropped (mirrors the unknown-org rule).
///
/// Cost: building this is the expensive walk of the netflow read path —
/// see [`project_run_meta`]'s doc for the per-run accounting. Project
/// scope builds it ONCE per request and derives the global-fallback id
/// set from it ([`project_scoped_flows_meta_and_dropped`]); global scope
/// does the same walk across EVERY registered workspace's store plus the
/// global one, so it is memoized on [`AppState`] instead — see
/// [`RunMetaCache`].
#[derive(Debug, Default)]
pub(crate) struct RunMetaIndex {
    map: HashMap<String, (String, String)>,
    spans: Vec<RunSpan>,
    /// Registered non-local hosts that in-scope runs actually EXECUTED on.
    ///
    /// A run placed on a remote host keeps its netflow ledger there. Run
    /// scope reads that host directly (see `worker_host_flows`), but
    /// project/global scope unions ledger DIRECTORIES on this machine and
    /// has no per-run fetch — so those flows are simply absent. Recording
    /// which hosts they belong to is what lets those scopes SAY so
    /// (`scope_gap_from_workers`) instead of serving a short total that
    /// looks complete.
    remote_workers: std::collections::BTreeSet<String>,
    /// Root run ids already folded in — the same run RECORD can appear in
    /// both the global store and a project-local one, and pushing its
    /// span twice would double-count it in the active-runs strip.
    seen_runs: std::collections::HashSet<String>,
}

impl RunMetaIndex {
    /// Fold one run record (looked up in ITS OWN store — see
    /// [`project_run_meta`]'s "same store each record was discovered
    /// from" rule) into the index. A run id already folded is skipped
    /// entirely (see `seen_runs`).
    fn insert_run(&mut self, store: &RunStore, record: &RunRecord) {
        if !self.seen_runs.insert(record.id.clone()) {
            return;
        }
        if let Some(worker) = record
            .worker_id
            .as_deref()
            .filter(|w| !w.is_empty() && *w != "local")
        {
            self.remote_workers.insert(worker.to_string());
        }
        for id in run_and_unit_ids(store, &record.id) {
            self.map
                .entry(id)
                .or_insert_with(|| (record.id.clone(), record.workflow_name.clone()));
        }
        self.spans.push(RunSpan {
            start: record.started_at,
            end: record.finished_at,
        });
    }

    fn attribution(&self, ledger_id: &str) -> (Option<String>, Option<String>) {
        match self.map.get(ledger_id) {
            Some((run, wf)) => (Some(run.clone()), Some(wf.clone())),
            None => (None, None),
        }
    }

    /// Every ledger-file id this index accounts for — the run/unit/
    /// sub-agent ids of every run folded in. At project scope this IS the
    /// project's own id set, which is what makes the index reusable as
    /// the global-fallback recovery pass's input
    /// ([`project_fallback_flows_and_dropped`]) instead of a second,
    /// identical walk producing the same ids.
    fn ledger_ids(&self) -> impl Iterator<Item = &String> {
        self.map.keys()
    }
}

/// [`RunMetaIndex`] over one project's own runs. Two sources, mirroring
/// `resolve_run_location`'s `Global`/`ProjectLocal` split: the global
/// store filtered to this workspace's own runs, plus a project-local
/// store rooted at `<workspace>/.rupu/runs` (whatever it holds belongs to
/// this project by construction — there is no `workspace_path` filter to
/// apply there).
///
/// This is the attribution-safety property the id-driven global-fallback
/// pass ([`project_fallback_flows_and_dropped`]) depends on: a run
/// belonging to some OTHER project never contributes an id here just
/// because its ledger happens to live in the same shared
/// `<global_dir>/netflow/` directory — only a run record that itself
/// declares THIS workspace as its own does.
///
/// Both sides of the `workspace_path` comparison go through
/// [`canonicalize_or_self`], not a bare `==`: `RunRecord::workspace_path`
/// is stamped canonicalized at run creation (`rupu-cli`'s
/// `canonicalize_if_exists`), but `workspace` here comes from the
/// registered `Workspace`'s own path, which this function has no control
/// over — comparing raw strings would silently miss a match across e.g. a
/// symlinked temp/mount path, the same hazard [`resolve_ledger_paths`] and
/// [`read_all_workspaces_sync`] already guard against for directory
/// identity.
///
/// [`RunMetaIndex::insert_run`] expands each record against the SAME
/// store the record was discovered from — never unconditionally against
/// `global_store` — so a project-local run's dispatched steps/units are
/// looked up in the project-local store's own `step_results.jsonl`, not
/// the global one's (which has no record of it at all, and
/// `unwrap_or_default()` would silently swallow that miss, resolving only
/// the parent run's own id and dropping every dispatched step's own
/// global-fallback ledger). Run scope ([`run_scoped_flows_and_dropped`])
/// already gets this right by constructing the correct store per branch;
/// this mirrors it.
///
/// Cost: `global_store.list()` is one `read_dir` plus one small
/// `run.json` parse per run IN THE WHOLE GLOBAL STORE (not filtered by
/// workspace at the I/O layer — `RunStore` has no index by
/// `workspace_path`), so this is O(total runs across every project) on
/// every project-scope request, not O(this project's own runs). At ~500
/// runs total that's ~500 small file opens. For each of this project's
/// own k run ids (k ≤ total runs) found in either store,
/// [`run_and_unit_ids`] then issues one `read_step_results` call (a small
/// `step_results.jsonl` read, `Ok(vec![])` if the run never dispatched a
/// step) plus one `sub_run_ids` `read_dir` per (run, step/item,
/// sub-agent-at-any-depth) id — bounded in practice by how deep/wide any
/// one run's dispatch tree actually grew (`MAX_SUB_RUN_RECURSION_DEPTH`
/// backstops the pathological case), not by the total run count. This
/// walk used to run TWICE per project request (once for the fallback id
/// set, once for attribution); it now runs once, with the fallback ids
/// derived from the index ([`RunMetaIndex::ledger_ids`]).
fn project_run_meta(global_store: &RunStore, workspace: &StdPath) -> RunMetaIndex {
    let canonical = canonicalize_or_self(workspace);
    let mut meta = RunMetaIndex::default();
    for r in global_store.list().unwrap_or_default() {
        if canonicalize_or_self(&r.workspace_path) == canonical {
            meta.insert_run(global_store, &r);
        }
    }
    let local_store = RunStore::new(canonical.join(".rupu").join("runs"));
    for r in local_store.list().unwrap_or_default() {
        meta.insert_run(&local_store, &r);
    }
    meta
}

/// The deduped, canonicalized project roots global scope enumerates —
/// shared by [`global_run_meta`] (which walks each root's local run
/// store) and [`global_store_fingerprint`] (which stats it) so the walk
/// and the cache key can never drift on WHICH stores "the global run-store
/// set" means. Dedup mirrors [`read_all_workspaces_sync`]'s ledger-dir
/// dedup: one physical store is never walked (or statted) twice, which
/// for the walk would double its spans in the active-runs strip.
fn global_meta_workspace_roots(global_dir: &StdPath) -> std::collections::HashSet<PathBuf> {
    (WorkspaceStore {
        root: global_dir.join("workspaces"),
    })
    .list()
    .unwrap_or_default()
    .iter()
    .map(|w| canonicalize_or_self(std::path::Path::new(&w.path)))
    .collect()
}

/// [`RunMetaIndex`] over every run any store knows about: the global
/// store plus each registered workspace's project-local store — the same
/// workspace enumeration [`read_all_workspaces_sync`] unions ledger
/// directories over, so a flow readable at global scope has its owning
/// run's record enumerated whenever that record exists at all.
///
/// Production requests reach this only through [`global_run_meta_cached`]
/// — see [`RunMetaCache`] for why the full walk stopped running on every
/// request.
fn global_run_meta(global_dir: &StdPath, global_store: &RunStore) -> RunMetaIndex {
    let mut meta = RunMetaIndex::default();
    for r in global_store.list().unwrap_or_default() {
        meta.insert_run(global_store, &r);
    }
    for root in global_meta_workspace_roots(global_dir) {
        let local_store = RunStore::new(root.join(".rupu").join("runs"));
        for r in local_store.list().unwrap_or_default() {
            meta.insert_run(&local_store, &r);
        }
    }
    meta
}

/// One run directory's cheap change stamp: its name plus the mtimes of
/// its `run.json` (rewritten on every status/span change — `write_atomic`
/// replaces the file, so the mtime always moves) and `step_results.jsonl`
/// (appended per completed step). `None` when the file doesn't exist —
/// distinct from "not statted".
type RunDirStamp = (
    std::ffi::OsString,
    Option<std::time::SystemTime>,
    Option<std::time::SystemTime>,
);

/// The whole global run-store set's change fingerprint, one entry per
/// store root (see [`global_meta_workspace_roots`]), each carrying a
/// sorted [`RunDirStamp`] list. Everything is sorted so equality is
/// order-independent of `read_dir`/`HashSet` iteration.
type GlobalStoreFingerprint = Vec<(PathBuf, Vec<RunDirStamp>)>;

/// Stamp one store root: one `read_dir` plus two `stat`s per run
/// directory — no file is opened, read, or parsed, which is the entire
/// point (compare [`project_run_meta`]'s cost accounting for what
/// the full walk does per run). A missing root stamps as an empty list,
/// matching `RunStore::list()`'s own missing-directory tolerance.
fn store_fingerprint(root: &StdPath) -> Vec<RunDirStamp> {
    let mut stamps = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return stamps;
    };
    let mtime = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        stamps.push((
            entry.file_name(),
            mtime(dir.join("run.json")),
            mtime(dir.join("step_results.jsonl")),
        ));
    }
    stamps.sort();
    stamps
}

/// Fingerprint every store [`global_run_meta`] would walk. Catches: a run
/// created/deleted/archived (directory entry appears/disappears), a
/// status or span change (`run.json` rewrite), a dispatched step or
/// fan-out unit completing (`step_results.jsonl` append), and a workspace
/// (de)registration (root list changes). Deliberately does NOT descend
/// into `sub/` trees — a sub-agent dispatched via `create_sub_run` writes
/// only under `<runs>/<parent>/sub/`, invisible to these stats. That gap
/// is closed by [`global_run_meta_cached`]'s unaccounted-ledger-id check,
/// not by statting deeper (which would rebuild the very walk this
/// fingerprint exists to avoid).
fn global_store_fingerprint(
    global_dir: &StdPath,
    global_store: &RunStore,
) -> GlobalStoreFingerprint {
    let mut roots: Vec<PathBuf> = global_meta_workspace_roots(global_dir)
        .into_iter()
        .map(|root| root.join(".rupu").join("runs"))
        .collect();
    roots.push(global_store.root.clone());
    roots.sort();
    roots.dedup();
    roots
        .into_iter()
        .map(|root| {
            let stamps = store_fingerprint(&root);
            (root, stamps)
        })
        .collect()
}

/// Process-wide memoization of the global-scope [`RunMetaIndex`], keyed
/// on [`global_store_fingerprint`].
///
/// Before this cache, [`global_run_meta`]'s full walk — every run record
/// in the global store plus every registered workspace's local store,
/// each expanded through [`run_and_unit_ids`] (a `step_results.jsonl`
/// read + recursive `sub/` `read_dir`s per run) — ran on EVERY
/// `/api/netflow` and `/api/netflow/explorer` request, twice per explorer
/// interaction (both endpoints build their own index). The fingerprint
/// reduces the steady-state cost to stats, and the index is rebuilt only
/// when a store actually changed.
///
/// **Honesty contract** (the part that keeps "unknown" attribution
/// honest): the fingerprint alone cannot see a freshly-created sub-run
/// (see [`global_store_fingerprint`]'s doc), so a cached index is only
/// served when every ledger-file id the current request actually read is
/// ACCOUNTED FOR — resolvable by the index, or already proven
/// unresolvable by a prior rebuild (`known_unknown`). An id that is
/// neither forces one rebuild before it can ever be reported as the
/// explicit `unknown` workflow; an id a rebuild still can't resolve (an
/// orphaned ledger whose run record is gone — permanently unresolvable
/// by construction, see [`project_fallback_flows_and_dropped`]'s closing
/// note) is remembered so it cannot silently degrade the cache to a
/// per-request full walk forever. `known_unknown` is reset whenever the
/// fingerprint changes — a store change is exactly the event that could
/// make a formerly-unknown id resolvable.
///
/// Lives on [`AppState`] rather than as a `static` for the same reason
/// [`AsnCache`] does: handlers only see it through `AppState`, and tests
/// construct a fresh `AppState` per case.
#[derive(Default)]
pub struct RunMetaCache {
    inner: std::sync::Mutex<Option<RunMetaCacheEntry>>,
}

struct RunMetaCacheEntry {
    fingerprint: GlobalStoreFingerprint,
    index: Arc<RunMetaIndex>,
    /// Ledger-file ids a rebuild at THIS fingerprint already failed to
    /// resolve — see the honesty contract in [`RunMetaCache`]'s doc.
    known_unknown: std::collections::HashSet<String>,
}

/// [`global_run_meta`] through [`RunMetaCache`]. `ledger_ids` is the
/// distinct set of ledger-file ids the caller's flow read actually
/// produced — the accounting universe for the honesty contract above.
///
/// The lock is deliberately held across the rebuild: every caller is
/// already on the blocking pool (`run_blocking`), and a burst of
/// concurrent requests against a changed store should collapse to ONE
/// walk with the rest reusing its result, not race N identical walks —
/// the same single-flight rationale as [`RefreshGuard`], solved here by
/// the mutex itself since (unlike the ASN download) the work is
/// synchronous.
fn global_run_meta_cached(
    cache: &RunMetaCache,
    global_dir: &StdPath,
    global_store: &RunStore,
    ledger_ids: &std::collections::HashSet<String>,
) -> Arc<RunMetaIndex> {
    let fingerprint = global_store_fingerprint(global_dir, global_store);
    let mut guard = cache.inner.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = guard.as_ref() {
        let accounted =
            |id: &String| entry.index.map.contains_key(id) || entry.known_unknown.contains(id);
        if entry.fingerprint == fingerprint && ledger_ids.iter().all(accounted) {
            return Arc::clone(&entry.index);
        }
    }
    let index = Arc::new(global_run_meta(global_dir, global_store));
    // Carry known-unknowns forward only while the fingerprint is stable
    // (kept from being re-proven on every request whose window happens to
    // exclude them); a changed fingerprint resets the set, since the
    // store change may be exactly what makes one resolvable.
    let mut known_unknown = match guard.take() {
        Some(entry) if entry.fingerprint == fingerprint => entry.known_unknown,
        _ => std::collections::HashSet::new(),
    };
    known_unknown.extend(ledger_ids.iter().cloned());
    known_unknown.retain(|id| !index.map.contains_key(id));
    *guard = Some(RunMetaCacheEntry {
        fingerprint,
        index: Arc::clone(&index),
        known_unknown,
    });
    index
}

/// Wrap ledger-tagged flows as [`ExplorerFlow`]s: run/workflow attribution
/// through `meta`, ASN through `table` (read-time enrichment, spec §6.2 —
/// the one place per scope it happens for the explorer path).
fn to_explorer_flows(
    tagged: Vec<(String, FlowRecord)>,
    meta: &RunMetaIndex,
    table: Option<&AsnTable>,
) -> Vec<ExplorerFlow> {
    tagged
        .into_iter()
        .map(|(src, flow)| {
            let (run_id, workflow) = meta.attribution(&src);
            let asn = flow.peer_ip.and_then(|ip| table.and_then(|t| t.lookup(ip)));
            ExplorerFlow {
                run_id,
                workflow,
                asn,
                flow,
            }
        })
        .collect()
}

/// Whether one already-enriched flow row passes the active cross-filters —
/// a thin delegate to [`ExplorerFilters::passes_parts`], the ONE
/// four-dimension predicate, so the table and the aggregate views above it
/// can never drift on matching rules or key alphabet.
fn flow_view_passes(v: &FlowView, filters: &ExplorerFilters) -> bool {
    filters.passes_parts(
        v.workflow.as_deref(),
        &v.flow.ctx.origin,
        v.asn.as_ref(),
        &v.flow.host,
        v.flow.port,
        None,
    )
}

/// [`build_response`]'s successor for the scoped flows-list routes: same
/// contract, plus per-flow run/workflow attribution (`meta`) and
/// server-side cross-filtering (`filters`). `hosts` is rolled up from the
/// RETAINED flows only, so the table and any rollup reader always
/// describe the same set; `dropped_total` stays whole-history and
/// `window` stays the server's own echo, both untouched by filtering.
pub(crate) fn build_filtered_response(
    tagged: Vec<(String, FlowRecord)>,
    meta: &RunMetaIndex,
    dropped: u64,
    table: Option<&AsnTable>,
    range: &rupu_netflow::ledger::TimeRange,
    filters: &ExplorerFilters,
) -> NetflowResponse {
    let mut views: Vec<FlowView> = tagged
        .into_iter()
        .map(|(src, f)| {
            let mut v = FlowView::from_flow(f, table);
            let (run_id, workflow) = meta.attribution(&src);
            v.run_id = run_id;
            v.workflow = workflow;
            v
        })
        .collect();
    views.retain(|v| flow_view_passes(v, filters));
    let hosts = rupu_netflow::ledger::host_rollup_iter(views.iter().map(|v| &v.flow));
    NetflowResponse {
        flows: views,
        hosts,
        dropped_total: dropped,
        window: WindowEcho::from(range),
        asn_loaded: table.is_some(),
        // Builders serve local data; a caller that also read a remote
        // source records any gap on the returned value.
        incomplete: Vec::new(),
    }
}

/// The ledger's run-scoped flows for `run_id` — unioned across every id
/// [`run_and_unit_ids`] names, each resolved through [`resolve_ledger_paths`]
/// — merged with the run's own transcript (see the module doc for why the
/// ledger alone under-reports), plus the summed dropped-count from those
/// same files. Synchronous — every caller runs this through
/// [`run_blocking`], never inline on the async task. Shared by
/// [`collect_run_netflow`] (which wraps this into the enriched
/// [`NetflowResponse`]) and the graph endpoint's `run:` scope (which only
/// needs raw [`FlowRecord`]s to feed [`rupu_netflow::ledger::graph_view`]).
///
/// `pub` because `rupu-cli`'s `netflow show` serves this same read to a
/// coordinator over ssh — a host with no generic-GET surface. One
/// implementation of the ledger+transcript merge, not two that can drift.
pub fn run_scoped_flows_and_dropped(
    store: &RunStore,
    run_id: &str,
    workspace: &StdPath,
    global_dir: &StdPath,
    range: &rupu_netflow::ledger::TimeRange,
) -> (Vec<FlowRecord>, u64) {
    run_scoped_flows_and_dropped_with(store, run_id, workspace, global_dir, range, Vec::new(), 0)
}

/// [`run_scoped_flows_and_dropped`] plus ledger flows read from somewhere
/// this machine cannot see — a run placed on a remote host writes its
/// ledger THERE, and only its transcript is mirrored back.
///
/// `extra_ledger_flows` joins the LEDGER side of the merge, not the result,
/// and that placement is the whole point: a mirrored transcript carries the
/// snapshot each flow had when it was first recorded, while the ledger
/// carries the finalized record (bytes, duration, completion). Appending
/// the remote flows afterwards would let the degraded local snapshot win on
/// an id collision; feeding them in as ledger flows makes the authoritative
/// copy win, exactly as a local ledger's would. See
/// [`merge_with_transcript`] for that precedence rule.
///
/// `extra_dropped` is added to the local dropped count — records lost on
/// the remote are lost just the same, and folding them to zero here would
/// claim a completeness we do not have.
pub fn run_scoped_flows_and_dropped_with(
    store: &RunStore,
    run_id: &str,
    workspace: &StdPath,
    global_dir: &StdPath,
    range: &rupu_netflow::ledger::TimeRange,
    extra_ledger_flows: Vec<FlowRecord>,
    extra_dropped: u64,
) -> (Vec<FlowRecord>, u64) {
    let mut all: Vec<FlowRecord> = extra_ledger_flows
        .into_iter()
        .filter(|f| range.contains(f.ts))
        .collect();
    let mut dropped = extra_dropped;
    for id in run_and_unit_ids(store, run_id) {
        for ledger_path in resolve_ledger_paths(workspace, global_dir, &id) {
            let (f, d) =
                rupu_netflow::ledger::read_flows_in_range(&ledger_path, range).unwrap_or_default();
            all.extend(f);
            dropped += d;
        }
    }
    let transcript_paths = crate::usage::run_transcript_paths(store, run_id);
    // `merge_with_transcript` can add flows the ledger read never saw at
    // all (the degraded-sink recovery case — see its own doc comment), so
    // filtering only the ledger side above is not enough: an out-of-window
    // transcript-only flow would otherwise leak back in unfiltered. Apply
    // `range` once more to the merged set so every surviving flow, from
    // EITHER source, satisfies the same window.
    let merged = merge_with_transcript(all, &transcript_paths)
        .into_iter()
        .filter(|f| range.contains(f.ts))
        .collect();
    (merged, dropped)
}

/// Read the ledger(s) + run transcript for a run whose artifacts live
/// under `workspace` (a project root; `global_dir` is the fallback root
/// `resolve_ledger_paths` also reads from when the workspace never got its
/// own `.rupu/netflow/` — see that function's doc) and whatever `store`
/// reports as this run's step transcript paths, scope to `run_id`, merge,
/// and build the response. A missing ledger is an empty result, not an
/// error. Synchronous — see [`run_scoped_flows_and_dropped`].
#[allow(clippy::too_many_arguments)]
fn collect_run_netflow(
    store: &RunStore,
    run_id: &str,
    workspace: &StdPath,
    global_dir: &StdPath,
    cache: &AsnCache,
    range: &rupu_netflow::ledger::TimeRange,
    filters: &ExplorerFilters,
    remote: (Vec<FlowRecord>, u64, Vec<IncompleteSource>),
) -> NetflowResponse {
    let (remote_flows, remote_dropped, incomplete) = remote;
    let (merged, dropped) = run_scoped_flows_and_dropped_with(
        store,
        run_id,
        workspace,
        global_dir,
        range,
        remote_flows,
        remote_dropped,
    );
    // Every flow at run scope folds into THIS run (the whole point of
    // `run_and_unit_ids`), so attribution is the run's own record; a
    // record that fails to load leaves run/workflow unattributed rather
    // than failing the read (the ledger may still be perfectly readable).
    let mut meta = RunMetaIndex::default();
    if let Ok(record) = store.load(run_id) {
        meta.insert_run(store, &record);
    }
    let table = load_asn_table(cache);
    let tagged: Vec<(String, FlowRecord)> = merged
        .into_iter()
        .map(|f| (run_id.to_string(), f))
        .collect();
    let mut resp =
        build_filtered_response(tagged, &meta, dropped, table.as_deref(), range, filters);
    resp.incomplete = incomplete;
    resp
}

/// A registered workspace store, rooted at `<global_dir>/workspaces/` —
/// mirrors `coverage.rs`'s `store()`, the established pattern for
/// enumerating every project a global netflow view must union over.
fn workspace_store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

/// Resolve a `:id` path param (a `WorkspaceStore` id, e.g. `ws_...`) to its
/// registered workspace path. 404 when no such workspace is registered.
///
/// Not run through [`run_blocking`]: this is a single small `<id>.toml`
/// lookup in the workspace registry, not a ledger — it doesn't grow
/// unbounded the way a netflow ledger does, so it doesn't carry the same
/// stall risk the module doc's "Blocking I/O" note is about (existing
/// call sites for the same registry, e.g. `coverage.rs`/`projects.rs`,
/// make the same call inline for the same reason).
pub(crate) fn workspace_for_project(s: &AppState, project_id: &str) -> ApiResult<PathBuf> {
    let ws = workspace_store(s)
        .load(project_id)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("project {project_id} not found")))?;
    Ok(PathBuf::from(ws.path))
}

/// Union every per-run ledger file (`NetflowPaths::for_run` writes one
/// `<run_id>.jsonl` per run) under `netflow_dir` into one flow list +
/// summed dropped-count. A directory that doesn't exist yet (no run has
/// ever written a ledger there) degrades to `([], 0)`, the same "missing
/// data" tolerance every other read in this module already relies on.
///
/// Which files count as a per-run ledger (accepts `*.jsonl`, rejects
/// the `.gitignore` `NetflowPaths::ensure_dir` drops into every such
/// directory and the pre-plan shared-ledger file literally named
/// `flows.jsonl`) is decided in exactly one place,
/// `rupu_netflow::ledger::is_per_run_ledger_path` — see its doc comment
/// for the reasoning. `rupu-cli`'s `netflow prune` calls the same
/// function so the read side and the destructive prune side can never
/// drift apart on what a "ledger" is.
fn read_all_run_ledgers_in_dir(
    netflow_dir: &StdPath,
    range: &rupu_netflow::ledger::TimeRange,
) -> (Vec<(String, FlowRecord)>, u64) {
    let mut flows = Vec::new();
    let mut dropped = 0u64;
    let Ok(entries) = std::fs::read_dir(netflow_dir) else {
        return (flows, dropped);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_per_run_ledger_path(&path) {
            continue;
        }
        // `NetflowPaths::for_run` always names a ledger `<id>.jsonl`, so
        // the file stem IS the owning run/step id -- this is the "the
        // read side already knows which file each flow came from" the
        // whole-branch review named as the fix for `graph_view`'s
        // single-node collapse. A file whose stem somehow isn't valid
        // UTF-8 is skipped rather than guessed at.
        let Some(run_id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let (f, d) = rupu_netflow::ledger::read_flows_in_range(&path, range).unwrap_or_default();
        flows.extend(f.into_iter().map(|flow| (run_id.to_string(), flow)));
        dropped += d;
    }
    (flows, dropped)
}

/// The id-driven recovery pass: every flow in `<global_dir>/netflow/`
/// belonging to one of THIS project's own runs (or their dispatched
/// steps/units — see [`run_and_unit_ids`]), for projects whose
/// `.rupu/netflow/` didn't exist at write time so their ledgers fell back
/// to the global root (the common case — `rupu init` only started
/// creating that directory once `crates/rupu-cli/src/cmd/init.rs`'s
/// `ensure_netflow_dir` landed; every project initialised before that, or
/// never `rupu init`'d at all, still has every historical run's ledger
/// sitting in the global root with no way to re-run `init` after the
/// fact).
///
/// Deliberately narrower than [`resolve_ledger_paths`] (which also
/// resolves the workspace-local candidate): the workspace-local root is
/// already covered by the whole-directory scan
/// ([`read_all_run_ledgers_in_dir`]) the caller runs separately, so this
/// function only ever computes the GLOBAL path for each id — resolving
/// the workspace-local one here too would re-read files the whole-
/// directory scan already read, double-counting them (no `FlowId` dedup
/// exists at project/global scope, unlike run scope's
/// `merge_with_transcript`).
///
/// Paths are collected into a `HashSet` before any read, so an id that
/// resolves to the same global path more than once (shouldn't happen in
/// practice — ids are unique per run/unit — but costs nothing to guard)
/// is still only opened once.
///
/// The id set comes from an already-built [`RunMetaIndex`] over this
/// project's own runs ([`RunMetaIndex::ledger_ids`]) — the same walk that
/// produces attribution, done once by the caller, rather than a second
/// identical `RunStore` enumeration of its own (which is exactly what
/// this function used to do). Cost, continued from [`project_run_meta`]'s
/// doc comment (which accounts for the walk that produces the id set):
/// one `read_flows_in_range` per distinct resolved global path on top of
/// that. This scales with the TOTAL run count in the global store (the
/// `workspace_path` filter inside `project_run_meta`) and with this
/// project's own run/unit count, never with the size of the global
/// directory itself — the opposite scaling from a `read_dir` over that
/// directory, which is what made a directory-scan-based fix unworkable
/// here (a project has no way to tell, from the global directory's
/// contents alone, which files are its own — see the removed KNOWN GAP
/// note this function replaces). All synchronous, but always run through
/// [`run_blocking`] by the caller, same as every other read in this
/// module, so it never stalls the async task.
///
/// Two things this can NEVER recover, by construction, not by omission:
/// a `system`-origin flow with `ctx.run_id: None` has no run record to be
/// enumerated from at all, so it stays correctly invisible at project
/// scope (it belongs only at global scope); and a ledger whose owning
/// run's record is gone from BOTH stores' active listings — deleted, or
/// merely archived (`RunStore::list()` reads only the active `runs/`
/// directory, never `runs-archive/`; run scope has the identical blind
/// spot, so this isn't a new asymmetry) — contributes no id here either.
/// That ledger is an orphan, and staying unreachable is the correct
/// outcome for a deleted run, not a bug to chase; an archived one is a
/// narrower, accepted gap in the same direction.
fn project_fallback_flows_and_dropped(
    meta: &RunMetaIndex,
    global_dir: &StdPath,
    range: &rupu_netflow::ledger::TimeRange,
) -> (Vec<(String, FlowRecord)>, u64) {
    let mut paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for id in meta.ledger_ids() {
        paths.insert(NetflowPaths::for_run(&global_netflow_dir(global_dir), id).flows);
    }

    let mut flows = Vec::new();
    let mut dropped = 0u64;
    for path in paths {
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let (f, d) = rupu_netflow::ledger::read_flows_in_range(&path, range).unwrap_or_default();
        flows.extend(f.into_iter().map(|flow| (id.to_string(), flow)));
        dropped += d;
    }
    (flows, dropped)
}

/// Every flow at project scope — plus the project's own [`RunMetaIndex`]:
/// the workspace-local ledger directory (whole-directory scan,
/// [`read_all_run_ledgers_in_dir`]) UNIONED with the id-driven
/// global-fallback recovery pass ([`project_fallback_flows_and_dropped`])
/// for runs whose ledgers landed in `<global_dir>/netflow/` instead — see
/// that function's doc for the full cost accounting and what it
/// deliberately cannot recover. Shared by [`get_project_netflow`], the
/// project explorer scope, and the graph endpoint's `project:` scope so
/// the table, the aggregates, and the graph are always built from the
/// identical set, mirroring [`run_scoped_flows_and_dropped`]'s equivalent
/// role at run scope.
///
/// Returns the index it built alongside the flows because the ONE record
/// walk ([`project_run_meta`]) serves both needs: the fallback pass's id
/// set and the response's attribution/spans. Callers that need no
/// attribution (the graph endpoint) simply drop it — the walk is not
/// avoidable for them anyway, since the fallback pass requires the id
/// set regardless.
///
/// Guards the same `$HOME`/`RUPU_HOME` collision [`resolve_ledger_paths`]
/// guards at run scope: when the workspace-local netflow dir and the
/// global one are literally the same directory (a workspace registered at
/// `$HOME` with the default `RUPU_HOME=~/.rupu`), the whole-directory scan
/// above has already read every file in it, so the id-driven pass is
/// skipped entirely rather than re-reading (and double-counting) that
/// same directory a second time.
fn project_scoped_flows_meta_and_dropped(
    global_store: &RunStore,
    workspace: &StdPath,
    global_dir: &StdPath,
    range: &rupu_netflow::ledger::TimeRange,
) -> (Vec<(String, FlowRecord)>, RunMetaIndex, u64) {
    let meta = project_run_meta(global_store, workspace);
    let local_dir = project_local_netflow_dir(workspace);
    let (mut flows, mut dropped) = read_all_run_ledgers_in_dir(&local_dir, range);

    if canonicalize_or_self(&local_dir) == canonicalize_or_self(&global_netflow_dir(global_dir)) {
        return (flows, meta, dropped);
    }

    let (fallback_flows, fallback_dropped) =
        project_fallback_flows_and_dropped(&meta, global_dir, range);
    flows.extend(fallback_flows);
    dropped += fallback_dropped;
    (flows, meta, dropped)
}

/// Every flow across every registered workspace's ledger directory, PLUS
/// `<global_dir>/netflow/` — the fallback root a run's ledger lands in
/// when its workspace has no `.rupu/netflow/` of its own (see
/// `rupu_netflow::netflow_dir`'s doc comment: `rupu init` now creates
/// that directory for a NEW project, but every project initialised
/// before that change, or never `rupu init`'d at all, still has every
/// historical run's ledger sitting here regardless of which project it
/// belongs to — and there is no way to retroactively route those old
/// ledgers back to a project-local directory short of re-running `init`).
/// Without this union, global scope would show nothing for that common
/// case — the same "recorded, then permanently unreachable" defect a
/// prior arc already had to fix once for the (now-removed) CP-daemon-wide
/// ledger.
///
/// Directories are DEDUPED (canonicalized, collected into a `HashSet`)
/// before any of them is read: a workspace registered at `$HOME` and the
/// default `RUPU_HOME=~/.rupu` name the SAME `~/.rupu/netflow/`
/// directory, and `rupu run` executed from `$HOME` produces exactly that
/// (`cmd/run.rs` upserts `pwd` as a workspace; `project_root_for` walks
/// up and matches `~/.rupu`). Without the dedup, that directory would be
/// read twice, every flow in it would be double-counted with no `FlowId`
/// dedup at this scope (unlike run scope's `merge_with_transcript`), and
/// `host_rollup`'s byte/count totals plus `dropped` would silently double
/// — a doubled egress total is worse than a missing one.
///
/// The summed dropped-count follows the same deduped union. Mirrors
/// `coverage.rs`'s `list_coverage`: a workspace whose path is
/// gone/unreadable is skipped (its ledger read degrades to empty via
/// [`read_all_run_ledgers_in_dir`]'s own missing-directory tolerance),
/// never a hard error.
///
/// Synchronous — always run through [`run_blocking`] (this is the read that
/// scales with the number of registered workspaces AND with the number of
/// runs each one has, so it's the one most worth keeping off the async
/// task).
fn read_all_workspaces_sync(
    global_dir: &StdPath,
    range: &rupu_netflow::ledger::TimeRange,
) -> (Vec<(String, FlowRecord)>, u64) {
    let workspaces = (WorkspaceStore {
        root: global_dir.join("workspaces"),
    })
    .list()
    .unwrap_or_default();

    let mut dirs: std::collections::HashSet<PathBuf> = workspaces
        .iter()
        .map(|w| canonicalize_or_self(&project_local_netflow_dir(std::path::Path::new(&w.path))))
        .collect();
    dirs.insert(canonicalize_or_self(&global_netflow_dir(global_dir)));

    let mut flows = Vec::new();
    let mut dropped = 0u64;
    for dir in &dirs {
        let (f, d) = read_all_run_ledgers_in_dir(dir, range);
        flows.extend(f);
        dropped += d;
    }

    (flows, dropped)
}

/// Canonicalize `path` for directory-identity comparisons (resolving
/// symlinks and `..`/`.` components so two different-looking paths that
/// name the SAME directory — e.g. a workspace registered at `$HOME` and
/// the global netflow root both resolving under the default
/// `RUPU_HOME=~/.rupu` — dedup correctly in a `HashSet`). Falls back to
/// the path as given when it doesn't exist yet: `canonicalize` requires
/// the path to exist, but a directory nobody has ever written a ledger to
/// can't collide with anything real either, so using it verbatim as the
/// dedup key is safe (worst case, two distinct nonexistent paths that
/// happen to be the same directory both get read — each yields `([], 0)`
/// per [`read_all_run_ledgers_in_dir`]'s own tolerance, so nothing is
/// double-counted, just a wasted `read_dir` call).
fn canonicalize_or_self(path: &StdPath) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The scope-level gap a project/global netflow view must declare.
///
/// These scopes union ledger DIRECTORIES on this machine; they have no
/// per-run remote fetch (run scope does — see `worker_host_flows`). So every
/// flow made by a run that executed on a remote host is absent from the
/// totals, and absent in a way no number on the page reveals. Naming the
/// hosts is the difference between an understated total and a knowingly
/// understated one.
///
/// Empty when no in-scope run executed remotely — the common single-machine
/// case declares nothing, so this never becomes a banner people learn to
/// ignore.
fn scope_gap_from_workers(meta: &RunMetaIndex) -> Vec<IncompleteSource> {
    meta.remote_workers
        .iter()
        .map(|host_id| IncompleteSource {
            host_id: host_id.clone(),
            reason: "runs in this scope executed on this host, and their flows are recorded \
                     there — this scope reads only local ledgers, so they are not included. \
                     Open one of those runs to see its own flows."
                .to_string(),
        })
        .collect()
}

/// `GET /api/projects/:id/netflow` — every flow attributable to a
/// project: its own `.rupu/netflow/` ledger directory (whole-directory
/// scan, including unattributed `run_id: None` egress that has no run to
/// attach to — e.g. `Origin::Scm` traffic from `Registry::discover`, NOT
/// `Origin::System`; see `ScopeDisclosure.tsx`'s header comment for why
/// that's a separate, unrecorded-at-any-scope case, and NOT the update
/// checker either, which despite also using `Origin::Update` reaches no
/// ledger at all — `rupu-update`'s client is wired to `Arc::new(NullSink)`)
/// UNIONED with an id-driven recovery pass over `<global_dir>/netflow/`
/// for this project's own runs whose ledgers landed there instead — see
/// [`project_scoped_flows_meta_and_dropped`]'s doc comment for the full
/// mechanism, cost accounting, and what it deliberately cannot recover
/// (an unattributable `run_id: None` global-scope flow, or an orphaned
/// ledger whose run record is gone).
async fn get_project_netflow(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(q): Query<TimeRangeQuery>,
) -> ApiResult<Json<NetflowResponse>> {
    maybe_refresh_asn(&netflow_config(&state), &state.asn_cache);
    let range = parse_time_range(&q.from, &q.to)?;
    let filters = parse_filters(&q.workflow, &q.origin, &q.org, &q.host);
    let workspace = workspace_for_project(&state, &project_id)?;
    let cache = Arc::clone(&state.asn_cache);
    let store = Arc::clone(&state.run_store);
    let global_dir = state.global_dir.clone();
    let resp = run_blocking(move || {
        let (flows, meta, dropped) =
            project_scoped_flows_meta_and_dropped(&store, &workspace, &global_dir, &range);
        let table = load_asn_table(&cache);
        let mut resp =
            build_filtered_response(flows, &meta, dropped, table.as_deref(), &range, &filters);
        resp.incomplete = scope_gap_from_workers(&meta);
        resp
    })
    .await?;
    Ok(Json(resp))
}

/// `GET /api/netflow` — the union across every registered workspace.
async fn get_global_netflow(
    State(state): State<AppState>,
    Query(q): Query<TimeRangeQuery>,
) -> ApiResult<Json<NetflowResponse>> {
    maybe_refresh_asn(&netflow_config(&state), &state.asn_cache);
    let range = parse_time_range(&q.from, &q.to)?;
    let filters = parse_filters(&q.workflow, &q.origin, &q.org, &q.host);
    let global_dir = state.global_dir.clone();
    let store = Arc::clone(&state.run_store);
    let cache = Arc::clone(&state.asn_cache);
    let meta_cache = Arc::clone(&state.run_meta_cache);
    let resp = run_blocking(move || {
        let (flows, dropped) = read_all_workspaces_sync(&global_dir, &range);
        let ledger_ids = flows.iter().map(|(id, _)| id.clone()).collect();
        let meta = global_run_meta_cached(&meta_cache, &global_dir, &store, &ledger_ids);
        let table = load_asn_table(&cache);
        let mut resp =
            build_filtered_response(flows, &meta, dropped, table.as_deref(), &range, &filters);
        resp.incomplete = scope_gap_from_workers(&meta);
        resp
    })
    .await?;
    Ok(Json(resp))
}

#[derive(Debug, Deserialize)]
pub struct GraphQuery {
    /// `run:<id>`, `project:<id>`, or absent for global. An unrecognized
    /// prefix (or any other value that isn't `run:`/`project:`) is NOT a
    /// 400 — it falls through to the same global branch as an absent
    /// `scope`, matching the brief's own catch-all for this endpoint. This
    /// is a deliberate permissive default, not an unhandled error path.
    pub scope: Option<String>,
    /// Same contract as [`TimeRangeQuery`]'s fields, duplicated here rather
    /// than composed via `#[serde(flatten)]`: `serde_urlencoded` (which
    /// Axum's `Query` extractor uses) does not reliably support flattening
    /// a nested struct out of a query string, so this endpoint carries its
    /// own `from`/`to` and parses them through the same
    /// [`parse_time_range`] the other three routes use.
    pub from: Option<String>,
    pub to: Option<String>,
}

/// `?from=`/`?to=` query parameters accepted by every netflow-read route
/// (`/api/runs/:id/netflow`, `/api/projects/:id/netflow`, `/api/netflow`,
/// and — via its own duplicated `from`/`to` fields, see [`GraphQuery`]'s
/// doc comment for why they're not composed from this struct —
/// `/api/netflow/graph`).
///
/// Both are RFC 3339 timestamps (e.g. `2026-08-17T14:00:00Z`), each
/// independently optional, and BOTH BOUNDS INCLUSIVE — see
/// `rupu_netflow::ledger::TimeRange`'s doc comment for the full contract:
/// which timestamp is filtered (`FlowRecord::ts`, stamped at response-header
/// time, not request start or body completion), and why `from > to` is an
/// empty window rather than a rejected request.
///
/// Kept as raw `Option<String>` rather than `Option<DateTime<Utc>>` so a
/// malformed value produces the NAMED 400 built by [`parse_time_range`]
/// instead of Axum's generic query-deserialization-failure 400 (which would
/// not name the accepted format or which parameter was at fault).
#[derive(Debug, Default, Deserialize)]
pub struct TimeRangeQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    /// Cross-filter params for the explorer's table (and the explorer
    /// endpoint itself) — each a comma-separated list of dimension KEYS
    /// (`ExplorerFlow`'s `workflow_key`/`origin_key`/`org_key`/
    /// `endpoint_key` respectively: workflow names or `unknown`;
    /// `provider:<name>`/`scm:<name>`; `as<number>`/`unknown`;
    /// `host:port`). Empty/absent = that dimension unfiltered. Comma is
    /// safe as a separator: none of the key alphabets contain one
    /// (workflow names are filename stems).
    pub workflow: Option<String>,
    pub origin: Option<String>,
    pub org: Option<String>,
    pub host: Option<String>,
}

/// Split one comma-separated filter param into its keys, dropping empty
/// segments (`?workflow=` and `?workflow=a,,b` behave as expected).
fn split_filter(raw: &Option<String>) -> Vec<String> {
    raw.as_deref()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Shared by [`TimeRangeQuery`] (the three flows-list routes) and
/// [`ExplorerQuery`] — the two carry the same four filter params but can't
/// share a struct (`serde_urlencoded` flatten limitation, see
/// [`GraphQuery`]'s doc comment).
fn parse_filters(
    workflow: &Option<String>,
    origin: &Option<String>,
    org: &Option<String>,
    host: &Option<String>,
) -> ExplorerFilters {
    ExplorerFilters {
        workflows: split_filter(workflow),
        origins: split_filter(origin),
        orgs: split_filter(org),
        hosts: split_filter(host),
    }
}

/// Parse `from`/`to` (see [`TimeRangeQuery`]) into a
/// `rupu_netflow::ledger::TimeRange`, independently for each side.
///
/// A PRESENT-but-unparseable value is a 400 naming the offending parameter
/// and the accepted format — NEVER a silent fall-back to unbounded.
/// Silently ignoring an unparseable filter would show the caller MORE data
/// than they asked for while the response still looks like their filter was
/// applied — worse than an error, because nothing signals it happened.
fn parse_time_range(
    from: &Option<String>,
    to: &Option<String>,
) -> ApiResult<rupu_netflow::ledger::TimeRange> {
    fn parse_one(
        name: &str,
        raw: &Option<String>,
    ) -> ApiResult<Option<chrono::DateTime<chrono::Utc>>> {
        let Some(s) = raw else {
            return Ok(None);
        };
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| Some(dt.with_timezone(&chrono::Utc)))
            .map_err(|e| {
                ApiError::bad_request(format!(
                    "invalid `{name}` query parameter {s:?}: expected an RFC 3339 timestamp \
                     (e.g. \"2026-08-17T14:00:00Z\") — {e}"
                ))
            })
    }
    Ok(rupu_netflow::ledger::TimeRange {
        from: parse_one("from", from)?,
        to: parse_one("to", to)?,
    })
}

/// Raw (unenriched) flows for the graph endpoint's `run:` scope — mirrors
/// `get_run_netflow`'s dispatch on [`resolve_run_location`] so a run graph
/// looks in exactly the same places the run's own netflow page does
/// (global store / project-local store / remote host / unpersisted / 404).
/// The `Global`/`ProjectLocal` branches read through [`run_blocking`]; the
/// `Host` branch is a network proxy call, not disk I/O, so it stays inline.
///
/// Every flow is tagged with THIS run's own `run_id`, not the finer id of
/// whichever per-step (or per-dispatched-sub-agent) ledger file it
/// actually came from (`run_and_unit_ids`/`resolve_ledger_paths` may read
/// several) — at run scope the graph is meant to answer "what did THIS
/// run reach", so it shows one source node for the run, not a fragmented
/// node per internally-dispatched step or sub-agent. Project/global scope
/// go the other way (one node per contributing run) via
/// `read_all_run_ledgers_in_dir`.
async fn run_scoped_flows_for_graph(
    s: &AppState,
    run_id: &str,
    range: &rupu_netflow::ledger::TimeRange,
) -> ApiResult<Vec<(String, FlowRecord)>> {
    let tag = |flows: Vec<FlowRecord>| flows.into_iter().map(|f| (run_id.to_string(), f)).collect();
    match resolve_run_location(s, run_id).await {
        RunLocation::Global => {
            let run = s
                .run_store
                .load(run_id)
                .map_err(|e| run_not_found_or_internal(run_id, e))?;
            let store = Arc::clone(&s.run_store);
            let rid = run_id.to_string();
            let workspace = run.workspace_path.clone();
            let global_dir = s.global_dir.clone();
            let range = range.clone();
            let flows = run_blocking(move || {
                run_scoped_flows_and_dropped(&store, &rid, &workspace, &global_dir, &range).0
            })
            .await?;
            Ok(tag(flows))
        }
        RunLocation::ProjectLocal { path } => {
            let rid = run_id.to_string();
            let global_dir = s.global_dir.clone();
            let range = range.clone();
            let flows = run_blocking(move || {
                let store = RunStore::new(path.join(".rupu").join("runs"));
                run_scoped_flows_and_dropped(&store, &rid, &path, &global_dir, &range).0
            })
            .await?;
            Ok(tag(flows))
        }
        RunLocation::Host { host_id } => {
            let resp =
                run_netflow_from_host(s, &host_id, run_id, range, &ExplorerFilters::default())
                    .await?;
            Ok(tag(resp.flows.into_iter().map(|v| v.flow).collect()))
        }
        // No artifacts anywhere to build a graph from — empty, not an
        // error, mirroring `get_run_netflow`'s `Unpersisted` branch.
        RunLocation::Unpersisted { .. } => Ok(Vec::new()),
        RunLocation::NotFound => Err(ApiError::not_found(format!("run {run_id} not found"))),
    }
}

/// `GET /api/netflow/graph?scope=&from=&to=` — the bipartite
/// source↔endpoint graph (`rupu_netflow::ledger::graph_view` does the
/// actual bipartite build; this only resolves `scope` to the (run id, flow)
/// set it runs over, after filtering that set to `from`/`to` — see
/// [`TimeRangeQuery`]). Filtering happens BEFORE `graph_view` so nodes/edges
/// for an out-of-window flow never appear at all, keeping the graph and the
/// table (`get_run_netflow`/`get_project_netflow`/`get_global_netflow`)
/// derived from the identical filtered set for the same window.
async fn get_netflow_graph(
    State(state): State<AppState>,
    Query(q): Query<GraphQuery>,
) -> ApiResult<Json<rupu_netflow::ledger::GraphView>> {
    let range = parse_time_range(&q.from, &q.to)?;
    let flows = if let Some(run_id) = q.scope.as_deref().and_then(|s| s.strip_prefix("run:")) {
        run_scoped_flows_for_graph(&state, run_id, &range).await?
    } else if let Some(project_id) = q.scope.as_deref().and_then(|s| s.strip_prefix("project:")) {
        let workspace = workspace_for_project(&state, project_id)?;
        let store = Arc::clone(&state.run_store);
        let global_dir = state.global_dir.clone();
        // Shares `project_scoped_flows_meta_and_dropped` with
        // `get_project_netflow` so the graph and the table are always
        // built from the identical set, including the id-driven
        // global-fallback recovery pass — see that function's doc comment
        // (including why the unused meta index costs this caller nothing
        // extra).
        run_blocking(move || {
            project_scoped_flows_meta_and_dropped(&store, &workspace, &global_dir, &range).0
        })
        .await?
    } else {
        let global_dir = state.global_dir.clone();
        run_blocking(move || read_all_workspaces_sync(&global_dir, &range).0).await?
    };
    Ok(Json(rupu_netflow::ledger::graph_view(&flows)))
}

// ── Explorer endpoint (GET /api/netflow/explorer) ───────────────────────

/// Same shape as [`GraphQuery`] plus the four cross-filter params — its
/// own struct rather than `#[serde(flatten)]` composition for the same
/// `serde_urlencoded` reason documented there.
#[derive(Debug, Deserialize)]
pub struct ExplorerQuery {
    /// `run:<id>`, `project:<id>`, or absent for global — same permissive
    /// fallthrough as [`GraphQuery::scope`].
    pub scope: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Same contract as [`TimeRangeQuery`]'s filter params.
    pub workflow: Option<String>,
    pub origin: Option<String>,
    pub org: Option<String>,
    pub host: Option<String>,
}

/// Everything the explorer surface renders, computed server-side in one
/// request (see `rupu_netflow::ledger::explorer`'s module doc for the
/// per-view filter/window semantics). `Deserialize` for the same
/// proxying reason as [`FlowView`].
/// Deliberate deviation from the implementation brief: no `hosts` rollup
/// field. The brief listed one, but every per-endpoint number the surface
/// renders (org cards, lane stats) comes from `timeline.lanes` — whose
/// facet semantics (filters-minus-host) are the ones the UI needs — and a
/// second rollup under *different* semantics (all filters) that nothing
/// reads would be dead payload waiting to contradict the lanes beside it.
/// The flows-list routes still carry their consumed `hosts` rollup.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExplorerResponse {
    pub sankey: SankeyView,
    pub timeline: TimelineView,
    pub histogram: HistogramView,
    pub kpis: KpiView,
    /// Whole-history, never window- or filter-scoped — see
    /// [`NetflowResponse::dropped_total`].
    pub dropped_total: u64,
    pub asn_loaded: bool,
    /// The applied `?from=`/`?to=` — see [`WindowEcho`]. The histogram's
    /// own bounds are deliberately NOT this (full retained range).
    pub window: WindowEcho,
    /// See [`NetflowResponse::incomplete`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incomplete: Vec<IncompleteSource>,
}

/// Assemble every explorer view from one scope's flows. `scope_flows` is
/// the UNWINDOWED, UNFILTERED in-scope set — each view applies exactly
/// the narrowing its own semantics call for (histogram: none; sankey
/// nodes: window + filters-minus-own-dimension over a scope-wide
/// universe; timeline lanes: window + filters-minus-host; links / KPIs:
/// window + all filters).
pub(crate) fn build_explorer_response(
    scope_flows: &[ExplorerFlow],
    dropped: u64,
    spans: &[RunSpan],
    asn_loaded: bool,
    range: &rupu_netflow::ledger::TimeRange,
    filters: &ExplorerFilters,
) -> ExplorerResponse {
    let histogram = explorer::histogram_view(scope_flows, EXPLORER_BUCKETS);
    // The timeline needs concrete bounds: each side takes the applied
    // window's bound when present, else the histogram's full-retained-
    // range bound, else (empty scope, unbounded request) a deterministic
    // epoch fallback. A requested bound is NEVER widened to reach the
    // data — if the resolved sides cross (the window lies entirely
    // outside the retained range), `timeline_view` treats that as the
    // empty window it is, so the lanes can never show flows the KPI
    // strip (built from the raw request range) reports as absent.
    let epoch = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;
    let from = range.from.or(histogram.from).unwrap_or(epoch);
    let to = range.to.or(histogram.to).unwrap_or(from);
    let timeline = explorer::timeline_view(scope_flows, from, to, filters, spans, EXPLORER_BUCKETS);
    let sankey = explorer::sankey_view(scope_flows, range, filters);
    let kpis = explorer::kpi_view(
        scope_flows
            .iter()
            .filter(|f| range.contains(f.flow.ts) && filters.passes(f, None)),
    );
    ExplorerResponse {
        sankey,
        timeline,
        histogram,
        kpis,
        dropped_total: dropped,
        asn_loaded,
        window: WindowEcho::from(range),
        // Builders serve local data; a caller that also read a remote
        // source records any gap on the returned value.
        incomplete: Vec::new(),
    }
}

/// `GET /api/netflow/explorer?scope=&from=&to=&workflow=&origin=&org=&host=`
/// — the aggregate read behind the Network explorer surface. Scope
/// resolution mirrors [`get_netflow_graph`]; the underlying flow sets are
/// the SAME ones the flows-list routes read
/// ([`run_scoped_flows_and_dropped`] / [`project_scoped_flows_meta_and_dropped`]
/// / [`read_all_workspaces_sync`]), so the explorer's aggregates and the
/// table beneath them can never be built from different data.
async fn get_netflow_explorer(
    State(state): State<AppState>,
    Query(q): Query<ExplorerQuery>,
) -> ApiResult<Json<ExplorerResponse>> {
    maybe_refresh_asn(&netflow_config(&state), &state.asn_cache);
    let range = parse_time_range(&q.from, &q.to)?;
    let filters = parse_filters(&q.workflow, &q.origin, &q.org, &q.host);
    if let Some(run_id) = q.scope.as_deref().and_then(|s| s.strip_prefix("run:")) {
        let resp = explorer_run_scope(&state, run_id, &range, &filters).await?;
        Ok(Json(resp))
    } else if let Some(project_id) = q.scope.as_deref().and_then(|s| s.strip_prefix("project:")) {
        let workspace = workspace_for_project(&state, project_id)?;
        let store = Arc::clone(&state.run_store);
        let global_dir = state.global_dir.clone();
        let cache = Arc::clone(&state.asn_cache);
        let resp = run_blocking(move || {
            let (tagged, meta, dropped) = project_scoped_flows_meta_and_dropped(
                &store,
                &workspace,
                &global_dir,
                &rupu_netflow::ledger::TimeRange::unbounded(),
            );
            let table = load_asn_table(&cache);
            let flows = to_explorer_flows(tagged, &meta, table.as_deref());
            let mut resp = build_explorer_response(
                &flows,
                dropped,
                &meta.spans,
                table.is_some(),
                &range,
                &filters,
            );
            resp.incomplete = scope_gap_from_workers(&meta);
            resp
        })
        .await?;
        Ok(Json(resp))
    } else {
        let store = Arc::clone(&state.run_store);
        let global_dir = state.global_dir.clone();
        let cache = Arc::clone(&state.asn_cache);
        let meta_cache = Arc::clone(&state.run_meta_cache);
        let resp = run_blocking(move || {
            let (tagged, dropped) = read_all_workspaces_sync(
                &global_dir,
                &rupu_netflow::ledger::TimeRange::unbounded(),
            );
            let ledger_ids = tagged.iter().map(|(id, _)| id.clone()).collect();
            let meta = global_run_meta_cached(&meta_cache, &global_dir, &store, &ledger_ids);
            let table = load_asn_table(&cache);
            let flows = to_explorer_flows(tagged, &meta, table.as_deref());
            let mut resp = build_explorer_response(
                &flows,
                dropped,
                &meta.spans,
                table.is_some(),
                &range,
                &filters,
            );
            resp.incomplete = scope_gap_from_workers(&meta);
            resp
        })
        .await?;
        Ok(Json(resp))
    }
}

/// The explorer's `run:` scope — dispatches on [`resolve_run_location`]
/// exactly like [`get_run_netflow`]. `Global`/`ProjectLocal` read the
/// run's own ledgers UNBOUNDED (the histogram wants the run's whole
/// recorded history; the views window internally); `Host` proxies the
/// whole explorer request; `Unpersisted` renders honestly empty views.
async fn explorer_run_scope(
    s: &AppState,
    run_id: &str,
    range: &rupu_netflow::ledger::TimeRange,
    filters: &ExplorerFilters,
) -> ApiResult<ExplorerResponse> {
    let unbounded = rupu_netflow::ledger::TimeRange::unbounded();
    match resolve_run_location(s, run_id).await {
        RunLocation::Global => {
            let run = s
                .run_store
                .load(run_id)
                .map_err(|e| run_not_found_or_internal(run_id, e))?;
            // See `get_run_netflow`'s `Global` branch: a placed run's ledger
            // lives on the host that executed it.
            let (remote_flows, remote_dropped, incomplete) = worker_host_flows(s, &run).await;
            let store = Arc::clone(&s.run_store);
            let rid = run_id.to_string();
            let workspace = run.workspace_path.clone();
            let global_dir = s.global_dir.clone();
            let cache = Arc::clone(&s.asn_cache);
            let range = range.clone();
            let filters = filters.clone();
            run_blocking(move || {
                let (merged, dropped) = run_scoped_flows_and_dropped_with(
                    &store,
                    &rid,
                    &workspace,
                    &global_dir,
                    &unbounded,
                    remote_flows,
                    remote_dropped,
                );
                let mut meta = RunMetaIndex::default();
                meta.insert_run(&store, &run);
                let table = load_asn_table(&cache);
                let tagged: Vec<(String, FlowRecord)> =
                    merged.into_iter().map(|f| (rid.clone(), f)).collect();
                let flows = to_explorer_flows(tagged, &meta, table.as_deref());
                let mut resp = build_explorer_response(
                    &flows,
                    dropped,
                    &meta.spans,
                    table.is_some(),
                    &range,
                    &filters,
                );
                resp.incomplete = incomplete;
                resp
            })
            .await
        }
        RunLocation::ProjectLocal { path } => {
            let (remote_flows, remote_dropped, incomplete) =
                match RunStore::new(path.join(".rupu").join("runs")).load(run_id) {
                    Ok(run) => worker_host_flows(s, &run).await,
                    Err(_) => (Vec::new(), 0, Vec::new()),
                };
            let rid = run_id.to_string();
            let global_dir = s.global_dir.clone();
            let cache = Arc::clone(&s.asn_cache);
            let range = range.clone();
            let filters = filters.clone();
            run_blocking(move || {
                let store = RunStore::new(path.join(".rupu").join("runs"));
                let (merged, dropped) = run_scoped_flows_and_dropped_with(
                    &store,
                    &rid,
                    &path,
                    &global_dir,
                    &unbounded,
                    remote_flows,
                    remote_dropped,
                );
                let mut meta = RunMetaIndex::default();
                if let Ok(record) = store.load(&rid) {
                    meta.insert_run(&store, &record);
                }
                let table = load_asn_table(&cache);
                let tagged: Vec<(String, FlowRecord)> =
                    merged.into_iter().map(|f| (rid.clone(), f)).collect();
                let flows = to_explorer_flows(tagged, &meta, table.as_deref());
                let mut resp = build_explorer_response(
                    &flows,
                    dropped,
                    &meta.spans,
                    table.is_some(),
                    &range,
                    &filters,
                );
                resp.incomplete = incomplete;
                resp
            })
            .await
        }
        RunLocation::Host { host_id } => {
            explorer_from_host(s, &host_id, run_id, range, filters).await
        }
        // No artifacts anywhere — honestly empty views, not an error;
        // mirrors `get_run_netflow`'s `Unpersisted` branch (including the
        // `run_blocking` for the potential table parse).
        RunLocation::Unpersisted { .. } => {
            let cache = Arc::clone(&s.asn_cache);
            let range = range.clone();
            let filters = filters.clone();
            run_blocking(move || {
                let table = load_asn_table(&cache);
                build_explorer_response(&[], 0, &[], table.is_some(), &range, &filters)
            })
            .await
        }
        RunLocation::NotFound => Err(ApiError::not_found(format!("run {run_id} not found"))),
    }
}

/// What a host could tell us about one run's raw flow records.
enum HostFlows {
    /// The host served the structured surface. Raw records plus the
    /// ledger's own dropped-record count.
    Records(Vec<FlowRecord>, u64),
    /// The host has no structured netflow surface. `reason` is that
    /// refusal, kept because it is the informative one: an HTTP host is
    /// simply expected to proxy instead, but an SSH host landing here means
    /// its remote `rupu` predates `netflow show`, and saying THAT beats
    /// letting the generic-GET attempt report "proxy_get_json is not
    /// supported for ssh hosts" — true, but not the operator's actual
    /// problem.
    NoStructuredSurface { reason: String },
}

/// Fetch one run's raw flow records from a host that exposes the structured
/// [`HostConnector::run_netflow`] surface.
///
/// Raw records, aggregated locally: the CP applies its own window, filters
/// and ASN table, so every host's flows are enriched identically and a
/// remote cannot return something that merely looks filtered.
async fn host_raw_flows(s: &AppState, host_id: &str, run_id: &str) -> ApiResult<HostFlows> {
    let conn = resolve_host(s, host_id)?;
    let value = match conn.run_netflow(run_id).await {
        Ok(v) => v,
        // No structured surface — the caller tries the generic-GET proxy,
        // and falls back to THIS reason if that has nothing to offer either.
        Err(HostConnectorError::Unsupported(reason)) | Err(HostConnectorError::Invalid(reason)) => {
            return Ok(HostFlows::NoStructuredSurface { reason })
        }
        Err(HostConnectorError::NotFound(m)) => return Err(ApiError::not_found(m)),
        Err(HostConnectorError::Unreachable(m)) => {
            return Err(ApiError::internal(format!(
                "host {host_id} unreachable: {m}"
            )))
        }
        Err(other) => return Err(ApiError::internal(other.to_string())),
    };

    let flows: Vec<FlowRecord> = serde_json::from_value(
        value
            .get("flows")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
    )
    .map_err(|e| {
        ApiError::internal(format!(
            "host {host_id} returned netflow records this build cannot read ({e}); \
             the remote rupu is likely older than this one"
        ))
    })?;
    let dropped = value
        .get("dropped_total")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    Ok(HostFlows::Records(flows, dropped))
}

/// Read the ledger held by the host a run actually EXECUTED on.
///
/// A run placed on a remote host is mirrored into our own `RunStore` — so
/// it resolves as a local run — but its netflow ledger stays on the host
/// that made the calls. Reading only local ledgers for such a run returns
/// an empty set that looks exactly like "this run made no network calls".
/// `worker_id` is what distinguishes the two, so it is what this keys on.
///
/// Never fails the caller: a host that cannot answer yields no flows plus
/// an [`IncompleteSource`] naming it, so the view renders what it has AND
/// says what is missing. Failing the whole read would throw away the local
/// half over an offline host; returning short and silent would be the
/// original bug.
async fn worker_host_flows(
    s: &AppState,
    run: &rupu_orchestrator::runs::RunRecord,
) -> (Vec<FlowRecord>, u64, Vec<IncompleteSource>) {
    let Some(host_id) = run.worker_id.as_deref().filter(|h| !h.is_empty()) else {
        return (Vec::new(), 0, Vec::new());
    };
    if host_id == "local" {
        return (Vec::new(), 0, Vec::new());
    }
    let incomplete = |reason: String| {
        (
            Vec::new(),
            0,
            vec![IncompleteSource {
                host_id: host_id.to_string(),
                reason,
            }],
        )
    };

    let conn = match s.hosts.resolve(host_id) {
        Ok(c) => c,
        Err(e) => {
            // The run says it executed there; we just cannot get to it.
            return incomplete(format!("host {host_id} could not be resolved: {e}"));
        }
    };
    match conn.run_netflow(&run.id).await {
        Ok(value) => {
            let flows: Result<Vec<FlowRecord>, _> = serde_json::from_value(
                value
                    .get("flows")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
            );
            match flows {
                Ok(flows) => (
                    flows,
                    value
                        .get("dropped_total")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    Vec::new(),
                ),
                Err(e) => incomplete(format!(
                    "host {host_id} returned netflow records this build cannot read ({e}); \
                     the remote rupu is likely older than this one"
                )),
            }
        }
        Err(e) => {
            tracing::warn!(
                host_id = %host_id,
                run_id = %run.id,
                error = %e,
                "worker_host_flows: could not read the run's own host ledger"
            );
            incomplete(e.to_string())
        }
    }
}

/// The attribution index for a host-sourced run.
///
/// A run placed on an SSH/Tunnel host is ALSO mirrored into our own
/// `RunStore`, so its record is usually right here and attribution is exact.
/// When it is not, an empty index leaves run/workflow unattributed rather
/// than failing the read — the same posture the `ProjectLocal` branch takes.
fn meta_for_host_run(s: &AppState, run_id: &str) -> RunMetaIndex {
    let mut meta = RunMetaIndex::default();
    if let Ok(record) = s.run_store.load(run_id) {
        meta.insert_run(&s.run_store, &record);
    }
    meta
}

/// Proxy the whole explorer request to a resolved host — the remote CP
/// owns the run's ledgers, run records, and its own ASN table, so it
/// builds the aggregates and we relay them. Unlike
/// [`run_netflow_from_host`] there is no local re-enforcement pass: the
/// explorer route and its `from`/`to`/filter params shipped together, so
/// any remote that HAS the route applies them; a remote without it 404s
/// loudly (surfaced via the error below), never silently unfiltered.
async fn explorer_from_host(
    s: &AppState,
    host_id: &str,
    run_id: &str,
    range: &rupu_netflow::ledger::TimeRange,
    filters: &ExplorerFilters,
) -> ApiResult<ExplorerResponse> {
    // A transport with a structured netflow surface (SSH) returns raw
    // records that we aggregate here, with our own window, filters and ASN
    // table. Only an HTTP host falls through to the proxy below.
    let no_structured_reason = match host_raw_flows(s, host_id, run_id).await? {
        HostFlows::NoStructuredSurface { reason } => reason,
        HostFlows::Records(flows, dropped) => {
            let meta = meta_for_host_run(s, run_id);
            let cache = Arc::clone(&s.asn_cache);
            let rid = run_id.to_string();
            let range = range.clone();
            let filters = filters.clone();
            return run_blocking(move || {
                let table = load_asn_table(&cache);
                let tagged: Vec<(String, FlowRecord)> =
                    flows.into_iter().map(|f| (rid.clone(), f)).collect();
                let explorer_flows = to_explorer_flows(tagged, &meta, table.as_deref());
                build_explorer_response(
                    &explorer_flows,
                    dropped,
                    &meta.spans,
                    table.is_some(),
                    &range,
                    &filters,
                )
            })
            .await;
        }
    };

    let conn = resolve_host(s, host_id)?;
    let mut parts = vec![format!("scope=run:{}", urlencode_query_value(run_id))];
    parts.extend(time_range_query_parts(range));
    parts.extend(filter_query_parts(filters));
    let path = format!("/api/netflow/explorer?{}", parts.join("&"));
    let value = conn.proxy_get_json(&path).await.map_err(|e| match e {
        HostConnectorError::NotFound(m) => ApiError::not_found(format!(
            "host {host_id} has no netflow explorer endpoint ({m}); the remote CP is \
             likely older than this one"
        )),
        HostConnectorError::Unreachable(m) => {
            ApiError::internal(format!("host {host_id} unreachable: {m}"))
        }
        // Neither surface: report why the STRUCTURED one declined, which
        // names the actual problem (an out-of-date remote `rupu`), not the
        // generic-GET refusal that is merely a property of the transport.
        HostConnectorError::Invalid(_) | HostConnectorError::Unsupported(_) => ApiError::internal(
            format!("host {host_id} cannot serve netflow: {no_structured_reason}"),
        ),
        other => ApiError::internal(other.to_string()),
    })?;
    serde_json::from_value(value).map_err(|e| {
        ApiError::internal(format!(
            "host {host_id} returned an explorer response this build cannot read ({e}); \
             the remote CP is likely older than this one"
        ))
    })
}

/// Proxy `GET /api/runs/:id/netflow` to a resolved host. Mirrors
/// `graph.rs`'s `run_graph_from_host` — the remote CP does the same
/// ledger+transcript merge locally and we just relay its response.
///
/// `range` is forwarded as a `?from=&to=` query string appended to the
/// proxied path so a CURRENT remote applies the SAME window before
/// replying — but [`enforce_range_on_proxied_response`] ALSO re-filters
/// `flows` (and recomputes `hosts`/`window` from the filtered set) after
/// deserializing, rather than trusting the remote to have applied it: an
/// OLDER remote that doesn't recognize `from`/`to` would otherwise
/// silently return everything unbounded while looking filtered.
/// `dropped_total` is the one field intentionally left as the remote
/// reported it — whole-file by definition, so filtering here has nothing
/// to do with it either way.
async fn run_netflow_from_host(
    s: &AppState,
    host_id: &str,
    id: &str,
    range: &rupu_netflow::ledger::TimeRange,
    filters: &ExplorerFilters,
) -> ApiResult<NetflowResponse> {
    // Same rule as `explorer_from_host`: raw records from a structured
    // transport, enriched and filtered here; only HTTP proxies.
    let no_structured_reason = match host_raw_flows(s, host_id, id).await? {
        HostFlows::NoStructuredSurface { reason } => reason,
        HostFlows::Records(flows, dropped) => {
            let meta = meta_for_host_run(s, id);
            let cache = Arc::clone(&s.asn_cache);
            let rid = id.to_string();
            let range = range.clone();
            let filters = filters.clone();
            return run_blocking(move || {
                let table = load_asn_table(&cache);
                let tagged: Vec<(String, FlowRecord)> =
                    flows.into_iter().map(|f| (rid.clone(), f)).collect();
                build_filtered_response(tagged, &meta, dropped, table.as_deref(), &range, &filters)
            })
            .await;
        }
    };

    let conn = resolve_host(s, host_id)?;
    let path = format!(
        "/api/runs/{id}/netflow{}",
        query_string(&time_range_query_parts(range), &filter_query_parts(filters))
    );
    let value = conn.proxy_get_json(&path).await.map_err(|e| match e {
        HostConnectorError::NotFound(m) => ApiError::not_found(m),
        HostConnectorError::Unreachable(m) => {
            ApiError::internal(format!("host {host_id} unreachable: {m}"))
        }
        // See `explorer_from_host`: prefer the structured surface's reason.
        HostConnectorError::Invalid(_) | HostConnectorError::Unsupported(_) => ApiError::internal(
            format!("host {host_id} cannot serve netflow: {no_structured_reason}"),
        ),
        other => ApiError::internal(other.to_string()),
    })?;
    // A deserialization failure here is most likely `dropped_total` not
    // parsing on an OLDER remote still emitting the pre-rename `dropped`
    // key (no `#[serde(default)]` on `NetflowResponse`, deliberately —
    // failing closed here beats silently defaulting `dropped_total` to 0
    // and rendering an older remote's response as if nothing was lost).
    // Name that likely cause instead of relaying `e.to_string()`, which is
    // a bare serde message an operator has no way to act on.
    let mut resp: NetflowResponse = serde_json::from_value(value).map_err(|e| {
        ApiError::internal(format!(
            "host {host_id} returned a netflow response this build cannot read ({e}); \
             the remote CP is likely older than this one"
        ))
    })?;
    enforce_range_on_proxied_response(&mut resp, range);
    enforce_filters_on_proxied_response(&mut resp, filters);
    Ok(resp)
}

/// Same defensive posture as [`enforce_range_on_proxied_response`], for
/// the cross-filter params: a remote old enough to ignore unfamiliar
/// `workflow`/`origin`/`org`/`host` params would silently return the
/// unfiltered set, so the retained-and-recomputed correction runs HERE
/// regardless. Uses the remote's own per-flow `asn`/`workflow` enrichment
/// (this side has no better source for a remote run's attribution). A
/// no-op when no filter is active, keeping unfiltered proxied responses
/// byte-identical to before this existed.
fn enforce_filters_on_proxied_response(resp: &mut NetflowResponse, filters: &ExplorerFilters) {
    if filters.is_empty() {
        return;
    }
    resp.flows.retain(|v| flow_view_passes(v, filters));
    resp.hosts = rupu_netflow::ledger::host_rollup_iter(resp.flows.iter().map(|v| &v.flow));
}

/// Correct a host-proxied [`NetflowResponse`] to match `range` regardless
/// of what the remote actually enforced — a no-op against a CURRENT remote
/// (which already applied the identical window server-side before
/// replying, so every field here is already consistent), and CORRECTIVE
/// against an OLDER remote that recognizes the route but silently ignores
/// unfamiliar `from`/`to` query params and returns everything unbounded.
/// Pulled out of [`run_netflow_from_host`] as its own function so the
/// correction (retain, then recompute `hosts` and stamp `window` from the
/// SAME retained set) is unit-testable without a real HTTP round trip
/// through a `HostConnector`.
///
/// `dropped_total` is deliberately left as the remote reported it: it is
/// whole-file by definition (see that field's doc comment) and is not
/// affected by which `flows` survive this filter either way.
fn enforce_range_on_proxied_response(
    resp: &mut NetflowResponse,
    range: &rupu_netflow::ledger::TimeRange,
) {
    resp.flows.retain(|v| range.contains(v.flow.ts));
    // `hosts` was computed by the REMOTE from its own (possibly unbounded,
    // on an older build) flow set, so leaving it as-is would show call
    // counts/byte totals for flows the retained `flows` list no longer
    // contains — the same "two adjacent widgets disagree" defect already
    // fixed once for the graph endpoint. Recompute it from the flows THIS
    // side actually kept.
    resp.hosts = rupu_netflow::ledger::host_rollup_iter(resp.flows.iter().map(|v| &v.flow));
    // Likewise `window`: an older remote either omits the field (defaults
    // to unbounded via `#[serde(default)]`) or echoes whatever range IT
    // parsed, neither of which describes what this side actually
    // enforced above. Stamp it from the range THIS function applied, so
    // `NetflowWindowReadout` can never read "Showing all recorded flows"
    // over a response that was, in fact, just filtered.
    resp.window = WindowEcho::from(range);
}

/// Render `range` as a `?from=&to=` query-string suffix (empty string when
/// unbounded on both sides) for [`run_netflow_from_host`]'s proxy call.
/// Both `:` and `+` need escaping in a form-encoded query value — mirrors
/// `usage.rs`'s `urlencoding_rfc3339`, which a doc comment there notes was
/// added after a bare `+` silently decoded as a space and corrupted a
/// forwarded timestamp.
fn time_range_query_parts(range: &rupu_netflow::ledger::TimeRange) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(from) = range.from {
        parts.push(format!("from={}", urlencoding_rfc3339(from)));
    }
    if let Some(to) = range.to {
        parts.push(format!("to={}", urlencoding_rfc3339(to)));
    }
    parts
}

/// The cross-filter params, re-encoded exactly as [`parse_filters`] reads
/// them back (comma-joined values, one param per dimension) so a proxied
/// request round-trips losslessly.
fn filter_query_parts(filters: &ExplorerFilters) -> Vec<String> {
    let join = |name: &str, values: &[String]| {
        (!values.is_empty()).then(|| {
            format!(
                "{name}={}",
                values
                    .iter()
                    .map(|v| urlencode_query_value(v))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
    };
    [
        join("workflow", &filters.workflows),
        join("origin", &filters.origins),
        join("org", &filters.orgs),
        join("host", &filters.hosts),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Join query-string parts into a `?a&b` suffix (empty when there are no
/// parts at all).
fn query_string(a: &[String], b: &[String]) -> String {
    let all: Vec<&String> = a.iter().chain(b.iter()).collect();
    if all.is_empty() {
        String::new()
    } else {
        format!(
            "?{}",
            all.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("&")
        )
    }
}

/// Percent-encode one query VALUE conservatively: unreserved characters
/// and `:` (legal in a query, and load-bearing for `provider:x` /
/// `host:port` keys) pass through; everything else — including `,`, so a
/// value can never masquerade as [`split_filter`]'s separator — is
/// escaped. `filter_query_parts` joins values with a LITERAL comma after
/// encoding, which the remote's form-decoder leaves alone.
fn urlencode_query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b':' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urlencoding_rfc3339(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.to_rfc3339().replace('+', "%2B").replace(':', "%3A")
}

/// `GET /api/runs/:id/netflow` — network flows attributed to one run
/// (including every sub-agent it dispatched, at any depth — see
/// `run_and_unit_ids`'s doc for the attribution decision), from the
/// workspace ledger merged with the run transcript (see module doc), with
/// read-time ASN enrichment and a server-computed per-host rollup.
///
/// Dispatches on [`resolve_run_location`] exactly like `run_graph`:
/// `Global`/`ProjectLocal` read locally, `Host` proxies, `Unpersisted` has
/// no workspace to read from so it returns an empty (not error) response,
/// `NotFound` → 404.
async fn get_run_netflow(
    State(s): State<AppState>,
    Path(run_id): Path<String>,
    Query(q): Query<TimeRangeQuery>,
) -> ApiResult<Json<NetflowResponse>> {
    maybe_refresh_asn(&netflow_config(&s), &s.asn_cache);
    let range = parse_time_range(&q.from, &q.to)?;
    let filters = parse_filters(&q.workflow, &q.origin, &q.org, &q.host);
    match resolve_run_location(&s, &run_id).await {
        RunLocation::Global => {
            let run = s
                .run_store
                .load(&run_id)
                .map_err(|e| run_not_found_or_internal(&run_id, e))?;
            // A run placed on a remote host keeps its ledger THERE; only
            // its transcript is mirrored back. Read the owning host too, or
            // this returns an empty set that reads as "no network calls".
            let remote = worker_host_flows(&s, &run).await;
            let store = Arc::clone(&s.run_store);
            let rid = run_id.clone();
            let workspace = run.workspace_path.clone();
            let global_dir = s.global_dir.clone();
            let cache = Arc::clone(&s.asn_cache);
            let resp = run_blocking(move || {
                collect_run_netflow(
                    &store,
                    &rid,
                    &workspace,
                    &global_dir,
                    &cache,
                    &range,
                    &filters,
                    remote,
                )
            })
            .await?;
            Ok(Json(resp))
        }
        RunLocation::ProjectLocal { path } => {
            // Same rule as the `Global` branch: a project-local record can
            // still name a remote worker.
            let remote = match RunStore::new(path.join(".rupu").join("runs")).load(&run_id) {
                Ok(run) => worker_host_flows(&s, &run).await,
                Err(_) => (Vec::new(), 0, Vec::new()),
            };
            let rid = run_id.clone();
            let global_dir = s.global_dir.clone();
            let cache = Arc::clone(&s.asn_cache);
            let resp = run_blocking(move || {
                let store = RunStore::new(path.join(".rupu").join("runs"));
                collect_run_netflow(
                    &store,
                    &rid,
                    &path,
                    &global_dir,
                    &cache,
                    &range,
                    &filters,
                    remote,
                )
            })
            .await?;
            Ok(Json(resp))
        }
        RunLocation::Host { host_id } => {
            run_netflow_from_host(&s, &host_id, &run_id, &range, &filters)
                .await
                .map(Json)
        }
        // No artifacts anywhere: the run never persisted a workspace to
        // read a ledger or transcript from. Empty, not an error — mirrors
        // `run_graph`'s `Unpersisted` branch. Still routed through
        // `run_blocking`: `load_asn_table()` can parse a multi-megabyte
        // table from disk on a cache miss, same rationale as every other
        // branch here.
        RunLocation::Unpersisted { .. } => {
            let cache = Arc::clone(&s.asn_cache);
            let resp = run_blocking(move || {
                let table = load_asn_table(&cache);
                build_filtered_response(
                    Vec::new(),
                    &RunMetaIndex::default(),
                    0,
                    table.as_deref(),
                    &range,
                    &filters,
                )
            })
            .await?;
            Ok(Json(resp))
        }
        RunLocation::NotFound => Err(ApiError::not_found(format!("run {run_id} not found"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_netflow::{Fidelity, FlowCtx, Origin, Outcome};

    fn flow(id: FlowId, run: Option<&str>, host: &str) -> FlowRecord {
        FlowRecord {
            id,
            ts: chrono::Utc::now(),
            ctx: FlowCtx {
                run_id: run.map(str::to_string),
                step_id: None,
                agent: None,
                workspace_id: None,
                origin: Origin::Provider("anthropic".into()),
            },
            fidelity: Fidelity::Http,
            method: "POST".into(),
            scheme: "https".into(),
            host: host.into(),
            port: 443,
            path: "/v1/messages".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: None,
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: Some(10),
            bytes_in: Some(20),
            body_complete: true,
            ttfb_ms: None,
            duration_ms: Some(30),
        }
    }

    fn write_transcript(path: &std::path::Path, flows: &[FlowRecord]) {
        let mut w = rupu_transcript::JsonlWriter::create(path).unwrap();
        for f in flows {
            w.write(&TranscriptEvent::NetFlow {
                flow: Box::new(f.clone()),
            })
            .unwrap();
        }
        w.flush().unwrap();
    }

    // ── macOS golden fixtures (apps/rupu-macos/Fixtures/) ─────────────────────
    //
    // `NetflowResponse`/`FlowView` are `pub`, but this fixture lives here
    // (rather than the integration test) per the Phase 2 plan, alongside the
    // `flow()` helper above. Same `check_fixture` contract as
    // `tests/macos_fixtures.rs` (duplicated: a unit test can't share code with
    // an integration test without a public module) — see `api/host_info.rs`'s
    // test module for the established pattern.

    fn check_fixture(name: &str, value: &impl serde::Serialize) {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/rupu-macos/Fixtures");
        let path = dir.join(name);
        let rendered = serde_json::to_string_pretty(value).expect("serialize fixture");
        if std::env::var_os("REGEN_FIXTURES").is_some() {
            std::fs::write(&path, rendered + "\n").expect("write fixture");
            return;
        }
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing fixture {name}; run `make macos-fixtures`"));
        assert_eq!(
            on_disk.trim_end(),
            rendered,
            "fixture {name} drifted from the Rust types; run `make macos-fixtures`"
        );
    }

    #[test]
    fn netflow_run_fixture_is_current() {
        let mut flow_ok = flow(
            FlowId::from_parts(1, 1),
            Some("run-40"),
            "api.anthropic.com",
        );
        flow_ok.peer_ip = Some("142.250.72.14".parse().unwrap());
        flow_ok.ts = chrono::DateTime::from_timestamp(1_755_691_200, 0).unwrap();

        let mut flow_err = flow(FlowId::from_parts(1, 2), Some("run-40"), "api.github.com");
        flow_err.outcome = Outcome::TransportError;
        flow_err.error = Some("connection reset".into());
        flow_err.status = None;
        flow_err.bytes_in = None;
        flow_err.bytes_out = None;
        flow_err.body_complete = false;
        flow_err.ts = chrono::DateTime::from_timestamp(1_755_691_260, 0).unwrap();

        let response = NetflowResponse {
            flows: vec![
                FlowView {
                    flow: flow_ok,
                    asn: Some(AsnInfo {
                        asn: 15169,
                        org: "Google LLC".into(),
                    }),
                    // Populated (not None) deliberately: the fixture must
                    // EXERCISE the attribution fields so the macOS drift
                    // guard actually fires if their shape/naming changes —
                    // a fixture pinning them to their invisible None state
                    // would neuter the rig (CLAUDE.md fixture rule).
                    run_id: Some("run-40".into()),
                    workflow: Some("nightly-scan".into()),
                },
                FlowView {
                    flow: flow_err,
                    asn: None,
                    // The unattributable shape: keys absent entirely.
                    run_id: None,
                    workflow: None,
                },
            ],
            hosts: vec![
                rupu_netflow::ledger::HostRollup {
                    host: "api.anthropic.com".into(),
                    port: 443,
                    calls: 1,
                    bytes_in: Some(20),
                    bytes_out: Some(10),
                    errors: 0,
                    p50_ms: Some(30),
                    p95_ms: Some(30),
                },
                rupu_netflow::ledger::HostRollup {
                    host: "api.github.com".into(),
                    port: 443,
                    calls: 3,
                    bytes_in: None,
                    bytes_out: None,
                    errors: 3,
                    p50_ms: None,
                    p95_ms: None,
                },
            ],
            window: WindowEcho::default(),
            dropped_total: 0,
            asn_loaded: true,
            incomplete: Vec::new(),
        };
        check_fixture("netflow_run.json", &response);
    }

    /// [`build_filtered_response`] with no attribution and no filters —
    /// the shape the old `build_response` had; these tests assert the
    /// builder's base contract (rollup / window echo / enrichment)
    /// independent of the attribution+filtering features layered on top.
    fn build_unfiltered(
        flows: Vec<FlowRecord>,
        dropped: u64,
        table: Option<&AsnTable>,
        range: &rupu_netflow::ledger::TimeRange,
    ) -> NetflowResponse {
        build_filtered_response(
            flows.into_iter().map(|f| (String::new(), f)).collect(),
            &RunMetaIndex::default(),
            dropped,
            table,
            range,
            &ExplorerFilters::default(),
        )
    }

    #[test]
    fn build_response_wires_a_server_computed_host_rollup() {
        let f = flow(FlowId::new(), Some("r"), "api.anthropic.com");
        let resp = build_unfiltered(
            vec![f],
            0,
            None,
            &rupu_netflow::ledger::TimeRange::unbounded(),
        );
        assert_eq!(resp.hosts.len(), 1);
        assert_eq!(resp.hosts[0].host, "api.anthropic.com");
        assert_eq!(resp.hosts[0].calls, 1);
    }

    /// Minor 4 (Task 3 review round 1): `window` must echo whatever
    /// `range` was actually applied, not stay at its `Default` regardless
    /// of input — proving `build_response` threads `range` through to
    /// `WindowEcho::from` rather than the field being hardcoded or
    /// disconnected from the range it claims to describe.
    #[test]
    fn build_response_echoes_the_applied_window() {
        let f = flow(FlowId::new(), Some("r"), "api.anthropic.com");
        let range = rupu_netflow::ledger::TimeRange {
            from: Some(chrono::DateTime::from_timestamp(100, 0).unwrap()),
            to: Some(chrono::DateTime::from_timestamp(200, 0).unwrap()),
        };

        let resp = build_unfiltered(vec![f], 0, None, &range);

        assert_eq!(resp.window.from, range.from);
        assert_eq!(resp.window.to, range.to);
    }

    /// The unbounded case: both sides `null`, not omitted — a caller must
    /// be able to tell "no filter was applied" from "the response has no
    /// opinion", which an omitted key would blur.
    #[test]
    fn build_response_with_an_unbounded_range_echoes_a_null_window() {
        let f = flow(FlowId::new(), Some("r"), "api.anthropic.com");

        let resp = build_unfiltered(
            vec![f],
            0,
            None,
            &rupu_netflow::ledger::TimeRange::unbounded(),
        );

        assert!(resp.window.from.is_none());
        assert!(resp.window.to.is_none());
    }

    /// Important 1 (Fix round 2 review): `enforce_range_on_proxied_response`
    /// must correct ALL THREE fields the retain touches transitively, not
    /// just `flows` — this is the exact "corrects flows but not window or
    /// hosts" defect the review caught. Simulates an OLDER remote's
    /// response: `hosts` rolled up from its own unfiltered flow set, and
    /// `window` left at its `#[serde(default)]` unbounded value (as if the
    /// remote never echoed the field at all).
    #[test]
    fn enforce_range_on_proxied_response_recomputes_hosts_and_window() {
        let mut in_window = flow(FlowId::new(), Some("r"), "in.example");
        in_window.ts = chrono::DateTime::from_timestamp(150, 0).unwrap();
        let mut out_of_window = flow(FlowId::new(), Some("r"), "out.example");
        out_of_window.ts = chrono::DateTime::from_timestamp(500, 0).unwrap();

        let mut resp = build_unfiltered(
            vec![in_window, out_of_window],
            3,
            None,
            &rupu_netflow::ledger::TimeRange::unbounded(),
        );
        assert_eq!(
            resp.hosts.len(),
            2,
            "sanity: both hosts present pre-correction"
        );
        assert!(
            resp.window.from.is_none(),
            "sanity: unbounded pre-correction"
        );

        let range = rupu_netflow::ledger::TimeRange {
            from: Some(chrono::DateTime::from_timestamp(100, 0).unwrap()),
            to: Some(chrono::DateTime::from_timestamp(200, 0).unwrap()),
        };
        enforce_range_on_proxied_response(&mut resp, &range);

        assert_eq!(resp.flows.len(), 1);
        assert_eq!(resp.flows[0].flow.host, "in.example");
        assert_eq!(
            resp.hosts.len(),
            1,
            "hosts must be recomputed from the RETAINED flows only, not left \
             as the remote's own unfiltered rollup: {:?}",
            resp.hosts
        );
        assert_eq!(resp.hosts[0].host, "in.example");
        assert_eq!(
            resp.window.from, range.from,
            "window must be stamped from the range THIS side enforced"
        );
        assert_eq!(resp.window.to, range.to);
        assert_eq!(
            resp.dropped_total, 3,
            "dropped_total is untouched by this correction — whole-file by definition"
        );
    }

    /// The other half of `build_response`'s contract, previously untested:
    /// with no table injected, `asn_loaded` must be `false` AND every
    /// flow's `asn` must be `None` — not just the boolean flipped while the
    /// enrichment silently still ran (or vice versa). Before this fix
    /// `build_response` called `load_asn_table()` internally, so no test
    /// could force the "table absent" branch independent of whatever
    /// happened to exist on the real machine's `$RUPU_HOME`.
    #[test]
    fn build_response_with_no_table_reports_unloaded_and_enriches_nothing() {
        let mut f = flow(FlowId::new(), Some("r"), "api.anthropic.com");
        f.peer_ip = Some("1.0.0.7".parse().unwrap());

        let resp = build_unfiltered(
            vec![f],
            0,
            None,
            &rupu_netflow::ledger::TimeRange::unbounded(),
        );

        assert!(!resp.asn_loaded, "no table was supplied");
        assert!(
            resp.flows.iter().all(|v| v.asn.is_none()),
            "asn_loaded: false must mean no flow got enriched, even one with a resolvable peer_ip"
        );
    }

    /// The mirror case: a table IS supplied and resolves the flow's peer.
    /// `asn_loaded` must be `true` and the resolving flow must carry its
    /// `AsnInfo` — proving `build_response` actually threads `table`
    /// through to `FlowView::from_flow` rather than the field being
    /// hardcoded or disconnected from the enrichment it claims to describe.
    #[test]
    fn build_response_with_a_resolving_table_reports_loaded_and_enriches_the_match() {
        use std::io::Cursor;
        let table = rupu_netflow::AsnTable::compact_from_tsv(Cursor::new(
            "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n",
        ))
        .unwrap();

        let mut f = flow(FlowId::new(), Some("r"), "api.anthropic.com");
        f.peer_ip = Some("1.0.0.7".parse().unwrap());

        let resp = build_unfiltered(
            vec![f],
            0,
            Some(&table),
            &rupu_netflow::ledger::TimeRange::unbounded(),
        );

        assert!(resp.asn_loaded);
        assert_eq!(resp.flows.len(), 1);
        let asn = resp.flows[0]
            .asn
            .as_ref()
            .expect("peer_ip resolves in the table");
        assert_eq!(asn.asn, 13335);
        assert_eq!(asn.org, "CLOUDFLARENET");
    }

    /// Register `host_id` resolving to `conn`. A `Local` transport under a
    /// non-`"local"` id is the registry's injection seam.
    fn state_with_host(
        tmp: &tempfile::TempDir,
        host_id: &str,
        conn: Arc<dyn crate::host::connector::HostConnector>,
    ) -> AppState {
        let host_store = rupu_workspace::HostStore {
            root: tmp.path().join("hosts"),
        };
        host_store
            .save(&rupu_workspace::Host {
                id: host_id.into(),
                name: host_id.into(),
                transport: rupu_workspace::HostTransport::Local,
                token_hash: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                last_seen_at: None,
            })
            .unwrap();
        let registry = Arc::new(crate::host::registry::HostRegistry::new(host_store, conn));
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_workspace_dir(tmp.path().to_path_buf())
        .with_hosts(registry)
    }

    fn placed_run(id: &str, host_id: &str, workspace: &std::path::Path) -> RunRecord {
        let mut r = seed_record(id, "wf", workspace);
        r.worker_id = Some(host_id.to_string());
        r
    }

    /// The reported behaviour: a run placed on a remote host resolves as a
    /// LOCAL run (it is mirrored here), so a local-only ledger read returned
    /// an empty set that reads as "this run made no network calls". Its
    /// ledger lives on the host that made the calls; go and read it.
    #[tokio::test]
    async fn a_placed_runs_flows_come_from_the_host_that_executed_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote_flow = flow(FlowId::from(9u128), Some("run_placed"), "api.openai.com");
        let conn: Arc<dyn crate::host::connector::HostConnector> =
            Arc::new(crate::host::connector::testing::StubConnector {
                run_netflow: Some(Ok(serde_json::json!({
                    "flows": [remote_flow],
                    "dropped_total": 2,
                }))),
            });
        let s = state_with_host(&tmp, "host_ssh", conn);
        let run = placed_run("run_placed", "host_ssh", tmp.path());

        let (flows, dropped, incomplete) = worker_host_flows(&s, &run).await;

        assert_eq!(flows.len(), 1, "the host's own ledger is read");
        assert_eq!(flows[0].host, "api.openai.com");
        assert_eq!(dropped, 2, "and its dropped count comes with it");
        assert!(incomplete.is_empty(), "nothing was missing");
    }

    /// A host that cannot answer must be NAMED, not silently skipped: a
    /// short flow list that looks complete is the failure this whole change
    /// exists to remove.
    #[tokio::test]
    async fn a_host_that_cannot_answer_is_reported_not_silently_dropped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn: Arc<dyn crate::host::connector::HostConnector> =
            Arc::new(crate::host::connector::testing::StubConnector {
                // Default: Unsupported — a remote whose rupu predates
                // `netflow show`.
                run_netflow: None,
            });
        let s = state_with_host(&tmp, "host_ssh", conn);
        let run = placed_run("run_placed", "host_ssh", tmp.path());

        let (flows, dropped, incomplete) = worker_host_flows(&s, &run).await;

        assert!(flows.is_empty());
        assert_eq!(dropped, 0);
        assert_eq!(incomplete.len(), 1, "the gap is declared");
        assert_eq!(incomplete[0].host_id, "host_ssh");
        assert!(
            !incomplete[0].reason.is_empty(),
            "and it says why, so an operator knows what to fix"
        );
    }

    /// A run that executed here has no remote half to read, and must not be
    /// marked incomplete for it.
    #[tokio::test]
    async fn a_local_run_reads_no_remote_and_is_not_marked_incomplete() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn: Arc<dyn crate::host::connector::HostConnector> =
            Arc::new(crate::host::connector::testing::StubConnector::default());
        let s = state_with_host(&tmp, "host_ssh", conn);
        // No worker_id at all, and the explicit "local" spelling.
        let plain = seed_record("run_here", "wf", tmp.path());
        let mut local = seed_record("run_here2", "wf", tmp.path());
        local.worker_id = Some("local".into());

        for run in [plain, local] {
            let (flows, dropped, incomplete) = worker_host_flows(&s, &run).await;
            assert!(flows.is_empty());
            assert_eq!(dropped, 0);
            assert!(incomplete.is_empty(), "a local run is not incomplete");
        }
    }

    /// Project/global scope unions ledger DIRECTORIES on this machine, so a
    /// run that executed elsewhere contributes nothing — and nothing on the
    /// page reveals that. Naming the host is the difference between an
    /// understated total and a knowingly understated one.
    #[test]
    fn scope_gap_names_the_hosts_in_scope_runs_executed_on() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let mut placed = seed_record("run_placed", "wf", tmp.path());
        placed.worker_id = Some("host_ssh".into());
        let mut here = seed_record("run_here", "wf", tmp.path());
        here.worker_id = None;

        let mut meta = RunMetaIndex::default();
        meta.insert_run(&store, &placed);
        meta.insert_run(&store, &here);

        let gap = scope_gap_from_workers(&meta);

        assert_eq!(gap.len(), 1, "one remote host contributed runs");
        assert_eq!(gap[0].host_id, "host_ssh");
        assert!(
            gap[0].reason.contains("not included"),
            "the reason must say the flows are absent: {}",
            gap[0].reason
        );
    }

    /// The single-machine case must declare NOTHING, or the warning becomes
    /// a permanent banner people learn to ignore.
    #[test]
    fn scope_gap_is_empty_when_every_run_executed_here() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let mut local = seed_record("run_here", "wf", tmp.path());
        local.worker_id = Some("local".into());
        let plain = seed_record("run_here2", "wf", tmp.path());

        let mut meta = RunMetaIndex::default();
        meta.insert_run(&store, &local);
        meta.insert_run(&store, &plain);

        assert!(scope_gap_from_workers(&meta).is_empty());
    }

    /// A run placed on a remote host writes its ledger THERE. Its
    /// transcript is mirrored back here, so a local-only read recovers a
    /// degraded snapshot of each flow at best — and nothing at all for a
    /// flow the transcript never carried. The remote ledger must join the
    /// LEDGER side of the merge, so its finalized copy wins over the
    /// mirrored transcript's, exactly as a local ledger's would.
    #[test]
    fn remote_ledger_flows_join_the_ledger_side_and_win_over_the_transcript() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let run = seed_record("run_placed", "wf", tmp.path());
        store.create(run, "").unwrap();

        // The mirrored transcript carries a mid-flight snapshot of flow 1
        // and nothing else. There is no local ledger at all.
        let shared_id = FlowId::from(1u128);
        let mut snapshot = flow(shared_id, Some("run_placed"), "api.anthropic.com");
        snapshot.duration_ms = Some(0);
        let transcript = tmp.path().join("t.jsonl");
        write_transcript(&transcript, &[snapshot]);
        store
            .append_step_result(
                "run_placed",
                &rupu_orchestrator::runs::StepResultRecord {
                    run_outcome: None,
                    step_id: "s1".into(),
                    run_id: "run_placed".into(),
                    transcript_path: transcript.clone(),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: rupu_orchestrator::runs::StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                    loop_iteration: None,
                    host: None,
                },
            )
            .unwrap();

        // The remote ledger holds the finalized copy of flow 1, plus a flow
        // the transcript never saw.
        let mut finalized = flow(shared_id, Some("run_placed"), "api.anthropic.com");
        finalized.duration_ms = Some(1234);
        let remote_only = flow(FlowId::from(2u128), Some("run_placed"), "api.github.com");

        let (merged, dropped) = run_scoped_flows_and_dropped_with(
            &store,
            "run_placed",
            tmp.path(),
            tmp.path(),
            &rupu_netflow::ledger::TimeRange::unbounded(),
            vec![finalized, remote_only],
            3,
        );

        assert_eq!(merged.len(), 2, "both remote flows survive");
        let one = merged.iter().find(|f| f.id == shared_id).unwrap();
        assert_eq!(
            one.duration_ms,
            Some(1234),
            "the remote LEDGER's finalized copy must win over the mirrored transcript snapshot"
        );
        assert_eq!(
            dropped, 3,
            "the remote's dropped count is carried, not discarded"
        );
    }

    /// The existing local-only contract must not shift: with no remote
    /// flows the function behaves exactly as before.
    #[test]
    fn no_remote_flows_leaves_the_local_read_unchanged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let run = seed_record("run_local", "wf", tmp.path());
        store.create(run, "").unwrap();

        let (merged, dropped) = run_scoped_flows_and_dropped_with(
            &store,
            "run_local",
            tmp.path(),
            tmp.path(),
            &rupu_netflow::ledger::TimeRange::unbounded(),
            Vec::new(),
            0,
        );

        assert!(merged.is_empty());
        assert_eq!(dropped, 0);
    }

    #[test]
    fn merge_adds_flows_the_ledger_read_never_produced() {
        // Simulates the ledger-degrade case this merge exists for now:
        // `netflow_sink::for_run` couldn't open the run's ledger file, so
        // that run's sink fell back to transcript-only capture — the
        // ledger read for this run comes back empty (`Vec::new()` below),
        // and the transcript is the only place this flow was ever
        // recorded. The flow's origin (`Scm` here) is incidental: the
        // merge doesn't special-case it, unlike the old run-id-field
        // filter it replaced.
        let tmp = tempfile::TempDir::new().unwrap();
        let scm_flow = {
            let mut f = flow(FlowId::new(), None, "api.github.com");
            f.ctx.origin = Origin::Scm("github".into());
            f
        };
        let transcript = tmp.path().join("step.jsonl");
        write_transcript(&transcript, std::slice::from_ref(&scm_flow));

        let merged = merge_with_transcript(Vec::new(), &[transcript]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, scm_flow.id);
        assert_eq!(merged[0].host, "api.github.com");
    }

    #[test]
    fn merge_prefers_the_ledgers_finalized_copy_on_id_collision() {
        // Same FlowId in both: the ledger's copy already folded in a
        // `Complete` line (finalized bytes_in/duration_ms); the
        // transcript's copy is the pre-completion snapshot. The ledger
        // must win, not "whichever was inserted first" by accident.
        let tmp = tempfile::TempDir::new().unwrap();
        let id = FlowId::new();

        let mut stale = flow(id, Some("r"), "api.anthropic.com");
        stale.bytes_in = None;
        stale.body_complete = false;

        let mut finalized = flow(id, Some("r"), "api.anthropic.com");
        finalized.bytes_in = Some(9999);
        finalized.body_complete = true;

        let transcript = tmp.path().join("step.jsonl");
        write_transcript(&transcript, &[stale]);

        let merged = merge_with_transcript(vec![finalized], &[transcript]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].bytes_in, Some(9999));
        assert!(merged[0].body_complete);
    }

    #[test]
    fn merge_skips_an_unreadable_transcript_path_without_failing() {
        let ledger_flow = flow(FlowId::new(), Some("r"), "api.anthropic.com");
        let merged = merge_with_transcript(
            vec![ledger_flow.clone()],
            &[PathBuf::from("/nonexistent/dir/gone.jsonl")],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, ledger_flow.id);
    }

    // ── AsnCache (Fix 5, netflow Plan 3 review round 3) ─────────────────────
    //
    // `#[serial_test::serial(rupu_home_env)]` mirrors
    // `rupu-netflow/src/asn/acquire.rs`'s own `RUPU_HOME`-mutating test —
    // these are the only tests in this file that touch the real filesystem
    // ASN path, so serializing them against each other (they're all in the
    // same test binary) is enough; nothing else in this module's test set
    // reads `asn_db_path()`.

    fn write_asn_table(path: &std::path::Path, tsv: &str) {
        let table = rupu_netflow::AsnTable::compact_from_tsv(std::io::Cursor::new(tsv)).unwrap();
        table.write(path).unwrap();
    }

    #[test]
    #[serial_test::serial(rupu_home_env)]
    fn asn_cache_reuses_the_parsed_table_when_mtime_is_unchanged() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("RUPU_HOME", tmp.path());
        let db_path = rupu_netflow::asn::asn_db_path().unwrap();
        write_asn_table(&db_path, "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n");

        let cache = AsnCache::default();
        let first = cache.load().expect("table loads");
        let second = cache.load().expect("table loads again");
        std::env::remove_var("RUPU_HOME");

        assert!(
            Arc::ptr_eq(&first, &second),
            "unchanged mtime must reuse the SAME parsed Arc, not reparse the file"
        );
    }

    #[test]
    #[serial_test::serial(rupu_home_env)]
    fn asn_cache_reparses_when_the_files_mtime_moves() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("RUPU_HOME", tmp.path());
        let db_path = rupu_netflow::asn::asn_db_path().unwrap();
        write_asn_table(&db_path, "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n");

        let cache = AsnCache::default();
        let first = cache.load().expect("table loads");

        // A real refresh both rewrites the content AND advances the mtime
        // (fresh download, fresh write) — mimic both.
        write_asn_table(&db_path, "2.0.0.0\t2.0.0.255\t7018\tUS\tATT\n");
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&db_path)
            .unwrap()
            .set_modified(later)
            .unwrap();

        let second = cache.load().expect("table reloads");
        std::env::remove_var("RUPU_HOME");

        assert!(
            !Arc::ptr_eq(&first, &second),
            "a moved mtime must trigger a reparse, not reuse the stale Arc"
        );
        assert!(
            first
                .lookup("1.0.0.7".parse::<std::net::IpAddr>().unwrap())
                .is_some(),
            "sanity: the first load resolved the original table's range"
        );
        assert!(
            second
                .lookup("2.0.0.7".parse::<std::net::IpAddr>().unwrap())
                .is_some(),
            "the second load resolved the NEW table's range, proving a reparse happened"
        );
    }

    #[test]
    #[serial_test::serial(rupu_home_env)]
    fn asn_cache_invalidate_forces_a_reparse_on_the_next_load() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("RUPU_HOME", tmp.path());
        let db_path = rupu_netflow::asn::asn_db_path().unwrap();
        write_asn_table(&db_path, "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n");

        let cache = AsnCache::default();
        let first = cache.load().expect("table loads");
        cache.invalidate();
        let second = cache.load().expect("table reloads after invalidate");
        std::env::remove_var("RUPU_HOME");

        assert!(
            !Arc::ptr_eq(&first, &second),
            "invalidate() must force a fresh parse on the next load even with an \
             unchanged mtime — the safety net for a refresh that completes within \
             the same mtime-resolution window as the copy already cached"
        );
    }

    #[test]
    #[serial_test::serial(rupu_home_env)]
    fn asn_cache_with_no_table_present_returns_none_not_a_panic() {
        // Point `RUPU_HOME` at an empty tempdir so "no file" is guaranteed
        // rather than depending on whatever happens to be on the machine
        // running the tests.
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("RUPU_HOME", tmp.path());
        let cache = AsnCache::default();
        let result = cache.load();
        std::env::remove_var("RUPU_HOME");

        assert!(result.is_none());
    }

    // ── RunMetaCache (global-scope RunMetaIndex memoization) ────────────────
    //
    // Same `Arc::ptr_eq` observability contract as the AsnCache tests
    // above: a cache hit returns the SAME Arc, a rebuild returns a fresh
    // one, so reuse-vs-rebuild is directly assertable without any
    // instrumentation counter.

    fn seed_record(id: &str, workflow: &str, workspace: &std::path::Path) -> RunRecord {
        RunRecord {
            id: id.into(),
            workflow_name: workflow.into(),
            status: rupu_orchestrator::runs::RunStatus::Completed,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: "ws_meta_cache".into(),
            workspace_path: workspace.to_path_buf(),
            transcript_dir: workspace.join(".rupu/transcripts"),
            started_at: chrono::DateTime::from_timestamp(50, 0).unwrap(),
            finished_at: Some(chrono::DateTime::from_timestamp(2_000, 0).unwrap()),
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            final_output: None,
            loop_progress: Default::default(),
        }
    }

    fn ids(list: &[&str]) -> std::collections::HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn run_meta_cache_reuses_the_index_while_the_stores_are_unchanged() {
        let global = tempfile::TempDir::new().unwrap();
        let store = RunStore::new(global.path().join("runs"));
        store
            .create(seed_record("run-a", "review-wf", global.path()), "x: 1\n")
            .unwrap();

        let cache = RunMetaCache::default();
        let first = global_run_meta_cached(&cache, global.path(), &store, &ids(&["run-a"]));
        let second = global_run_meta_cached(&cache, global.path(), &store, &ids(&["run-a"]));

        assert!(
            Arc::ptr_eq(&first, &second),
            "unchanged stores + fully-accounted ids must reuse the SAME index Arc"
        );
        assert_eq!(
            first.attribution("run-a"),
            (Some("run-a".into()), Some("review-wf".into()))
        );
    }

    #[test]
    fn run_meta_cache_rebuilds_when_a_run_store_changes() {
        let global = tempfile::TempDir::new().unwrap();
        let store = RunStore::new(global.path().join("runs"));
        store
            .create(seed_record("run-a", "review-wf", global.path()), "x: 1\n")
            .unwrap();

        let cache = RunMetaCache::default();
        let first = global_run_meta_cached(&cache, global.path(), &store, &ids(&["run-a"]));
        assert_eq!(first.attribution("run-b"), (None, None), "sanity");

        store
            .create(seed_record("run-b", "nightly-wf", global.path()), "x: 1\n")
            .unwrap();
        let second =
            global_run_meta_cached(&cache, global.path(), &store, &ids(&["run-a", "run-b"]));

        assert_eq!(
            second.attribution("run-b"),
            (Some("run-b".into()), Some("nightly-wf".into())),
            "a run created after the cache warmed must be attributed on the next read"
        );
    }

    #[test]
    fn run_meta_cache_rebuilds_for_an_unaccounted_ledger_id_despite_unchanged_mtimes() {
        // `create_sub_run` writes only under `<runs>/<parent>/sub/` — it
        // touches neither `run.json` nor `step_results.jsonl`, so the
        // store fingerprint alone cannot see it. The honesty backstop is
        // the ledger-id check: a tagged flow set naming an id the cached
        // index can't account for forces one rebuild BEFORE that id is
        // ever served as "unknown".
        let global = tempfile::TempDir::new().unwrap();
        let store = RunStore::new(global.path().join("runs"));
        store
            .create(seed_record("run-a", "review-wf", global.path()), "x: 1\n")
            .unwrap();

        let cache = RunMetaCache::default();
        let first = global_run_meta_cached(&cache, global.path(), &store, &ids(&["run-a"]));

        let (sub_id, _) = store.create_sub_run("run-a", "reviewer").unwrap();
        let second =
            global_run_meta_cached(&cache, global.path(), &store, &ids(&["run-a", &sub_id]));

        assert!(
            !Arc::ptr_eq(&first, &second),
            "an unaccounted ledger id must force a rebuild even with an unchanged fingerprint"
        );
        assert_eq!(
            second.attribution(&sub_id),
            (Some("run-a".into()), Some("review-wf".into())),
            "the sub-agent's ledger id must fold into its dispatching run"
        );
    }

    #[test]
    fn run_meta_cache_is_not_defeated_by_a_genuinely_unknown_id() {
        // An orphaned ledger (its run record deleted, or never recorded)
        // stays unknown across rebuilds. Once one rebuild has proven an id
        // unresolvable, that id must be remembered as known-unknown — NOT
        // trigger a fresh rebuild on every subsequent request, which would
        // silently degrade the cache to a per-request full walk forever on
        // any installation with a single orphan.
        let global = tempfile::TempDir::new().unwrap();
        let store = RunStore::new(global.path().join("runs"));
        store
            .create(seed_record("run-a", "review-wf", global.path()), "x: 1\n")
            .unwrap();

        let cache = RunMetaCache::default();
        let first = global_run_meta_cached(
            &cache,
            global.path(),
            &store,
            &ids(&["run-a", "orphan-run"]),
        );
        let second = global_run_meta_cached(
            &cache,
            global.path(),
            &store,
            &ids(&["run-a", "orphan-run"]),
        );

        assert!(
            Arc::ptr_eq(&first, &second),
            "a known-unknown id must be served from cache, not rebuilt every request"
        );
        assert_eq!(first.attribution("orphan-run"), (None, None));
    }
}
