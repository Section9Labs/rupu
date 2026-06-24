# rupu-cp — Scoped findings (project/workflow) + CWE identifier + badge polish — Design

**Date:** 2026-06-23
**Surface:** `crates/rupu-cp` (axum read-adapter) + `crates/rupu-cp/web`
**Status:** approved (matt), ready for plan
**Builds on:** `2026-06-22-rupu-cp-findings-and-timeline-design.md` (the base Findings view, shipped in v0.11.0)

## Goal
Extend the Findings feature with three improvements:
1. **Scoped findings lists** — a findings section inside each **project** (filtered by `ws_id`) and inside each **workflow** (filtered by `workflow_name`), each being the main findings list scoped down.
2. **CWE / identifier display** — each finding row shows a linked `CWE-NNN` chip when one is derivable, alongside the `concern_id` (the canonical identifier).
3. **Badge polish** — the severity pill is a uniform width so CRITICAL/HIGH/MEDIUM/LOW/INFO align.

## Non-goals
- No catalog resolution (we do NOT load the bundled concern catalog server-side). CWE is parsed from data already on the finding.
- No finding mutation. No new finding source.
- No filter UI on the main `/findings` page itself (scoping lives on the project/workflow detail pages). The main page continues to show everything.

---

## ① Scoped findings

### Backend — `GET /api/findings` gains a workflow join + optional filters
File: `crates/rupu-cp/src/api/findings.rs`.

**Workflow association.** A `FindingRecord` carries `declared_by.run_id` but no workflow name. Add `workflow_name: Option<String>` to `FindingOut`, resolved by joining `run_id → RunStore::load(run_id).workflow_name`:
- Collect the DISTINCT `run_id`s across all gathered findings, `load` each once into a `HashMap<String, String>` (`run_id → workflow_name`). A `RunStoreError::NotFound` (or any load error) → the run_id is simply absent from the map (tolerant; `workflow_name` stays `None`). Findings from non-workflow surfaces (`session`/`agent`/`autoflow`) naturally resolve to `None`.
- `RunStore::load` is a single `run.json` read; run records persist (no prune path), so historical findings still resolve.

**Optional query filters.** Two optional, plain-string query params (NOT a flattened struct — avoids the serde_urlencoded numeric-flatten bug):
```rust
#[derive(serde::Deserialize)]
struct FindingsQuery { ws_id: Option<String>, workflow: Option<String> }
```
- `ws_id` present → keep only findings whose provenance `ws_id` matches.
- `workflow` present → keep only findings whose resolved `workflow_name == Some(workflow)`.
- Both absent → all findings (current behavior).
- The `summary` is computed over the FILTERED set, so a scoped view's tiles reflect that scope.

Response shape is unchanged except each `FindingOut` now also carries `workflow_name: Option<String>`. Sort order (severity desc, then `declared_at` desc) is unchanged.

**Verification gate (during implementation):** confirm `declared_by.run_id` is the orchestrator run id that `RunStore` is keyed by (i.e. the join actually resolves for a real workflow finding) — check where the coverage `report_finding` path stamps `Attribution.run_id`. If it stamps a synthetic/agent-local id, the workflow filter would silently return empty; surface that in the task report rather than shipping a dead filter.

### Frontend — scoped sections
- `lib/api.ts`: `getFindings(opts?: { wsId?: string; workflow?: string }): Promise<FindingsResponse>` builds the query string from provided opts; add `workflow_name?: string | null` to `FindingOut`.
- `pages/ProjectDetail.tsx` (route `/projects/:wsId`): add a **Findings** section that calls `getFindings({ wsId })` and renders `<FindingMetrics summary />` (static) + the `<FindingRow>` list (with provenance chips). Placed near the existing coverage rollup. Standard loading/empty states; empty → a small "No findings" note (not a broken section).
- `pages/WorkflowDetail.tsx` (route `/workflows/:name`): add the same **Findings** section calling `getFindings({ workflow: name })`.
- Both reuse the existing `FindingMetrics` + `FindingRow` components verbatim.

---

## ② CWE / identifier display
Frontend-only (data already shipped). File: `crates/rupu-cp/web/src/components/findings/FindingRow.tsx` + a small helper (e.g. `lib/cwe.ts`).

- `cweFromFinding(finding): { id: string; url: string } | null` — derive a CWE:
  1. From `concern_id`: match `/cwe[-_]?(\d+)/i` (handles slugs like `cwe-top25-2023:cwe-787-...`).
  2. Else from `evidence.references`: match a `cwe.mitre.org/data/definitions/(\d+)` URL.
  3. Else `null`.
  Returns `{ id: "CWE-787", url: "https://cwe.mitre.org/data/definitions/787.html" }`.
- `FindingRow` renders, when non-null, a small **linked** `CWE-787` chip (`<a target="_blank" rel="noreferrer">`) next to the location. The `concern_id` continues to render as the identifier text (unchanged). When no CWE is derivable, only `concern_id` shows (as today); when neither exists, neither shows.
- The helper is pure and unit-tested (slug match, reference-URL match, none).

---

## ③ Severity badge width
File: `crates/rupu-cp/web/src/components/findings/FindingRow.tsx`. The pill currently is `...inline-flex items-center rounded px-2 py-0.5 ... uppercase ...` with no width → variable. Add static `min-w-[72px] justify-center text-center` so every severity pill is the same width with centered text. Static Tailwind only; no change to `SEVERITY_STYLE`.

---

## Components & files
**Backend**
- `crates/rupu-cp/src/api/findings.rs` *(modify)* — `FindingsQuery`, `workflow_name` on `FindingOut`, run_id→workflow_name join, filtered summary; tests.

**Frontend**
- `crates/rupu-cp/web/src/lib/cwe.ts` *(new)* — `cweFromFinding` + test.
- `crates/rupu-cp/web/src/components/findings/FindingRow.tsx` *(modify)* — CWE chip + uniform pill width.
- `crates/rupu-cp/web/src/lib/api.ts` *(modify)* — `getFindings(opts)` + `workflow_name` on `FindingOut`.
- `crates/rupu-cp/web/src/pages/ProjectDetail.tsx` *(modify)* — Findings section (by `ws_id`).
- `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx` *(modify)* — Findings section (by `workflow`).

## Data flow
```
GET /api/findings[?ws_id=&workflow=]
   gather findings (all workspaces/targets, provenance)
   → join distinct run_id → workflow_name (RunStore.load, NotFound→None)
   → filter by ws_id / workflow (if given)
   → summary over filtered set
   → { findings: (FindingOut + workflow_name)[], summary }

Findings.tsx        → getFindings()                → all
ProjectDetail.tsx   → getFindings({ wsId })        → that project
WorkflowDetail.tsx  → getFindings({ workflow })    → that workflow
FindingRow          → cweFromFinding() → linked CWE chip + concern_id; uniform pill
```

## Error handling
- Bad/missing run_id on join → `workflow_name: None` (never errors the endpoint).
- A bad workspace/target is still skipped with a warn (unchanged).
- Scoped page with no findings → empty-state note, zeroed/absent metric strip — not a broken layout.

## Testing
- Backend `cargo test -p rupu-cp`: join attaches `workflow_name`; `?ws_id=` filters to one project; `?workflow=` filters by resolved workflow; summary reflects the filtered set; the pure filter/summary logic is unit-tested without a server. `cargo clippy -p rupu-cp --all-targets` clean.
- Frontend `npm test -- --run`: `cweFromFinding` (slug, reference URL, none); existing suite green; `npm run build` strict; recharts stays out of main chunk; no `any`; static Tailwind.
- Visual validation by matt: project + workflow pages show their scoped findings; CWE chips link out; severity pills align.
