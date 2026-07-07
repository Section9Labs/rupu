# CP Build area: project-aware definition lists

> **For agentic workers:** subagent-driven-development. Steps use `- [ ]`.

**Goal:** The top-level Build pages (Agents, Workflows, Autoflows) list definitions from **every registered project's `.rupu/`** plus the global dir — each row tagged with its **scope** (`global` or the project name) — instead of scanning only the global dir. So project-level autoflows (e.g. the rupu repo's `issue-triage`/`pr-code-review`/nightly ones) appear in Build.

**Why:** Today `/api/agents`, `/api/workflows`, `/api/autoflows` each scan only `<global>/…`. All three are global-only, so project-level defs never show in the top-level Build nav (they only show under Project → Overview). Approved fix: make all three aggregate global + all registered projects, with a scope/project column.

**Architecture:** Reuse the existing per-project scan/merge (`scan_autoflow_defs`/`scan_workflow_names`/`load_agents` + the shadow-merge in `api/projects.rs`) but iterate over `WorkspaceStore::list()` (all registered projects) instead of one. Tag project rows with the project name as `scope`. Make the definition **detail** endpoints project-aware so a project-scoped row opens instead of 404ing.

## Global Constraints
- Backward compatible: a deployment with no registered projects (or only global defs) behaves exactly as today (global rows, `scope: "global"`).
- Reuse existing scan/merge helpers; don't reimplement YAML/agent parsing.
- Project defs shadow a **global** def of the same name (existing rule); defs with the same name across **different projects** both appear, distinguished by scope.
- `#![deny(clippy::all)]`; no unsafe; ApiError/thiserror; workspace deps only; hexagonal. Per-file rustfmt only (never lib.rs/mod.rs; `--skip-children` doesn't exist in rustfmt 1.9.0 → hand-format). Web: `cd crates/rupu-cp/web && npm test && npx tsc --noEmit && npm run build`. matt validates the Build UI before merge.

## Grounded shapes (verified)
- `crates/rupu-cp/src/api/autoflows.rs`: `AutoflowDefRow { …, scope: &'static str }` (:21); `scan_autoflow_defs(dir, scope: &'static str)` (:28); `list_autoflow_defs` scans `s.global_dir.join("workflows")` (:110).
- `crates/rupu-cp/src/api/workflows.rs`: `WorkflowDto { …, scope: String }` (:41, already String); `scan_workflow_names(dir, scope: &'static str)` (:50); `list_workflows` (:84); detail via `load_detail` reading `global/workflows/{name}.yaml` (:100+).
- `crates/rupu-cp/src/api/agents.rs`: `list_agents` = `load_agents(&s.global_dir, None)` (:142); `AgentDto::from_spec(spec, "global")`; `get_agent` = `load_agent(&s.global_dir, None, name)` (:75).
- `crates/rupu-cp/src/api/projects.rs`: `store(&s) -> WorkspaceStore` (:49); `store(&s).list()` → workspaces (`.path`, `.ws_id`, `.name`); `project_autoflows`/`project_workflows`/`project_agents` (:336/374/404) do global+one-project merge; `merge_autoflow_defs` shadows global by name.
- Web: `crates/rupu-cp/web/src/pages/{AutoflowsDefs,WorkflowsDefs,AgentsDefs}.tsx`; AutoflowsDefs rows `rowHref = /workflows/${slug}` (:130); `crates/rupu-cp/web/src/lib/api.ts` `AutoflowDefRow`/`WorkflowSummary`/`AgentSummary` types + `getAutoflowDefs`/list clients.

---

## Task 1: Backend — aggregate list endpoints + project-aware detail

**Files:** Modify `crates/rupu-cp/src/api/autoflows.rs`, `workflows.rs`, `agents.rs` (+ maybe a small shared helper); Test: those files' test modules.

**Interfaces — Produces:** `/api/autoflows`, `/api/workflows`, `/api/agents` each return global + every registered project's defs, `scope` = `"global"` or the project name; workflow/agent detail endpoints resolve a name found in any project when absent from global.

- [ ] **Step 1: Failing tests** (mirror the existing endpoint tests + the `project_*` merge tests):
  - `list_autoflow_defs_includes_project_defs`: seed a global workflows dir (0 autoflows) + register a project whose `.rupu/workflows/` has an autoflow-enabled YAML → `/api/autoflows` returns it with `scope` = the project name.
  - `list_workflows_includes_project_defs` + `list_agents_includes_project_defs`: analogous.
  - `list_*_no_projects_is_global_only`: with no registered projects, behavior == today (global rows, scope "global") — backward compat.
  - `workflow_detail_resolves_project_def`: a workflow that exists ONLY in a project's `.rupu/workflows/` resolves via the detail path (not 404).
- [ ] **Step 2: Run → FAIL.** `cargo test -p rupu-cp --lib -- api::autoflows api::workflows api::agents`
- [ ] **Step 3: Implement.**
  - Change `AutoflowDefRow.scope` from `&'static str` to `String`; make `scan_autoflow_defs` accept `scope: impl Into<String>` (or `&str` → owned). (`WorkflowDto.scope` is already `String`; adjust `scan_workflow_names` similarly. `AgentDto.scope` → String if needed.)
  - In each `list_*` handler: start with the global scan (`scope: "global"`), then `for w in store(&s).list()`: scan `Path::new(&w.path).join(".rupu").join(<agents|workflows>)` with `scope = w.name` (or ws_id if name absent) and extend. Apply the existing global-shadow rule (a project def shadows the same-named GLOBAL row) but keep distinct-project rows. Sort by (scope, name) or name. Reuse `merge_*` where it fits; for N projects, a global def is shadowed if ANY project has that name (match current per-project semantics) — keep it simple + documented.
  - **Detail (project-aware):** in the workflow detail (`load_detail`) and `get_agent`, if the name isn't found in global, search registered projects' `.rupu/` for `<name>.{yaml,md}` and load the first match (document collision = first match; a later task can thread scope through the row href). Keep the existing global-first behavior.
  - Handle the usage-rollup joins already present (they key by name — unaffected).
- [ ] **Step 4: Run → PASS** + full `cargo test -p rupu-cp --lib` green.
- [ ] **Step 5:** per-file rustfmt the 3 changed files; `cargo clippy -p rupu-cp --no-deps`; commit `feat(cp): Build lists aggregate global + project defs (agents/workflows/autoflows) + project-aware detail`.

## Task 2: Web — scope/project column on the 3 Build pages

**Files:** Modify `crates/rupu-cp/web/src/pages/{AutoflowsDefs,WorkflowsDefs,AgentsDefs}.tsx`, `src/lib/api.ts` (DTO types gain/confirm `scope: string`); Test: their `*.test.tsx` (or add).

**Interfaces — Consumes:** the Task 1 rows with `scope: string`.

- [ ] **Step 1: Failing vitest** — a Build list page rendered with rows of mixed scope (`global` + a project name) shows a **Scope/Project** column/badge with the right values; a project-scoped autoflow row renders + its link/`rowHref` is present. Mirror an existing Defs-page test.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement.** Add `scope: string` to the `AutoflowDefRow`/`WorkflowSummary`/`AgentSummary` types in api.ts. In each of the 3 Defs pages, render a **Scope** column/badge (e.g. `global` vs the project name) alongside the existing columns. Keep row hrefs; a project row still links to the detail (resolved by Task 1's project-aware detail). Consistent styling across the 3 pages.
- [ ] **Step 4:** `cd crates/rupu-cp/web && npm test && npx tsc --noEmit && npm run build` clean.
- [ ] **Step 5:** commit `feat(cp-web): scope/project column on Build defs pages (agents/workflows/autoflows)`.

---

## Self-Review
Spec coverage: aggregate all 3 list endpoints (T1) + project-aware detail (T1) + web scope column (T2). Backward-compat test (no projects ⇒ today). Type consistency: `scope: String` across DTOs (T1) consumed as `scope: string` in web (T2). Detail-link 404 addressed by project-aware detail (T1); cross-project name collision documented as first-match (follow-up: thread scope/ws into the href).

## Execution
Subagent-driven: T1 (backend) → review → T2 (web) → review → final review → one PR to main (no self-merge; matt validates the Build UI).
