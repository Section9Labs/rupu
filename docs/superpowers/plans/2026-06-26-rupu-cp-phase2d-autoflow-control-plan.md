# rupu-cp Phase 2d — Autoflow control (requeue + release) — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** From the web CP, **requeue** an issue's autoflow (enqueue a wake) and **release** a stuck claim — pure-state, via the existing `rupu-runtime`/`rupu-workspace` library stores (rupu-cp already deps both). Includes the missing **claims read** endpoint + a Claims tab.

**Design source:** the Phase-2d surface analysis (this session). Part of CP Phase 2 (Control); see `docs/superpowers/specs/2026-06-18-rupu-control-plane-design.md` + `TODO.md`.

**Constraints:** no `any` (TS); static Tailwind; recharts out of main chunk; stage only specific files; never `-A`/`.rupu/*`; never package-wide `cargo fmt`. Toolchain: rupu-cp/web clean on worktree 1.95.

## Key facts (from analysis)
- `WakeStore::new(global_dir.join("autoflows").join("wakes"))`; `enqueue(WakeEnqueueRequest) -> WakeRecord` (`crates/rupu-runtime/src/wake.rs:114`). Manual requeue mirrors the CLI `enqueue_issue_wake`: `source: WakeSource::Manual`, `repo_ref` (from the claim), `entity: WakeEntity { kind: Issue, ref_text: issue_ref }`, `event: WakeEvent { id: "autoflow.manual.requeue", delivery_id: None, dedupe_key: None }`, `payload: None`, `received_at`/`not_before` = rfc3339 now (or now+defer).
- `AutoflowClaimStore { root: global_dir.join("autoflows").join("claims") }`; `list() -> Vec<AutoflowClaimRecord>`, `load(issue_ref) -> Option<..>`, `delete(issue_ref) -> bool` (`crates/rupu-workspace/src/autoflow_claim_store.rs`). Keyed by `issue_ref` (a string with `/`+`:` → pass in JSON body, never a path segment).
- `AutoflowClaimRecord` fields for UI: `issue_ref, issue_display_ref, repo_ref, issue_title, issue_url, workflow, status (ClaimStatus), last_run_id, last_error, last_summary, pr_url, claim_owner, lease_expires_at, updated_at`.
- **Release v1 = `delete(issue_ref)` only** (unblocks the autoflow). The CLI also tears down the git worktree via `cleanup_claim_artifacts` (needs `RepoRegistryStore` + a CLI-private fn, bails on untracked repos) — **defer worktree cleanup** to a follow-up (note in TODO) so the endpoint never hard-fails.
- Mirror the existing mutation pattern in `crates/rupu-cp/src/api/runs.rs` (approve/reject: `State<AppState>` + `Json` body + store call + `Json<Value>`).
- UI: `crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx` already has a `Tab` toggle (~line 75/172) — add a **Claims** tab. `api.ts` mutation pattern = `approveRun`/`cancelRun`.

---

### Task 1: Backend — claims list + release + requeue (rupu-cp)

**Files:** Create `crates/rupu-cp/src/api/autoflow_claims.rs` (or extend `api/autoflows.rs`); Modify `api/mod.rs` + `server.rs` (route registration).

- [ ] **Step 1: Write failing tests.** Over a tempdir `AutoflowClaimStore` + `WakeStore` (build from a temp global_dir): (a) listing returns the written claims; (b) `release(issue_ref)` deletes the claim (subsequent list omits it; releasing an absent ref → handled, not a 500); (c) `requeue` reads the claim's `repo_ref` and enqueues a `WakeSource::Manual` wake whose `entity.ref_text == issue_ref` + `event.id == "autoflow.manual.requeue"` (assert via `WakeStore::list_queued()`). Factor the pure logic (build the `WakeEnqueueRequest` from a claim + now) into a testable fn.
- [ ] **Step 2:** Run `cargo test -p rupu-cp`, confirm failure.
- [ ] **Step 3: Implement.**
  - `GET /api/autoflows/claims` → `AutoflowClaimStore { root: s.global_dir.join("autoflows").join("claims") }.list()` → `Json<Vec<ClaimRow>>` (serialize the record, or a slim `ClaimRow` DTO with the UI fields above + `status` lowercased).
  - `POST /api/autoflows/claims/release` body `{ issue_ref: String }` → `store.delete(&issue_ref)`; return `Json({ released: bool })` (200 even if it was absent → `released: false`). Map store IO errors → 500.
  - `POST /api/autoflows/claims/requeue` body `{ issue_ref: String, #[serde(default)] not_before: Option<String> }` → `store.load(&issue_ref)?` (None → 404 "no such claim"); build the Manual `WakeEnqueueRequest` from `claim.repo_ref` + `issue_ref` (+ `not_before` if a valid rfc3339, else now); `WakeStore::new(global_dir/autoflows/wakes).enqueue(req)` → `Json({ wake_id })`. Map errors → 500.
  - Register the three routes (`get`/`post`) in the module's `routes()` and merge in `server.rs`.
- [ ] **Step 4:** `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` green/clean.
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/src/api/autoflow_claims.rs crates/rupu-cp/src/api/mod.rs crates/rupu-cp/src/server.rs` → `feat(cp): autoflow claims list + release + requeue endpoints`.

---

### Task 2: Frontend — Claims tab + Release/Requeue buttons

**Files:** Modify `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx`; Test.

- [ ] **Step 1:** `api.ts`: an `AutoflowClaim` interface (matching the backend `ClaimRow`); `getAutoflowClaims(): Promise<AutoflowClaim[]>` (GET); `releaseClaim(issueRef: string): Promise<{ released: boolean }>` (POST `/api/autoflows/claims/release` body `{ issue_ref }`); `requeueClaim(issueRef: string): Promise<{ wake_id: string }>` (POST `/api/autoflows/claims/requeue` body `{ issue_ref }`). No `any`.
- [ ] **Step 2:** `AutoflowRuns.tsx`: extend the `Tab` union with `'claims'`; add a Claims tab button. The Claims panel fetches `getAutoflowClaims()` and renders a row per claim: `issue_display_ref` (or `issue_ref`), `workflow`, a `status` pill, `last_error`/`last_summary` if present, links (`issue_url`/`pr_url`) — plus two actions: **Requeue** (confirm → `requeueClaim` → toast/refresh) and **Release** (confirm → `releaseClaim` → refresh, removing the row). In-flight disable + inline error. Static Tailwind; reuse the app's row/list + StatusPill-style components where they fit. Empty state ("No active claims").
- [ ] **Step 3: Test** (`AutoflowRuns` or a focused claims test): with mocked `getAutoflowClaims` returning a claim, clicking Release (confirmed) calls `releaseClaim(issueRef)`; clicking Requeue calls `requeueClaim(issueRef)`. `vi.spyOn` the api.
- [ ] **Step 4:** `npm test -- --run` + `npm run build` green/exit 0; recharts grep = 0.
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx <test>` → `feat(cp/web): autoflow Claims tab with Requeue + Release`.

---

### Final verification
- `cargo test -p rupu-cp` green; clippy clean. `npm test -- --run` green; `npm run build` strict; recharts out of main chunk.
- Final scoped review (requeue builds the right Manual wake; release deletes the claim; issue_ref-in-body; defer-worktree-cleanup noted), then matt visual-validates.
- Add a TODO note: "2d release defers git-worktree cleanup (`cleanup_claim_artifacts`); claim delete unblocks the autoflow but may leave an orphaned worktree under `<global>/autoflows/worktrees/`."
