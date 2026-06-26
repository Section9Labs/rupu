# rupu-cp — Phase 2a: Run lifecycle control (cancel + approve-with-mode) — Design

**Date:** 2026-06-26
**Surfaces:** `rupu-orchestrator` (extract cancel, add resume_mode), `rupu-cp` (endpoints), `rupu-cli` (CLI reuses lib + worker passes mode), `rupu-cp/web` (buttons)
**Status:** pending matt's spec review
**Part of:** Phase 2 (Control) of the Control Plane. Builds on the shipped approve/reject + the `cp serve` resume worker (`project_rupu_cp_write_path`).

## Goal
Two run-lifecycle control actions from the web CP:
1. **Cancel a run** — stop an in-flight / pending / awaiting run from the browser.
2. **Approve with mode** — let the web Approve choose the permission mode (Ask / Bypass / Read-only) the resumed steps run under, instead of always defaulting to `ask`.

These complete the design's "`runs.rs`: approve / reject / **cancel**" trio and make the headless resume mode-aware.

## ① Cancel a run

### Engine — extract `cancel_run` into the orchestrator
Today the cancel logic is the CLI-private `cancel_with_store` (`crates/rupu-cli/src/cmd/workflow.rs:2298`). Lift it (and the `pid_is_running` / `terminate_pid` helpers) into `crates/rupu-orchestrator/src/runs.rs` as a reusable function so BOTH the CLI and the CP call one implementation (honoring "no business logic reimplemented"):

```rust
pub enum CancelOutcome { RejectedAwaitingApproval, MarkedCancelled { pid: Option<u32>, was_running: bool } }

impl RunStore {
    pub fn cancel(&self, run_id: &str, reason: &str, now: DateTime<Utc>) -> Result<CancelOutcome, CancelError>;
}
```
Behavior (identical to today's `cancel_with_store`):
- **Terminal** (`Completed`/`Failed`/`Rejected`) → `CancelError::AlreadyTerminal(status)`.
- **AwaitingApproval** → `self.reject(run_id, approver, reason, now)` → `RejectedAwaitingApproval`.
- **Pending / Running** → if `runner_pid` is live, `terminate_pid` (host-local SIGTERM); flip `status = Failed`, set `finished_at`/`error_message = reason`, clear pause + active-step + `runner_pid` fields → `MarkedCancelled`.

`cmd/workflow.rs::cancel_with_store` becomes a thin call to `RunStore::cancel` (CLI output/behavior unchanged). The CLI's `whoami::username()` approver for the awaiting case moves to the caller (the CP passes `"web"`); `cancel` takes the approver via the reject path — to keep `cancel`'s signature lean, it derives the awaiting-approver from a param: `cancel(&self, run_id, approver: &str, reason, now)`.

**Terminal status decision:** cancel reuses `RunStatus::Failed` with `error_message = reason` (matches the CLI today — no new enum variant, no ripple). The CP defaults the reason to `"Cancelled from control plane"` so the run list/detail shows that text. *(A dedicated `RunStatus::Cancelled` would read nicer than "Failed" but ripples through every status match + the frontend status union + the CLI's cancel display; deferred as an easy follow-up. Flagged for matt — say the word and I'll do Cancelled instead.)*

**Host-locality note:** the pid SIGTERM only works because `cp serve` runs on the same host as the runs it manages (single-host CP, Phase 1/2 scope). A run started by `rupu run` that already exited at a gate has no live pid → pure state flip. Cross-host cancel is a Phase 4 concern.

### CP — `POST /api/runs/:id/cancel`
`crates/rupu-cp/src/api/runs.rs`. Body `{ reason?: string }` (default `"Cancelled from control plane"`). Calls `s.run_store.cancel(&id, "web", &reason, now)`. Maps `AlreadyTerminal` → 409, not-found → 404, else 500. Returns the updated run envelope (same shape as approve/reject). **Synchronous, pure-ish** — no worker needed (cancel terminates, it doesn't execute).

### Frontend — Cancel button
`crates/rupu-cp/web`: `cancelRun(id, reason?)` in `api.ts`. A **Cancel** button on the run detail page for **non-terminal** runs (`Running` / `Pending` / `AwaitingApproval`) — placed in the header near the status (and reusing the awaiting banner for the AwaitingApproval case, where Cancel sits alongside Approve/Reject). Confirm prompt ("Cancel this run?"). On success the run flips to Failed; optimistic + live-stream update. Disable while in-flight; inline error on failure.

## ② Approve with mode

### Engine — carry the mode through the resume marker
The worker currently resumes with `mode: None` (= ask) — wrong for several headless cases and not operator-controllable. Add a mode to the marker:
- New `RunRecord` field `#[serde(default)] resume_mode: Option<String>` (alongside the existing `resume_requested_at` / `resume_claimed_*`).
- `request_resume_approval(&self, run_id, approver, mode: Option<&str>, now)` stores `resume_mode = mode.map(str::to_string)` (validated against `ask` / `bypass` / `readonly`; unknown → treated as `None`/ask).
- The `cp serve` worker (`cmd/cp.rs`) reads `run.resume_mode.as_deref()` and passes it to `resume::resume_run(store, run_id, step_id, mode)` (which already accepts `mode: Option<&str>`).
- `clear_resume` nulls `resume_mode` too.

Adding the field means re-touching the ~16 `RunRecord` struct literals across cli + cp tests (mechanical `resume_mode: None`, same as the prior resume-field add).

### CP — `POST /api/runs/:id/approve` gains an optional mode
Body `{ mode?: "ask" | "bypass" | "readonly" }` (default `ask`). `request_resume_approval(&id, "web", mode.as_deref(), now)`. Same response. Reject unchanged.

### Frontend — mode picker on Approve
The Approve control gains a small **mode picker** (Ask / Bypass / Read-only, default Ask) — a segmented control or dropdown next to the Approve button in the awaiting banner. `approveRun(id, mode)` sends `{ mode }`. After approve, the existing "Approved — resuming…" state shows (now with the chosen mode driving the worker's `resume_run`).

## Files
**Backend:**
- `crates/rupu-orchestrator/src/runs.rs` — `RunStore::cancel` + `CancelOutcome`/`CancelError` + pid helpers (lifted); `resume_mode` field; `request_resume_approval` takes `mode`; `clear_resume` nulls it; tests.
- `crates/rupu-cli/src/cmd/workflow.rs` — `cancel_with_store` → thin call to `RunStore::cancel` (behavior unchanged).
- `crates/rupu-cli/src/cmd/cp.rs` — worker passes `run.resume_mode` to `resume_run`.
- `crates/rupu-cp/src/api/runs.rs` — `POST /cancel`; `POST /approve` accepts `{mode}`.
- Literal fixup: `resume_mode: None` across the RunRecord literals (cli + cp tests).

**Frontend:**
- `crates/rupu-cp/web/src/lib/api.ts` — `cancelRun`, `approveRun(id, mode)`, `RunRecord.resume_mode?`.
- `crates/rupu-cp/web/src/pages/RunDetail.tsx` — Cancel button + Approve mode picker.

## Testing
- `rupu-orchestrator`: `RunStore::cancel` — terminal→error, awaiting→rejected, running→failed+pid-cleared+fields-cleared; `request_resume_approval` stores `resume_mode`; `clear_resume` nulls it. clippy clean.
- `rupu-cp`: `POST /cancel` (running→failed; terminal→409; missing→404); `POST /approve` with mode stores `resume_mode`.
- `rupu-cli`: compiles; `cancel_with_store` still behaves (its existing test passes via the lib call); worker passes mode (CI on 1.88 for full cli tests).
- Web: `cancelRun`/`approveRun(mode)` client; Cancel button + mode picker render + call the right endpoints; suite green; build strict; no `any`.
- Visual validation by matt: cancel a running run from the browser → it stops (Failed); approve a gate with Bypass → it resumes in bypass.

## Non-goals
- No dedicated `RunStatus::Cancelled` (reuse Failed; flagged as a follow-up).
- No run delete/archive (no engine support; Phase 2e/3).
- No cross-host cancel (Phase 4).
