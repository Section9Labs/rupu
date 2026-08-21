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
    global_netflow_dir, is_per_run_ledger_path, project_local_netflow_dir, AsnInfo, AsnTable,
    FlowId, FlowRecord, NetflowPaths,
};
use rupu_orchestrator::runs::RunStore;
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
}

impl FlowView {
    pub fn from_flow(flow: FlowRecord, table: Option<&AsnTable>) -> Self {
        let asn = flow.peer_ip.and_then(|ip| table.and_then(|t| t.lookup(ip)));
        Self { flow, asn }
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

/// Build the response, enriching with `table` if given.
///
/// `table` is a parameter rather than an internal `load_asn_table()` call so
/// this function is testable in both directions (table present / absent)
/// without touching the real `$RUPU_HOME` on disk — every caller (the route
/// handlers below) loads the table itself and passes it in. `range` is
/// echoed into the response's `window` field (see [`WindowEcho`]) — it is
/// NOT re-applied to `flows` here; every caller has already filtered
/// `flows` (via `read_flows_in_range` or an explicit `range.contains`
/// pass) before calling this function, so this is purely reporting what
/// was already enforced, not enforcing it again.
pub(crate) fn build_response(
    flows: Vec<FlowRecord>,
    dropped: u64,
    table: Option<&AsnTable>,
    range: &rupu_netflow::ledger::TimeRange,
) -> NetflowResponse {
    let hosts = rupu_netflow::ledger::host_rollup(&flows);
    NetflowResponse {
        flows: flows
            .into_iter()
            .map(|f| FlowView::from_flow(f, table))
            .collect(),
        hosts,
        dropped_total: dropped,
        window: WindowEcho::from(range),
        asn_loaded: table.is_some(),
    }
}

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

/// The ledger's run-scoped flows for `run_id` — unioned across every id
/// [`run_and_unit_ids`] names, each resolved through [`resolve_ledger_paths`]
/// — merged with the run's own transcript (see the module doc for why the
/// ledger alone under-reports), plus the summed dropped-count from those
/// same files. Synchronous — every caller runs this through
/// [`run_blocking`], never inline on the async task. Shared by
/// [`collect_run_netflow`] (which wraps this into the enriched
/// [`NetflowResponse`]) and the graph endpoint's `run:` scope (which only
/// needs raw [`FlowRecord`]s to feed [`rupu_netflow::ledger::graph_view`]).
fn run_scoped_flows_and_dropped(
    store: &RunStore,
    run_id: &str,
    workspace: &StdPath,
    global_dir: &StdPath,
    range: &rupu_netflow::ledger::TimeRange,
) -> (Vec<FlowRecord>, u64) {
    let mut all = Vec::new();
    let mut dropped = 0u64;
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
fn collect_run_netflow(
    store: &RunStore,
    run_id: &str,
    workspace: &StdPath,
    global_dir: &StdPath,
    cache: &AsnCache,
    range: &rupu_netflow::ledger::TimeRange,
) -> NetflowResponse {
    let (merged, dropped) =
        run_scoped_flows_and_dropped(store, run_id, workspace, global_dir, range);
    let table = load_asn_table(cache);
    build_response(merged, dropped, table.as_deref(), range)
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

/// Every run+unit id (see [`run_and_unit_ids`]) belonging to a project's
/// OWN run records, named via their `workspace_path` field — never
/// inferred from scanning a directory's file names. Two sources, mirroring
/// `resolve_run_location`'s `Global`/`ProjectLocal` split: `global_store`
/// (rooted at `<global_dir>/runs`) filtered to `workspace_path ==
/// workspace`, and a project-local store rooted at `<workspace>/.rupu/runs`
/// (whatever it holds belongs to this project by construction — there is
/// no `workspace_path` filter to apply there).
///
/// This is the attribution-safety property the id-driven global-fallback
/// pass depends on: a run belonging to some OTHER project never
/// contributes an id here just because its ledger happens to live in the
/// same shared `<global_dir>/netflow/` directory — only a run record that
/// itself declares THIS workspace as its own does.
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
/// [`run_and_unit_ids`] is called against the SAME store each run id was
/// discovered from — never unconditionally against `global_store` — so a
/// project-local run's dispatched steps/units are looked up in the
/// project-local store's own `step_results.jsonl`, not the global one's
/// (which has no record of it at all, and `unwrap_or_default()` would
/// silently swallow that miss, resolving only the parent run's own id and
/// dropping every dispatched step's own global-fallback ledger). Run scope
/// ([`run_scoped_flows_and_dropped`]) already gets this right by
/// constructing the correct store per branch; this mirrors it.
///
/// Cost: `global_store.list()` is one `read_dir` plus one small
/// `run.json` parse per run IN THE WHOLE GLOBAL STORE (not filtered by
/// workspace at the I/O layer — `RunStore` has no index by
/// `workspace_path`), so this is O(total runs across every project) on
/// every project-scope request, not O(this project's own runs). At ~500
/// runs total that's ~500 small file opens. For each of this project's own
/// k run ids (k ≤ total runs) found in either store, one additional
/// `read_step_results` call (a small `step_results.jsonl` read, `Ok(vec![])`
/// if the run never dispatched a step) discovers its units' own ids — see
/// the caller's doc comment for what happens to the ids after this.
///
/// On top of that (added when [`run_and_unit_ids`] started folding in
/// sub-agent ids): `run_and_unit_ids` now issues one `sub_run_ids`
/// directory listing (a `read_dir`, cheap and Ok(empty)-on-missing) per id
/// it already has — the run itself, plus every step/item id — and
/// `sub_run_ids_recursive` issues one more per sub-agent id it discovers
/// while walking that run's dispatch tree. So the true cost is the above
/// PLUS one `read_dir` per (run, step/item, sub-agent-at-any-depth) id
/// across this project's own k runs — bounded in practice by how deep/wide
/// any one run's dispatch tree actually grew (`MAX_SUB_RUN_RECURSION_DEPTH`
/// backstops the pathological case), not by the total run count.
fn project_run_and_unit_ids(global_store: &RunStore, workspace: &StdPath) -> Vec<String> {
    let workspace = canonicalize_or_self(workspace);
    let mut ids: Vec<String> = Vec::new();

    for r in global_store.list().unwrap_or_default() {
        if canonicalize_or_self(&r.workspace_path) == workspace {
            ids.extend(run_and_unit_ids(global_store, &r.id));
        }
    }

    let local_store = RunStore::new(workspace.join(".rupu").join("runs"));
    for r in local_store.list().unwrap_or_default() {
        ids.extend(run_and_unit_ids(&local_store, &r.id));
    }

    ids.sort();
    ids.dedup();
    ids
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
/// Cost, continued from [`project_run_and_unit_ids`]'s doc comment (which
/// already accounts for the `RunStore::list()`/`read_step_results` calls
/// that produce the id set): one `read_flows_in_range` per distinct
/// resolved global path on top of that. This scales with the TOTAL run
/// count in the global store (the `workspace_path` filter inside
/// `project_run_and_unit_ids`) and with this project's own run/unit count,
/// never with the size of the global directory itself — the opposite
/// scaling from a `read_dir` over that directory, which is what made a
/// directory-scan-based fix unworkable here (a project has no way to
/// tell, from the global directory's contents alone, which files are its
/// own — see the removed KNOWN GAP note this function replaces). All
/// synchronous, but always run through [`run_blocking`] by the caller,
/// same as every other read in this module, so it never stalls the async
/// task.
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
    global_store: &RunStore,
    workspace: &StdPath,
    global_dir: &StdPath,
    range: &rupu_netflow::ledger::TimeRange,
) -> (Vec<(String, FlowRecord)>, u64) {
    let mut paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for id in project_run_and_unit_ids(global_store, workspace) {
        paths.insert(NetflowPaths::for_run(&global_netflow_dir(global_dir), &id).flows);
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

/// Every flow at project scope: the workspace-local ledger directory
/// (whole-directory scan, [`read_all_run_ledgers_in_dir`]) UNIONED with
/// the id-driven global-fallback recovery pass
/// ([`project_fallback_flows_and_dropped`]) for runs whose ledgers landed
/// in `<global_dir>/netflow/` instead — see that function's doc for the
/// full cost accounting and what it deliberately cannot recover. Shared
/// by [`get_project_netflow`] and the graph endpoint's `project:` scope so
/// the table and the graph are always built from the identical set,
/// mirroring [`run_scoped_flows_and_dropped`]'s equivalent role at run
/// scope.
///
/// Guards the same `$HOME`/`RUPU_HOME` collision [`resolve_ledger_paths`]
/// guards at run scope: when the workspace-local netflow dir and the
/// global one are literally the same directory (a workspace registered at
/// `$HOME` with the default `RUPU_HOME=~/.rupu`), the whole-directory scan
/// above has already read every file in it, so the id-driven pass is
/// skipped entirely rather than re-reading (and double-counting) that
/// same directory a second time.
fn project_scoped_flows_and_dropped(
    global_store: &RunStore,
    workspace: &StdPath,
    global_dir: &StdPath,
    range: &rupu_netflow::ledger::TimeRange,
) -> (Vec<(String, FlowRecord)>, u64) {
    let local_dir = project_local_netflow_dir(workspace);
    let (mut flows, mut dropped) = read_all_run_ledgers_in_dir(&local_dir, range);

    if canonicalize_or_self(&local_dir) == canonicalize_or_self(&global_netflow_dir(global_dir)) {
        return (flows, dropped);
    }

    let (fallback_flows, fallback_dropped) =
        project_fallback_flows_and_dropped(global_store, workspace, global_dir, range);
    flows.extend(fallback_flows);
    dropped += fallback_dropped;
    (flows, dropped)
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
/// [`project_scoped_flows_and_dropped`]'s doc comment for the full
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
    let workspace = workspace_for_project(&state, &project_id)?;
    let cache = Arc::clone(&state.asn_cache);
    let store = Arc::clone(&state.run_store);
    let global_dir = state.global_dir.clone();
    let resp = run_blocking(move || {
        let (flows, dropped) =
            project_scoped_flows_and_dropped(&store, &workspace, &global_dir, &range);
        let flows: Vec<FlowRecord> = flows.into_iter().map(|(_, f)| f).collect();
        let table = load_asn_table(&cache);
        build_response(flows, dropped, table.as_deref(), &range)
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
    let global_dir = state.global_dir.clone();
    let cache = Arc::clone(&state.asn_cache);
    let resp = run_blocking(move || {
        let (flows, dropped) = read_all_workspaces_sync(&global_dir, &range);
        let flows: Vec<FlowRecord> = flows.into_iter().map(|(_, f)| f).collect();
        let table = load_asn_table(&cache);
        build_response(flows, dropped, table.as_deref(), &range)
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
            let resp = run_netflow_from_host(s, &host_id, run_id, range).await?;
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
        // Shares `project_scoped_flows_and_dropped` with `get_project_netflow`
        // so the graph and the table are always built from the identical
        // set, including the id-driven global-fallback recovery pass — see
        // that function's doc comment.
        run_blocking(move || {
            project_scoped_flows_and_dropped(&store, &workspace, &global_dir, &range).0
        })
        .await?
    } else {
        let global_dir = state.global_dir.clone();
        run_blocking(move || read_all_workspaces_sync(&global_dir, &range).0).await?
    };
    Ok(Json(rupu_netflow::ledger::graph_view(&flows)))
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
) -> ApiResult<NetflowResponse> {
    let conn = resolve_host(s, host_id)?;
    let path = format!("/api/runs/{id}/netflow{}", time_range_query_string(range));
    let value = conn.proxy_get_json(&path).await.map_err(|e| match e {
        HostConnectorError::NotFound(m) => ApiError::not_found(m),
        HostConnectorError::Unreachable(m) => {
            ApiError::internal(format!("host {host_id} unreachable: {m}"))
        }
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
    Ok(resp)
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
    resp.hosts = rupu_netflow::ledger::host_rollup(
        &resp
            .flows
            .iter()
            .map(|v| v.flow.clone())
            .collect::<Vec<_>>(),
    );
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
fn time_range_query_string(range: &rupu_netflow::ledger::TimeRange) -> String {
    let mut parts = Vec::new();
    if let Some(from) = range.from {
        parts.push(format!("from={}", urlencoding_rfc3339(from)));
    }
    if let Some(to) = range.to {
        parts.push(format!("to={}", urlencoding_rfc3339(to)));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
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
    match resolve_run_location(&s, &run_id).await {
        RunLocation::Global => {
            let run = s
                .run_store
                .load(&run_id)
                .map_err(|e| run_not_found_or_internal(&run_id, e))?;
            let store = Arc::clone(&s.run_store);
            let rid = run_id.clone();
            let workspace = run.workspace_path.clone();
            let global_dir = s.global_dir.clone();
            let cache = Arc::clone(&s.asn_cache);
            let resp = run_blocking(move || {
                collect_run_netflow(&store, &rid, &workspace, &global_dir, &cache, &range)
            })
            .await?;
            Ok(Json(resp))
        }
        RunLocation::ProjectLocal { path } => {
            let rid = run_id.clone();
            let global_dir = s.global_dir.clone();
            let cache = Arc::clone(&s.asn_cache);
            let resp = run_blocking(move || {
                let store = RunStore::new(path.join(".rupu").join("runs"));
                collect_run_netflow(&store, &rid, &path, &global_dir, &cache, &range)
            })
            .await?;
            Ok(Json(resp))
        }
        RunLocation::Host { host_id } => run_netflow_from_host(&s, &host_id, &run_id, &range)
            .await
            .map(Json),
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
                build_response(Vec::new(), 0, table.as_deref(), &range)
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
        let mut flow_ok = flow(FlowId::from_parts(1, 1), Some("run-40"), "api.anthropic.com");
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
                },
                FlowView {
                    flow: flow_err,
                    asn: None,
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
        };
        check_fixture("netflow_run.json", &response);
    }

    #[test]
    fn build_response_wires_a_server_computed_host_rollup() {
        let f = flow(FlowId::new(), Some("r"), "api.anthropic.com");
        let resp = build_response(
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

        let resp = build_response(vec![f], 0, None, &range);

        assert_eq!(resp.window.from, range.from);
        assert_eq!(resp.window.to, range.to);
    }

    /// The unbounded case: both sides `null`, not omitted — a caller must
    /// be able to tell "no filter was applied" from "the response has no
    /// opinion", which an omitted key would blur.
    #[test]
    fn build_response_with_an_unbounded_range_echoes_a_null_window() {
        let f = flow(FlowId::new(), Some("r"), "api.anthropic.com");

        let resp = build_response(
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

        let mut resp = build_response(
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

        let resp = build_response(
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

        let resp = build_response(
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
}
