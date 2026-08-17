//! Netflow API — run-scoped read of the workspace ledger + run transcript,
//! merged, with read-time ASN enrichment.
//!
//! ASN is resolved HERE, at render time, not stamped into the record (spec
//! §6.2). A table that arrives later therefore improves every historical
//! flow with no backfill.
//!
//! ## Why this reads the ledger AND the transcript (do not "simplify")
//!
//! Only *provider* flows carry `ctx.run_id` — `Origin::Scm`, `Auth`/
//! `System`, `Update` and `Cp` flows are ALWAYS `run_id: None` by design
//! (see `rupu_netflow::ctx::FlowCtx::system`). Filtering the
//! workspace-level ledger by `run_id` alone therefore silently drops every
//! SCM/auth/update call the process made *while this run was active* — the
//! response would look complete but isn't. The run's own transcript carries
//! an `Event::NetFlow` line for every flow the process observed during the
//! run regardless of `ctx.run_id`, which is exactly the set that recovers
//! that gap. [`merge_with_transcript`] does the merge; see its doc for the
//! dedup rule.
//!
//! ## Blocking I/O never runs on the async task
//!
//! `rupu-netflow` has no ledger rotation, retention or compaction — a
//! ledger grows unbounded for the life of a `cp serve` daemon. Every
//! ledger/transcript/ASN-table read in this module is a synchronous
//! `std::fs` call, so running it directly on an async handler would block
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
use rupu_netflow::{AsnInfo, AsnTable, FlowId, FlowRecord, NetflowPaths};
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

#[derive(Debug, Serialize, Deserialize)]
pub struct NetflowResponse {
    pub flows: Vec<FlowView>,
    /// Per-host rollup, computed here rather than in the browser so the
    /// percentile and unknown-bytes logic has exactly ONE implementation
    /// (`rupu_netflow::ledger::host_rollup`).
    pub hosts: Vec<rupu_netflow::ledger::HostRollup>,
    /// Records lost to writer-channel overflow. Surfaced so the UI can say
    /// "N flows dropped" instead of quietly under-reporting.
    pub dropped: u64,
    /// Whether the ASN table was available for this request. `false` means
    /// the `asn` fields are absent because we could not look them up, NOT
    /// because the flows had no ASN.
    pub asn_loaded: bool,
}

/// Flows belonging to one run, as recorded in the ledger. Callers almost
/// always want [`merge_with_transcript`] applied on top of this — see the
/// module doc for why the ledger alone under-reports.
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

/// Merge the ledger's run-scoped flows with the `Event::NetFlow` lines
/// found in the run's own transcript files, deduped by [`FlowId`].
///
/// On an id collision the LEDGER's copy wins: `ledger::read_flows` folds
/// any `LedgerLine::Complete` into it, so its `bytes_in`/`duration_ms` are
/// finalized, whereas the transcript's copy is only the snapshot taken at
/// the moment the flow was first recorded (a streaming flow is written to
/// the transcript before it completes). Transcript-only flows — everything
/// with `ctx.run_id: None` that the ledger's run-id filter dropped — are
/// added as-is; there is no more-authoritative copy of them anywhere.
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
/// handlers below) loads the table itself and passes it in.
pub(crate) fn build_response(
    flows: Vec<FlowRecord>,
    dropped: u64,
    table: Option<&AsnTable>,
) -> NetflowResponse {
    let hosts = rupu_netflow::ledger::host_rollup(&flows);
    NetflowResponse {
        flows: flows
            .into_iter()
            .map(|f| FlowView::from_flow(f, table))
            .collect(),
        hosts,
        dropped,
        asn_loaded: table.is_some(),
    }
}

/// The ledger's run-scoped flows for `run_id`, merged with the run's own
/// transcript (see the module doc for why the ledger alone under-reports),
/// plus the workspace's total dropped-count — both from ONE pass over the
/// ledger file via [`rupu_netflow::ledger::read_flows_and_dropped`], not two
/// (re-scanning the same file just to recount `dropped` doubles exactly the
/// I/O the "Blocking I/O" module-doc note is about). Synchronous — every
/// caller runs this through [`run_blocking`], never inline on the async
/// task. Shared by [`collect_run_netflow`] (which wraps this into the
/// enriched [`NetflowResponse`]) and the graph endpoint's `run:` scope
/// (which only needs raw [`FlowRecord`]s to feed
/// [`rupu_netflow::ledger::graph_view`]).
fn run_scoped_flows_and_dropped(
    store: &RunStore,
    run_id: &str,
    workspace: &StdPath,
) -> (Vec<FlowRecord>, u64) {
    // One ledger file per run (`NetflowPaths::for_run`) — the file is
    // already scoped to `run_id`, so no `filter_by_run` pass is needed
    // here any more (it's still exported/used by tests exercising the
    // merge logic directly against an in-memory flow list).
    let paths = NetflowPaths::for_run(&workspace.join(".rupu/netflow"), run_id);
    let (all, dropped) =
        rupu_netflow::ledger::read_flows_and_dropped(&paths.flows).unwrap_or_default();
    let transcript_paths = crate::usage::run_transcript_paths(store, run_id);
    let merged = merge_with_transcript(all, &transcript_paths);
    (merged, dropped)
}

/// Read the ledger + run transcript for a run whose artifacts live under
/// `workspace` (a project root, `<workspace>/.rupu/netflow/flows.jsonl` and
/// whatever `store` reports as this run's step transcript paths), scope to
/// `run_id`, merge, and build the response. A missing ledger is an empty
/// result, not an error. Synchronous — see [`run_scoped_flows_and_dropped`].
fn collect_run_netflow(
    store: &RunStore,
    run_id: &str,
    workspace: &StdPath,
    cache: &AsnCache,
) -> NetflowResponse {
    let (merged, dropped) = run_scoped_flows_and_dropped(store, run_id, workspace);
    let table = load_asn_table(cache);
    build_response(merged, dropped, table.as_deref())
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
/// `<run_id>.jsonl` per run, not one shared `flows.jsonl`) under
/// `netflow_dir` into one flow list + summed dropped-count. A directory
/// that doesn't exist yet (no run has ever written a ledger there)
/// degrades to `([], 0)`, the same "missing data" tolerance every other
/// read in this module already relies on. The `.gitignore` that
/// `NetflowPaths::ensure_dir` drops into every such directory is skipped
/// (no `.jsonl` extension).
fn read_all_run_ledgers_in_dir(netflow_dir: &StdPath) -> (Vec<FlowRecord>, u64) {
    let mut flows = Vec::new();
    let mut dropped = 0u64;
    let Ok(entries) = std::fs::read_dir(netflow_dir) else {
        return (flows, dropped);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let (f, d) = rupu_netflow::ledger::read_flows_and_dropped(&path).unwrap_or_default();
        flows.extend(f);
        dropped += d;
    }
    (flows, dropped)
}

/// Every flow across every registered workspace's ledger directory, and the
/// summed dropped-count. Mirrors `coverage.rs`'s `list_coverage`: a
/// workspace whose path is gone/unreadable is skipped (its ledger read
/// degrades to empty via [`read_all_run_ledgers_in_dir`]'s own
/// missing-directory tolerance), never a hard error.
///
/// Synchronous — always run through [`run_blocking`] (this is the read that
/// scales with the number of registered workspaces AND with the number of
/// runs each one has, so it's the one most worth keeping off the async
/// task).
///
/// Does NOT union in a CP-daemon-wide ledger the way an earlier revision of
/// this function did (Fix 1, netflow Plan 3 review): per the netflow
/// per-run plan, `cp serve`'s own fleet/ASN-refresh traffic is
/// deliberately not recorded at all any more — it has no run to attribute
/// to and installing a daemon-lifetime sink is exactly the "first run
/// wins" defect this plan removes (see `rupu-cli/src/cmd/cp.rs`'s deleted
/// `http::init` call). There is no global ledger file left to read.
fn read_all_workspaces_sync(global_dir: &StdPath) -> (Vec<FlowRecord>, u64) {
    let workspaces = (WorkspaceStore {
        root: global_dir.join("workspaces"),
    })
    .list()
    .unwrap_or_default();

    let mut flows = Vec::new();
    let mut dropped = 0u64;
    for w in &workspaces {
        let wp = std::path::Path::new(&w.path);
        let (f, d) = read_all_run_ledgers_in_dir(&wp.join(".rupu/netflow"));
        flows.extend(f);
        dropped += d;
    }

    (flows, dropped)
}

/// `GET /api/projects/:id/netflow` — every flow in a workspace's ledger,
/// including `system`-origin egress (`run_id: None`) that has no run to
/// attach to (e.g. a `rupu run` invocation's own update-notice check, which
/// carries `Origin::Update` but no `run_id`). Unlike run scope, this reads
/// the ledger directly with no `run_id` filter.
///
/// This does NOT include the CP daemon's own global ledger
/// (`$RUPU_HOME/netflow/flows.jsonl`, see [`read_all_workspaces_sync`]'s
/// doc comment) — `cp serve`'s own fleet traffic, `Origin::Cp`, and any
/// `Origin::Update`/ASN-refresh egress it produces are not this project's
/// traffic, so they surface only at global scope, never here.
async fn get_project_netflow(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> ApiResult<Json<NetflowResponse>> {
    maybe_refresh_asn(&netflow_config(&state), &state.asn_cache);
    let workspace = workspace_for_project(&state, &project_id)?;
    let cache = Arc::clone(&state.asn_cache);
    let resp = run_blocking(move || {
        let (flows, dropped) = read_all_run_ledgers_in_dir(&workspace.join(".rupu/netflow"));
        let table = load_asn_table(&cache);
        build_response(flows, dropped, table.as_deref())
    })
    .await?;
    Ok(Json(resp))
}

/// `GET /api/netflow` — the union across every registered workspace.
async fn get_global_netflow(State(state): State<AppState>) -> ApiResult<Json<NetflowResponse>> {
    maybe_refresh_asn(&netflow_config(&state), &state.asn_cache);
    let global_dir = state.global_dir.clone();
    let cache = Arc::clone(&state.asn_cache);
    let resp = run_blocking(move || {
        let (flows, dropped) = read_all_workspaces_sync(&global_dir);
        let table = load_asn_table(&cache);
        build_response(flows, dropped, table.as_deref())
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
}

/// Raw (unenriched) flows for the graph endpoint's `run:` scope — mirrors
/// `get_run_netflow`'s dispatch on [`resolve_run_location`] so a run graph
/// looks in exactly the same places the run's own netflow page does
/// (global store / project-local store / remote host / unpersisted / 404).
/// The `Global`/`ProjectLocal` branches read through [`run_blocking`]; the
/// `Host` branch is a network proxy call, not disk I/O, so it stays inline.
async fn run_scoped_flows_for_graph(s: &AppState, run_id: &str) -> ApiResult<Vec<FlowRecord>> {
    match resolve_run_location(s, run_id).await {
        RunLocation::Global => {
            let run = s
                .run_store
                .load(run_id)
                .map_err(|e| run_not_found_or_internal(run_id, e))?;
            let store = Arc::clone(&s.run_store);
            let run_id = run_id.to_string();
            let workspace = run.workspace_path.clone();
            run_blocking(move || run_scoped_flows_and_dropped(&store, &run_id, &workspace).0).await
        }
        RunLocation::ProjectLocal { path } => {
            let run_id = run_id.to_string();
            run_blocking(move || {
                let store = RunStore::new(path.join(".rupu").join("runs"));
                run_scoped_flows_and_dropped(&store, &run_id, &path).0
            })
            .await
        }
        RunLocation::Host { host_id } => {
            let resp = run_netflow_from_host(s, &host_id, run_id).await?;
            Ok(resp.flows.into_iter().map(|v| v.flow).collect())
        }
        // No artifacts anywhere to build a graph from — empty, not an
        // error, mirroring `get_run_netflow`'s `Unpersisted` branch.
        RunLocation::Unpersisted { .. } => Ok(Vec::new()),
        RunLocation::NotFound => Err(ApiError::not_found(format!("run {run_id} not found"))),
    }
}

/// `GET /api/netflow/graph?scope=` — the bipartite source↔endpoint graph
/// (`rupu_netflow::ledger::graph_view` does the actual bipartite build; this
/// only resolves `scope` to the flow set it runs over).
async fn get_netflow_graph(
    State(state): State<AppState>,
    Query(q): Query<GraphQuery>,
) -> ApiResult<Json<rupu_netflow::ledger::GraphView>> {
    let flows = if let Some(run_id) = q.scope.as_deref().and_then(|s| s.strip_prefix("run:")) {
        run_scoped_flows_for_graph(&state, run_id).await?
    } else if let Some(project_id) = q.scope.as_deref().and_then(|s| s.strip_prefix("project:")) {
        let workspace = workspace_for_project(&state, project_id)?;
        run_blocking(move || read_all_run_ledgers_in_dir(&workspace.join(".rupu/netflow")).0)
            .await?
    } else {
        let global_dir = state.global_dir.clone();
        run_blocking(move || read_all_workspaces_sync(&global_dir).0).await?
    };
    Ok(Json(rupu_netflow::ledger::graph_view(&flows)))
}

/// Proxy `GET /api/runs/:id/netflow` to a resolved host. Mirrors
/// `graph.rs`'s `run_graph_from_host` — the remote CP does the same
/// ledger+transcript merge locally and we just relay its response.
async fn run_netflow_from_host(
    s: &AppState,
    host_id: &str,
    id: &str,
) -> ApiResult<NetflowResponse> {
    let conn = resolve_host(s, host_id)?;
    let value = conn
        .proxy_get_json(&format!("/api/runs/{id}/netflow"))
        .await
        .map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Unreachable(m) => {
                ApiError::internal(format!("host {host_id} unreachable: {m}"))
            }
            other => ApiError::internal(other.to_string()),
        })?;
    serde_json::from_value(value).map_err(|e| ApiError::internal(e.to_string()))
}

/// `GET /api/runs/:id/netflow` — network flows attributed to one run, from
/// the workspace ledger merged with the run transcript (see module doc),
/// with read-time ASN enrichment and a server-computed per-host rollup.
///
/// Dispatches on [`resolve_run_location`] exactly like `run_graph`:
/// `Global`/`ProjectLocal` read locally, `Host` proxies, `Unpersisted` has
/// no workspace to read from so it returns an empty (not error) response,
/// `NotFound` → 404.
async fn get_run_netflow(
    State(s): State<AppState>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<NetflowResponse>> {
    maybe_refresh_asn(&netflow_config(&s), &s.asn_cache);
    match resolve_run_location(&s, &run_id).await {
        RunLocation::Global => {
            let run = s
                .run_store
                .load(&run_id)
                .map_err(|e| run_not_found_or_internal(&run_id, e))?;
            let store = Arc::clone(&s.run_store);
            let rid = run_id.clone();
            let workspace = run.workspace_path.clone();
            let cache = Arc::clone(&s.asn_cache);
            let resp =
                run_blocking(move || collect_run_netflow(&store, &rid, &workspace, &cache)).await?;
            Ok(Json(resp))
        }
        RunLocation::ProjectLocal { path } => {
            let rid = run_id.clone();
            let cache = Arc::clone(&s.asn_cache);
            let resp = run_blocking(move || {
                let store = RunStore::new(path.join(".rupu").join("runs"));
                collect_run_netflow(&store, &rid, &path, &cache)
            })
            .await?;
            Ok(Json(resp))
        }
        RunLocation::Host { host_id } => {
            run_netflow_from_host(&s, &host_id, &run_id).await.map(Json)
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
                build_response(Vec::new(), 0, table.as_deref())
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

    #[test]
    fn build_response_wires_a_server_computed_host_rollup() {
        let f = flow(FlowId::new(), Some("r"), "api.anthropic.com");
        let resp = build_response(vec![f], 0, None);
        assert_eq!(resp.hosts.len(), 1);
        assert_eq!(resp.hosts[0].host, "api.anthropic.com");
        assert_eq!(resp.hosts[0].calls, 1);
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

        let resp = build_response(vec![f], 0, None);

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

        let resp = build_response(vec![f], 0, Some(&table));

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
    fn merge_adds_transcript_only_flows_the_ledger_run_filter_dropped() {
        // Simulates the SCM/auth/update gap: an Scm flow is `run_id: None`
        // so it never survives `filter_by_run` against the ledger, but it
        // DID land in the run's transcript (the process saw it while the
        // run was active).
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
