# rupu — Web Approve / Reject for workflow approval gates — Design

**Date:** 2026-06-25
**Surfaces:** `rupu-orchestrator` (state marker), `rupu-cp` (record endpoints), `rupu-cli` (`cp serve` resume worker + extract resume), `rupu-cp/web` (buttons)
**Status:** approved direction (matt), pending spec review

## Goal
Let an operator **approve or reject** a workflow run's "awaiting approval" gate **from the web Control Plane**, and have an approved run actually **resume and finish** — without turning the read-only `rupu-cp` adapter into a workflow executor.

## The problem (from investigation)
- A run paused at a gate is persisted to disk as `RunStatus::AwaitingApproval`; the original `rupu run` process has **exited**.
- `RunStore::approve` only flips state (`AwaitingApproval → Running`, clears pause fields). The actual **resume** (re-entering `run_workflow` to execute the remaining steps) is done **only** by `rupu-cli`'s `workflow approve` handler, inline, using the full runtime (providers, agent dispatcher, keychain, SCM).
- **Nothing** watches for approved-but-unfinished runs to resume them. `rupu-cp` deliberately lacks the execution runtime.

## Architecture: record in CP, resume in the `cp serve` process
`rupu cp serve` is a **rupu-cli** command (it already links the full runtime) that calls `rupu_cp::serve(...)`. We host the resumer there:
- **`rupu-cp` (crate) stays a read adapter.** Its new write endpoints only **record the decision** via the `RunStore` it already holds (`AppState.run_store`). No execution deps added.
- **`rupu cp serve` (rupu-cli) spawns a background resume worker** alongside the axum server. The worker polls the run store for approved-pending-resume runs, claims each, and re-enters `run_workflow` (reusing the CLI's existing resume code). The operator runs the same `rupu cp serve` they already do — Approve works end-to-end; nothing extra to launch.

Reject needs no resumer (terminal), so it works the moment the endpoint + button land.

## Components

### 1. `rupu-orchestrator` — pending-resume marker + claim
File: `crates/rupu-orchestrator/src/runs.rs`.
- Add optional fields to `RunRecord`: `resume_requested_at: Option<DateTime<Utc>>` (set when a gate is approved via a path that delegates resume) and a claim lease `resume_claimed_at: Option<DateTime<Utc>>` (+ a `resume_claimed_by: Option<String>` worker id). All `#[serde(default)]` so older `run.json` parse.
- New methods:
  - `request_resume_approval(run_id, approver, now) -> Result<ApprovalDecision>` — same state flip as `approve` (→ `Running`, clear pause fields, record approver) **plus** sets `resume_requested_at = Some(now)`. Used by the CP path.
  - `list_pending_resume() -> Vec<RunRecord>` — runs with `resume_requested_at.is_some()` and no live claim (claim absent or older than a lease TTL, e.g. 5 min → reclaimable after a crash).
  - `claim_resume(run_id, worker_id, now) -> Result<bool>` — atomically set the claim lease; returns `false` if already claimed (lease live). Prevents double-resume.
  - `clear_resume(run_id) -> Result<()>` — clear `resume_requested_at` + claim (called after a resume run completes, re-pauses at the next gate, or fails).
- The existing `approve` (no marker) stays for the CLI **inline** path (`rupu workflow approve` resumes in-process, so it must NOT also be picked up by the worker). `reject` is unchanged (terminal).

### 2. `rupu-cp` — first write endpoints (record only)
File: `crates/rupu-cp/src/api/runs.rs` (+ route registration). The first POST routes in rupu-cp.
- `POST /api/runs/:id/approve` → `state.run_store.request_resume_approval(id, "web", now)`. 404 if run not found; 409 if not currently awaiting (status mismatch → the store method already validates); 200 with the updated run summary (so the UI reflects `Running` + "resuming"). No execution.
- `POST /api/runs/:id/reject` (JSON body `{ "reason": string }`) → `state.run_store.reject(id, reason, now)`. 200 with updated run. Terminal.
- Both are guarded by the same optional bearer-token auth as the rest of `/api/*`. Unit tests via `RunStore` over a tempdir (approve sets the marker + Running; reject → Rejected; not-awaiting → error).

### 3. `rupu-cli` — extract resume + `cp serve` worker
Files: `crates/rupu-cli/src/cmd/workflow.rs`, a new `crates/rupu-cli/src/resume.rs` (or `cmd/cp.rs`).
- **Extract** the resume body from `workflow.rs::approve` (the part that reloads the run, rebuilds `KeychainResolver`/config/`rupu_scm::Registry`/`CliAgentDispatcher`/`DefaultStepFactory`, builds `ResumeState::from_approval`, and calls `run_workflow`) into a reusable `resume::resume_run(store, run_id) -> Result<ResumeOutcome>`. `workflow approve` calls `store.approve(...)` then `resume::resume_run(...)` (unchanged behavior). The worker calls `resume::resume_run(...)` after claiming.
- **Worker:** in `cmd/cp.rs` `Action::Serve`, before/around `rupu_cp::serve(...).await`, `tokio::spawn` a resume loop: every N seconds, `list_pending_resume()` → for each, `claim_resume(id, worker_id, now)`; on success, `resume::resume_run(store, id)`, then `clear_resume(id)` (resume_run itself persists the run's terminal/re-paused state; clear just drops the marker/claim). Log each resume. Graceful shutdown when the server stops (`tokio::select!` on a shutdown signal / the serve future). Backoff on errors; a failed resume clears the marker and leaves the run `Failed` (no infinite retry of a poisoned run — or a bounded retry count; pick bounded retry, then mark failed with a clear message).
- `rupu_cp::serve` may need to return a handle or accept a shutdown channel so the worker can be torn down cleanly; if simpler, run both under `tokio::select!` in `cp.rs`.

### 4. `rupu-cp/web` — the buttons
Files: `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/RunDetail.tsx`.
- `api.ts`: `approveRun(id): Promise<void>` and `rejectRun(id, reason): Promise<void>` using the existing `request` wrapper with `{ method: 'POST', body }` (it already handles 204 + sets JSON content-type).
- `RunDetail.tsx`: in the awaiting-approval banner (currently view-only with the "controls arrive in a later phase" line), render **Approve** and **Reject** buttons. Reject opens a small reason prompt/inline input. On **Approve** → call `approveRun`, then show "Approved — resuming…" (the run flips to `Running`; the live event stream / poll will reflect progress as the worker resumes it). On **Reject** → call `rejectRun`, the run shows `Rejected`. Optimistic UI + error toast on failure. Disable buttons while the request is in flight.

## Data flow
```
[web] Approve  → POST /api/runs/:id/approve → run_store.request_resume_approval (→ Running + resume_requested_at)
                                                       │
              ┌────────────────────────────────────────┘
   [rupu cp serve background worker]  list_pending_resume → claim_resume → resume::resume_run (run_workflow) → clear_resume
                                                       │
                                            run finishes (or re-pauses at next gate / fails)

[web] Reject   → POST /api/runs/:id/reject  → run_store.reject (→ Rejected)   [terminal, no worker]
```

## Phasing (for the plan)
The spec is one feature; the plan ships it in independently-valuable slices:
1. **Reject end-to-end** — orchestrator (reject already exists) + rupu-cp `POST /reject` + web Reject button. Immediate value, no worker.
2. **Approve state model** — `request_resume_approval` / `list_pending_resume` / `claim_resume` / `clear_resume` + fields.
3. **Resume extraction** — factor `resume::resume_run` out of `workflow approve` (no behavior change to the CLI).
4. **`cp serve` worker** — the background resume loop wired into `cp serve`.
5. **Approve button** — web `POST /approve` endpoint + the button (works once 2–4 land).

## Error / edge handling
- Approve a run that's no longer awaiting (already approved/rejected/finished) → the store method errors; endpoint returns 409 with a clear message; the UI refreshes.
- Worker not running (operator used a bare `rupu cp serve` build without the worker, or it crashed) → approved runs sit with `resume_requested_at` set until a worker runs; the claim lease TTL lets a restarted worker reclaim. The UI's "resuming…" should not imply instantaneous completion.
- A resume that hits another gate re-pauses (status `AwaitingApproval` again) — the worker clears the prior marker; the operator approves the next gate normally.
- Double-resume prevented by the claim lease; a crashed worker's stale claim is reclaimable after the TTL.
- Reject is idempotent-ish: rejecting an already-terminal run → 409.

## Testing
- `rupu-orchestrator`: `request_resume_approval` sets Running + marker; `list_pending_resume` excludes claimed; `claim_resume` is exclusive; `clear_resume` drops marker; reject terminal. (Clean on the worktree's 1.95.)
- `rupu-cp`: endpoint handlers (approve records marker + Running; reject → Rejected; not-awaiting → error). `cargo test -p rupu-cp` + clippy clean on 1.95.
- `rupu-cli`: the resume extraction keeps `workflow approve` behavior; the worker loop claims+resumes+clears. **Toolchain caveat:** rupu-cli has a pre-existing red baseline on the worktree's Homebrew 1.95 — verify rupu-cli changes compile, rely on CI (1.88) for its full test suite; gate locally on rupu-orchestrator/rupu-cp/web.
- Web: `approveRun`/`rejectRun` client; the banner renders buttons and calls them; `npm test`/`build` green; no `any`; static Tailwind.
- Visual validation by matt: approve a gated run from the browser → it resumes and finishes (with `rupu cp serve` running); reject → it shows Rejected.

## Non-goals
- No separate standalone resume daemon (the worker lives in `cp serve`).
- No eager timeout ticker (existing lazy expiry stands).
- No change to the CLI `workflow approve` inline behavior beyond the no-op refactor.
- No multi-operator approval / RBAC (single "web" approver attribution for now).
