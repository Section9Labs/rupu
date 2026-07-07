# Autoflow runs as first-class CP runs — Implementation Plan

> **For agentic workers:** subagent-driven-development. Steps use `- [ ]`.

**Goal:** The CP resolves a run's artifacts across global → project-local → remote-host on demand; a failed/unpersisted autoflow run renders as a failed run with its cause (no 404); autoflow runs use the shared `RunDetail` view + an Autoflow panel.

**Architecture:** A `resolve_run_location(run_id)` in rupu-cp that tries the global `RunStore`, then an autoflow-history reader (run_id → repo/workspace/host/status), then a bounded host-probe; the run endpoints dispatch on the location (read global / read project-local RunStore / `proxy_get_json` to the host / synthesize from the cycle). Web: autoflow runs open in `RunDetail` + an Autoflow panel fed by `GET /api/runs/:id/autoflow`.

Spec: `docs/superpowers/specs/2026-07-07-rupu-autoflow-runs-firstclass-design.md`.

## Global Constraints
- **On-demand only** — never copy/mirror artifacts into the global store.
- **Fail-closed** — unreachable host / unreadable project dir → a clear typed error in the detail, never a panic/500; 404 only when global + history + all hosts miss.
- A **failed/unpersisted** run linked from the autoflow history renders with status + cause (the cycle failure), not a bare 404.
- Hexagonal: resolver + history reader live in `rupu-cp`, composing `RunStore` (port) + `HostConnector` + read-only `rupu_runtime` autoflow-history types; **no** `rupu-orchestrator`/`rupu-runtime` → `rupu-cp` dependency.
- Reuse PR #463's `api/repo_scope.rs::distinct_repo_workspaces` for project enumeration; reuse the existing `proxy_get_json` host proxy.
- Backward compatible: a normal global run behaves exactly as today (resolver returns `Global` first).
- `#![deny(clippy::all)]`; no `unsafe`; `thiserror`/`ApiError`; workspace deps only. Per-file rustfmt only (never lib.rs/mod.rs; `--skip-children` not in rustfmt 1.9.0 → hand-format). Web: `cd crates/rupu-cp/web && npm test && npx tsc --noEmit && npm run build`. Pre-existing 1.88-vs-1.95 clippy in `host/ssh.rs` unrelated. matt validates the CP UI before merge.

## Grounded shapes (verified)
- `crates/rupu-cp/src/state.rs:65`: `run_store = RunStore::new(global_dir.join("runs"))` (global only); `AppState` also has host registry + `distinct_repo_workspaces` (via `api/repo_scope.rs`, PR #463) + `store(&s)->WorkspaceStore`.
- `crates/rupu-cp/src/api/runs.rs`: `get_run` (:690), `get_run_log` (:724), `get_run_usage_timeline` (:766, already `proxy_get_json`s on `?host=`). `crates/rupu-cp/src/api/graph.rs`: `run_graph` (:35, `proxy_get_json` on `?host=` :44; PR #460 handles empty-snapshot agent runs via a single-node dag :66+).
- Autoflow data on disk: `~/.rupu/autoflows/history/cycles/<date>/afc_*.json` (has `run_id`, `status`, `workflow`, repo, entity), `.../events/<date>/afe_*.json` (`run_launched`/`cycle_failed` with run_id+status+detail), `~/.rupu/autoflows/claims/<repo--issue>/claim.toml`. Types: `rupu_runtime::autoflow_history` (`AutoflowCycleRecord`/events), `AutoflowHistoryStore` (autoflow_runtime.rs:99, in rupu-cli — for the CP, add a read-only reader in rupu-cp over the same on-disk shape, or lift a shared reader into rupu-runtime if clean).
- `RunStore` (`rupu_orchestrator::runs::RunStore`) is constructible on any root dir (`RunStore::new(dir)`) — used to read a project-local store.
- Web: `crates/rupu-cp/web/src/pages/RunDetail.tsx` (rich single-run view), `pages/runs/AutoflowRuns.tsx` (list+claims), `lib/api.ts`, `lib/runGraphModel`, `components/ScopeChip.tsx`.

---

## Task 1: Autoflow-history reader + `resolve_run_location` (backend)

**Files:** Create `crates/rupu-cp/src/api/run_resolve.rs` (+ a `autoflow_history` reader module, or fold in); Modify `crates/rupu-cp/src/api/mod.rs` (wire module); Test: `run_resolve.rs`.

**Interfaces — Produces:** `enum RunLocation { Global, ProjectLocal{path: PathBuf}, Host{host_id: String}, Unpersisted{cycle_id: String, status: RunStatus, failure: String, workflow_name: String, entity: Option<..>}, NotFound }`; `fn resolve_run_location(s: &AppState, run_id: &str) -> RunLocation`; an autoflow-history reader `fn autoflow_run_context(global_dir: &Path, run_id: &str) -> Option<AutoflowRunContext>` (+ `entity_cycles(...)`).

- [ ] **Step 1: Failing tests** (build a tempdir `~/.rupu`-shaped fixture: `runs/<id>/run.json`, `autoflows/history/cycles|events/*`, a registered project store):
  - `resolves_global_when_run_json_present`
  - `resolves_unpersisted_from_history_when_no_run_json` (history has run_id + a `cycle_failed`/`run_launched` with a failure message → `Unpersisted{failure}`)
  - `resolves_project_local_when_run_in_project_store` (run.json only under a registered project's `.rupu/runs/<id>`)
  - `resolves_host_when_history_records_remote_host`
  - `not_found_when_nowhere`
- [ ] **Step 2:** `cargo test -p rupu-cp --lib -- run_resolve` → FAIL.
- [ ] **Step 3: Implement.** The autoflow-history reader scans `global_dir/autoflows/history/{cycles,events}` for the run_id (parse the JSON; extract status/failure/workflow/entity/repo/workspace_path/host); the claim dir for claim state. `resolve_run_location`: global `run_store.load` hit → `Global`; else history lookup → `Host`/`ProjectLocal`/`Unpersisted` per what the history says + whether artifacts exist at the resolved path; else bounded host-probe (iterate registered hosts, `proxy_get_json` `/api/runs/:id`, first hit → `Host`, cache); else `NotFound`. Reuse `distinct_repo_workspaces` for project stores. Keep it read-only + fail-closed (unreadable dir → skip, don't panic).
- [ ] **Step 4:** tests pass; `cargo test -p rupu-cp --lib` green.
- [ ] **Step 5:** rustfmt the new/changed non-root files; `cargo clippy -p rupu-cp --no-deps`; commit `feat(cp): resolve_run_location + autoflow-history reader (run resolution across stores/hosts)`.

## Task 2: Location-aware run endpoints + `/autoflow` + unpersisted synthesis (backend)

**Files:** Modify `crates/rupu-cp/src/api/runs.rs` (get_run/log/usage), `crates/rupu-cp/src/api/graph.rs` (run_graph), transcript stream if separate; add route `/api/runs/:id/autoflow`; Test: those files.

**Interfaces — Consumes:** Task 1's `resolve_run_location` + `autoflow_run_context`. **Produces:** `get_run`/`run_graph`/`get_run_log`/`get_run_usage_timeline` dispatch on `RunLocation`; `GET /api/runs/:id/autoflow -> AutoflowRunContext` (entity/claim/cycle/prior-cycles/project/host).

- [ ] **Step 1: Failing tests:**
  - `get_run_unpersisted_returns_failed_record_not_404` — a history-only run_id → 200 with a synthesized record: status failed/blocked, `error_message` = the failure, workflow_name, cycle id (the `run_01KWYZ2QY4…` shape).
  - `run_graph_unpersisted_returns_single_node` — graph endpoint returns a minimal graph (mirror PR #460) so RunDetail renders.
  - `get_run_project_local_reads_project_store`; `get_run_host_proxies` (fake connector).
  - `autoflow_endpoint_returns_context` — `/api/runs/:id/autoflow` returns entity/claim/cycle/prior-cycles; a non-autoflow run → 404/empty.
  - Backward compat: `get_run_global_unchanged`.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: Implement.** In each endpoint: `match resolve_run_location(&s, &id)`:
  - `Global` → existing code.
  - `ProjectLocal{path}` → build a `RunStore::new(path.join(".rupu").join("runs"))` (or the resolved run dir) and load/serve from it with the same DTOs.
  - `Host{host_id}` → `resolve_host(&s, &host_id)?.proxy_get_json("/api/runs/:id[/graph|/log|/usage-timeline]")` (generalize the existing `?host=` branch to also fire on resolution).
  - `Unpersisted{..}` → synthesize the DTO from the autoflow context (a `RunRecord`-shaped JSON: status, workflow_name, error_message=failure, entity, timestamps, cycle id); `run_graph` returns a single-node/failed graph.
  - `NotFound` → 404.
  Add `/api/runs/:id/autoflow` handler returning `autoflow_run_context` (404 when the run isn't autoflow-sourced). Fail-closed on unreachable host (clear error state).
- [ ] **Step 4:** tests pass; full `cargo test -p rupu-cp --lib` green.
- [ ] **Step 5:** rustfmt changed files; clippy; commit `feat(cp): location-aware run endpoints + /api/runs/:id/autoflow + unpersisted-run synthesis`.

## Task 3: Web — RunDetail parity + Autoflow panel (web)

**Files:** Modify `crates/rupu-cp/web/src/pages/RunDetail.tsx`, `pages/runs/AutoflowRuns.tsx`, `lib/api.ts` (+ a new `components/AutoflowPanel.tsx`); Test: `RunDetail.test.tsx`, `AutoflowRuns.test.tsx`.

**Interfaces — Consumes:** Task 2's `get_run` (works for autoflow/unpersisted runs) + `GET /api/runs/:id/autoflow`.

- [ ] **Step 1: Failing vitest:**
  - RunDetail with an autoflow run (mock `/api/runs/:id/autoflow` returns context) renders an **Autoflow panel** (entity link, claim status, cycle id, project/host); a non-autoflow run renders NO panel.
  - RunDetail with a failed/unpersisted run (get_run returns status failed + error_message) shows the failure + reason (not a crash/404).
  - `AutoflowRuns` list rows link to `/runs/:id` (the shared RunDetail).
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: Implement.** `api.ts`: `getRunAutoflow(id)` + the `AutoflowRunContext` type. New `AutoflowPanel.tsx` (entity/claim/cycle/prior-cycles/project-host, using ScopeChip for project/host). In `RunDetail.tsx`: fetch `/api/runs/:id/autoflow` (tolerate 404 → no panel) and render `AutoflowPanel` when present; ensure a failed/unpersisted run (status failed + error_message, minimal graph) renders gracefully. In `AutoflowRuns.tsx`: link run rows to `/runs/:id`; remove the separate per-run detail rendering (keep list + claims tabs).
- [ ] **Step 4:** `cd crates/rupu-cp/web && npm test && npx tsc --noEmit && npm run build` clean.
- [ ] **Step 5:** commit `feat(cp-web): autoflow runs use shared RunDetail + Autoflow panel; retire separate autoflow detail`.

---

## Self-Review
Coverage: resolver + history reader (T1); location-aware endpoints + unpersisted synthesis + `/autoflow` (T2); web parity + panel + failed-run render (T3). Fail-closed + 404-only-when-truly-missing (T1/T2). Backward compat (Global-first, non-autoflow run unchanged) tested in T1/T2/T3. Type flow: `RunLocation`/`AutoflowRunContext` (T1) → endpoints (T2) → `getRunAutoflow`/panel (T3). No placeholders; open questions from the spec (project-local reality, probe bound, autoflow-detection signal) resolved inline in T1/T2 with stated defaults.

## Execution
Subagent-driven: T1 → review → T2 → review → T3 → review → final whole-branch review → one PR to main (no self-merge; matt validates the CP UI: an autoflow run opening in RunDetail with the panel, and the failed `run_01KWYZ2QY4…`-style run showing its 401 cause instead of a 404).
