# rupu-cp — Per-run findings tab (move findings off the workflow definition) — Design

**Date:** 2026-06-23
**Surface:** `crates/rupu-cp` + `crates/rupu-cp/web`
**Status:** approved (matt), ready for implementation
**Corrects:** `2026-06-23-rupu-cp-scoped-findings-and-cwe-design.md` — findings were placed on the workflow *definition* page; they belong at the *run* level.

## Why
A workflow definition has no findings of its own — findings are produced by individual **runs**. A project can have many runs of a workflow, each with its own findings. So findings must be viewable per run, and the workflow-definition page should not carry a findings list.

## Changes
1. **Add a `Findings` tab to `RunDetail`** (`/runs/:id`). The page currently has `Graph` and `Events` tabs (`TabBar`/`TabButton`). Add a third `Findings` tab (icon `ShieldAlert`) whose body lists the findings declared during THIS run: a `FindingMetrics` summary strip + a `FindingRow` list, fetched via `getFindings({ runId: id })`.
2. **Backend `?run_id=` filter** on `GET /api/findings`. `FindingsQuery` currently has `ws_id`/`workflow`; add `run_id: Option<String>`, filtering on each finding's `declared_by.run_id` (mirrors the existing filters; summary over the filtered set). `RunDetail`'s `:id` is the same orchestrator run id as `declared_by.run_id`, so the join is exact.
3. **Remove the findings section from `WorkflowDetail`** (`/workflows/:name`). This also retires the stem-vs-YAML-name footgun (no more filtering by workflow name from the route). The backend `?workflow=` filter + `workflow_name` field stay (harmless; `workflow_name` remains useful provenance), just unused by the UI.
4. **Keep** the project findings section on `ProjectDetail` (a project aggregates findings across its runs) — unchanged.

## API
- `getFindings(opts?: { wsId?; workflow?; runId? })` — adds `runId` → `?run_id=`.
- `GET /api/findings?run_id=<id>` → only findings whose `declared_by.run_id == id`, summary scoped to them.

## Files
- `crates/rupu-cp/src/api/findings.rs` *(modify)* — `run_id` filter + test.
- `crates/rupu-cp/web/src/lib/api.ts` *(modify)* — `runId` opt on `getFindings`.
- `crates/rupu-cp/web/src/pages/RunDetail.tsx` *(modify)* — `Findings` tab.
- `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx` *(modify)* — remove findings section.

## Testing
- Backend: `cargo test -p rupu-cp` — `run_id` filter scopes findings + summary; existing filters unaffected. clippy clean.
- Frontend: `npm test -- --run` green; `npm run build` strict; recharts out of main chunk; no `any`; static Tailwind.
- Visual: a workflow run's Findings tab shows that run's findings; the workflow-definition page no longer shows findings; project findings unchanged.
