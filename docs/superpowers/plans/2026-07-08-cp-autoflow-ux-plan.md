# CP Autoflow UX + Security nav — Implementation Plan

> **For agentic workers:** subagent-driven-development. Steps use `- [ ]`.

**Goal:** Four CP improvements matt asked for:
1. **Runs → Autoflows** page mirrors **Runs → Workflows** — a clean run list (row → RunDetail) with **Cycles** + **Claims** as secondary tabs (not the current claims/cycles-first view).
2. Inside a run, the autoflow **cycle history** becomes a proper **"Cycles" tab** in RunDetail (a linked table, each row → its run) — moved out of the current side-panel.
3. **Disable / Resume the autoflow itself** — a UI button that writes `autoflow.enabled` in the workflow YAML (durable, launcher-gated, validate→backup→atomic).
4. **Coverage + Findings** move out of **Observe** into a new top-level **Security** sidebar group.

**Design decisions (approved):** #1 row opens the RUN (workflow reachable via the Autoflow panel link in RunDetail); #3 edits the YAML `autoflow.enabled` via the config-write path (durable source of truth the reconciler reads), NOT a runtime marker.

## Global Constraints
- #3 write is **launcher-gated** (full `cp serve` runtime; 501 on read-only), **project-aware** (writes the correct project's `.rupu/workflows/<name>.yaml`), **validated** (the candidate must `Workflow::parse` cleanly), **backed up** + **atomic** (reuse `config_write::write_atomic`; a rejected write leaves the file untouched). No silent no-op: Disable/Resume genuinely flips the on-disk `enabled` flag the reconciler reads.
- Backward compatible; routes for Coverage/Findings unchanged (only their nav group moves). Reuse `WorkflowRuns.tsx` as the template for #1 and the existing `RunDetail` TabBar for #2.
- `#![deny(clippy::all)]`; no `unsafe`; `ApiError`/`thiserror`; workspace deps only. Per-file rustfmt only (never lib.rs/mod.rs; `--skip-children` not in rustfmt 1.9.0 → hand-format). Web: `cd crates/rupu-cp/web && npm test && npx tsc --noEmit && npm run build`. Pre-existing 1.88-vs-1.95 `ssh.rs` clippy unrelated. matt validates the CP UI before merge.

## Grounded shapes (verified)
- Nav: `crates/rupu-cp/web/src/lib/sidebarNav.ts` — sections: Runs (agents/workflows/autoflows/sessions), **Observe** (Live Events/Coverage/Findings), Build (workflows/agents/autoflows), Fleet, Settings. Coverage=`/coverage`, Findings=`/findings` (routes in App.tsx:61-90, unchanged).
- Runs pages: `crates/rupu-cp/web/src/pages/runs/{WorkflowRuns,AutoflowRuns,AgentRuns}.tsx`. **WorkflowRuns.tsx** = the clean run-list template to mirror. **AutoflowRuns.tsx** = current Launched-runs/Cycles/Claims tabbed view (SortableTable-based).
- RunDetail: `crates/rupu-cp/web/src/pages/RunDetail.tsx` — `type Tab = 'transcript' | 'events' | 'findings'`, `TabBar`/`TabButton` (components/TabBar), `AutoflowPanel` currently a side panel (shows cycles). `GET /api/runs/:id/autoflow` returns the AutoflowRunContext incl. prior cycles (from PR #465).
- Backend autoflow defs: `crates/rupu-cp/src/api/autoflows.rs` (list + project-aware detail resolution via `distinct_repo_workspaces`); `crates/rupu-cp/src/api/projects.rs` `store(&s)`/`distinct_repo_workspaces`. Config-write: `crates/rupu-cp/src/config_write.rs` (`write_atomic` = backup+atomic; `validate_toml` is TOML-only — for a workflow YAML use `rupu_orchestrator::Workflow::parse` to validate). Launcher gate pattern: `s.launcher... .ok_or_else(|| ApiError::not_available(...))`.
- `rupu_orchestrator::Workflow` has the `autoflow: Option<Autoflow{enabled,...}>` field; toggling `enabled` should preserve the rest of the YAML (comments/other keys) — prefer a minimal targeted edit (e.g. `serde_yaml` round-trip is acceptable if it preserves content adequately, or a `yaml`-aware set of the `autoflow.enabled` scalar; document the approach).

---

## Task 1: Autoflow enable/disable endpoint (backend)

**Files:** Modify `crates/rupu-cp/src/api/autoflows.rs` (+ route registration); Test: same file.

**Interfaces — Produces:** `POST /api/autoflows/:name/enable` and `/disable` (or `PUT /api/autoflows/:name/enabled {enabled: bool}`) — launcher-gated; resolves the workflow file (global or the owning project's `.rupu/workflows/<name>.yaml` via the existing project-aware resolution); sets `autoflow.enabled`; validates the result `Workflow::parse`s + still has an `autoflow:` block; backup + atomic write; returns the updated autoflow row/enabled state.

- [ ] **Step 1: Failing tests** (mirror the autoflows.rs test harness — tempdir global + a project workflow file):
  - `disable_sets_autoflow_enabled_false_in_yaml` — POST disable → the on-disk YAML now has `autoflow.enabled: false` and still `Workflow::parse`s; a `.bak` exists.
  - `enable_sets_true`; `enable_requires_launcher` (no launcher → 501); `enable_unknown_autoflow_404`; `enable_invalid_result_rejected_file_untouched` (if the edit would produce unparseable YAML, reject + leave file unchanged).
- [ ] **Step 2:** `cargo test -p rupu-cp --lib -- autoflows` → FAIL.
- [ ] **Step 3:** Add the route(s) + handler. Launcher-gate. Resolve the workflow file path (reuse the project-aware resolution the autoflow-detail/list already uses; the file is `<scope-dir>/workflows/<name>.yaml`). Read → set `autoflow.enabled` preserving the rest of the file (document the edit approach) → validate the candidate `Workflow::parse`s and has `autoflow` → `config_write::write_atomic` (backup + atomic). Map errors to `ApiError` (404 unknown, 501 no launcher, 400/500 on write/validate failure — file untouched on reject).
- [ ] **Step 4:** tests pass; `cargo test -p rupu-cp --lib` green.
- [ ] **Step 5:** rustfmt autoflows.rs; `cargo clippy -p rupu-cp --no-deps`; commit `feat(cp): enable/disable an autoflow (write autoflow.enabled to YAML, launcher-gated)`.

## Task 2: Runs → Autoflows mirrors Runs → Workflows (web)

**Files:** Rewrite `crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx`; Test: `AutoflowRuns.test.tsx`.

- [ ] **Step 1: Failing vitest** — the page's PRIMARY view is a run list matching WorkflowRuns' columns/table; a row click navigates to `/runs/:id`; the **Cycles** and **Claims** views are secondary tabs still reachable (their existing functionality preserved).
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** READ `WorkflowRuns.tsx` and mirror its structure for autoflow runs: a primary "Runs" tab = the launched/autoflow-run list with the same SortableTable columns + row → `/runs/:id`. Keep the existing **Cycles** (batch view) and **Claims** (requeue/release) as secondary tabs (fold the current page's three-tab content so "Runs" is primary and matches WorkflowRuns, with Cycles/Claims retained). Reuse the existing data fetches (autoflow events/cycles/claims).
- [ ] **Step 4:** `npm test && npx tsc --noEmit && npm run build` clean.
- [ ] **Step 5:** commit `feat(cp-web): Runs→Autoflows mirrors Runs→Workflows (run list primary; Cycles/Claims secondary)`.

## Task 3: RunDetail "Cycles" tab (web)

**Files:** Modify `crates/rupu-cp/web/src/pages/RunDetail.tsx`, `components/AutoflowPanel.tsx` (or a new `components/run/CyclesTab.tsx`); Test: `RunDetail.test.tsx`.

- [ ] **Step 1: Failing vitest** — for an autoflow run, RunDetail shows a **"Cycles"** tab (alongside Transcript/Events/Findings); the tab renders a **table** of the entity's cycle history from `/api/runs/:id/autoflow` (prior cycles), each row **linking to its run** (`/runs/:runId`); a non-autoflow run shows NO Cycles tab.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Add `'cycles'` to `type Tab`; conditionally show the tab when the run is autoflow-sourced (autoflow context present). Render the prior-cycles list as a SortableTable-style table with a run link per row (reuse the ScopeChip/existing table components). Keep the AutoflowPanel's entity/claim summary (as the panel or fold the header info in), but the **cycle history moves into the tab** as a referenceable linked table.
- [ ] **Step 4:** `npm test && npx tsc --noEmit && npm run build` clean.
- [ ] **Step 5:** commit `feat(cp-web): Cycles tab in RunDetail (linked cycle-history table)`.

## Task 4: Disable/Resume autoflow button (web)

**Files:** Modify `crates/rupu-cp/web/src/pages/AutoflowsDefs.tsx` and/or `WorkflowDetail.tsx` (the workflow detail for an autoflow) + `lib/api.ts`; Test: the touched page's test.

- [ ] **Step 1: Failing vitest** — on an autoflow (Build→Autoflows row action and/or the workflow detail when `autoflow.enabled` is set), a **Disable** button (when enabled) calls `POST /api/autoflows/:name/disable`; a **Resume** button (when disabled) calls `/enable`; a 501 (read-only) renders a clear message; the enabled/disabled state reflects after the call.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** `api.ts`: `setAutoflowEnabled(name, enabled)` (→ enable/disable). Add a Disable/Resume toggle where an autoflow is shown — the WorkflowDetail page (when `workflow.autoflow` present) is the natural home; optionally a row action on Build→Autoflows. Reflect state (enabled/disabled) + 501 handling. Mirror the existing pause/approve button patterns.
- [ ] **Step 4:** `npm test && npx tsc --noEmit && npm run build` clean.
- [ ] **Step 5:** commit `feat(cp-web): Disable/Resume an autoflow (toggle autoflow.enabled)`.

## Task 5: Security nav section (web)

**Files:** Modify `crates/rupu-cp/web/src/lib/sidebarNav.ts` (+ CommandPalette grouping if it mirrors sections); Test: a sidebarNav test if present, else a smoke render.

- [ ] **Step 1: Failing/assert** — `sidebarNav` has a new **`security`** group `{ label: 'Security', items: [Coverage(/coverage), Findings(/findings)] }`; the **Observe** group no longer contains Coverage/Findings (keeps Live Events). Routes unchanged.
- [ ] **Step 2:** implement: add the `security` group (choose sensible placement + a Security icon, e.g. `ShieldCheck`/`ShieldAlert` already imported); remove Coverage+Findings from `observe`. Update `CommandPalette.tsx` category labels only if it derives them from sections.
- [ ] **Step 3:** `npm test && npx tsc --noEmit && npm run build` clean; commit `feat(cp-web): move Coverage + Findings into a new Security nav section`.

---

## Self-Review
Coverage: enable/disable endpoint (T1) + Disable/Resume UI (T4); Runs→Autoflows parity (T2); Cycles tab (T3); Security section (T4→T5). Decisions honored (#1 row→run; #3 YAML edit). Launcher-gated + validated + atomic write (T1). Type flow: T1 endpoint → T4 `setAutoflowEnabled`. Parallelizable: T1 (backend), T5 (sidebarNav — disjoint), T2 (AutoflowRuns — disjoint) can go together; T3 (RunDetail) + T4 (Disable UI) after / mind AutoflowPanel overlap.

## Execution
Subagent-driven. Parallel wave: T1 (backend) + T5 (nav) + T2 (AutoflowRuns) — disjoint files. Then T3 (RunDetail Cycles) + T4 (Disable UI, needs T1). Review each; final whole-branch review; one PR to main (no self-merge; matt validates the CP UI: Runs→Autoflows like Runs→Workflows, the Cycles tab, Disable/Resume, and Coverage/Findings under Security).
