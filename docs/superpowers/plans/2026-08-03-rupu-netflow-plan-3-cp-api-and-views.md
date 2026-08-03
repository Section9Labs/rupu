# Netflow Plan 3 — CP API and views

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Serve netflow data from CP at run, project and global scope with read-time ASN enrichment, and render it as a sortable table, a per-host summary, and a bipartite topology graph — placed as a RunDetail tab, a project panel, and a global page.

**Architecture:** `api/netflow.rs` reads the `.rupu/netflow/` ledger through `rupu-netflow`'s view functions, enriches `peer_ip` → ASN at render time from the auto-refreshed table, and returns JSON. Three React components consume it, reusing the existing `components/graph` palette tokens but not the DAG layout engine — netflow topology is bipartite, not a DAG. Placement follows the established rule: a flow belongs to a run, never to a workflow definition.

**Tech Stack:** Rust + `axum` for the API; React + TypeScript + `vitest` + Testing Library for the web.

**Spec:** `docs/superpowers/specs/2026-08-03-rupu-netflow-observability-design.md`
**Depends on:** Plan 1 and Plan 2 — both complete, with flows landing in `.rupu/netflow/flows.jsonl`.

## Global Constraints

- **Fidelity is always rendered.** Every view that shows a flow shows its `Coarse | Http | Full` badge. No view may imply coverage the data does not have.
- **Never on the workflow definition page.** Netflow is per-run, project-aggregate and global only.
- **`Coarse` records must render missing fields as "unknown", never as `0`.** `bytes_in: null` means "we could not see it", which is not the same as zero bytes.
- **Dropped flows are surfaced.** If `read_dropped_total` is non-zero, the UI says so. Silent loss is the thing this design exists to avoid.
- **ASN absence is an honest empty state**, not a blank column with no explanation.
- **Rebuild the embedded UI before any release:** `make cp-web` then `make release`. Skipping `cp-web` ships a stale UI inside the binary.
- **Never run package-wide `cargo fmt`** — format ONLY files you touched, with `rustfmt --edition 2021 <path>`. `cargo fmt -- <path>` does NOT scope and reformats the whole workspace.
- **Never use bare `git stash` / `git stash pop`** — the stash stack is shared across worktrees.

---

### Task 1: Run-scoped netflow API with ASN enrichment

**Files:**
- Create: `crates/rupu-cp/src/api/netflow.rs`
- Modify: `crates/rupu-cp/src/api/mod.rs`
- Modify: `crates/rupu-cp/src/server.rs:69` (add the `.merge`)
- Modify: `crates/rupu-cp/Cargo.toml`
- Test: `crates/rupu-cp/tests/netflow_api.rs` (create)

**Interfaces:**
- Consumes: `read_flows`, `read_dropped_total`, `AsnTable`, `asn_db_path` (Plans 1–2).
- Produces: `GET /api/runs/:id/netflow` returning `NetflowResponse { flows: Vec<FlowView>, dropped: u64, asn_loaded: bool }`, where `FlowView` is a `FlowRecord` plus `asn: Option<AsnInfo>`. Tasks 2 and 4 build on these exact type names.

Read `crates/rupu-cp/src/api/findings.rs` first — it is the closest existing shape (run-scoped read of a workspace-level ledger) and its workspace-resolution helper is the one to reuse.

- [ ] **Step 1: Add the dependency**

`crates/rupu-cp/Cargo.toml`:

```toml
rupu-netflow = { workspace = true, features = ["http"] }
```

The `http` feature is needed for the lazy refresh in Task 3.

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-cp/tests/netflow_api.rs`:

```rust
//! The API must never invent data. A Coarse record's unknown byte count
//! stays null through serialization; a missing ASN table is reported as
//! `asn_loaded: false` rather than silently producing empty enrichment.

use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowRecord, LedgerLine, Origin, Outcome};

fn flow(run: Option<&str>, host: &str, fidelity: Fidelity, peer: Option<&str>) -> FlowRecord {
    FlowRecord {
        id: FlowId::new(),
        ts: chrono::Utc::now(),
        ctx: FlowCtx {
            run_id: run.map(str::to_string),
            step_id: None,
            agent: None,
            workspace_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        fidelity,
        method: "POST".into(),
        scheme: "https".into(),
        host: host.into(),
        port: 443,
        path: "/v1/messages".into(),
        peer_ip: peer.map(|p| p.parse().unwrap()),
        resolved_ips: vec![],
        http_version: None,
        status: Some(200),
        outcome: Outcome::Ok,
        error: None,
        bytes_out: if fidelity == Fidelity::Coarse { None } else { Some(10) },
        bytes_in: if fidelity == Fidelity::Coarse { None } else { Some(20) },
        body_complete: true,
        ttfb_ms: None,
        duration_ms: Some(30),
    }
}

fn write_ledger(workspace: &std::path::Path, lines: &[LedgerLine]) {
    let paths = rupu_netflow::NetflowPaths::new(workspace);
    paths.ensure_dir().unwrap();
    let body: String = lines
        .iter()
        .map(|l| format!("{}\n", serde_json::to_string(l).unwrap()))
        .collect();
    std::fs::write(&paths.flows, body).unwrap();
}

#[test]
fn run_scope_filters_to_that_run_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_ledger(
        tmp.path(),
        &[
            LedgerLine::Flow(Box::new(flow(Some("run-a"), "api.anthropic.com", Fidelity::Http, Some("1.1.1.1")))),
            LedgerLine::Flow(Box::new(flow(Some("run-b"), "api.github.com", Fidelity::Http, None))),
            LedgerLine::Flow(Box::new(flow(None, "iptoasn.com", Fidelity::Http, None))),
        ],
    );

    let paths = rupu_netflow::NetflowPaths::new(tmp.path());
    let all = rupu_netflow::ledger::read_flows(&paths.flows).unwrap();
    let scoped = rupu_cp::api::netflow::filter_by_run(&all, "run-a");

    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].host, "api.anthropic.com");
}

#[test]
fn coarse_unknown_bytes_serialize_as_null_not_zero() {
    let f = flow(Some("r"), "api.github.com", Fidelity::Coarse, None);
    let view = rupu_cp::api::netflow::FlowView::from_flow(f, None);
    let json = serde_json::to_value(&view).unwrap();

    assert_eq!(json["fidelity"], "coarse");
    assert!(
        json["bytes_in"].is_null(),
        "unknown must stay null — 0 would read as a fact"
    );
    assert!(json["asn"].is_null());
}

#[test]
fn enrichment_attaches_asn_when_the_table_resolves_the_peer() {
    use std::io::Cursor;
    let table = rupu_netflow::AsnTable::compact_from_tsv(Cursor::new(
        "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n",
    ))
    .unwrap();

    let f = flow(Some("r"), "api.anthropic.com", Fidelity::Http, Some("1.0.0.7"));
    let view = rupu_cp::api::netflow::FlowView::from_flow(f, Some(&table));

    let asn = view.asn.unwrap();
    assert_eq!(asn.asn, 13335);
    assert_eq!(asn.org, "CLOUDFLARENET");
}

#[test]
fn a_flow_with_no_peer_ip_gets_no_asn() {
    use std::io::Cursor;
    let table = rupu_netflow::AsnTable::compact_from_tsv(Cursor::new(
        "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n",
    ))
    .unwrap();

    let f = flow(Some("r"), "api.github.com", Fidelity::Coarse, None);
    let view = rupu_cp::api::netflow::FlowView::from_flow(f, Some(&table));
    assert!(view.asn.is_none());
}

#[test]
fn dropped_lines_are_reported_not_swallowed() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_ledger(
        tmp.path(),
        &[LedgerLine::Dropped {
            count: 12,
            ts: chrono::Utc::now(),
        }],
    );
    let paths = rupu_netflow::NetflowPaths::new(tmp.path());
    assert_eq!(
        rupu_netflow::ledger::read_dropped_total(&paths.flows).unwrap(),
        12
    );
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-cp --test netflow_api`
Expected: FAIL to compile — `could not find netflow in rupu_cp::api`.

- [ ] **Step 4: Implement `api/netflow.rs`**

```rust
//! Netflow API — run, project and global scope.
//!
//! ASN is resolved HERE, at render time, not stamped into the record
//! (spec §6.2). A table that arrives later therefore improves every
//! historical flow with no backfill.

use crate::{error::ApiResult, state::AppState};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use rupu_netflow::{AsnInfo, AsnTable, FlowRecord, NetflowPaths};
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/runs/:id/netflow", get(get_run_netflow))
}

/// A flow plus its read-time enrichment.
#[derive(Debug, Clone, Serialize)]
pub struct FlowView {
    #[serde(flatten)]
    pub flow: FlowRecord,
    /// `None` when the peer IP is unknown (Coarse fidelity) or the ASN
    /// table has no entry. The UI distinguishes both from "AS0".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asn: Option<AsnInfo>,
}

impl FlowView {
    pub fn from_flow(flow: FlowRecord, table: Option<&AsnTable>) -> Self {
        let asn = flow
            .peer_ip
            .and_then(|ip| table.and_then(|t| t.lookup(ip)));
        Self { flow, asn }
    }
}

#[derive(Debug, Serialize)]
pub struct NetflowResponse {
    pub flows: Vec<FlowView>,
    /// Per-host rollup, computed here rather than in the browser so the
    /// percentile and unknown-bytes logic has exactly ONE implementation.
    pub hosts: Vec<rupu_netflow::ledger::HostRollup>,
    /// Records lost to writer-channel overflow. Surfaced so the UI can
    /// say "N flows dropped" instead of quietly under-reporting.
    pub dropped: u64,
    /// Whether the ASN table was available for this request. `false`
    /// means the `asn` fields are absent because we could not look
    /// them up, NOT because the flows had no ASN.
    pub asn_loaded: bool,
}

/// Flows belonging to one run.
pub fn filter_by_run(flows: &[FlowRecord], run_id: &str) -> Vec<FlowRecord> {
    flows
        .iter()
        .filter(|f| f.ctx.run_id.as_deref() == Some(run_id))
        .cloned()
        .collect()
}

/// Load the ASN table if present. A missing table is not an error —
/// enrichment simply degrades.
pub(crate) fn load_asn_table() -> Option<AsnTable> {
    rupu_netflow::asn::asn_db_path().and_then(|p| AsnTable::load(&p).ok())
}

pub(crate) fn build_response(flows: Vec<FlowRecord>, dropped: u64) -> NetflowResponse {
    let table = load_asn_table();
    let hosts = rupu_netflow::ledger::host_rollup(&flows);
    NetflowResponse {
        flows: flows
            .into_iter()
            .map(|f| FlowView::from_flow(f, table.as_ref()))
            .collect(),
        hosts,
        dropped,
        asn_loaded: table.is_some(),
    }
}

async fn get_run_netflow(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<NetflowResponse>> {
    // Resolve the run to its workspace exactly the way findings does —
    // copy the helper from `crate::api::findings`, do not re-invent it.
    let workspace = crate::api::run_resolve::workspace_for_run(&state, &run_id)?;
    let paths = NetflowPaths::new(&workspace);

    let all = rupu_netflow::ledger::read_flows(&paths.flows).unwrap_or_default();
    let dropped = rupu_netflow::ledger::read_dropped_total(&paths.flows).unwrap_or(0);

    Ok(Json(build_response(filter_by_run(&all, &run_id), dropped)))
}
```

`crate::api::run_resolve::workspace_for_run` may not exist under that exact name. Read `crates/rupu-cp/src/api/run_resolve.rs` and `api/findings.rs` and use whatever they actually call; do not add a new resolution path.

- [ ] **Step 5: Register the module and route**

`crates/rupu-cp/src/api/mod.rs`:

```rust
pub mod netflow;
```

`crates/rupu-cp/src/server.rs`, alongside the other merges around line 69:

```rust
        .merge(crate::api::netflow::routes())
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p rupu-cp --test netflow_api`
Expected: PASS — 5 tests.

- [ ] **Step 7: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/api/netflow.rs crates/rupu-cp/src/api/mod.rs crates/rupu-cp/src/server.rs
git add crates/rupu-cp/
git commit -m "feat(cp): run-scoped netflow API with read-time ASN enrichment"
```

---

### Task 2: Project, global and graph endpoints

**Files:**
- Modify: `crates/rupu-cp/src/api/netflow.rs`
- Modify: `crates/rupu-cp/tests/netflow_api.rs`

**Interfaces:**
- Consumes: `build_response`, `filter_by_run` (Task 1); `graph_view` (Plan 1 Task 4).
- Produces: `GET /api/projects/:id/netflow`, `GET /api/netflow`, `GET /api/netflow/graph?scope=`. All three reuse `build_response`, so the rollup is never recomputed anywhere else. Task 4's client calls all of them.

- [ ] **Step 1: Write the failing tests**

Append to `crates/rupu-cp/tests/netflow_api.rs`:

```rust
#[test]
fn summary_rolls_up_by_host_and_port() {
    let flows = vec![
        flow(Some("r1"), "api.anthropic.com", Fidelity::Http, Some("1.0.0.1")),
        flow(Some("r1"), "api.anthropic.com", Fidelity::Http, Some("1.0.0.1")),
        flow(Some("r2"), "api.github.com", Fidelity::Http, None),
    ];
    let mut hosts = rupu_netflow::ledger::host_rollup(&flows);
    hosts.sort_by(|a, b| a.host.cmp(&b.host));

    assert_eq!(hosts.len(), 2);
    assert_eq!(hosts[0].host, "api.anthropic.com");
    assert_eq!(hosts[0].calls, 2);
}

#[test]
fn graph_scope_is_bipartite_with_one_endpoint_per_host_port() {
    let flows = vec![
        flow(Some("r1"), "api.anthropic.com", Fidelity::Http, None),
        flow(Some("r2"), "api.anthropic.com", Fidelity::Http, None),
    ];
    let g = rupu_netflow::ledger::graph_view(&flows);

    assert_eq!(
        g.nodes
            .iter()
            .filter(|n| n.side == rupu_netflow::ledger::NodeSide::Endpoint)
            .count(),
        1
    );
    assert_eq!(
        g.nodes
            .iter()
            .filter(|n| n.side == rupu_netflow::ledger::NodeSide::Source)
            .count(),
        2
    );
    assert_eq!(g.edges.len(), 2);
}

#[test]
fn unattributed_system_egress_is_visible_at_project_scope() {
    // The updater and CP's host registry have no run. Project scope is
    // where that egress becomes visible — run scope would hide it.
    let flows = vec![flow(None, "iptoasn.com", Fidelity::Http, None)];
    assert!(rupu_cp::api::netflow::filter_by_run(&flows, "r1").is_empty());
    let g = rupu_netflow::ledger::graph_view(&flows);
    assert!(g.nodes.iter().any(|n| n.id == "system"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp --test netflow_api summary`
Expected: FAIL — the `host_rollup` re-export or `NodeSide` path is wrong until Task 1's exports are complete, or PASS immediately if Plan 1's exports already cover it. If it passes, the endpoints below are still missing — proceed.

- [ ] **Step 3: Add the three handlers**

```rust
/// Every flow in a workspace, including `system`-origin egress that has
/// no run to attach to.
async fn get_project_netflow(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> ApiResult<Json<NetflowResponse>> {
    let workspace = crate::api::netflow::workspace_for_project(&state, &project_id)?;
    let paths = NetflowPaths::new(&workspace);
    let flows = rupu_netflow::ledger::read_flows(&paths.flows).unwrap_or_default();
    let dropped = rupu_netflow::ledger::read_dropped_total(&paths.flows).unwrap_or(0);
    Ok(Json(build_response(flows, dropped)))
}

/// Union across every known workspace.
async fn get_global_netflow(State(state): State<AppState>) -> ApiResult<Json<NetflowResponse>> {
    let (flows, dropped) = read_all_workspaces(&state)?;
    Ok(Json(build_response(flows, dropped)))
}

#[derive(serde::Deserialize)]
pub struct GraphQuery {
    /// `run:<id>`, `project:<id>`, or absent for global.
    pub scope: Option<String>,
}

async fn get_netflow_graph(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<GraphQuery>,
) -> ApiResult<Json<rupu_netflow::ledger::GraphView>> {
    let flows = match q.scope.as_deref() {
        Some(s) if s.starts_with("run:") => {
            let run_id = &s[4..];
            let workspace = crate::api::run_resolve::workspace_for_run(&state, run_id)?;
            let paths = NetflowPaths::new(&workspace);
            let all = rupu_netflow::ledger::read_flows(&paths.flows).unwrap_or_default();
            filter_by_run(&all, run_id)
        }
        Some(s) if s.starts_with("project:") => {
            let workspace = workspace_for_project(&state, &s[8..])?;
            let paths = NetflowPaths::new(&workspace);
            rupu_netflow::ledger::read_flows(&paths.flows).unwrap_or_default()
        }
        _ => read_all_workspaces(&state)?.0,
    };
    Ok(Json(rupu_netflow::ledger::graph_view(&flows)))
}
```

Add `read_all_workspaces` and `workspace_for_project` by copying how `api/coverage.rs`'s `list_coverage` enumerates workspaces via `WorkspaceStore` — that is the established pattern for a global view over per-workspace ledgers.

- [ ] **Step 4: Register the routes**

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs/:id/netflow", get(get_run_netflow))
        .route("/api/projects/:id/netflow", get(get_project_netflow))
        .route("/api/netflow", get(get_global_netflow))
        .route("/api/netflow/graph", get(get_netflow_graph))
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-cp --test netflow_api`
Expected: PASS — 8 tests.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/api/netflow.rs
git add crates/rupu-cp/
git commit -m "feat(cp): project, global and graph netflow endpoints"
```

---

### Task 3: Lazy ASN refresh on read

**Files:**
- Modify: `crates/rupu-cp/src/api/netflow.rs`
- Modify: `crates/rupu-cp/tests/netflow_api.rs`

**Interfaces:**
- Consumes: `should_refresh_asn` policy shape (Plan 2 Task 9), `is_stale`, `refresh` (Plan 1 Task 7).
- Produces: an ASN table that appears for operators who never run `cp serve`'s sweep.

The `cp serve` tick is the primary trigger. This is the backstop, and it must never make a request block: the refresh is spawned, the current response goes out with `asn_loaded: false`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_stale_table_triggers_at_most_one_concurrent_refresh() {
    // The guard is a flag, not a lock — a second request while a
    // refresh is in flight must not spawn a second download.
    let guard = rupu_cp::api::netflow::RefreshGuard::default();
    assert!(guard.try_begin(), "first caller wins");
    assert!(!guard.try_begin(), "second caller is turned away");
    guard.finish();
    assert!(guard.try_begin(), "after finishing, a later caller may retry");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp --test netflow_api refresh`
Expected: FAIL — `cannot find struct RefreshGuard`.

- [ ] **Step 3: Implement the guard and the trigger**

```rust
use std::sync::atomic::{AtomicBool, Ordering};

/// Ensures at most one ASN refresh is in flight at a time.
///
/// A burst of CP requests against a missing table must produce one
/// download, not one per request.
#[derive(Debug, Default)]
pub struct RefreshGuard {
    running: AtomicBool,
}

impl RefreshGuard {
    /// `true` if the caller now owns the refresh.
    pub fn try_begin(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn finish(&self) {
        self.running.store(false, Ordering::Release);
    }
}

static ASN_REFRESH: std::sync::OnceLock<std::sync::Arc<RefreshGuard>> = std::sync::OnceLock::new();

fn refresh_guard() -> std::sync::Arc<RefreshGuard> {
    ASN_REFRESH
        .get_or_init(|| std::sync::Arc::new(RefreshGuard::default()))
        .clone()
}

/// Spawn a refresh if the table is missing or stale. Never blocks the
/// caller — this request answers with `asn_loaded: false` and the next
/// one benefits.
pub(crate) fn maybe_refresh_asn(cfg: &rupu_config::NetflowConfig) {
    if !cfg.asn_auto_refresh {
        return;
    }
    let Some(db) = rupu_netflow::asn::asn_db_path() else {
        return;
    };
    if !rupu_netflow::asn::is_stale(&db, cfg.asn_refresh_interval_days) {
        return;
    }
    let guard = refresh_guard();
    if !guard.try_begin() {
        return;
    }
    let url = cfg.asn_source_url.clone();
    tokio::spawn(async move {
        let client = rupu_netflow::http::client(rupu_netflow::FlowCtx::system(
            rupu_netflow::Origin::System,
        ));
        match rupu_netflow::asn::refresh(&url, &db, &client).await {
            Ok(()) => tracing::info!("netflow ASN table refreshed on demand"),
            Err(e) => tracing::warn!(error = %e, "on-demand ASN refresh failed"),
        }
        guard.finish();
    });
}
```

Call `maybe_refresh_asn(&state.config().netflow)` at the top of `build_response`. Read `crates/rupu-cp/src/state.rs` for the actual accessor name for config on `AppState`; if config is not reachable there, pass `&NetflowConfig` into `build_response` from each handler instead.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cp --test netflow_api`
Expected: PASS — 9 tests.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/api/netflow.rs
git add crates/rupu-cp/
git commit -m "feat(cp): lazy ASN refresh on netflow read, single-flight guarded"
```

---

### Task 4: Web API client and shared types

**Files:**
- Create: `crates/rupu-cp/web/src/lib/netflow.ts`
- Create: `crates/rupu-cp/web/src/lib/netflow.test.ts`

**Interfaces:**
- Consumes: the four endpoints from Tasks 1–2.
- Produces: TypeScript `Fidelity`, `Outcome`, `FlowView`, `NetflowResponse`, `HostRollup`, `GraphView`, `GraphNode`, `GraphEdge`, `NodeSide`; functions `fetchRunNetflow(runId)`, `fetchProjectNetflow(projectId)`, `fetchGlobalNetflow()`, `fetchNetflowGraph(scope?)`, and the display helper `formatBytes`. Tasks 5–8 import these exact names.

**No client-side rollup.** The server already returns `hosts` (Task 1). Re-deriving percentiles and the unknown-bytes rule in TypeScript would mean two implementations of the same logic drifting apart — the browser only formats what it is given.

Read `crates/rupu-cp/web/src/lib/` for the existing fetch helper and error convention, and use it rather than raw `fetch`.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/lib/netflow.test.ts`:

```ts
import { describe, expect, it, vi } from 'vitest';
import { fetchNetflowGraph, fetchRunNetflow, formatBytes } from './netflow';

describe('formatBytes', () => {
  it('renders unknown as an em dash, never as 0 B', () => {
    // The distinction the whole Fidelity system exists to preserve:
    // "we could not see it" is not "it was zero".
    expect(formatBytes(null)).toBe('—');
    expect(formatBytes(undefined)).toBe('—');
    expect(formatBytes(0)).toBe('0 B');
  });

  it('scales to KB and MB', () => {
    expect(formatBytes(2048)).toBe('2.0 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB');
  });
});

describe('fetch helpers', () => {
  it('encodes the run id into the path', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ flows: [], hosts: [], dropped: 0, asn_loaded: true }),
    });
    vi.stubGlobal('fetch', fetchMock);

    await fetchRunNetflow('run/with slash');
    expect(fetchMock).toHaveBeenCalledWith('/api/runs/run%2Fwith%20slash/netflow');
    vi.unstubAllGlobals();
  });

  it('omits the scope parameter entirely when global', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ nodes: [], edges: [] }),
    });
    vi.stubGlobal('fetch', fetchMock);

    await fetchNetflowGraph();
    expect(fetchMock).toHaveBeenCalledWith('/api/netflow/graph');
    vi.unstubAllGlobals();
  });

  it('throws on a non-ok response rather than returning junk', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 500 }));
    await expect(fetchRunNetflow('r1')).rejects.toThrow(/500/);
    vi.unstubAllGlobals();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/netflow.test.ts`
Expected: FAIL — cannot resolve `./netflow`.

- [ ] **Step 3: Implement `netflow.ts`**

```ts
// Netflow API client and pure helpers.
//
// Types mirror the Rust API exactly. A null byte count means the value
// was not observable (Coarse fidelity), which is NOT the same as zero —
// rendering it as 0 would state something false. All aggregation happens
// server-side; this module only fetches and formats.

export type Fidelity = 'coarse' | 'http' | 'full';
export type Outcome = 'ok' | 'http_error' | 'transport_error' | 'timeout';
export type NodeSide = 'source' | 'endpoint';

export interface Origin {
  kind: 'provider' | 'scm' | 'mcp' | 'webhook' | 'update' | 'cp' | 'system';
  name?: string;
}

export interface FlowCtx {
  run_id?: string | null;
  step_id?: string | null;
  agent?: string | null;
  workspace_id?: string | null;
  origin: Origin;
}

export interface AsnInfo {
  asn: number;
  org: string;
}

export interface FlowView {
  id: string;
  ts: string;
  ctx: FlowCtx;
  fidelity: Fidelity;
  method: string;
  scheme: string;
  host: string;
  port: number;
  path: string;
  peer_ip?: string | null;
  resolved_ips?: string[];
  http_version?: string | null;
  status?: number | null;
  outcome: Outcome;
  error?: string | null;
  bytes_out?: number | null;
  bytes_in?: number | null;
  body_complete: boolean;
  ttfb_ms?: number | null;
  duration_ms?: number | null;
  asn?: AsnInfo | null;
}

/** Mirrors `rupu_netflow::ledger::HostRollup` exactly. Computed server-side
 *  — do not re-derive any of it here. */
export interface HostRollup {
  host: string;
  port: number;
  calls: number;
  /** Null when ANY contributing flow had an unobservable byte count. */
  bytes_in: number | null;
  bytes_out: number | null;
  errors: number;
  p50_ms: number;
  p95_ms: number;
}

export interface NetflowResponse {
  flows: FlowView[];
  hosts: HostRollup[];
  /** Records lost to writer overflow. Non-zero means the UI must say so. */
  dropped: number;
  /** False means enrichment was unavailable, not that flows lack an ASN. */
  asn_loaded: boolean;
}

export interface GraphNode {
  id: string;
  label: string;
  side: NodeSide;
}

export interface GraphEdge {
  from: string;
  to: string;
  calls: number;
  bytes: number;
  errors: number;
}

export interface GraphView {
  nodes: GraphNode[];
  edges: GraphEdge[];
}

async function getJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url} → HTTP ${res.status}`);
  return (await res.json()) as T;
}

export const fetchRunNetflow = (runId: string) =>
  getJson<NetflowResponse>(`/api/runs/${encodeURIComponent(runId)}/netflow`);

export const fetchProjectNetflow = (projectId: string) =>
  getJson<NetflowResponse>(`/api/projects/${encodeURIComponent(projectId)}/netflow`);

export const fetchGlobalNetflow = () => getJson<NetflowResponse>('/api/netflow');

export const fetchNetflowGraph = (scope?: string) =>
  getJson<GraphView>(
    scope ? `/api/netflow/graph?scope=${encodeURIComponent(scope)}` : '/api/netflow/graph',
  );

/** Unknown renders as an em dash — never as `0 B`, which would be a claim. */
export function formatBytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return '—';
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/netflow.test.ts`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/netflow.ts crates/rupu-cp/web/src/lib/netflow.test.ts
git commit -m "feat(cp-web): netflow API client and byte formatting"
```

---

### Task 5: `NetflowTable` and `NetflowSummary`

**Files:**
- Create: `crates/rupu-cp/web/src/components/netflow/FidelityBadge.tsx`
- Create: `crates/rupu-cp/web/src/components/netflow/NetflowTable.tsx`
- Create: `crates/rupu-cp/web/src/components/netflow/NetflowSummary.tsx`
- Create: `crates/rupu-cp/web/src/components/netflow/NetflowTable.test.tsx`

**Interfaces:**
- Consumes: `FlowView`, `HostRollup`, `formatBytes` (Task 4).
- Produces: `<FidelityBadge fidelity>`, `<NetflowTable flows dropped asnLoaded>`, `<NetflowSummary hosts>`. Tasks 7–8 mount both.

Read `crates/rupu-cp/web/src/components/findings/FindingsTable.tsx` first and match its table markup, sorting affordance and empty-state conventions.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/components/netflow/NetflowTable.test.tsx`:

```tsx
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import NetflowTable from './NetflowTable';
import type { FlowView } from '../../lib/netflow';

function flow(over: Partial<FlowView> = {}): FlowView {
  return {
    id: '01J',
    ts: '2026-08-03T00:00:00Z',
    ctx: { origin: { kind: 'provider', name: 'anthropic' }, run_id: 'r1' },
    fidelity: 'http',
    method: 'POST',
    scheme: 'https',
    host: 'api.anthropic.com',
    port: 443,
    path: '/v1/messages',
    outcome: 'ok',
    body_complete: true,
    bytes_in: 2048,
    bytes_out: 512,
    duration_ms: 30,
    asn: { asn: 13335, org: 'CLOUDFLARENET' },
    ...over,
  };
}

describe('NetflowTable', () => {
  it('renders host, path and ASN org', () => {
    render(<NetflowTable flows={[flow()]} dropped={0} asnLoaded />);
    expect(screen.getByText('api.anthropic.com')).toBeInTheDocument();
    expect(screen.getByText('/v1/messages')).toBeInTheDocument();
    expect(screen.getByText(/CLOUDFLARENET/)).toBeInTheDocument();
    expect(screen.getByText(/AS13335/)).toBeInTheDocument();
  });

  it('always shows the fidelity badge', () => {
    render(<NetflowTable flows={[flow({ fidelity: 'coarse' })]} dropped={0} asnLoaded />);
    expect(screen.getByText('coarse')).toBeInTheDocument();
  });

  it('renders an unknown byte count as a dash, not zero', () => {
    render(
      <NetflowTable
        flows={[flow({ fidelity: 'coarse', bytes_in: null, bytes_out: null })]}
        dropped={0}
        asnLoaded
      />,
    );
    expect(screen.queryByText('0 B')).not.toBeInTheDocument();
    expect(screen.getAllByText('—').length).toBeGreaterThan(0);
  });

  it('surfaces dropped flows rather than under-reporting silently', () => {
    render(<NetflowTable flows={[flow()]} dropped={17} asnLoaded />);
    expect(screen.getByText(/17 flows dropped/i)).toBeInTheDocument();
  });

  it('explains a missing ASN table instead of leaving a blank column', () => {
    render(<NetflowTable flows={[flow({ asn: null })]} dropped={0} asnLoaded={false} />);
    expect(screen.getByText(/ASN data not loaded/i)).toBeInTheDocument();
  });

  it('states the phase-1 scope limit on the empty state', () => {
    render(<NetflowTable flows={[]} dropped={0} asnLoaded />);
    expect(screen.getByText(/No network flows recorded/i)).toBeInTheDocument();
    expect(screen.getByText(/does not cover.*subprocess/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/netflow/NetflowTable.test.tsx`
Expected: FAIL — cannot resolve `./NetflowTable`.

- [ ] **Step 3: Implement `FidelityBadge.tsx`**

```tsx
import type { Fidelity } from '../../lib/netflow';

const TITLE: Record<Fidelity, string> = {
  coarse:
    'Coarse — host, outcome and timing are real; byte counts and peer IP were not observable for this connector.',
  http: 'HTTP — exact request and response metadata from the instrumented client.',
  full: 'Full — frame-level capture from the isolated runtime.',
};

/** Always rendered next to a flow. The subsystem never implies coverage
 *  it does not have. */
export default function FidelityBadge({ fidelity }: { fidelity: Fidelity }) {
  return (
    <span className={`netflow-fidelity netflow-fidelity--${fidelity}`} title={TITLE[fidelity]}>
      {fidelity}
    </span>
  );
}
```

- [ ] **Step 4: Implement `NetflowTable.tsx`**

```tsx
import { useMemo, useState } from 'react';
import { formatBytes, type FlowView } from '../../lib/netflow';
import FidelityBadge from './FidelityBadge';

type SortKey = 'ts' | 'host' | 'duration_ms' | 'bytes_in';

export interface NetflowTableProps {
  flows: FlowView[];
  dropped: number;
  asnLoaded: boolean;
}

export default function NetflowTable({ flows, dropped, asnLoaded }: NetflowTableProps) {
  const [sort, setSort] = useState<SortKey>('ts');

  const sorted = useMemo(() => {
    const rows = flows.slice();
    rows.sort((a, b) => {
      if (sort === 'host') return a.host.localeCompare(b.host);
      if (sort === 'ts') return b.ts.localeCompare(a.ts);
      const av = (a[sort] as number | null | undefined) ?? -1;
      const bv = (b[sort] as number | null | undefined) ?? -1;
      return bv - av;
    });
    return rows;
  }, [flows, sort]);

  if (flows.length === 0) {
    return (
      <div className="netflow-empty">
        <p>No network flows recorded for this scope.</p>
        <p className="netflow-note">
          Netflow currently covers rupu&apos;s own egress — provider APIs, SCM connectors, MCP and
          webhooks. It does not cover traffic from the agent&apos;s bash subprocess.
        </p>
      </div>
    );
  }

  return (
    <div className="netflow-table">
      {dropped > 0 && (
        <p className="netflow-warning" role="status">
          {dropped} flows dropped — the capture buffer overflowed, so this list is incomplete.
        </p>
      )}
      {!asnLoaded && (
        <p className="netflow-note">
          ASN data not loaded yet — network enrichment will appear once the table has been fetched.
        </p>
      )}
      <table>
        <thead>
          <tr>
            <th><button onClick={() => setSort('ts')}>Time</button></th>
            <th>Origin</th>
            <th><button onClick={() => setSort('host')}>Host</button></th>
            <th>Path</th>
            <th>Network</th>
            <th>Status</th>
            <th><button onClick={() => setSort('bytes_in')}>In</button></th>
            <th>Out</th>
            <th><button onClick={() => setSort('duration_ms')}>Duration</button></th>
            <th>Fidelity</th>
          </tr>
        </thead>
        <tbody>
          {sorted.map((f) => (
            <tr key={f.id} className={f.outcome === 'ok' ? undefined : 'netflow-row--error'}>
              <td>{new Date(f.ts).toLocaleTimeString()}</td>
              <td>{f.ctx.origin.name ?? f.ctx.origin.kind}</td>
              <td>{f.host}</td>
              <td className="netflow-path">{f.path}</td>
              <td>{f.asn ? `AS${f.asn.asn} ${f.asn.org}` : '—'}</td>
              <td>{f.status ?? '—'}</td>
              <td>{formatBytes(f.bytes_in)}</td>
              <td>{formatBytes(f.bytes_out)}</td>
              <td>{f.duration_ms != null ? `${f.duration_ms} ms` : '—'}</td>
              <td><FidelityBadge fidelity={f.fidelity} /></td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
```

- [ ] **Step 5: Implement `NetflowSummary.tsx`**

```tsx
import { useMemo } from 'react';
import { formatBytes, type HostRollup } from '../../lib/netflow';

/** Renders the server-computed rollup. Deliberately does no arithmetic
 *  beyond ordering — the percentile and unknown-bytes rules live in
 *  `rupu_netflow::ledger::host_rollup` and must not be duplicated here. */
export default function NetflowSummary({ hosts }: { hosts: HostRollup[] }) {
  const rows = useMemo(
    () => hosts.slice().sort((a, b) => b.calls - a.calls),
    [hosts],
  );

  if (rows.length === 0) return null;

  return (
    <table className="netflow-summary">
      <thead>
        <tr>
          <th>Host</th>
          <th>Calls</th>
          <th>Errors</th>
          <th>In</th>
          <th>Out</th>
          <th>p50</th>
          <th>p95</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => (
          <tr key={`${r.host}:${r.port}`}>
            <td>{r.host}</td>
            <td>{r.calls}</td>
            <td>{r.errors}</td>
            <td>{formatBytes(r.bytes_in)}</td>
            <td>{formatBytes(r.bytes_out)}</td>
            <td>{r.p50_ms} ms</td>
            <td>{r.p95_ms} ms</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
```

- [ ] **Step 6: Add the styles**

Append to `crates/rupu-cp/web/src/styles.css`, reusing the existing colour tokens — grep for how `findings` severity chips pick theirs and follow suit:

```css
.netflow-fidelity { font-size: 0.75rem; padding: 0 0.35rem; border-radius: 3px; }
.netflow-fidelity--http { background: var(--ok-bg); color: var(--ok-fg); }
.netflow-fidelity--coarse { background: var(--warn-bg); color: var(--warn-fg); }
.netflow-fidelity--full { background: var(--accent-bg); color: var(--accent-fg); }
.netflow-warning { color: var(--warn-fg); }
.netflow-note { color: var(--muted-fg); font-size: 0.85rem; }
.netflow-row--error td { color: var(--error-fg); }
.netflow-path { font-family: var(--mono); }
```

- [ ] **Step 7: Run tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/netflow/`
Expected: PASS — 6 tests.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-cp/web/src/components/netflow/ crates/rupu-cp/web/src/styles.css
git commit -m "feat(cp-web): netflow table, summary and fidelity badge"
```

---

### Task 6: `NetflowGraph` — bipartite topology

**Files:**
- Create: `crates/rupu-cp/web/src/components/netflow/layout.ts`
- Create: `crates/rupu-cp/web/src/components/netflow/layout.test.ts`
- Create: `crates/rupu-cp/web/src/components/netflow/NetflowGraph.tsx`
- Create: `crates/rupu-cp/web/src/components/netflow/NetflowGraph.test.tsx`

**Interfaces:**
- Consumes: `GraphView`, `GraphNode`, `GraphEdge` (Task 4).
- Produces: `layoutBipartite(graph, opts) -> PositionedGraph`; `<NetflowGraph graph weightBy>`. Tasks 7–8 mount the component.

The layout is a pure function in its own file so it is testable without rendering. Do **not** reuse the DAG layout engine from `components/graph` — this topology is bipartite. Do reuse that directory's colour tokens.

- [ ] **Step 1: Write the failing layout test**

Create `crates/rupu-cp/web/src/components/netflow/layout.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { layoutBipartite } from './layout';
import type { GraphView } from '../../lib/netflow';

const graph: GraphView = {
  nodes: [
    { id: 'run-1', label: 'run-1', side: 'source' },
    { id: 'run-2', label: 'run-2', side: 'source' },
    { id: 'api.anthropic.com:443', label: 'api.anthropic.com', side: 'endpoint' },
  ],
  edges: [
    { from: 'run-1', to: 'api.anthropic.com:443', calls: 5, bytes: 500, errors: 0 },
    { from: 'run-2', to: 'api.anthropic.com:443', calls: 1, bytes: 100, errors: 1 },
  ],
};

describe('layoutBipartite', () => {
  it('puts sources in a left column and endpoints in a right column', () => {
    const out = layoutBipartite(graph, { width: 800, rowHeight: 40 });
    const sources = out.nodes.filter((n) => n.side === 'source');
    const endpoints = out.nodes.filter((n) => n.side === 'endpoint');
    expect(new Set(sources.map((n) => n.x)).size).toBe(1);
    expect(new Set(endpoints.map((n) => n.x)).size).toBe(1);
    expect(sources[0].x).toBeLessThan(endpoints[0].x);
  });

  it('gives each node in a column a distinct y', () => {
    const out = layoutBipartite(graph, { width: 800, rowHeight: 40 });
    const ys = out.nodes.filter((n) => n.side === 'source').map((n) => n.y);
    expect(new Set(ys).size).toBe(ys.length);
  });

  it('scales edge weight by the chosen metric', () => {
    const byCalls = layoutBipartite(graph, { width: 800, rowHeight: 40, weightBy: 'calls' });
    const heavy = byCalls.edges.find((e) => e.from === 'run-1')!;
    const light = byCalls.edges.find((e) => e.from === 'run-2')!;
    expect(heavy.width).toBeGreaterThan(light.width);
  });

  it('marks an edge with errors so it can be coloured', () => {
    const out = layoutBipartite(graph, { width: 800, rowHeight: 40 });
    expect(out.edges.find((e) => e.from === 'run-2')!.hasErrors).toBe(true);
    expect(out.edges.find((e) => e.from === 'run-1')!.hasErrors).toBe(false);
  });

  it('handles an empty graph without dividing by zero', () => {
    const out = layoutBipartite({ nodes: [], edges: [] }, { width: 800, rowHeight: 40 });
    expect(out.nodes).toEqual([]);
    expect(out.height).toBeGreaterThan(0);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/netflow/layout.test.ts`
Expected: FAIL — cannot resolve `./layout`.

- [ ] **Step 3: Implement `layout.ts`**

```ts
import type { GraphEdge, GraphNode, GraphView } from '../../lib/netflow';

export interface PositionedNode extends GraphNode {
  x: number;
  y: number;
}

export interface PositionedEdge extends GraphEdge {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  /** Stroke width, 1–8, scaled against the heaviest edge. */
  width: number;
  hasErrors: boolean;
}

export interface PositionedGraph {
  nodes: PositionedNode[];
  edges: PositionedEdge[];
  height: number;
}

export interface LayoutOpts {
  width: number;
  rowHeight: number;
  weightBy?: 'calls' | 'bytes';
}

const PAD_X = 140;
const PAD_Y = 24;
const MIN_W = 1;
const MAX_W = 8;

/** Two columns: sources left, endpoints right. Not a DAG layout —
 *  netflow topology is bipartite by construction. */
export function layoutBipartite(graph: GraphView, opts: LayoutOpts): PositionedGraph {
  const { width, rowHeight, weightBy = 'calls' } = opts;

  const sources = graph.nodes.filter((n) => n.side === 'source');
  const endpoints = graph.nodes.filter((n) => n.side === 'endpoint');

  const place = (list: GraphNode[], x: number): PositionedNode[] =>
    list.map((n, i) => ({ ...n, x, y: PAD_Y + i * rowHeight }));

  const positioned = [
    ...place(sources, PAD_X),
    ...place(endpoints, Math.max(width - PAD_X, PAD_X * 2)),
  ];
  const byId = new Map(positioned.map((n) => [n.id, n]));

  const maxWeight = graph.edges.reduce(
    (m, e) => Math.max(m, weightBy === 'bytes' ? e.bytes : e.calls),
    0,
  );

  const edges: PositionedEdge[] = graph.edges.flatMap((e) => {
    const a = byId.get(e.from);
    const b = byId.get(e.to);
    if (!a || !b) return [];
    const w = weightBy === 'bytes' ? e.bytes : e.calls;
    // maxWeight is 0 only when every edge is 0 — keep the minimum width
    // rather than dividing by zero.
    const scaled = maxWeight > 0 ? MIN_W + (w / maxWeight) * (MAX_W - MIN_W) : MIN_W;
    return [
      {
        ...e,
        x1: a.x,
        y1: a.y,
        x2: b.x,
        y2: b.y,
        width: scaled,
        hasErrors: e.errors > 0,
      },
    ];
  });

  const rows = Math.max(sources.length, endpoints.length, 1);
  return { nodes: positioned, edges, height: PAD_Y * 2 + rows * rowHeight };
}
```

- [ ] **Step 4: Write the failing component test**

Create `crates/rupu-cp/web/src/components/netflow/NetflowGraph.test.tsx`:

```tsx
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import NetflowGraph from './NetflowGraph';
import type { GraphView } from '../../lib/netflow';

const graph: GraphView = {
  nodes: [
    { id: 'run-1', label: 'run-1', side: 'source' },
    { id: 'api.anthropic.com:443', label: 'api.anthropic.com', side: 'endpoint' },
  ],
  edges: [{ from: 'run-1', to: 'api.anthropic.com:443', calls: 3, bytes: 300, errors: 0 }],
};

describe('NetflowGraph', () => {
  it('labels both sides of the topology', () => {
    render(<NetflowGraph graph={graph} />);
    expect(screen.getByText('run-1')).toBeInTheDocument();
    expect(screen.getByText('api.anthropic.com')).toBeInTheDocument();
  });

  it('draws one line per edge', () => {
    const { container } = render(<NetflowGraph graph={graph} />);
    expect(container.querySelectorAll('line')).toHaveLength(1);
  });

  it('renders an explicit empty state rather than a blank canvas', () => {
    render(<NetflowGraph graph={{ nodes: [], edges: [] }} />);
    expect(screen.getByText(/No flows to graph/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 5: Implement `NetflowGraph.tsx`**

```tsx
import { useMemo } from 'react';
import type { GraphView } from '../../lib/netflow';
import { layoutBipartite } from './layout';

export interface NetflowGraphProps {
  graph: GraphView;
  weightBy?: 'calls' | 'bytes';
  width?: number;
}

export default function NetflowGraph({ graph, weightBy = 'calls', width = 880 }: NetflowGraphProps) {
  const laid = useMemo(
    () => layoutBipartite(graph, { width, rowHeight: 34, weightBy }),
    [graph, width, weightBy],
  );

  if (graph.nodes.length === 0) {
    return <p className="netflow-note">No flows to graph for this scope.</p>;
  }

  return (
    <svg className="netflow-graph" width={width} height={laid.height} role="img"
         aria-label="Network topology: sources on the left, endpoints on the right">
      {laid.edges.map((e) => (
        <line
          key={`${e.from}->${e.to}`}
          x1={e.x1} y1={e.y1} x2={e.x2} y2={e.y2}
          strokeWidth={e.width}
          className={e.hasErrors ? 'netflow-edge netflow-edge--error' : 'netflow-edge'}
        />
      ))}
      {laid.nodes.map((n) => (
        <g key={n.id} transform={`translate(${n.x}, ${n.y})`}>
          <circle r={4} className={`netflow-node netflow-node--${n.side}`} />
          <text x={n.side === 'source' ? -10 : 10}
                textAnchor={n.side === 'source' ? 'end' : 'start'}
                dominantBaseline="middle">
            {n.label}
          </text>
        </g>
      ))}
    </svg>
  );
}
```

Add matching styles to `styles.css`, taking the stroke and node colours from the same tokens `components/graph` uses so the two graphs read as one system:

```css
.netflow-edge { stroke: var(--graph-edge); opacity: 0.7; }
.netflow-edge--error { stroke: var(--error-fg); }
.netflow-node--source { fill: var(--graph-node-source); }
.netflow-node--endpoint { fill: var(--graph-node-endpoint); }
.netflow-graph text { font-size: 12px; fill: var(--fg); }
```

- [ ] **Step 6: Run tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/netflow/`
Expected: PASS — 14 tests.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/web/src/components/netflow/ crates/rupu-cp/web/src/styles.css
git commit -m "feat(cp-web): bipartite netflow topology graph"
```

---

### Task 7: The RunDetail netflow tab

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/RunDetail.tsx:49` (the `Tab` union), `:145` (state), and the tab-bar and panel bodies
- Create: `crates/rupu-cp/web/src/pages/RunDetail.netflow.test.tsx`

**Interfaces:**
- Consumes: `fetchRunNetflow`, `fetchNetflowGraph` (Task 4), `NetflowTable`, `NetflowSummary` (Task 5), `NetflowGraph` (Task 6).
- Produces: a fifth tab. Task 8 reuses the same composition at project and global scope.

Follow the **findings** tab exactly: it lazy-loads on first open, keyed on `(id, tab)` with a ref guard for single-fetch. Read lines 160 and 313–334 before editing.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/pages/RunDetail.netflow.test.tsx`:

```tsx
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const fetchRunNetflow = vi.fn();
const fetchNetflowGraph = vi.fn();
vi.mock('../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../lib/netflow')>('../lib/netflow');
  return { ...actual, fetchRunNetflow, fetchNetflowGraph };
});

describe('RunDetail netflow tab', () => {
  beforeEach(() => {
    fetchRunNetflow.mockReset();
    fetchNetflowGraph.mockReset();
    fetchRunNetflow.mockResolvedValue({ flows: [], hosts: [], dropped: 0, asn_loaded: true });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });
  });

  it('does not fetch netflow until the tab is opened', async () => {
    const { renderRunDetail } = await import('./RunDetail.testkit');
    renderRunDetail({ runId: 'run-1' });
    expect(fetchRunNetflow).not.toHaveBeenCalled();
  });

  it('fetches once when the tab is opened, and not again on re-click', async () => {
    const { renderRunDetail } = await import('./RunDetail.testkit');
    renderRunDetail({ runId: 'run-1' });

    await userEvent.click(screen.getByRole('button', { name: /network/i }));
    await waitFor(() => expect(fetchRunNetflow).toHaveBeenCalledWith('run-1'));

    await userEvent.click(screen.getByRole('button', { name: /transcript/i }));
    await userEvent.click(screen.getByRole('button', { name: /network/i }));
    expect(fetchRunNetflow).toHaveBeenCalledTimes(1);
  });
});
```

If no `RunDetail.testkit` helper exists, look at how `Agents.test.tsx` or the existing `RunDetail` tests mount the page (router context, mocked fetches) and mount it the same way inline instead of inventing a testkit module.

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/RunDetail.netflow.test.tsx`
Expected: FAIL — no button named "network".

- [ ] **Step 3: Extend the Tab union and state**

At line 49:

```tsx
type Tab = 'transcript' | 'events' | 'findings' | 'cycles' | 'netflow';
```

Add the imports:

```tsx
import NetflowTable from '../components/netflow/NetflowTable';
import NetflowSummary from '../components/netflow/NetflowSummary';
import NetflowGraph from '../components/netflow/NetflowGraph';
import { fetchNetflowGraph, fetchRunNetflow, type GraphView, type NetflowResponse } from '../lib/netflow';
```

- [ ] **Step 4: Add the lazy load**

Mirroring the findings loader at lines 313–334:

```tsx
  const [netflow, setNetflow] = useState<NetflowResponse | null>(null);
  const [netflowGraph, setNetflowGraph] = useState<GraphView | null>(null);
  const netflowRequestedRef = useRef(false);

  // Lazy-load this run's flows the first time the Network tab is opened.
  // Keyed on (id, tab); the ref guard ensures a single fetch per run id.
  useEffect(() => {
    if (!id || tab !== 'netflow' || netflowRequestedRef.current) return;
    netflowRequestedRef.current = true;
    fetchRunNetflow(id).then(setNetflow).catch(() => setNetflow(null));
    fetchNetflowGraph(`run:${id}`).then(setNetflowGraph).catch(() => setNetflowGraph(null));
  }, [id, tab]);

  // A different run means a different fetch.
  useEffect(() => {
    netflowRequestedRef.current = false;
    setNetflow(null);
    setNetflowGraph(null);
  }, [id]);
```

Check whether the findings loader already has an equivalent reset-on-id effect; if it does, add the netflow reset to that same effect rather than adding a second one.

- [ ] **Step 5: Add the tab button and panel**

In the `TabBar`:

```tsx
        <TabButton active={tab === 'netflow'} onClick={() => setTab('netflow')}>
          Network
        </TabButton>
```

In the panel body:

```tsx
      {tab === 'netflow' && (
        <div className="netflow-panel">
          {netflowGraph && <NetflowGraph graph={netflowGraph} />}
          {netflow && <NetflowSummary hosts={netflow.hosts} />}
          {netflow && (
            <NetflowTable
              flows={netflow.flows}
              dropped={netflow.dropped}
              asnLoaded={netflow.asn_loaded}
            />
          )}
        </div>
      )}
```

- [ ] **Step 6: Run tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/`
Expected: PASS, including the existing RunDetail tests — the new tab must not disturb them.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/web/src/pages/RunDetail.tsx crates/rupu-cp/web/src/pages/RunDetail.netflow.test.tsx
git commit -m "feat(cp-web): Network tab on the run detail page"
```

---

### Task 8: Project panel and global page

**Files:**
- Create: `crates/rupu-cp/web/src/pages/Netflow.tsx`
- Create: `crates/rupu-cp/web/src/pages/Netflow.test.tsx`
- Modify: `crates/rupu-cp/web/src/App.tsx` (route + nav entry)
- Modify: the project detail page (find it with `ls crates/rupu-cp/web/src/pages | grep -i project`)

**Interfaces:**
- Consumes: everything from Tasks 4–6.
- Produces: `/netflow` global page and a project-scoped panel. This completes the placement rule: run tab, project aggregate, global page — and nothing on the workflow definition.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/pages/Netflow.test.tsx`:

```tsx
import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';

const fetchGlobalNetflow = vi.fn();
const fetchNetflowGraph = vi.fn();
vi.mock('../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../lib/netflow')>('../lib/netflow');
  return { ...actual, fetchGlobalNetflow, fetchNetflowGraph };
});

import Netflow from './Netflow';

describe('Netflow global page', () => {
  it('renders the summary and table once loaded', async () => {
    fetchGlobalNetflow.mockResolvedValue({
      flows: [
        {
          id: '1', ts: '2026-08-03T00:00:00Z',
          ctx: { origin: { kind: 'update' } },
          fidelity: 'http', method: 'GET', scheme: 'https',
          host: 'api.github.com', port: 443, path: '/releases',
          outcome: 'ok', body_complete: true,
          bytes_in: 10, bytes_out: 5, duration_ms: 12,
        },
      ],
      hosts: [{ host: 'api.github.com', port: 443, calls: 1, bytes_in: 10, bytes_out: 5, errors: 0, p50_ms: 12, p95_ms: 12 }],
      dropped: 0,
      asn_loaded: true,
    });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<MemoryRouter><Netflow /></MemoryRouter>);

    await waitFor(() => expect(screen.getByText('api.github.com')).toBeInTheDocument());
  });

  it('surfaces dropped flows at global scope', async () => {
    fetchGlobalNetflow.mockResolvedValue({ flows: [], hosts: [], dropped: 9, asn_loaded: true });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<MemoryRouter><Netflow /></MemoryRouter>);
    await waitFor(() =>
      expect(screen.getByText(/No network flows recorded/i)).toBeInTheDocument(),
    );
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/Netflow.test.tsx`
Expected: FAIL — cannot resolve `./Netflow`.

- [ ] **Step 3: Implement the page**

```tsx
import { useEffect, useState } from 'react';
import NetflowGraph from '../components/netflow/NetflowGraph';
import NetflowSummary from '../components/netflow/NetflowSummary';
import NetflowTable from '../components/netflow/NetflowTable';
import {
  fetchGlobalNetflow,
  fetchNetflowGraph,
  type GraphView,
  type NetflowResponse,
} from '../lib/netflow';

export default function Netflow() {
  const [data, setData] = useState<NetflowResponse | null>(null);
  const [graph, setGraph] = useState<GraphView | null>(null);
  const [weightBy, setWeightBy] = useState<'calls' | 'bytes'>('calls');

  useEffect(() => {
    fetchGlobalNetflow().then(setData).catch(() => setData(null));
    fetchNetflowGraph().then(setGraph).catch(() => setGraph(null));
  }, []);

  return (
    <div className="page netflow-page">
      <h1>Network</h1>
      <p className="netflow-note">
        Egress rupu itself made — provider APIs, SCM connectors, MCP, webhooks and the updater.
        Traffic from an agent&apos;s bash subprocess is not covered.
      </p>

      <label>
        Weight edges by{' '}
        <select value={weightBy} onChange={(e) => setWeightBy(e.target.value as 'calls' | 'bytes')}>
          <option value="calls">calls</option>
          <option value="bytes">bytes</option>
        </select>
      </label>

      {graph && <NetflowGraph graph={graph} weightBy={weightBy} />}
      {data && <NetflowSummary hosts={data.hosts} />}
      {data && (
        <NetflowTable flows={data.flows} dropped={data.dropped} asnLoaded={data.asn_loaded} />
      )}
    </div>
  );
}
```

- [ ] **Step 4: Register the route**

In `crates/rupu-cp/web/src/App.tsx`, add the route beside the other top-level pages:

```tsx
        <Route path="/netflow" element={<Netflow />} />
```

and a nav entry labelled **Network**, placed next to Coverage. Match the existing nav markup exactly.

- [ ] **Step 5: Add the project-scoped panel**

On the project detail page, mount the same three components against `fetchProjectNetflow(projectId)` and `fetchNetflowGraph(\`project:${projectId}\`)`, following whatever lazy-load convention that page already uses for its other panels.

Do **not** add netflow to any workflow-definition page. A flow belongs to a run.

- [ ] **Step 6: Run the full web suite**

Run: `cd crates/rupu-cp/web && npx vitest run`
Expected: PASS.

Run: `cd crates/rupu-cp/web && npx tsc --noEmit`
Expected: no type errors.

- [ ] **Step 7: Build the embedded UI and verify end to end**

```bash
make cp-web
cargo run -p rupu-cli -- cp serve
```

Open the CP, run a workflow, and confirm: the run's **Network** tab lists provider flows with an `http` fidelity badge; GitHub flows show `coarse` with `—` for bytes; the global `/netflow` page shows the union including `system`-origin updater traffic; and the ASN column populates once `~/.rupu/netflow/asn.db` exists.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-cp/web/src/pages/ crates/rupu-cp/web/src/App.tsx
git commit -m "feat(cp-web): global netflow page and project-scoped panel"
```

---

## Done when

- `cargo test --workspace` and `npx vitest run` both pass; `npx tsc --noEmit` is clean.
- A run's Network tab shows its flows, its topology graph, and a per-host summary.
- GitHub flows render `coarse` with `—` byte counts; every other flow renders `http` with real counts.
- The global page includes `system`-origin egress that belongs to no run.
- A non-zero dropped count is visible in the UI rather than silently under-reporting.
- No netflow surface exists on any workflow-definition page.

**Before any release:** `make cp-web` then `make release` — skipping `cp-web` ships a stale embedded UI.

The microVM capture backend (spec §9) is the next arc. It introduces the `FlowSource` port, emits `Fidelity::Full`, and requires no change to the schema, ledger, API or views built here.
