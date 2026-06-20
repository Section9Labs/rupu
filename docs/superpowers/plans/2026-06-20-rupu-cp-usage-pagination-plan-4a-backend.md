# CP Usage + Pagination — Plan 4a (Backend) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `offset`/`limit` pagination to the growable CP list endpoints (slicing *before* computing usage) and attach a `usage` summary to the Agent-Runs, autoflow cycle/event, and Dashboard-recent rows.

**Architecture:** A shared `PageQuery` extractor + `paginate` helper. Each growable list handler sorts newest-first, slices to the page, then computes per-row usage on the slice only (reusing A.3's `crate::usage`). New rows gain a `usage: UsageSummary` field filled the same way.

**Tech Stack:** Rust 2021, axum, serde, `rupu-cp::usage` (A.3), `rupu-orchestrator` `RunStore`.

**Conventions (enforced — READ before starting):**
- Branch `feat-cp-usage-pagination` (already created off `main`). NEVER touch `main`.
- `#![deny(clippy::all)]` incl. `cargo clippy --all-targets`.
- **DO NOT run `rustfmt`/`cargo fmt`** — this worktree runs Rust 1.95 but the repo pins 1.88; its rustfmt creates spurious whole-file drift. Match surrounding style by hand. **Before each commit run `git status --short`** and confirm ONLY your intended files are modified; `git checkout --` any drift. Stage only your files (`git add <paths>`, never `-A`; untracked `.rupu/*` stays uncommitted).
- Toolchain: `rupu-cp` is clean on 1.95 — `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` are real gates.
- End commits with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

---

## File structure

| File | Responsibility | Task |
|---|---|---|
| `crates/rupu-cp/src/pagination.rs` (new) + `lib.rs` | `PageQuery` + `paginate` | 1 |
| `crates/rupu-cp/src/api/runs.rs` | paginate `/api/runs` + `/api/runs/workflows` (slice→usage) | 2 |
| `crates/rupu-cp/src/api/run_streams.rs` | `AgentRunRow.usage` + paginate `/api/runs/agents` | 3 |
| `crates/rupu-cp/src/api/run_streams.rs` | `AutoflowCycleRow.usage` + `AutoflowEventRow.usage` + paginate both | 4 |
| `crates/rupu-cp/src/api/dashboard.rs` | `RecentRun.usage` | 5 |
| `crates/rupu-cp/src/api/sessions.rs` + `projects.rs` | paginate `/api/sessions`, `/api/projects/:id/runs`, `/api/projects/:id/sessions` | 6 |

---

## Task 1: `PageQuery` + `paginate` helper

**Files:**
- Create: `crates/rupu-cp/src/pagination.rs`
- Modify: `crates/rupu-cp/src/lib.rs` (add `pub mod pagination;`)

Offset/limit slicing. `limit` defaults to **20** and is clamped to `[1, 200]`; `offset` defaults to 0. Slicing past the end yields an empty vec (not an error).

- [ ] **Step 1: Declare the module**

In `crates/rupu-cp/src/lib.rs`, add after `pub mod error;` (any position works):

```rust
pub mod pagination;
```

- [ ] **Step 2: Write the module with tests**

Create `crates/rupu-cp/src/pagination.rs`:

```rust
//! Shared offset/limit pagination for the list endpoints.
//!
//! Query params are lenient: a missing or unparseable bound falls back to the
//! default (offset 0, limit 20) rather than erroring, so a bad query string
//! never 500s a list. `limit` is clamped to `[1, 200]`.

use serde::Deserialize;

/// Default page size when `limit` is absent.
pub const DEFAULT_LIMIT: usize = 20;
/// Hard cap on `limit`.
pub const MAX_LIMIT: usize = 200;

/// Optional `?offset=&limit=` query params for a list endpoint.
#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

impl PageQuery {
    /// Resolved offset (default 0).
    pub fn offset(&self) -> usize {
        self.offset.unwrap_or(0)
    }
    /// Resolved limit (default `DEFAULT_LIMIT`, clamped to `[1, MAX_LIMIT]`).
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }
}

/// Slice `items` to the `[offset, offset+limit)` window. Out-of-range offset
/// yields an empty vec. Consumes the input so handlers can compute expensive
/// per-row work on the returned page only.
pub fn paginate<T>(items: Vec<T>, page: &PageQuery) -> Vec<T> {
    let offset = page.offset();
    let limit = page.limit();
    items.into_iter().skip(offset).take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(offset: Option<usize>, limit: Option<usize>) -> PageQuery {
        PageQuery { offset, limit }
    }

    #[test]
    fn default_limit_is_20() {
        let items: Vec<u32> = (0..50).collect();
        let page = paginate(items, &q(None, None));
        assert_eq!(page.len(), 20);
        assert_eq!(page[0], 0);
        assert_eq!(page[19], 19);
    }

    #[test]
    fn offset_and_limit_slice() {
        let items: Vec<u32> = (0..50).collect();
        let page = paginate(items, &q(Some(20), Some(5)));
        assert_eq!(page, vec![20, 21, 22, 23, 24]);
    }

    #[test]
    fn offset_past_end_is_empty() {
        let items: Vec<u32> = (0..10).collect();
        assert!(paginate(items, &q(Some(100), Some(20))).is_empty());
    }

    #[test]
    fn limit_is_clamped() {
        assert_eq!(q(None, Some(0)).limit(), 1);
        assert_eq!(q(None, Some(9999)).limit(), MAX_LIMIT);
        assert_eq!(q(None, Some(50)).limit(), 50);
    }
}
```

- [ ] **Step 3: Test + clippy**

Run: `cargo test -p rupu-cp pagination` → 4 pass.
Run: `cargo clippy -p rupu-cp --all-targets` → clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cp/src/pagination.rs crates/rupu-cp/src/lib.rs
git commit -m "feat(cp): PageQuery + paginate helper (offset/limit, default 20)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Paginate the run lists (slice before usage)

**Files:**
- Modify: `crates/rupu-cp/src/api/runs.rs`

`list_runs` and `list_workflow_runs` currently map `RunListRow::with_usage` over ALL runs (reading every transcript). Change them to sort newest-first, **paginate**, then compute usage on the page only.

- [ ] **Step 1: Add the `Query` extractor import**

At the top of `crates/rupu-cp/src/api/runs.rs`, the axum import currently is:

```rust
use axum::{
    extract::{Path, State},
    response::{IntoResponse as _, Response},
    routing::get,
    Json, Router,
};
```

Change `extract::{Path, State}` to `extract::{Path, Query, State}`.

- [ ] **Step 2: Paginate `list_runs`**

Replace `list_runs` with:

```rust
async fn list_runs(
    State(s): State<AppState>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<RunListRow>>> {
    let mut runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    let page_runs = crate::pagination::paginate(runs, &page);
    Ok(Json(
        page_runs
            .iter()
            .map(|r| RunListRow::with_usage(r, &s.run_store, &s.pricing))
            .collect(),
    ))
}
```

- [ ] **Step 3: Paginate `list_workflow_runs`**

Replace `list_workflow_runs` with:

```rust
async fn list_workflow_runs(
    State(s): State<AppState>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<RunListRow>>> {
    let mut runs: Vec<rupu_orchestrator::RunRecord> = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_iter()
        .filter(|r| r.event.is_none() && r.source_wake_id.is_none())
        .collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    let page_runs = crate::pagination::paginate(runs, &page);
    Ok(Json(
        page_runs
            .iter()
            .map(|r| RunListRow::with_usage(r, &s.run_store, &s.pricing))
            .collect(),
    ))
}
```

(`RunRecord` is already imported in this file via `use rupu_orchestrator::{RunRecord, RunStatus, RunStoreError};`.)

- [ ] **Step 4: Verify**

Run: `cargo test -p rupu-cp` → green. `cargo clippy -p rupu-cp --all-targets` → clean. `cargo build -p rupu-cp` → builds.
`git status --short` → only `runs.rs` modified (+ untracked `.rupu/*`).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/runs.rs
git commit -m "feat(cp): paginate /api/runs + /api/runs/workflows (slice before usage)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `AgentRunRow.usage` + paginate `/api/runs/agents`

**Files:**
- Modify: `crates/rupu-cp/src/api/run_streams.rs`

`AgentRunRow` gains `usage: UsageSummary`, defaulted in the two collect helpers and filled (from each row's `transcript_path`) AFTER pagination — so only the page reads transcripts.

- [ ] **Step 1: Add the `Query` import + the `usage` field**

Change the axum import at the top of `crates/rupu-cp/src/api/run_streams.rs`:

```rust
use axum::{extract::State, routing::get, Json, Router};
```
to:
```rust
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
```

Add the field to `AgentRunRow` (after `transcript_path`):

```rust
    transcript_path: Option<String>,
    usage: crate::usage::UsageSummary,
```

- [ ] **Step 2: Default the field in both collect helpers**

In `collect_standalone_runs`, the `rows.push(AgentRunRow { … })` literal — add:

```rust
            transcript_path,
            usage: crate::usage::UsageSummary::default(),
```

In `collect_session_runs_from_dir`, the `out.push(AgentRunRow { … })` literal — add:

```rust
                    transcript_path: run.transcript_path,
                    usage: crate::usage::UsageSummary::default(),
```

- [ ] **Step 3: Paginate + fill usage in the handler**

Replace `list_agent_runs` with:

```rust
async fn list_agent_runs(
    State(s): State<AppState>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<AgentRunRow>>> {
    let mut rows = collect_standalone_runs(&s.global_dir);
    collect_session_runs_from_dir(&s.global_dir.join("sessions"), &mut rows);
    collect_session_runs_from_dir(&s.global_dir.join("sessions-archive"), &mut rows);

    // Newest-first: rows with a timestamp sort before those without; ISO-8601
    // strings sort lexicographically.
    rows.sort_by(|a, b| match (&b.started_at, &a.started_at) {
        (Some(bt), Some(at)) => bt.cmp(at),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    // Slice to the page BEFORE reading transcripts, then fill usage per row.
    let mut page_rows = crate::pagination::paginate(rows, &page);
    for row in &mut page_rows {
        if let Some(tp) = &row.transcript_path {
            row.usage = crate::usage::summarize_paths(
                &[std::path::PathBuf::from(tp)],
                &s.pricing,
            );
        }
    }
    Ok(Json(page_rows))
}
```

- [ ] **Step 4: Verify**

Run: `cargo test -p rupu-cp` → green. `cargo clippy -p rupu-cp --all-targets` → clean.
`git status --short` → only `run_streams.rs` modified.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/run_streams.rs
git commit -m "feat(cp): agent-run usage + paginate /api/runs/agents

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Autoflow cycle + event usage + pagination

**Files:**
- Modify: `crates/rupu-cp/src/api/run_streams.rs`

`AutoflowCycleRow` gains `usage` (rolled up over the cycle's `run_ids`); `AutoflowEventRow` gains `usage` (from its `run_id` when present). Both paginate, filling usage on the page only.

- [ ] **Step 1: `AutoflowCycleRow.usage` + default in `From`**

Add the field to `AutoflowCycleRow` (after `run_ids`):

```rust
    run_ids: Vec<String>,
    usage: crate::usage::UsageSummary,
```

In `impl From<AutoflowCycleRecord> for AutoflowCycleRow`, the returned `Self { … }` — add as the last field:

```rust
            run_ids,
            usage: crate::usage::UsageSummary::default(),
```

- [ ] **Step 2: Paginate + roll up usage in `list_autoflow_runs`**

Replace `list_autoflow_runs` with:

```rust
async fn list_autoflow_runs(
    State(s): State<AppState>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<AutoflowCycleRow>>> {
    let store_root = s.global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);

    let records = match store.list_recent(100) {
        Ok(r) => r,
        Err(AutoflowHistoryStoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Vec::new()
        }
        Err(e) => return Err(crate::error::ApiError::internal(e.to_string())),
    };

    // list_recent already returns newest-first. Convert, paginate, then roll up
    // usage across each cycle's runs on the page only.
    let rows: Vec<AutoflowCycleRow> = records.into_iter().map(AutoflowCycleRow::from).collect();
    let mut page_rows = crate::pagination::paginate(rows, &page);
    for row in &mut page_rows {
        row.usage = crate::usage::rollup(
            row.run_ids
                .iter()
                .map(|id| crate::usage::summarize_run(&s.run_store, id, &s.pricing)),
        );
    }
    Ok(Json(page_rows))
}
```

- [ ] **Step 3: `AutoflowEventRow.usage`**

Add the field to `AutoflowEventRow` (after `worker_name`):

```rust
    worker_name: Option<String>,
    usage: crate::usage::UsageSummary,
```

- [ ] **Step 4: Paginate + fill usage in `list_autoflow_events`**

Replace `list_autoflow_events` with:

```rust
async fn list_autoflow_events(
    State(s): State<AppState>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<AutoflowEventRow>>> {
    let store_root = s.global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);

    let records = match store.list_recent_events(200) {
        Ok(r) => r,
        Err(AutoflowHistoryStoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Vec::new()
        }
        Err(e) => return Err(crate::error::ApiError::internal(e.to_string())),
    };

    let rows: Vec<AutoflowEventRow> = records
        .into_iter()
        .filter(|rec| is_actionable_kind(rec.event.kind))
        .map(|rec| AutoflowEventRow {
            event_id: rec.event_id,
            cycle_id: rec.cycle_id,
            at: rec.at,
            kind: kind_to_snake_case(rec.event.kind),
            workflow: rec.event.workflow,
            issue_display_ref: rec.event.issue_display_ref,
            run_id: rec.event.run_id,
            status: rec.event.status,
            worker_name: rec.worker_name,
            usage: crate::usage::UsageSummary::default(),
        })
        .collect();

    // Paginate, then fill usage from each event's run_id (when present).
    let mut page_rows = crate::pagination::paginate(rows, &page);
    for row in &mut page_rows {
        if let Some(id) = &row.run_id {
            row.usage = crate::usage::summarize_run(&s.run_store, id, &s.pricing);
        }
    }
    Ok(Json(page_rows))
}
```

- [ ] **Step 5: Verify**

Run: `cargo test -p rupu-cp` → green. `cargo clippy -p rupu-cp --all-targets` → clean.
`git status --short` → only `run_streams.rs` modified.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/api/run_streams.rs
git commit -m "feat(cp): autoflow cycle + event usage + pagination

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `RecentRun.usage` on the Dashboard

**Files:**
- Modify: `crates/rupu-cp/src/api/dashboard.rs`

The Dashboard's `recent_runs` (≤10) gains `usage`. This list is already capped at 10 — no pagination needed, just usage.

- [ ] **Step 1: Add the field to `RecentRun`**

`RecentRun` currently is:

```rust
#[derive(Serialize)]
struct RecentRun {
    id: String,
    workflow_name: String,
    status: &'static str,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}
```

Add:

```rust
    finished_at: Option<DateTime<Utc>>,
    usage: crate::usage::UsageSummary,
```

- [ ] **Step 2: Fill it in the `recent_runs` builder**

In `get_dashboard`, the `recent_runs` map currently builds each `RecentRun { … }`. Add the usage computation. The mapping is:

```rust
    let recent_runs: Vec<RecentRun> = runs_sorted
        .into_iter()
        .take(10)
        .map(|r| RecentRun {
            id: r.id.clone(),
            workflow_name: r.workflow_name.clone(),
            status: r.status.as_str(),
            started_at: r.started_at,
            finished_at: r.finished_at,
            usage: crate::usage::summarize_run(&s.run_store, &r.id, &s.pricing),
        })
        .collect();
```

(`s` is the `AppState` in `get_dashboard`; `run_store` + `pricing` are available.)

- [ ] **Step 3: Verify**

Run: `cargo test -p rupu-cp` → green. `cargo clippy -p rupu-cp --all-targets` → clean.
`git status --short` → only `dashboard.rs` modified.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cp/src/api/dashboard.rs
git commit -m "feat(cp): usage on dashboard recent runs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Paginate sessions + project runs/sessions

**Files:**
- Modify: `crates/rupu-cp/src/api/sessions.rs`, `crates/rupu-cp/src/api/projects.rs`

`/api/sessions`, `/api/projects/:ws_id/runs`, `/api/projects/:ws_id/sessions` gain pagination. Session usage is already cheap (from `session.json` fields, no transcript), so compute-then-slice is acceptable for sessions; project *runs* slice before `with_usage` (transcript reads).

- [ ] **Step 1: Paginate `list_sessions` (`sessions.rs`)**

Add the `Query` import — change:
```rust
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
```
to add `Query`:
```rust
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
```

Replace `list_sessions`:

```rust
async fn list_sessions(
    State(s): State<AppState>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let sessions = collect_sessions(&s.global_dir, &s.pricing);
    Ok(Json(crate::pagination::paginate(sessions, &page)))
}
```

(`collect_sessions` already returns newest-relevant order from the active+archive scan; sessions are few and usage is cheap, so slicing the assembled vec is fine.)

- [ ] **Step 2: Paginate `project_runs` + `project_sessions` (`projects.rs`)**

Add the `Query` import in `projects.rs` — change:
```rust
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
```
to add `Query`:
```rust
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
```

Replace `project_runs`:

```rust
async fn project_runs(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<RunListRow>>> {
    load_workspace(&s, &ws_id)?;
    let runs = scoped_runs(&s, &ws_id)?; // already sorted newest-first
    let page_runs = crate::pagination::paginate(runs, &page);
    Ok(Json(
        page_runs
            .iter()
            .map(|r| RunListRow::with_usage(r, &s.run_store, &s.pricing))
            .collect(),
    ))
}
```

Replace `project_sessions`:

```rust
async fn project_sessions(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    load_workspace(&s, &ws_id)?;
    let scoped: Vec<Value> = crate::api::sessions::collect_sessions(&s.global_dir, &s.pricing)
        .into_iter()
        .filter(|v| v["workspace_id"].as_str() == Some(ws_id.as_str()))
        .collect();
    Ok(Json(crate::pagination::paginate(scoped, &page)))
}
```

- [ ] **Step 3: Verify**

Run: `cargo test -p rupu-cp` → green. `cargo clippy -p rupu-cp --all-targets` → clean. `cargo build -p rupu-cp` → builds.
`git status --short` → only `sessions.rs` + `projects.rs` modified.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cp/src/api/sessions.rs crates/rupu-cp/src/api/projects.rs
git commit -m "feat(cp): paginate /api/sessions + project runs/sessions

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Integration smoke + workspace clippy

**Files:** none (verification)

- [ ] **Step 1: Full crate gate**

Run: `cargo test -p rupu-cp` → all green. `cargo clippy -p rupu-cp --all-targets` → clean.

- [ ] **Step 2: Manual pagination + usage smoke (optional)**

```bash
cargo run -p rupu-cli -- cp serve --bind 127.0.0.1:8788 &
sleep 1
curl -s 'http://127.0.0.1:8788/api/runs?offset=0&limit=2' | head -c 300   # ≤2 rows, each with usage
curl -s 'http://127.0.0.1:8788/api/runs?offset=2&limit=2' | head -c 300   # next slice, no overlap
curl -s 'http://127.0.0.1:8788/api/runs/agents?limit=2' | head -c 400     # agent rows carry usage
kill %1
```
Expected: each row carries a `usage` object; `offset` advances the window.

- [ ] **Step 3: Confirm clean tree**

Run: `git status --short` → only untracked `.rupu/*`.

(No commit — verification only.)

---

## Done criteria

- `cargo test -p rupu-cp` green; `cargo clippy -p rupu-cp --all-targets` clean.
- `/api/runs`, `/api/runs/workflows`, `/api/runs/agents`, `/api/runs/autoflows`, `/api/runs/autoflows/events`, `/api/sessions`, `/api/projects/:id/runs`, `/api/projects/:id/sessions` all accept `?offset&limit` (default 20, clamp 200) and slice **before** computing usage.
- `AgentRunRow`, `AutoflowCycleRow`, `AutoflowEventRow`, and Dashboard `RecentRun` all carry a `usage` summary.
- `rupu-cp` still has no `rupu-cli` dependency; no rustfmt drift committed.
- Frontend wiring (the hook + chips + infinite-scroll) is **Plan 4b**.
