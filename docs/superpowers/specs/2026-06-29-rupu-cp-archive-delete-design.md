# CP archive / delete for runs & sessions — design

- **Status:** Approved (2026-06-29)
- **Author:** rupu agent (paired with matt)
- **Scope:** Expose archive / restore / delete actions for **runs** and **sessions**
  in the CP web control panel. Transcripts are handled implicitly (they are owned
  by runs/sessions). Runs gain net-new archive/delete logic; sessions reuse the
  existing CLI logic via a subprocess adapter.

## 1. Motivation

The CLI already manages session and transcript lifecycle
(`rupu session archive|restore|delete|prune`, `rupu transcript archive|delete|prune`),
but none of it is reachable from CP. Runs have only `cancel` — no archive/delete
anywhere. matt wants these actions usable as buttons in CP for runs and sessions.

Findings that shape the design:
- **Sessions**: archive moves `<global>/sessions/<id>/` → `<global>/sessions-archive/<id>/`
  (soft, reversible); delete is `remove_dir_all` (hard). The logic lives **in the
  CLI** (`crates/rupu-cli/src/cmd/session.rs`), not a library crate. CP's
  `Sessions.tsx` already has active/archived tabs but no action buttons.
- **Runs**: `RunStore` (`crates/rupu-orchestrator/src/runs.rs`) has `cancel` but no
  `archive`/`delete`. Runs live at `<global>/runs/<id>/` (run.json + step results +
  events + transcript). `AppState` already holds `run_store: Arc<RunStore>`.
- **Transcripts**: owned by runs/sessions — archiving/deleting the owner carries the
  transcript. No standalone CP surface (per design decision).
- **Precedent**: agent/workflow definition DELETE already happens directly in
  `rupu-cp` via `std::fs`; session start/send already go through subprocess
  adapters (`SessionStarter`, `SessionSender`) wired only under `rupu cp serve`.

## 2. Goals / non-goals

**Goals**
- Net-new run lifecycle: `RunStore::archive/restore/delete` + CP endpoints (direct)
  + CLI parity + UI buttons + an "Archived" run filter.
- Session lifecycle in CP: a `SessionMutator` subprocess adapter (reuses CLI logic)
  + CP endpoints + UI buttons on the existing active/archived tabs and detail.
- One shared destructive-action UX: archive is reversible (no confirm); delete is
  hard and **always** confirms.

**Non-goals**
- No standalone transcript management surface in CP (transcripts ride with their
  owner).
- No `prune` (bulk time-based deletion) in CP for this slice — single-item actions
  only. (CLI `prune` stays CLI-only.)
- No change to how runs/sessions are created or executed.
- No new bulk/multi-select delete.

## 3. Runs — net-new lifecycle

### 3.1 Library (`rupu-orchestrator`, `runs.rs`)

Add to `RunStore` (mirrors the session soft/hard model):

```rust
impl RunStore {
    /// Move `runs/<id>` → `runs-archive/<id>` (reversible). Errors if the run
    /// is missing, already archived, or not in a terminal state (running /
    /// awaiting_approval cannot be archived — cancel first).
    pub fn archive(&self, run_id: &str) -> Result<(), RunStoreError>;

    /// Move `runs-archive/<id>` → `runs/<id>`.
    pub fn restore(&self, run_id: &str) -> Result<(), RunStoreError>;

    /// Permanently remove the run directory (`remove_dir_all`). Looks in both
    /// `runs/` and `runs-archive/`. Hard / irreversible.
    pub fn delete(&self, run_id: &str) -> Result<(), RunStoreError>;

    /// List archived runs (reads `runs-archive/`), newest first — same row
    /// shape as `list()`.
    pub fn list_archived(&self) -> Result<Vec<RunRecord>, RunStoreError>;
}
```

- `list()` is unchanged (reads `runs/` only), so archived runs disappear from the
  default views automatically.
- The run directory contains the transcript artifacts, so the dir move/remove
  carries them. If a run also owns a transcript in a separate transcripts dir
  (the standalone-transcript path), `delete` removes it and `archive` moves it,
  matching the session "owned transcripts" behavior. The plan verifies the exact
  on-disk layout and covers both.
- A new `RunStoreError` variant (or reuse existing) distinguishes `NotFound`,
  `AlreadyArchived`/`NotArchived`, and `NotTerminal` so callers map to the right
  HTTP status.

### 3.2 CP endpoints (direct via `AppState.run_store`)

In `crates/rupu-cp/src/api/runs.rs`, add to `routes()`:

```rust
.route("/api/runs/:id/archive", post(archive_run))
.route("/api/runs/:id/restore", post(restore_run))
.route("/api/runs/:id", delete(delete_run))   // add DELETE to the existing :id route
.route("/api/runs/archived", get(list_archived_runs))
```

Handlers call `s.run_store.archive/restore/delete/list_archived` directly (no
adapter — `run_store` is always present in `AppState`). Error mapping:
`NotFound → 404`, `NotTerminal → 409`, `AlreadyArchived/NotArchived → 409`,
other → 500.

### 3.3 CLI parity

Add run-management subcommands under `rupu workflow` (where run management already
lives — `runs`, `show-run`, `cancel`, `approve`, `reject`, `resume`):

```
rupu workflow archive-run <run_id>
rupu workflow restore-run <run_id>
rupu workflow delete-run  <run_id> --force
```

These call the same `RunStore` methods. `--force` required for delete (mirrors
`session delete --force`).

## 4. Sessions — expose existing via adapter

### 4.1 `SessionMutator` port (rupu-cp)

Session archive/restore/delete logic lives in the CLI, so CP reaches it through a
new optional adapter, mirroring `SessionStarter`:

```rust
// crates/rupu-cp/src/session_mutator.rs
#[derive(Debug, Clone, Copy)]
pub enum SessionAction { Archive, Restore, Delete }

#[derive(Debug, thiserror::Error)]
pub enum SessionMutateError {
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("invalid session state: {0}")]
    Invalid(String),
    #[error("failed to {action}: {message}")]
    Failed { action: &'static str, message: String },
}

#[async_trait::async_trait]
pub trait SessionMutator: Send + Sync {
    async fn mutate(&self, id: &str, action: SessionAction)
        -> Result<(), SessionMutateError>;
}
```

Stored on `AppState` as `Option<Arc<dyn SessionMutator>>`; `ServeOpts.session_mutator`
threaded like `session_starter`. The concrete adapter
(`crates/rupu-cli/src/cp_session_mutator.rs`, `SubprocessSessionMutator { exe }`) is
wired in `cmd/cp.rs` and shells `rupu session archive|restore|delete <id> [--force]`,
parsing the child's exit/stderr into the error variants. `None` (bare `rupu-cp`) →
endpoints return **501**, exactly like `start_session`.

### 4.2 CP endpoints

In `crates/rupu-cp/src/api/sessions.rs`, add to `routes()`:

```rust
.route("/api/sessions/:id/archive", post(archive_session))
.route("/api/sessions/:id/restore", post(restore_session))
.route("/api/sessions/:id", delete(delete_session))
```

Each clones `s.session_mutator` or 501s, calls `mutate`, maps
`NotFound → 404`, `Invalid → 409`, `Failed → 500`. The session list already
supports a `scope=active|archived` query, so archived sessions are already
viewable; restore/delete operate on the archived scope.

## 5. UI — shared destructive-action pattern

Reuse the existing `AgentDetail` delete idiom: a `danger-outline` `Button` with a
`Trash2` icon; **delete** wraps in `window.confirm`. Archive/Restore are reversible
→ no confirm. Errors render inline (toast/alert) and the list refreshes on success.

New API client methods in `web/src/lib/api.ts`: `archiveRun`, `restoreRun`,
`deleteRun`, `getArchivedRuns`, `archiveSession`, `restoreSession`, `deleteSession`.

- **Sessions** (`Sessions.tsx`, has active/archived tabs): row actions in the
  existing action column — **active** rows → Archive + Delete; **archived** rows →
  Restore + Delete. Same buttons in `SessionDetail.tsx` header.
- **Runs** (`AgentRuns.tsx` / `WorkflowRuns.tsx` / `AutoflowRuns.tsx` + `RunDetail.tsx`):
  row actions Archive + Delete; an **Archived** filter/toggle on the run lists
  (calls `getArchivedRuns`) where archived rows show Restore + Delete. `RunDetail`
  header gets Archive/Restore + Delete next to the existing Cancel.

## 6. Error handling & guards

| Condition | Backend | UI |
|---|---|---|
| Delete (run or session) | hard `remove_dir_all` | `window.confirm` before the call |
| Archive a running/awaiting run | `409 NotTerminal` | inline "cancel the run first" |
| Archive an active session | `409 Invalid` (CLI rejects) | inline message |
| Unknown id | `404` | inline "not found", refresh |
| Session adapter absent (bare `rupu-cp`) | `501` | actions hidden/disabled when models indicate unavailable, else inline 501 |
| Restore a non-archived item | `409` | inline message |

## 7. Testing

- **Library (`rupu-orchestrator`)**: `RunStore::archive` moves the dir and drops it
  from `list()` / adds to `list_archived()`; `restore` reverses; `delete` removes
  from both scopes; archive of a non-terminal run errors `NotTerminal`; transcript
  artifacts travel with the run. Tempdir-based, deterministic.
- **CLI**: `rupu workflow archive-run/restore-run/delete-run` happy paths + `--force`
  guard, against a tempdir run store.
- **CP backend**: run handlers against a tempdir `run_store` (archive→list flips,
  delete 404s afterward, archive-running→409); session handlers with a stub
  `SessionMutator` (success + each error variant) and **501 when the adapter is
  `None`** (mirror the `start_session` 501 test).
- **Frontend (vitest)**: the Archive/Delete/Restore buttons call the right API
  methods; delete triggers `window.confirm` and aborts on cancel; the Archived
  filter fetches archived rows; success refreshes the list.

## 8. Build / release

React changes flow through `make cp-web` → `web/dist` → rust-embed; remember
`make cp-web` before any release that ships this UI. Otherwise ordinary Rust.

## 9. Implementation slices (for the plan)

1. **Runs library** — `RunStore::archive/restore/delete/list_archived` + error
   variants + unit tests. No surface yet.
2. **Runs surface** — CP endpoints (direct), CLI `workflow {archive,restore,delete}-run`,
   run-list/detail UI + Archived filter, handler + vitest tests.
3. **Sessions surface** — `SessionMutator` port + `SubprocessSessionMutator` adapter
   wired in `cp serve`, CP endpoints (501 when absent), Sessions/detail UI, handler
   + vitest tests.

Slice 1 is a prerequisite for slice 2; slice 3 is independent of 1–2.
