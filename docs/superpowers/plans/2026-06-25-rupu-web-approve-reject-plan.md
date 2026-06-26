# Web Approve / Reject — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Approve/reject a workflow run's approval gate from the web CP. Reject is a terminal file mutation. Approve records the decision (run.json flip + resume marker) and a background worker inside `rupu cp serve` resumes execution.

**Spec:** `docs/superpowers/specs/2026-06-25-rupu-web-approve-reject-design.md`

**Constraints (every task):** no `any` in TS; static Tailwind only; recharts out of the main chunk; stage only specific changed files (`git add <paths>`, never `-A`, never `.rupu/*`); never package-wide `cargo fmt`. **Toolchain:** the worktree runs Homebrew Rust 1.95 — `rupu-orchestrator` / `rupu-cp` / web are clean gates here; **`rupu-cli` has a pre-existing red baseline on 1.95**, so for rupu-cli tasks verify the crate *compiles* (`cargo build -p rupu-cli`) and rely on CI (1.88) for its tests; do not treat the rupu-cli baseline failures as regressions.

---

### Task 1: Orchestrator — pending-resume marker + claim methods

**Files:** Modify `crates/rupu-orchestrator/src/runs.rs`.

**Context:** `RunRecord` holds gate state (`status: RunStatus`, `awaiting_step_id`, `approval_prompt`, `awaiting_since`, `expires_at`). `RunStore::approve(run_id, approver, now) -> Result<ApprovalDecision, _>` flips `AwaitingApproval → Running` + clears pause fields; `reject(run_id, reason, now)` is terminal. Read these (~lines 800–888) first.

- [ ] **Step 1: Add fields.** On `RunRecord`: `#[serde(default)] resume_requested_at: Option<DateTime<Utc>>`, `#[serde(default)] resume_claimed_at: Option<DateTime<Utc>>`, `#[serde(default)] resume_claimed_by: Option<String>`. All default so existing `run.json` parse.
- [ ] **Step 2: Write failing tests** (in runs.rs `#[cfg(test)]`): (a) `request_resume_approval` flips an awaiting run to `Running`, clears pause fields, and sets `resume_requested_at`; on a non-awaiting run it errors like `approve`. (b) `list_pending_resume` returns runs with `resume_requested_at.is_some()` whose claim is absent or older than the lease TTL, and excludes a freshly-claimed one. (c) `claim_resume(id, worker, now)` returns `true` and sets the lease the first time, `false` while a live lease exists, `true` again once the lease is older than TTL. (d) `clear_resume(id)` drops `resume_requested_at` + claim.
- [ ] **Step 3: Run `cargo test -p rupu-orchestrator runs`, confirm failure.**
- [ ] **Step 4: Implement.**
  - `request_resume_approval(&self, run_id, approver: &str, now) -> Result<ApprovalDecision, _>` — same validation + flip as `approve`, plus `record.resume_requested_at = Some(now)` before `update`.
  - `const RESUME_LEASE: Duration` (e.g. 5 min). `list_pending_resume(&self, now) -> Result<Vec<RunRecord>, _>` — `list()` filtered to `resume_requested_at.is_some()` AND (`resume_claimed_at` is `None` OR `now - claimed_at > RESUME_LEASE`).
  - `claim_resume(&self, run_id, worker_id: &str, now) -> Result<bool, _>` — load; if a live lease by anyone exists return `Ok(false)`; else set `resume_claimed_at = Some(now)`, `resume_claimed_by = Some(worker_id)`, `update`, return `Ok(true)`.
  - `clear_resume(&self, run_id, now) -> Result<(), _>` — load; clear the three resume fields; `update`.
- [ ] **Step 5: Run `cargo test -p rupu-orchestrator` + `cargo clippy -p rupu-orchestrator --all-targets`, confirm green/clean.**
- [ ] **Step 6: Commit.** `git add crates/rupu-orchestrator/src/runs.rs` → `feat(orchestrator): pending-resume marker + claim/clear for delegated approval`.

---

### Task 2: rupu-cp — approve/reject write endpoints

**Files:** Modify `crates/rupu-cp/src/api/runs.rs` (handlers + routes).

**Context:** All rupu-cp routes are GET today; these are the first POSTs. `AppState.run_store: Arc<RunStore>` (rupu-cp depends on rupu-orchestrator). Use `axum::routing::post`, `axum::Json` for the reject body. Depends on Task 1's `request_resume_approval`.

- [ ] **Step 1: Write failing tests** — handler-level (build an `AppState` over a tempdir RunStore with an awaiting run, like other rupu-cp api tests): `POST approve` → run becomes `Running` with `resume_requested_at` set; `POST reject` with a reason → run becomes `Rejected` with the reason; approve/reject on a non-awaiting run → error (mapped to 409). If full-handler wiring is heavy, test the thin handler logic against a real `RunStore` directly.
- [ ] **Step 2: Run `cargo test -p rupu-cp`, confirm failure.**
- [ ] **Step 3: Implement.**
  - `async fn approve_run(State(s), Path(id)) -> ApiResult<Json<...>>` → `s.run_store.request_resume_approval(&id, "web", now)`; map `NotFound → 404`, a status/precondition error → `ApiError` 409, success → 200 with the updated run (reuse the existing run-summary serialization or return the `RunRecord`).
  - `async fn reject_run(State(s), Path(id), Json(body): Json<RejectBody>)` where `struct RejectBody { reason: Option<String> }` → `s.run_store.reject(&id, body.reason.unwrap_or_default(), now)`; same error mapping; 200 with updated run.
  - Routes: `.route("/api/runs/:id/approve", post(approve_run))` and `.route("/api/runs/:id/reject", post(reject_run))`. Use a real `now` (chrono::Utc::now()).
- [ ] **Step 4: `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` green/clean.**
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/src/api/runs.rs` → `feat(cp): POST approve/reject endpoints (record decision)`.

---

### Task 3: rupu-cli — extract `resume::resume_run`

**Files:** Create `crates/rupu-cli/src/resume.rs` (or a module under `cmd/`); Modify `crates/rupu-cli/src/cmd/workflow.rs` (+ `lib.rs`/`main.rs` module decl).

**Context:** `workflow.rs::approve` (~lines 1885–2064) does: `store.approve(...)` then a big inline RESUME — reload run record + workflow YAML snapshot + prior step results, rebuild `KeychainResolver`, layered config, `rupu_scm::Registry::discover`, `CliAgentDispatcher`, `DefaultStepFactory`, build `ResumeState::from_approval(...)`, call `run_workflow(opts).await`, handle re-pause/completion. Read it fully.

- [ ] **Step 1: Extract** the resume body (everything AFTER the `store.approve` call) into `pub async fn resume_run(store: &RunStore, run_id: &str) -> anyhow::Result<ResumeOutcome>` in the new module. It reloads the run by id and performs the rebuild + `run_workflow` exactly as today. Define `ResumeOutcome` (e.g. `{ status: RunStatus, awaiting_step_id: Option<String> }`) or reuse whatever `approve` already computes for its printout.
- [ ] **Step 2: Rewire `workflow approve`** to call `store.approve(...)` then `resume::resume_run(&store, run_id).await?` and print from the outcome — preserving the current user-facing behavior (same messages, same re-pause handling). No functional change to the CLI command.
- [ ] **Step 3: Build + behavior check.** `cargo build -p rupu-cli` compiles. (rupu-cli tests are red-baseline on 1.95 — confirm no NEW compile errors from this change; rely on CI for tests.) If there's an existing `workflow approve` test that passes on 1.95, keep it green.
- [ ] **Step 4: Commit.** `git add crates/rupu-cli/src/resume.rs crates/rupu-cli/src/cmd/workflow.rs <module decl file>` → `refactor(cli): extract resume::resume_run from workflow approve`.

---

### Task 4: rupu-cli — `cp serve` background resume worker

**Files:** Modify `crates/rupu-cli/src/cmd/cp.rs`; possibly `crates/rupu-cp/src/lib.rs` (`serve` shutdown handle) if needed.

**Context:** `cmd/cp.rs` `Action::Serve` builds `global_dir` and calls `rupu_cp::serve(ServeOpts { bind, token, global_dir, open_browser }).await`. Depends on Task 1 (store methods) + Task 3 (`resume::resume_run`). The worker needs a `RunStore` — construct from `global_dir.join("runs")` (the same path rupu-cp's AppState uses; confirm via `crates/rupu-cp/src/state.rs`).

- [ ] **Step 1: Implement the worker loop** `async fn run_resume_worker(store: Arc<RunStore>, worker_id: String, shutdown: <signal>)`: every ~4s (and on start), `store.list_pending_resume(now)?`; for each, if `store.claim_resume(&id, &worker_id, now)?` is `true`, `tokio::spawn` a task that `resume::resume_run(&store, &id).await` then `store.clear_resume(&id, now)` (clear regardless of resume Ok/Err so a poisoned run isn't retried forever; on Err, log it — the run is left in whatever state `resume_run` persisted, e.g. Failed). Log each claim/resume. Backoff on store errors.
- [ ] **Step 2: Wire into `Action::Serve`.** Build the `RunStore` + a `worker_id` (e.g. `format!("cp-serve-{pid}")`), `tokio::spawn` the worker, then run `rupu_cp::serve(...).await`. When serve returns (server stopped), signal the worker to stop (a `tokio::sync::watch`/`Notify`, or simply abort the handle). Keep startup logging that mentions the resume worker is active.
- [ ] **Step 3: Build.** `cargo build -p rupu-cli` compiles; `cargo build -p rupu-cp` if touched. (rupu-cli tests red-baseline on 1.95 — verify compile; CI on 1.88 covers tests.)
- [ ] **Step 4: Commit.** `git add crates/rupu-cli/src/cmd/cp.rs <+ rupu-cp/src/lib.rs if changed>` → `feat(cli): resume worker inside cp serve (delegated approval resume)`.

---

### Task 5: Web — Approve / Reject buttons

**Files:** Modify `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/RunDetail.tsx`.

**Context:** The `request<T>` wrapper already supports `{ method:'POST', body }` + 204. The RunDetail awaiting banner (~lines 360–373) is view-only with a "controls arrive in a later phase" line + `run.id` in scope. Depends on Task 2's endpoints.

- [ ] **Step 1: API client.** `approveRun(id: string): Promise<void>` → `request(\`/api/runs/${id}/approve\`, { method: 'POST' })`. `rejectRun(id: string, reason: string): Promise<void>` → `request(\`/api/runs/${id}/reject\`, { method: 'POST', body: JSON.stringify({ reason }) })`. No `any`.
- [ ] **Step 2: Buttons.** Replace the "view only for now" `<p>` in the awaiting banner with **Approve** + **Reject** buttons (static Tailwind, amber/emerald + red). Reject reveals a small inline reason input (optional reason) + confirm. On Approve → call `approveRun(run.id)`, then set a local "approved" state showing "Approved — resuming…"; on Reject → call `rejectRun(run.id, reason)`. Disable while in-flight; show an inline error on failure. The live event stream / existing poll will reflect the status change.
- [ ] **Step 3: Test.** A RunDetail (or banner) test: with an awaiting run, clicking Approve calls `approveRun` (spy) with the run id; clicking Reject (with a reason) calls `rejectRun`. Mock the api.
- [ ] **Step 4: `npm test -- --run` + `npm run build`** (strict) green/exit 0; recharts grep = 0; no `any`.
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/pages/RunDetail.tsx <test>` → `feat(cp/web): Approve/Reject buttons on the run approval gate`.

---

### Final verification (after all tasks)
- `cargo test -p rupu-orchestrator -p rupu-cp` green; clippy clean on both; `cargo build -p rupu-cli` compiles.
- `npm test -- --run` green; `npm run build` strict; recharts out of main chunk.
- Final whole-branch review (the approve→marker→worker→resume chain end-to-end; reject terminal; the cp-serve worker shutdown), then hand to matt for visual validation (`rupu cp serve`, approve a gated run → it resumes; reject → Rejected).
