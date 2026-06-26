# rupu-cp Phase 2a — Run lifecycle control — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Cancel a run from the web (new `RunStatus::Cancelled`) + approve-with-mode (Ask/Bypass/Read-only), reusing one engine impl across CLI and CP.

**Spec:** `docs/superpowers/specs/2026-06-26-rupu-cp-phase2a-run-lifecycle-design.md`

**Constraints (every task):** no `any` in TS; static Tailwind; recharts out of main chunk; stage only specific changed files (`git add <paths>`, never `-A`, never `.rupu/*`); never package-wide `cargo fmt`. **Toolchain:** worktree Rust 1.95 — `rupu-orchestrator`/`rupu-cp`/web are clean gates; `rupu-cli` has a pre-existing red TEST baseline (verify it *compiles*; CI on 1.88 is authoritative for its tests).

---

### Task 1: Orchestrator — `Cancelled` status, `RunStore::cancel`, `resume_mode`

**Files:** Modify `crates/rupu-orchestrator/src/runs.rs`.

**Context:** `RunStatus` (line 50): `Pending, Running, Completed, Failed, AwaitingApproval, Rejected`. `is_terminal()`/`as_str()` exist. `RunRecord` has `runner_pid`, `active_step_*`, pause fields, and the `resume_*` fields. The cancel logic to lift is `cmd/workflow.rs::cancel_with_store` (lines 2298–2346) + `pid_is_running`/`terminate_pid` (2348–2364).

- [ ] **Step 1: Add `RunStatus::Cancelled`.** New enum variant; add to `as_str()` (`"cancelled"`); include in `is_terminal()` (terminal). Fix any exhaustive `RunStatus` matches *within rupu-orchestrator* that the new variant breaks (compiler-driven: `cargo build -p rupu-orchestrator`).
- [ ] **Step 2: Add `resume_mode` field.** `#[serde(default, skip_serializing_if = "Option::is_none")] resume_mode: Option<String>` on `RunRecord` (next to `resume_requested_at`). Fix the orchestrator's own `RunRecord` literals (`runner.rs`, `executor/in_process.rs`, tests) with `resume_mode: None`.
- [ ] **Step 3: Write failing tests** for: `RunStore::cancel` on a Running run (→ `Cancelled`, `finished_at`/`error_message` set, `runner_pid`/active/pause fields cleared, returns `MarkedCancelled`); on an AwaitingApproval run (→ `Rejected` via reject, returns `RejectedAwaitingApproval`); on a terminal run (→ `CancelError::AlreadyTerminal`). `request_resume_approval` with `mode: Some("bypass")` stores `resume_mode = Some("bypass")` and still leaves status `AwaitingApproval` + marker; `clear_resume` nulls `resume_mode`.
- [ ] **Step 4: Run `cargo test -p rupu-orchestrator runs`, confirm failure.**
- [ ] **Step 5: Implement.**
  - `pub enum CancelOutcome { RejectedAwaitingApproval, MarkedCancelled { pid: Option<u32>, was_running: bool } }` + a `CancelError` (`AlreadyTerminal(RunStatus)`, `Store(...)`, `NotFound`).
  - `pub fn cancel(&self, run_id, approver: &str, reason: &str, now) -> Result<CancelOutcome, CancelError>` — port `cancel_with_store` verbatim EXCEPT: the Pending/Running branch sets `status = RunStatus::Cancelled` (not `Failed`); the AwaitingApproval branch calls `self.reject(run_id, approver, reason, now)`. Move `pid_is_running`/`terminate_pid` into this module (private).
  - `request_resume_approval(&self, run_id, approver, mode: Option<&str>, now)` — validate `mode` is one of `ask`/`bypass`/`readonly` (else treat as `None`); set `resume_mode`. `clear_resume` also nulls `resume_mode`.
- [ ] **Step 6: `cargo test -p rupu-orchestrator` + `cargo clippy -p rupu-orchestrator --all-targets` green/clean.**
- [ ] **Step 7: Commit.** `git add crates/rupu-orchestrator/src/runs.rs crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/src/executor/in_process.rs` → `feat(orchestrator): RunStatus::Cancelled, RunStore::cancel, resume_mode marker`.

---

### Task 2: Workspace fixup — new variant + field across cli/cp

**Files:** Modify the exhaustive `RunStatus` match sites + `RunRecord` literals across `crates/rupu-cli/**` and `crates/rupu-cp/**` (compiler-driven).

**Context:** `RunStatus::Cancelled` breaks every exhaustive match on `RunStatus` outside orchestrator; `resume_mode` breaks every `RunRecord { … }` literal. Use the compiler to find them all.

- [ ] **Step 1: Find + fix.** `cargo build --workspace 2>&1 | grep -E "non-exhaustive|missing field resume_mode|match arms"` → for each: add a `RunStatus::Cancelled =>` arm (sensible behavior — treat like a terminal/failed-ish state for printers/status mapping; e.g. in the CLI workflow printer + status-string mappers, render "cancelled"), and add `resume_mode: None` to each `RunRecord` literal. Loop build→fix until `cargo build --workspace 2>&1 | grep -cE "non-exhaustive|resume_mode"` is 0.
- [ ] **Step 2: Verify** `cargo test -p rupu-cp` green; `cargo build -p rupu-cli` compiles (ignore the pre-existing 1.95 red test baseline — only NEW errors from this change matter).
- [ ] **Step 3: Commit.** `git add <the edited files>` → `fix: handle RunStatus::Cancelled + resume_mode across cli/cp`.

---

### Task 3: rupu-cli — CLI cancel reuses the lib + worker passes mode

**Files:** Modify `crates/rupu-cli/src/cmd/workflow.rs`, `crates/rupu-cli/src/cmd/cp.rs`.

- [ ] **Step 1: Rewire `cancel_with_store`** to a thin call to `RunStore::cancel(run_id, &whoami::username(), reason, now)`, mapping the returned `CancelOutcome` to the SAME CLI printouts as today. Remove the now-duplicated pid helpers from `workflow.rs` if they're unused there (they moved to the lib). Keep the existing `cancel_with_store_*` tests passing (adjust their expected status to `Cancelled`).
- [ ] **Step 2: Worker passes mode.** In `cmd/cp.rs` `run_resume_worker`, after claiming + `store.approve(...)`, read the run's `resume_mode` (reload or use the record in hand) and pass `run.resume_mode.as_deref()` to `resume::resume_run(&store, &id, &step_id, mode)` instead of `None`.
- [ ] **Step 3: `cargo build -p rupu-cli` compiles** (no new errors). Commit. `git add crates/rupu-cli/src/cmd/workflow.rs crates/rupu-cli/src/cmd/cp.rs` → `refactor(cli): cancel reuses RunStore::cancel; resume worker honors resume_mode`.

---

### Task 4: rupu-cp — `POST /cancel` + approve `{mode}`

**Files:** Modify `crates/rupu-cp/src/api/runs.rs`.

- [ ] **Step 1: Write failing tests** (handler/store level, like the existing approve/reject tests): `cancel` on a Running run → `Cancelled` + error_message; on terminal → mapped error; `approve` with `{mode:"bypass"}` → `resume_mode = Some("bypass")`, run stays `AwaitingApproval`.
- [ ] **Step 2: Implement.**
  - `POST /api/runs/:id/cancel` — body `struct CancelBody { #[serde(default)] reason: Option<String> }`; `s.run_store.cancel(&id, "web", &reason.unwrap_or("Cancelled from control plane".into()), now)`; map `AlreadyTerminal`→409, `NotFound`→404, else 500; 200 + updated run envelope.
  - Extend `approve_run` to accept an optional JSON body `struct ApproveBody { #[serde(default)] mode: Option<String> }` → `request_resume_approval(&id, "web", body.mode.as_deref(), now)`. (Keep it tolerant of an empty/absent body → mode `None` = ask.)
  - Register the `/cancel` POST route.
  - Update `in_lifecycle` (the run-list lifecycle filter) so `Cancelled` falls in the `failed` group (terminal, non-success).
- [ ] **Step 3: `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` green/clean.** Commit. `git add crates/rupu-cp/src/api/runs.rs` → `feat(cp): POST cancel + approve mode`.

---

### Task 5: Web — Cancel button + Approve mode picker + Cancelled badge

**Files:** Modify `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/RunDetail.tsx`, the StatusPill / run-status mapping, `crates/rupu-cp/web/src/lib/runGraphModel.ts` if it maps run status.

- [ ] **Step 1: Status plumbing.** Add `'cancelled'` to the `RunStatusStr` union (`api.ts`) + `RunRecord.resume_mode?: string | null`. Add a `Cancelled` case to the `StatusPill`/status-style mapping (a neutral slate/grey badge, distinct from Failed's red). Wherever run status drives a color/label, handle `cancelled`.
- [ ] **Step 2: API client.** `cancelRun(id, reason?)` → `POST /cancel`; change `approveRun(id, mode?)` to send `{ mode }` when provided.
- [ ] **Step 3: Buttons.** In `RunDetail.tsx`: a **Cancel** button for non-terminal runs (`running`/`pending`/`awaiting_approval`) — in the header area; for the awaiting case it sits in the banner next to Approve/Reject. Confirm prompt. In the awaiting banner, add a small **mode picker** (Ask / Bypass / Read-only, default Ask) feeding `approveRun(run.id, mode)`. In-flight disable + inline error; optimistic update.
- [ ] **Step 4: Test.** Extend `RunDetail.test.tsx`: clicking Cancel calls `cancelRun(runId)`; approving with a non-default mode calls `approveRun(runId, 'bypass')`. Mock the api.
- [ ] **Step 5: `npm test -- --run` + `npm run build`** green/exit 0; recharts grep = 0. Commit. `git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/pages/RunDetail.tsx <status-pill file> <test>` → `feat(cp/web): Cancel button + Approve mode picker + Cancelled badge`.

---

### Final verification
- `cargo test -p rupu-orchestrator -p rupu-cp` green; clippy clean; `cargo build -p rupu-cli` compiles.
- `npm test -- --run` green; `npm run build` strict; recharts out of main chunk.
- Final whole-branch review (cancel state transitions, the CLI/CP shared impl, the mode flowing marker→worker→resume_run, Cancelled badge), then matt visual-validates.
