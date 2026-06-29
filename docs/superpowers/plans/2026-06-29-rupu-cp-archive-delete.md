# CP archive / delete for runs & sessions — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose archive / restore / delete for runs and sessions in the CP web control panel. Runs get net-new `RunStore` lifecycle methods + direct CP endpoints + CLI parity; sessions reuse the existing CLI logic via a `SessionMutator` subprocess adapter.

**Architecture:** Runs are addressable directly through `AppState.run_store` (no adapter): new `RunStore::archive/restore/delete/list_archived` move/remove the run dir (`<global>/runs/<id>` ↔ `<global>/runs-archive/<id>`). Sessions reach the CLI's archive/restore/delete logic through a new optional `SessionMutator` port wired only under `rupu cp serve` (501 when absent), mirroring `SessionStarter`. Transcripts ride with their owning run/session. The frontend mirrors the existing `AgentDetail` delete idiom (danger-outline button + `window.confirm`); archive/restore are reversible (no confirm).

**Tech Stack:** Rust 2021 (rupu-orchestrator, rupu-cp axum handlers, rupu-cli clap + tokio subprocess), React 18 + TypeScript + vitest (CP frontend).

## Global Constraints

- Workspace deps only — versions pinned in root `Cargo.toml`; never in crate `Cargo.toml` files.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden. Errors: `thiserror` for libraries, `anyhow` for the CLI binary.
- `rupu-cli` stays thin: arg parsing + delegation. `rupu-cp` must NOT gain a provider/credential dependency; session mutation is reached only through the adapter port (501 when absent).
- **Delete is hard/irreversible and MUST confirm in the UI (`window.confirm`). Archive/restore are reversible — no confirm.**
- Run archive/delete require a **terminal** run status (`Completed | Failed | Rejected | Cancelled`); non-terminal → 409 (`NotTerminal`). Session archive/delete require a non-running session (the CLI already enforces this).
- HTTP mapping: not-found → 404, wrong-state/conflict → 409, adapter-absent → 501, other → 500.
- ⚠️ **FORMATTING HAZARD:** never run `cargo fmt -p <crate>` or `cargo fmt` (reflows the whole crate → drift). Format only the file you touched: `rustfmt --edition 2021 <file>`. Before each commit, `git status --short` must show only intended files.
- Frontend commands run from `crates/rupu-cp/web`. `make cp-web` rebuilds the embedded UI before any release (not required for the Rust tasks).
- Tests that set `RUPU_HOME`/env or spawn subprocesses must serialize on a shared lock where the env is process-global; mirror existing tests in the touched crate.

---

## File Structure

**Slice 1 — runs library**
- Modify `crates/rupu-orchestrator/src/runs.rs` — `RunStore::archive/restore/delete/list_archived`, `RunStoreError::NotTerminal`, helpers, unit tests.

**Slice 2 — runs surface**
- Modify `crates/rupu-cp/src/api/runs.rs` — `archive_run`/`restore_run`/`delete_run`/`list_archived_runs` handlers + routes + tests.
- Modify `crates/rupu-cli/src/cmd/workflow.rs` — `archive-run`/`restore-run`/`delete-run` subcommands + tests.
- Modify `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/RunDetail.tsx`, `crates/rupu-cp/web/src/pages/WorkflowRuns.tsx` (+ sibling run lists) — API methods, buttons, Archived filter + vitest.

**Slice 3 — sessions surface**
- Create `crates/rupu-cp/src/session_mutator.rs` — the port.
- Modify `crates/rupu-cp/src/lib.rs`, `crates/rupu-cp/src/state.rs` — `ServeOpts.session_mutator`, `AppState` field + wither.
- Modify `crates/rupu-cp/src/api/sessions.rs` — `archive_session`/`restore_session`/`delete_session` + routes + tests.
- Create `crates/rupu-cli/src/cp_session_mutator.rs` — `SubprocessSessionMutator`; modify `crates/rupu-cli/src/lib.rs`, `crates/rupu-cli/src/cmd/cp.rs` to wire it.
- Modify `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/Sessions.tsx`, `crates/rupu-cp/web/src/pages/SessionDetail.tsx` — API methods, row actions, detail buttons + vitest.

---

## Slice 1 — Runs library

### Task 1: `RunStore` archive / restore / delete / list_archived

**Files:**
- Modify: `crates/rupu-orchestrator/src/runs.rs`
- Test: inline `#[cfg(test)] mod tests` in the same file

**Interfaces:**
- Consumes: `RunStore { pub root: PathBuf }` (`root` = `<global>/runs`), `run_dir(id) = self.root.join(id)`, `list()` (reads `self.root`), `RunRecord { id, status: RunStatus, started_at, .. }`, `RunStatus` (terminal = Completed/Failed/Rejected/Cancelled), `RunStoreError { Io, Json, NotFound(String), AlreadyExists(String) }`, test helper `sample_record(id)`.
- Produces: `RunStoreError::NotTerminal(String)`; `RunStore::{archive, restore, delete, list_archived}` with signatures below.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/rupu-orchestrator/src/runs.rs`. The existing tests build a store over a tempdir and write a record — mirror that. (`sample_record` defaults `status: RunStatus::Pending`; set the status explicitly where a terminal run is needed.)

```rust
    #[test]
    fn archive_moves_run_out_of_list_and_restore_brings_it_back() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let mut rec = sample_record("run_01ARCHIVE");
        rec.status = RunStatus::Completed;
        store.put(&rec).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);

        store.archive(&rec.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 0, "archived run leaves active list");
        let archived = store.list_archived().unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, rec.id);
        // The archive dir is a sibling of the runs dir.
        assert!(tmp.path().join("runs-archive").join(&rec.id).join("run.json").is_file());

        store.restore(&rec.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(store.list_archived().unwrap().len(), 0);
    }

    #[test]
    fn archive_requires_terminal_status() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let mut rec = sample_record("run_01RUNNING");
        rec.status = RunStatus::Running;
        store.put(&rec).unwrap();
        match store.archive(&rec.id) {
            Err(RunStoreError::NotTerminal(_)) => {}
            other => panic!("expected NotTerminal, got {other:?}"),
        }
        // Still listed, untouched.
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn delete_removes_from_either_scope_and_then_404s() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let mut rec = sample_record("run_01DELETE");
        rec.status = RunStatus::Failed;
        store.put(&rec).unwrap();

        store.delete(&rec.id).unwrap();
        assert!(!tmp.path().join("runs").join(&rec.id).exists());
        match store.delete(&rec.id) {
            Err(RunStoreError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn archive_missing_run_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        match store.archive("run_NOPE") {
            Err(RunStoreError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
```

If the store has no `put`/write helper used by existing tests, use whatever the existing tests call to persist a `RunRecord` (search the `tests` module for how `sample_record` is written to disk — e.g. `store.put(&rec)` or a direct `run.json` write). Match it.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-orchestrator runs::tests::archive runs::tests::delete_removes`
Expected: FAIL — `no method named archive` / `no variant NotTerminal`.

- [ ] **Step 3: Add the `NotTerminal` error variant**

In `RunStoreError` (the `#[derive(Debug, Error)] pub enum RunStoreError`), add:

```rust
    #[error("run `{0}` is not in a terminal state (cancel it first)")]
    NotTerminal(String),
```

- [ ] **Step 4: Add a `RunStatus::is_terminal` check if absent, then the four methods**

If `RunStatus` does not already expose a terminal check, add one near the enum:

```rust
impl RunStatus {
    /// A run that will not change state on its own.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Rejected | RunStatus::Cancelled
        )
    }
}
```

(If a terminal helper already exists, reuse it and skip this addition.)

Add the lifecycle methods to `impl RunStore` (near `list`):

```rust
    /// Directory holding archived runs — sibling of the active runs dir
    /// (`<global>/runs` → `<global>/runs-archive`).
    fn archive_root(&self) -> PathBuf {
        self.root.with_file_name("runs-archive")
    }

    /// Read run records from an arbitrary runs root (active or archive),
    /// newest first. Shared by `list` / `list_archived`.
    fn list_in(root: &std::path::Path) -> Result<Vec<RunRecord>, RunStoreError> {
        let mut out: Vec<RunRecord> = Vec::new();
        let rd = match std::fs::read_dir(root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for entry in rd.flatten() {
            let p = entry.path().join("run.json");
            if !p.is_file() {
                continue;
            }
            if let Ok(body) = std::fs::read(&p) {
                if let Ok(rec) = serde_json::from_slice::<RunRecord>(&body) {
                    out.push(rec);
                }
            }
        }
        out.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        Ok(out)
    }

    /// List archived runs (reads `<root>/../runs-archive`), newest first.
    pub fn list_archived(&self) -> Result<Vec<RunRecord>, RunStoreError> {
        Self::list_in(&self.archive_root())
    }

    /// Move `runs/<id>` → `runs-archive/<id>` (reversible). Requires the run
    /// to exist and be in a terminal state. The run dir carries its own
    /// transcript artifacts, so the rename takes them with it.
    pub fn archive(&self, run_id: &str) -> Result<(), RunStoreError> {
        let src = self.run_dir(run_id);
        if !src.join("run.json").is_file() {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        }
        let rec: RunRecord = serde_json::from_slice(&std::fs::read(src.join("run.json"))?)?;
        if !rec.status.is_terminal() {
            return Err(RunStoreError::NotTerminal(run_id.to_string()));
        }
        let dst = self.archive_root().join(run_id);
        if dst.exists() {
            return Err(RunStoreError::AlreadyExists(run_id.to_string()));
        }
        std::fs::create_dir_all(self.archive_root())?;
        std::fs::rename(&src, &dst)?;
        Ok(())
    }

    /// Move `runs-archive/<id>` → `runs/<id>`.
    pub fn restore(&self, run_id: &str) -> Result<(), RunStoreError> {
        let src = self.archive_root().join(run_id);
        if !src.join("run.json").is_file() {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        }
        let dst = self.run_dir(run_id);
        if dst.exists() {
            return Err(RunStoreError::AlreadyExists(run_id.to_string()));
        }
        std::fs::create_dir_all(&self.root)?;
        std::fs::rename(&src, &dst)?;
        Ok(())
    }

    /// Permanently remove the run directory from whichever scope holds it.
    pub fn delete(&self, run_id: &str) -> Result<(), RunStoreError> {
        let active = self.run_dir(run_id);
        let archived = self.archive_root().join(run_id);
        let target = if active.is_dir() {
            active
        } else if archived.is_dir() {
            archived
        } else {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        };
        std::fs::remove_dir_all(&target)?;
        Ok(())
    }
```

Note: `delete` does not require terminal status here — the CP/CLI layer enforces the terminal guard before calling (Slice 2). If the existing `list()` body is identical to `list_in(&self.root)`, refactor `list` to `Self::list_in(&self.root)` to avoid duplication; otherwise leave `list` untouched.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rupu-orchestrator runs::tests`
Expected: PASS (all existing + 4 new).

- [ ] **Step 6: Format + clippy**

Run: `rustfmt --edition 2021 crates/rupu-orchestrator/src/runs.rs && cargo clippy -p rupu-orchestrator --all-targets 2>&1 | grep "runs.rs" || echo "no runs.rs clippy"`
Expected: no `runs.rs` warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-orchestrator/src/runs.rs
git commit -m "feat(orchestrator): RunStore archive/restore/delete/list_archived"
```

---

## Slice 2 — Runs surface

### Task 2: CP run endpoints

**Files:**
- Modify: `crates/rupu-cp/src/api/runs.rs`
- Test: inline `#[cfg(test)] mod tests` in `runs.rs` (mirror existing handler tests there if present; otherwise build `AppState::new(tmp, Default::default())` and a tempdir run store)

**Interfaces:**
- Consumes: `s.run_store` (`Arc<RunStore>`), `RunStore::{archive, restore, delete, list_archived, load}`, `RunStoreError`, `ApiError::{not_found, conflict, internal}`, the existing `RunListRow` shape used by `list_runs`.
- Produces: `archive_run`, `restore_run`, `delete_run`, `list_archived_runs` handlers + routes.

- [ ] **Step 1: Add the routes**

In `routes()` (the `Router::new()...` block), add:

```rust
        .route("/api/runs/archived", get(list_archived_runs))
        .route("/api/runs/:id/archive", post(archive_run))
        .route("/api/runs/:id/restore", post(restore_run))
```

and add `.delete(delete_run)` to the existing `/api/runs/:id` route so it reads:

```rust
        .route("/api/runs/:id", get(get_run).delete(delete_run))
```

(Register `/api/runs/archived` BEFORE `/api/runs/:id` is not required — axum prefers the static segment — but keep it adjacent for readability.)

- [ ] **Step 2: Write the failing handler tests**

Add to the `tests` module in `runs.rs` (mirror how existing tests construct state; if none, use this shape):

```rust
    fn terminal_record(id: &str) -> rupu_orchestrator::runs::RunRecord {
        // Build via the orchestrator's own constructor/builder used elsewhere in
        // these tests; set status to Completed so archive/delete are allowed.
        // (Mirror the existing run-record fixture in this test module.)
        todo!("use the existing run-record fixture; set status = Completed")
    }

    #[tokio::test]
    async fn archive_then_delete_run_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default());
        let rec = terminal_record("run_01CPFLOW");
        state.run_store.put(&rec).unwrap();

        // archive
        archive_run(State(state.clone()), Path(rec.id.clone())).await.expect("archive ok");
        assert_eq!(state.run_store.list().unwrap().len(), 0);
        assert_eq!(state.run_store.list_archived().unwrap().len(), 1);

        // delete (from archive)
        delete_run(State(state.clone()), Path(rec.id.clone())).await.expect("delete ok");
        let err = delete_run(State(state.clone()), Path(rec.id.clone())).await.unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn archive_running_run_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default());
        let mut rec = terminal_record("run_01RUN");
        rec.status = rupu_orchestrator::runs::RunStatus::Running;
        state.run_store.put(&rec).unwrap();
        let err = archive_run(State(state.clone()), Path(rec.id.clone())).await.unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }
```

Replace the `terminal_record` placeholder with the real run-record fixture used by the existing `runs.rs` tests (search the module). If `AppState::new`'s second arg type differs, match the existing test calls.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p rupu-cp --lib api::runs::tests::archive_then_delete_run_flow`
Expected: FAIL — handlers not defined.

- [ ] **Step 4: Implement the handlers**

Add a shared error mapper + the handlers in `runs.rs`:

```rust
fn map_run_store_err(id: &str, e: rupu_orchestrator::runs::RunStoreError) -> ApiError {
    use rupu_orchestrator::runs::RunStoreError as E;
    match e {
        E::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        E::NotTerminal(_) => ApiError::conflict(format!("run {id} is not terminal — cancel it first")),
        E::AlreadyExists(_) => ApiError::conflict(format!("run {id} already exists in the target scope")),
        E::Io(err) => ApiError::internal(err.to_string()),
        E::Json(err) => ApiError::internal(err.to_string()),
    }
}

async fn archive_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    s.run_store.archive(&id).map_err(|e| map_run_store_err(&id, e))?;
    Ok(Json(serde_json::json!({ "ok": true, "id": id, "archived": true })))
}

async fn restore_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    s.run_store.restore(&id).map_err(|e| map_run_store_err(&id, e))?;
    Ok(Json(serde_json::json!({ "ok": true, "id": id, "archived": false })))
}

async fn delete_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Guard: refuse to delete a non-terminal run (mirror archive). A run only
    // present in the archive is already terminal; load from whichever scope.
    if let Ok(rec) = s.run_store.load(&id) {
        if !rec.status.is_terminal() {
            return Err(ApiError::conflict(format!(
                "run {id} is not terminal — cancel it first"
            )));
        }
    }
    s.run_store.delete(&id).map_err(|e| map_run_store_err(&id, e))?;
    Ok(Json(serde_json::json!({ "ok": true, "id": id, "deleted": true })))
}

async fn list_archived_runs(
    State(s): State<AppState>,
) -> ApiResult<Json<Vec<RunListRow>>> {
    let records = s
        .run_store
        .list_archived()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(records.into_iter().map(RunListRow::from).collect()))
}
```

`list_archived_runs` must produce the SAME row type as `list_runs`. Inspect how `list_runs` converts `RunRecord` → `RunListRow` (it may use a `From` impl, a `row_from_record` helper, or inline mapping) and reuse that EXACT conversion — do not invent a new shape. If `load` returns archived runs too, the delete guard works for both scopes; if `load` only checks the active scope, the guard simply skips for archived runs (which are already terminal), which is fine.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p rupu-cp --lib api::runs::tests`
Expected: PASS.

- [ ] **Step 6: Format, clippy, commit**

Run: `rustfmt --edition 2021 crates/rupu-cp/src/api/runs.rs && cargo clippy -p rupu-cp --all-targets 2>&1 | grep "api/runs.rs" || echo "clean"`

```bash
git add crates/rupu-cp/src/api/runs.rs
git commit -m "feat(cp): /api/runs archive/restore/delete + archived list endpoints"
```

### Task 3: CLI run-management commands

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs`
- Test: `crates/rupu-cli/tests/` (add a focused integration test, or unit-test the store calls)

**Interfaces:**
- Consumes: `RunStore::{archive, restore, delete}`, `paths::global_dir()`, the existing `workflow` clap `Action` enum + `handle` dispatcher.
- Produces: `ArchiveRun`/`RestoreRun`/`DeleteRun` subcommands.

- [ ] **Step 1: Add the clap variants**

In `crates/rupu-cli/src/cmd/workflow.rs`, add to the `Action` enum (next to `Cancel`/`Approve`/`Reject`/`Resume`):

```rust
    /// Archive a terminal run (move it out of the active list; reversible).
    ArchiveRun {
        /// Full run id (`run_<ULID>`).
        run_id: String,
    },
    /// Restore an archived run back to the active list.
    RestoreRun {
        /// Full run id (`run_<ULID>`).
        run_id: String,
    },
    /// Permanently delete a run and its transcripts. Requires `--force`.
    DeleteRun {
        /// Full run id (`run_<ULID>`).
        run_id: String,
        #[arg(long)]
        force: bool,
    },
```

- [ ] **Step 2: Dispatch + handlers**

Add arms to the `handle` match and the handler fns. Resolve the store as the CLI already does for run commands (the workflow run commands build a `RunStore` over `paths::global_dir()?.join("runs")` — reuse that exact construction):

```rust
        Action::ArchiveRun { run_id } => archive_run(&run_id).await,
        Action::RestoreRun { run_id } => restore_run(&run_id).await,
        Action::DeleteRun { run_id, force } => delete_run(&run_id, force).await,
```

```rust
async fn archive_run(run_id: &str) -> anyhow::Result<()> {
    let store = rupu_orchestrator::runs::RunStore::new(paths::global_dir()?.join("runs"));
    store.archive(run_id)?;
    println!("archived run {run_id}");
    Ok(())
}

async fn restore_run(run_id: &str) -> anyhow::Result<()> {
    let store = rupu_orchestrator::runs::RunStore::new(paths::global_dir()?.join("runs"));
    store.restore(run_id)?;
    println!("restored run {run_id}");
    Ok(())
}

async fn delete_run(run_id: &str, force: bool) -> anyhow::Result<()> {
    if !force {
        anyhow::bail!("run delete requires --force");
    }
    let store = rupu_orchestrator::runs::RunStore::new(paths::global_dir()?.join("runs"));
    store.delete(run_id)?;
    println!("deleted run {run_id}");
    Ok(())
}
```

`RunStoreError` implements `std::error::Error` (via `thiserror`), so `?` converts into `anyhow::Error` automatically. Match the existing handler signatures/return type in this file (they are `async fn ... -> anyhow::Result<()>` dispatched from `handle`).

- [ ] **Step 3: Write a focused test**

Mirror an existing `workflow.rs` handler test if present (a `#[cfg(test)]` calling the store directly over a tempdir). Minimal:

```rust
    #[tokio::test]
    async fn delete_run_requires_force() {
        let err = delete_run("run_x", false).await.unwrap_err();
        assert!(err.to_string().contains("--force"));
    }
```

(The archive/restore happy paths are already covered at the store level in Task 1; this guards the CLI `--force` gate without needing a global dir.)

- [ ] **Step 4: Build, test, format, clippy, commit**

Run: `cargo test -p rupu-cli --lib cmd::workflow 2>&1 | grep -E "test result|delete_run_requires" | head` then `cargo build -p rupu-cli`
Run: `rustfmt --edition 2021 crates/rupu-cli/src/cmd/workflow.rs && cargo clippy -p rupu-cli --all-targets 2>&1 | grep "cmd/workflow.rs" || echo "clean"`
Expected: green; build ok (note: rupu-cli may have a pre-existing clippy baseline in `autoflow.rs` — ignore anything not in `workflow.rs`).

```bash
git add crates/rupu-cli/src/cmd/workflow.rs
git commit -m "feat(cli): rupu workflow archive-run/restore-run/delete-run"
```

### Task 4: Runs frontend (API + RunDetail buttons + Archived filter)

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/RunDetail.tsx`, `crates/rupu-cp/web/src/pages/WorkflowRuns.tsx`
- Test: `crates/rupu-cp/web/src/pages/RunDetail.archive.test.tsx` (new)

**Interfaces:**
- Consumes: the `request<T>` wrapper, existing `cancelRun`/`getRun`/`getRuns`, the `AgentDetail` delete idiom, the `RunDetail` cancel button.
- Produces: `api.archiveRun/restoreRun/deleteRun/getArchivedRuns`; archive/delete UI.

- [ ] **Step 1: Add API client methods**

In `crates/rupu-cp/web/src/lib/api.ts`, in the `// --- Runs ---` section (next to `cancelRun`):

```typescript
  async archiveRun(id: string): Promise<void> {
    await request(`/api/runs/${encodeURIComponent(id)}/archive`, { method: 'POST' });
  },
  async restoreRun(id: string): Promise<void> {
    await request(`/api/runs/${encodeURIComponent(id)}/restore`, { method: 'POST' });
  },
  async deleteRun(id: string): Promise<void> {
    await request(`/api/runs/${encodeURIComponent(id)}`, { method: 'DELETE' });
  },
  getArchivedRuns(): Promise<RunListRow[]> {
    return request<RunListRow[]>('/api/runs/archived');
  },
```

- [ ] **Step 2: Write the failing RunDetail test**

Create `crates/rupu-cp/web/src/pages/RunDetail.archive.test.tsx`. Mirror an existing `RunDetail` test for how it mocks `api.getRun` and renders the page (read a sibling `*.test.tsx` in `src/pages` first for the render harness + router). Skeleton:

```tsx
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';
import RunDetail from './RunDetail';
import { api } from '../lib/api';

afterEach(() => vi.restoreAllMocks());

function renderAt(id: string) {
  return render(
    <MemoryRouter initialEntries={[`/runs/${id}`]}>
      <Routes><Route path="/runs/:id" element={<RunDetail />} /></Routes>
    </MemoryRouter>,
  );
}

describe('RunDetail archive/delete', () => {
  it('deletes after confirm and archives without confirm', async () => {
    // Mock getRun to return a terminal (completed) run — match the real shape.
    vi.spyOn(api, 'getRun').mockResolvedValue(/* completed run fixture */ {} as never);
    const archive = vi.spyOn(api, 'archiveRun').mockResolvedValue();
    const del = vi.spyOn(api, 'deleteRun').mockResolvedValue();
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderAt('run_01X');
    fireEvent.click(await screen.findByRole('button', { name: /archive/i }));
    await waitFor(() => expect(archive).toHaveBeenCalledWith('run_01X'));

    fireEvent.click(screen.getByRole('button', { name: /delete/i }));
    await waitFor(() => expect(del).toHaveBeenCalledWith('run_01X'));
    expect(window.confirm).toHaveBeenCalled();
  });
});
```

Fill the `getRun` fixture to match the real `{ run, steps, usage }` shape (copy from the sibling RunDetail test). Adjust button-name matchers to the labels you render in Step 3.

- [ ] **Step 3: Add Archive + Delete buttons to RunDetail**

In `RunDetail.tsx`, next to the existing Cancel button cluster, add Archive (shown when the run is terminal and not archived) and Delete (always, when terminal). Mirror the `cancelError`/`onCancel` pattern and the `AgentDetail` confirm:

```tsx
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionPending, setActionPending] = useState(false);

  async function onArchive() {
    if (actionPending) return;
    setActionPending(true); setActionError(null);
    try { await api.archiveRun(id); navigate('/runs'); }
    catch (e) { setActionError(e instanceof Error ? e.message : 'Archive failed'); setActionPending(false); }
  }

  async function onDelete() {
    if (actionPending) return;
    if (!window.confirm('Permanently delete this run and its transcripts? This cannot be undone.')) return;
    setActionPending(true); setActionError(null);
    try { await api.deleteRun(id); navigate('/runs'); }
    catch (e) { setActionError(e instanceof Error ? e.message : 'Delete failed'); setActionPending(false); }
  }
```

Render (in the header action cluster, gated on the run being terminal — reuse the existing `isRunning` to know it is NOT running):

```tsx
  {!isRunning && (
    <>
      <Button variant="secondary" onClick={onArchive} disabled={actionPending} className="gap-1.5">
        <Archive size={14} /> Archive
      </Button>
      <Button variant="danger-outline" onClick={onDelete} disabled={actionPending} className="gap-1.5">
        <Trash2 size={14} /> Delete
      </Button>
    </>
  )}
  {actionError && <p role="alert" className="text-ui font-medium text-err">{actionError}</p>}
```

Import `Archive` and `Trash2` from `lucide-react` (RunDetail already imports lucide icons — add to that import).

- [ ] **Step 4: Run the test**

Run (from `crates/rupu-cp/web`): `npx vitest run src/pages/RunDetail.archive.test.tsx`
Expected: PASS.

- [ ] **Step 5: Add an Archived toggle + row actions to WorkflowRuns**

In `WorkflowRuns.tsx`, add an `archived` boolean toggle (a segmented control or checkbox) that switches the fetch between `api.getRuns(...)` and `api.getArchivedRuns()`, and add an action column rendering: when not archived → Archive + Delete; when archived → Restore + Delete (each calling the api method then refetching). Mirror the `Sessions.tsx` active/archived tab pattern for the toggle and the `AgentDetail` confirm for Delete. Keep the same `SortableTable` + `Column<RunListRow>[]` structure already in the file. This is the canonical run list; the sibling lists (`AgentRuns.tsx`, `AutoflowRuns.tsx`) follow the IDENTICAL pattern — apply it to them too ONLY if their rows are run-store-backed (their row id resolves via `/api/runs/:id`); if a list's rows are not individually addressable, leave it and note it in the report.

- [ ] **Step 6: Typecheck + build + full suite, then commit**

Run (from `crates/rupu-cp/web`): `npm run build && npx vitest run`
Expected: clean build; suite green.

```bash
git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/pages/RunDetail.tsx crates/rupu-cp/web/src/pages/RunDetail.archive.test.tsx crates/rupu-cp/web/src/pages/WorkflowRuns.tsx
git commit -m "feat(cp/web): archive/restore/delete runs (RunDetail + run lists)"
```

---

## Slice 3 — Sessions surface

### Task 5: `SessionMutator` port + CP endpoints

**Files:**
- Create: `crates/rupu-cp/src/session_mutator.rs`
- Modify: `crates/rupu-cp/src/lib.rs` (mod decl + `ServeOpts.session_mutator` + serve wiring), `crates/rupu-cp/src/state.rs` (field + `None` init + `with_session_mutator`), `crates/rupu-cp/src/api/sessions.rs` (routes + handlers + tests)
- Modify: `crates/rupu-cli/src/cmd/cp.rs` — add `session_mutator: None` to the `ServeOpts { ... }` literal so the workspace still compiles (Task 6 flips it to the real adapter).

**Interfaces:**
- Produces: `SessionAction`, `SessionMutateError`, `SessionMutator` trait; `AppState.session_mutator`; `archive_session`/`restore_session`/`delete_session` handlers.

- [ ] **Step 1: Create the port with an inline dispatch test**

Create `crates/rupu-cp/src/session_mutator.rs`:

```rust
//! Port: archive / restore / delete sessions. rupu-cp defines it; rupu-cli's
//! `cp serve` provides the subprocess adapter that shells `rupu session
//! archive|restore|delete <id>`. `None` → the endpoints return 501.

use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    Archive,
    Restore,
    Delete,
}

impl SessionAction {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionAction::Archive => "archive",
            SessionAction::Restore => "restore",
            SessionAction::Delete => "delete",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionMutateError {
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("invalid session state: {0}")]
    Invalid(String),
    #[error("failed to {action} session: {message}")]
    Failed { action: &'static str, message: String },
}

#[async_trait]
pub trait SessionMutator: Send + Sync {
    async fn mutate(&self, id: &str, action: SessionAction) -> Result<(), SessionMutateError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Stub;
    #[async_trait]
    impl SessionMutator for Stub {
        async fn mutate(&self, _id: &str, action: SessionAction) -> Result<(), SessionMutateError> {
            if action == SessionAction::Restore {
                return Err(SessionMutateError::NotFound("x".into()));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn dispatches_through_trait_object() {
        let m: Arc<dyn SessionMutator> = Arc::new(Stub);
        assert!(m.mutate("s1", SessionAction::Archive).await.is_ok());
        assert!(matches!(
            m.mutate("s1", SessionAction::Restore).await,
            Err(SessionMutateError::NotFound(_))
        ));
    }
}
```

- [ ] **Step 2: Wire the module + AppState + ServeOpts**

In `crates/rupu-cp/src/lib.rs`: add `pub mod session_mutator;` (with the other `pub mod`s) and a `ServeOpts` field after `session_starter`:

```rust
    /// Optional session-mutator adapter. rupu-cli's `cp serve` provides the
    /// subprocess impl; `None` → the session archive/restore/delete endpoints
    /// return 501.
    pub session_mutator: Option<std::sync::Arc<dyn crate::session_mutator::SessionMutator>>,
```

In `serve()`, add `.with_session_mutator(opts.session_mutator.clone())` to the `AppState::new(...)` builder chain.

In `crates/rupu-cp/src/state.rs`: add `pub session_mutator: Option<Arc<dyn crate::session_mutator::SessionMutator>>,` to `AppState`, init `session_mutator: None` in `new`, and add the wither mirroring `with_session_starter`:

```rust
    pub fn with_session_mutator(
        mut self,
        m: Option<Arc<dyn crate::session_mutator::SessionMutator>>,
    ) -> Self {
        self.session_mutator = m;
        self
    }
```

In `crates/rupu-cli/src/cmd/cp.rs`: add `session_mutator: None, // wired in Task 6` to the `rupu_cp::ServeOpts { ... }` literal so rupu-cli still compiles.

- [ ] **Step 3: Write failing session handler tests**

Add to the `tests` module in `crates/rupu-cp/src/api/sessions.rs` (mirror the existing session test harness; if a stub-adapter pattern exists for `session_sender`, copy it):

```rust
    use crate::session_mutator::{SessionAction, SessionMutateError, SessionMutator};

    struct StubMutator;
    #[async_trait::async_trait]
    impl SessionMutator for StubMutator {
        async fn mutate(&self, id: &str, action: SessionAction) -> Result<(), SessionMutateError> {
            if id == "missing" {
                return Err(SessionMutateError::NotFound(id.into()));
            }
            if action == SessionAction::Archive && id == "active-running" {
                return Err(SessionMutateError::Invalid("session is running".into()));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn archive_session_ok_and_not_found_and_409() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default())
            .with_session_mutator(Some(std::sync::Arc::new(StubMutator)));
        archive_session(State(state.clone()), Path("s1".to_string())).await.expect("ok");
        let nf = archive_session(State(state.clone()), Path("missing".to_string())).await.unwrap_err();
        assert_eq!(nf.0, axum::http::StatusCode::NOT_FOUND);
        let conflict = archive_session(State(state.clone()), Path("active-running".to_string())).await.unwrap_err();
        assert_eq!(conflict.0, axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn archive_session_without_adapter_is_501() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default()); // no mutator
        let err = archive_session(State(state), Path("s1".to_string())).await.unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }
```

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p rupu-cp --lib api::sessions::tests::archive_session`
Expected: FAIL — handlers not defined.

- [ ] **Step 5: Add routes + handlers**

In `sessions.rs` `routes()`:

```rust
        .route("/api/sessions/:id/archive", post(archive_session))
        .route("/api/sessions/:id/restore", post(restore_session))
        .route("/api/sessions/:id", get(get_session).delete(delete_session))
```

(Change the existing `/api/sessions/:id` route to chain `.delete(delete_session)`.)

Handlers (a shared helper keeps them DRY):

```rust
async fn mutate_session(
    s: &AppState,
    id: &str,
    action: crate::session_mutator::SessionAction,
) -> ApiResult<Json<serde_json::Value>> {
    use crate::session_mutator::SessionMutateError as E;
    let m = s
        .session_mutator
        .clone()
        .ok_or_else(|| ApiError::not_available("session archive/delete requires `rupu cp serve`"))?;
    m.mutate(id, action).await.map_err(|e| match e {
        E::NotFound(_) => ApiError::not_found(format!("session {id} not found")),
        E::Invalid(msg) => ApiError::conflict(msg),
        E::Failed { message, .. } => ApiError::internal(message),
    })?;
    Ok(Json(serde_json::json!({ "ok": true, "id": id })))
}

async fn archive_session(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    mutate_session(&s, &id, crate::session_mutator::SessionAction::Archive).await
}
async fn restore_session(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    mutate_session(&s, &id, crate::session_mutator::SessionAction::Restore).await
}
async fn delete_session(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    mutate_session(&s, &id, crate::session_mutator::SessionAction::Delete).await
}
```

- [ ] **Step 6: Run, build both crates, format, clippy, commit**

Run: `cargo test -p rupu-cp --lib api::sessions::tests::archive_session session_mutator` then `cargo build -p rupu-cp -p rupu-cli`
Run: `rustfmt --edition 2021 crates/rupu-cp/src/session_mutator.rs crates/rupu-cp/src/api/sessions.rs && cargo clippy -p rupu-cp --all-targets 2>&1 | grep -E "session_mutator|api/sessions" || echo clean`

```bash
git add crates/rupu-cp/src/session_mutator.rs crates/rupu-cp/src/lib.rs crates/rupu-cp/src/state.rs crates/rupu-cp/src/api/sessions.rs crates/rupu-cli/src/cmd/cp.rs
git commit -m "feat(cp): SessionMutator port + session archive/restore/delete endpoints"
```

### Task 6: `SubprocessSessionMutator` adapter wired in `cp serve`

**Files:**
- Create: `crates/rupu-cli/src/cp_session_mutator.rs`
- Modify: `crates/rupu-cli/src/lib.rs` (mod decl), `crates/rupu-cli/src/cmd/cp.rs` (build + flip the `None` placeholder)

**Interfaces:**
- Consumes: `rupu_cp::session_mutator::{SessionMutator, SessionAction, SessionMutateError}`, the CLI's own `session archive|restore|delete` subcommands.
- Produces: `SubprocessSessionMutator { exe: PathBuf }`.

- [ ] **Step 1: Create the adapter with a unit test for argv + error parsing**

Create `crates/rupu-cli/src/cp_session_mutator.rs`. Shell `rupu session <action> <id> [--force]`; parse exit code + stderr into the error variants. Mirror `cp_session_starter.rs` style (pure argv/parse helpers + the async impl):

```rust
//! `cp serve` adapter for rupu-cp's SessionMutator port. Shells
//! `rupu session archive|restore|delete <id>` using this same binary.

use std::path::PathBuf;

use rupu_cp::session_mutator::{SessionAction, SessionMutateError, SessionMutator};

pub struct SubprocessSessionMutator {
    pub exe: PathBuf,
}

/// argv after the executable for a session mutation.
pub(crate) fn build_argv(id: &str, action: SessionAction) -> Vec<String> {
    let mut argv = vec!["session".to_string(), action.as_str().to_string(), id.to_string()];
    if action == SessionAction::Delete {
        argv.push("--force".to_string());
    }
    argv
}

/// Map a failed child's stderr to the right error variant. The CLI prints
/// `anyhow::bail!` messages to stderr: "is already archived"/"already active"/
/// "is running"/"requires --force" → Invalid; "not found"/"no session" → NotFound.
pub(crate) fn classify_failure(action: SessionAction, stderr: &str) -> SessionMutateError {
    let s = stderr.to_ascii_lowercase();
    if s.contains("not found") || s.contains("no session") || s.contains("unknown session") {
        SessionMutateError::NotFound(stderr.trim().to_string())
    } else if s.contains("already")
        || s.contains("running")
        || s.contains("requires --force")
        || s.contains("not archived")
    {
        SessionMutateError::Invalid(stderr.trim().to_string())
    } else {
        SessionMutateError::Failed {
            action: action.as_str(),
            message: if stderr.trim().is_empty() {
                "session command failed".into()
            } else {
                stderr.trim().to_string()
            },
        }
    }
}

#[async_trait::async_trait]
impl SessionMutator for SubprocessSessionMutator {
    async fn mutate(&self, id: &str, action: SessionAction) -> Result<(), SessionMutateError> {
        let argv = build_argv(id, action);
        let out = tokio::process::Command::new(&self.exe)
            .args(&argv)
            .output()
            .await
            .map_err(|e| SessionMutateError::Failed { action: action.as_str(), message: e.to_string() })?;
        if out.status.success() {
            return Ok(());
        }
        Err(classify_failure(action, &String::from_utf8_lossy(&out.stderr)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_includes_force_only_for_delete() {
        assert_eq!(build_argv("s1", SessionAction::Archive), vec!["session", "archive", "s1"]);
        assert_eq!(build_argv("s1", SessionAction::Restore), vec!["session", "restore", "s1"]);
        assert_eq!(build_argv("s1", SessionAction::Delete), vec!["session", "delete", "s1", "--force"]);
    }

    #[test]
    fn classify_maps_stderr_to_variants() {
        assert!(matches!(classify_failure(SessionAction::Archive, "session x not found"), SessionMutateError::NotFound(_)));
        assert!(matches!(classify_failure(SessionAction::Archive, "session x is already archived"), SessionMutateError::Invalid(_)));
        assert!(matches!(classify_failure(SessionAction::Archive, "session x is running"), SessionMutateError::Invalid(_)));
        assert!(matches!(classify_failure(SessionAction::Delete, "disk error"), SessionMutateError::Failed { .. }));
    }
}
```

Verify the exact CLI stderr strings against `crates/rupu-cli/src/cmd/session.rs` (`anyhow::bail!` messages: "is already archived", "is already active", "requires --force", and the not-found message from `read_session`). Adjust `classify_failure`'s substrings to match the real text.

- [ ] **Step 2: Declare the module + wire into `cp serve`**

In `crates/rupu-cli/src/lib.rs`: add `mod cp_session_mutator;` with the other `mod cp_*` decls.

In `crates/rupu-cli/src/cmd/cp.rs`: near where `session_starter` is built, add (reuse the same resolved `exe` — note `session_starter` currently moves `exe`; clone it for whichever is built first, or build the mutator before moving `exe` into the starter):

```rust
    let session_mutator: Option<Arc<dyn rupu_cp::session_mutator::SessionMutator>> =
        Some(Arc::new(crate::cp_session_mutator::SubprocessSessionMutator { exe: exe.clone() }));
```

and replace `session_mutator: None, // wired in Task 6` in the `ServeOpts { ... }` literal with `session_mutator,`. Ensure `exe` is cloned where needed so both `session_starter` and `session_mutator` get it.

- [ ] **Step 3: Test, build, format, clippy, commit**

Run: `cargo test -p rupu-cli --lib cp_session_mutator` then `cargo build -p rupu-cli`
Run: `rustfmt --edition 2021 crates/rupu-cli/src/cp_session_mutator.rs && cargo clippy -p rupu-cli --all-targets 2>&1 | grep cp_session_mutator || echo clean`
Run: `git status --short` (expect only the 3 files)

```bash
git add crates/rupu-cli/src/cp_session_mutator.rs crates/rupu-cli/src/lib.rs crates/rupu-cli/src/cmd/cp.rs
git commit -m "feat(cli): wire SubprocessSessionMutator into cp serve"
```

### Task 7: Sessions frontend (API + row actions + detail buttons)

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/Sessions.tsx`, `crates/rupu-cp/web/src/pages/SessionDetail.tsx`
- Test: `crates/rupu-cp/web/src/pages/Sessions.archive.test.tsx` (new)

**Interfaces:**
- Consumes: `request<T>`, `getSessions({ scope })`, the active/archived tab state in `Sessions.tsx`, the `AgentDetail` confirm idiom.
- Produces: `api.archiveSession/restoreSession/deleteSession`; row actions + detail buttons.

- [ ] **Step 1: Add API client methods**

In `api.ts`, `// --- Sessions ---` section:

```typescript
  async archiveSession(id: string): Promise<void> {
    await request(`/api/sessions/${encodeURIComponent(id)}/archive`, { method: 'POST' });
  },
  async restoreSession(id: string): Promise<void> {
    await request(`/api/sessions/${encodeURIComponent(id)}/restore`, { method: 'POST' });
  },
  async deleteSession(id: string): Promise<void> {
    await request(`/api/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' });
  },
```

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-cp/web/src/pages/Sessions.archive.test.tsx` (mirror the existing Sessions test harness for render + how it mocks `api.getSessions` and which tab is active):

```tsx
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Sessions from './Sessions';
import { api } from '../lib/api';

afterEach(() => vi.restoreAllMocks());

describe('Sessions row archive/delete', () => {
  it('archives an active session from the row action', async () => {
    vi.spyOn(api, 'getSessions').mockResolvedValue([
      /* one active SessionSummary fixture — match the real shape */ { session_id: 's1' } as never,
    ]);
    const archive = vi.spyOn(api, 'archiveSession').mockResolvedValue();
    render(<MemoryRouter><Sessions /></MemoryRouter>);
    fireEvent.click(await screen.findByRole('button', { name: /archive/i }));
    await waitFor(() => expect(archive).toHaveBeenCalledWith('s1'));
  });
});
```

Fill the `SessionSummary` fixture from the sibling Sessions test. Adjust the active-tab assumption to the page's default tab.

- [ ] **Step 3: Add row actions in Sessions.tsx**

In the `SESSION_COLUMNS` action cell, render buttons by tab: active tab rows → **Archive** + **Delete**; archived tab rows → **Restore** + **Delete**. Each calls the api method, then refreshes the list (reuse the page's existing refetch/poll trigger). Delete wraps in `window.confirm`. Mirror the existing action-cell rendering and the `AgentDetail` confirm. Keep the buttons small (icon + label) to fit the row.

- [ ] **Step 4: Add buttons to SessionDetail.tsx**

In the `SessionDetail` header, add Archive/Restore (by current scope) + Delete (confirm), mirroring the RunDetail button cluster from Task 4 (state `actionError`/`actionPending`, `navigate('/sessions')` on success).

- [ ] **Step 5: Test, build, full suite, commit**

Run (from `crates/rupu-cp/web`): `npx vitest run src/pages/Sessions.archive.test.tsx && npm run build && npx vitest run`
Expected: green.

```bash
git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/pages/Sessions.tsx crates/rupu-cp/web/src/pages/SessionDetail.tsx crates/rupu-cp/web/src/pages/Sessions.archive.test.tsx
git commit -m "feat(cp/web): archive/restore/delete sessions (list rows + detail)"
```

---

## Final verification (after all tasks)

- [ ] **Backend**: `cargo test -p rupu-orchestrator runs:: && cargo test -p rupu-cp --lib api::runs api::sessions session_mutator && cargo build -p rupu-cp -p rupu-cli` → green.
- [ ] **Clippy (touched crates)**: `cargo clippy -p rupu-orchestrator -p rupu-cp --all-targets` → no new warnings (ignore the pre-existing rupu-cli `autoflow.rs` baseline).
- [ ] **Frontend**: from `crates/rupu-cp/web`, `npx vitest run && npm run build` → green.
- [ ] **Rebuild embedded UI before any release**: `make cp-web`.
- [ ] **Manual smoke (matt, recommended)**: `rupu cp serve`; on a terminal run use RunDetail Archive → it leaves the active list and appears under the Archived toggle → Restore/Delete; archive/restore/delete a session from the Sessions tabs. Confirm bare `rupu-cp` (no `cp serve`) surfaces 501 gracefully for session actions.

## Self-review notes

- Spec §3 (runs lib) → Task 1; §3.2 (CP endpoints) → Task 2; §3.3 (CLI) → Task 3; §4 (sessions adapter + endpoints) → Tasks 5/6; §5 (UI) → Tasks 4/7; §6 (guards/error map) → Tasks 2/5 mappers; §7 (testing) → each task's tests. All spec sections covered.
- `RunStore::delete` is unguarded at the library layer (by design — Task 1 note); the terminal guard for delete lives in the CP handler (Task 2 `delete_run`) and the CLI `--force` gate (Task 3). The library stays a mechanism; policy lives at the edges.
- Transcripts: runs carry transcripts inside the run dir (verified) so the dir rename/remove suffices; sessions' owned-transcript handling is the CLI's job, reached via the adapter.
