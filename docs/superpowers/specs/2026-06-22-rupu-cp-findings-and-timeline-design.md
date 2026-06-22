# rupu-cp — Findings view + diverging usage timeline + fan-out status fix — Design

**Date:** 2026-06-22
**Surface:** `crates/rupu-cp` (axum read-adapter) + `crates/rupu-cp/web` (React/TS Control Plane UI)
**Status:** approved (matt), ready for implementation plan

## Goal

Four related Control-Plane improvements, shipped as one branch:

1. **Fix** — a completed workflow run no longer shows `for_each` fan-out units as "awaiting".
2. **Rework** — the per-turn token-usage timeline renders as a **diverging** chart so output and cached tokens stop being visually swallowed by input.
3. **Add** — a global **Findings** view: a severity-ordered list of all findings across every project, backed by the durable coverage findings ledger.
4. **Add** — a small **findings metric dashboard**, reused on both the new Findings view and the existing Coverage detail UI.

## Non-goals

- No new finding *source*. Findings come from the existing `findings.jsonl` ledger (`report_finding` writes it; transcript cards are the live view of the same records). Transcript-only findings are not aggregated.
- No finding *mutation* (no ack/dismiss/resolve). The CP is a read adapter.
- No trend sparkline / by-agent panels (Okesu has them; out of scope for v1).
- No `rupu-cli` dependency added to `rupu-cp`.

---

## ① Fix: `for_each` units stuck "awaiting" on a completed run

### Root cause
`crates/rupu-cp/web/src/lib/runGraphModel.ts` derives a fan-out unit's state from its `success` field:

```ts
const unitState = cp.success === true ? 'done' : cp.success === false ? 'failed' : 'running';
```

A unit whose terminal record is missing or unmatched (`success: null` — e.g. a `UnitStarted` event with no durable checkpoint or matched `UnitCompleted`) defaults to a non-terminal state and surfaces in the UI as in-flight ("awaiting"). Nothing reconciles unit/step state against the overall **run** status, so a *completed* run can still display in-flight units.

### Design
After Phase 5 (unit folding) in `buildRunGraphModel`, add a **reconciliation pass** keyed off the run record's status:

- When `g.run.status` is terminal-success (`completed`): any unit still in `running` / `awaiting_approval` and any step still in `pending` / `running` / `awaiting_approval` is promoted to `done`. A finished, successful run cannot have in-flight work.
- Other terminal statuses (`failed` / `rejected`) and non-terminal statuses (`running`, `pending`, `awaiting_approval`) are left untouched — genuine in-flight and genuine failures must still render truthfully.

`byState` counts on each node's `fanout` are recomputed after promotion so the parent fan-out badges stay consistent.

The run status is already available to the model builder via the `RunGraphResponse` (`g.run.status`). The mapping uses the existing `StepState` union; no new states.

### Test
A `runGraphModel` unit test: build a model from a `completed` run containing a fan-out step with one `success: null` unit (no `unit_completed`) → that unit resolves to `done`, and the parent step is `done`. A companion negative test: a `failed` run with a `success: null` unit is left non-terminal (not silently marked done).

---

## ② Rework: diverging per-turn usage timeline

### Current state
`crates/rupu-cp/web/src/components/charts/RunUsageTimeline.tsx` stacks `tokens_in` / `tokens_out` / `tokens_cached` as three areas on one Y axis. Input dominates by an order of magnitude, so output and cached render as invisible slivers. Backend data (`UsageTimelinePoint { turn, label, tokens_in, tokens_out, tokens_cached }` from `GET /api/runs/:id/usage-timeline` and `/api/sessions/:id/usage-timeline`) is unchanged.

### Design — diverging chart
- **Input** — positive area above zero, left Y axis.
- **Output** — area rendered **below zero** (value negated for display) on the **same left axis**, so in-vs-out magnitudes are directly comparable across the baseline. A `ReferenceLine y={0}` draws the baseline.
- **Cached** — a line on a **secondary right Y axis** with its own scale, so sporadic/small cache values remain legible instead of dying out.
- **Tooltip** shows the *true* (absolute) values for all three — the negation is display-only. The Y-axis tick formatter shows absolute magnitudes on both halves (no negative numbers shown to the user).
- Existing `separators` (dashed step-boundary `ReferenceLine`s) are preserved; both RunDetail (with separators) and SessionDetail (without) keep working unchanged at the call site.

Colors stay on the established ramp: in `#1860f2`, out `#22c55e`, cached `#f59e0b`. recharts is already lazy-chunked; no new dependency. Y-axis negation is purely a transform in the chart data mapping (`out: -p.tokens_out`) plus an absolute tick/tooltip formatter.

### Test
`RunUsageTimeline` renders without error for a series with all three token kinds; a small data-mapping unit (the negation + abs-formatter helper) is unit-tested directly.

---

## ③ Add: global Findings view

### Backend — `GET /api/findings`
New route in `crates/rupu-cp/src/api/findings.rs` (registered in `server.rs`). Reuses the existing coverage-aggregation pattern (`discover_targets` + `read_findings`, as in `coverage.rs::list_coverage`) but emits the full records:

Response shape:
```jsonc
{
  "findings": [
    {
      // flattened FindingRecord fields
      "id": "fnd_…", "severity": "high", "summary": "…", "scope": "file",
      "file_path": "…", "line_range": [10, 20], "concern_id": "…",
      "evidence": { "rationale": "…", "code_excerpt": "…", "references": [] },
      "declared_by": { "run_id": "…", "model": "…", "surface": "…" },
      "declared_at": "…",
      // provenance injected by the endpoint:
      "ws_id": "…", "project": "…", "target_id": "…"
    }
  ],
  "summary": { "total": 42, "critical": 3, "high": 7, "medium": 12, "low": 14, "info": 6 }
}
```

- Findings are sorted server-side by severity (critical→info) then `declared_at` desc, so the client renders in order without re-sorting (it may still re-sort on filter).
- Provenance (`ws_id` / `project` / `target_id`) is attached per finding so each row can show where it came from and link back to the coverage target.
- `summary` is the per-severity histogram for the metric tiles.
- Handler unit tests cover: empty (no targets → empty list + zeroed summary), severity sort order, and summary counts. A `Query::try_from_uri` regression test is **not** needed (no query params in v1); the route takes none.

### Frontend — `/findings` page
- New nav leaf in `crates/rupu-cp/web/src/lib/sidebarNav.ts` under the **Observe** group, after Coverage: `{ to: '/findings', label: 'Findings', icon: ShieldAlert }`.
- New lazy route in `App.tsx` → `pages/Findings.tsx`.
- New typed client method in `lib/api.ts`: `getFindings(): Promise<FindingsResponse>` with `FindingsResponse { findings: FindingRow[]; summary: FindingsSummary }`. `FindingRow` extends the existing wire `FindingRecord` with `ws_id` / `project` / `target_id`.
- Page layout: `FindingMetrics` tile strip on top, then the severity-sorted list using the shared `FindingRow` component. Each row shows severity badge · summary · location (`file:line0–line1`) · concern chip · `[project · target]` provenance chip · expandable evidence. Clicking a metric tile filters the list to that severity (clear by clicking again / an "All" tile).

---

## ④ Add: findings metric dashboard + coverage reuse

### Shared components (lifted out of `CoverageDetail.tsx`)
- `crates/rupu-cp/web/src/components/findings/FindingRow.tsx` — the existing Okesu-style row (severity pill, summary, location, concern chip, collapsible evidence), generalized to accept an optional provenance chip (`project` / `target_id`). `CoverageDetail` imports it instead of its local copy.
- `crates/rupu-cp/web/src/components/findings/FindingMetrics.tsx` — a tile strip: **Total · Critical · High · Medium · Low · Info**, each tile using the `SEVERITY_STYLE` ramp accent. Props: `summary: FindingsSummary`, optional `active?: Severity | null`, optional `onSelect?: (sev: Severity | null) => void` (when provided, tiles become click-to-filter; when omitted, tiles are static display). Built with static Tailwind only (severity accent via the existing `SEVERITY_STYLE` classes — no dynamic class strings).

### Coverage reuse
- `CoverageDetail.tsx` renders `<FindingMetrics summary={…} />` above its findings section (computing the per-severity summary from the target's findings it already loads) and uses the shared `<FindingRow />`. This keeps the two surfaces visually identical and removes the duplicated row.
- `Coverage.tsx` (list) is unchanged beyond benefiting from the shared severity styling already in use.

---

## Components & files

**Backend (`crates/rupu-cp/src/`)**
- `api/findings.rs` *(new)* — `GET /api/findings` aggregate + summary; unit tests.
- `api/mod.rs` / `server.rs` *(modify)* — register `findings::routes()`.

**Frontend (`crates/rupu-cp/web/src/`)**
- `lib/runGraphModel.ts` *(modify)* — terminal-run reconciliation pass + test.
- `components/charts/RunUsageTimeline.tsx` *(modify)* — diverging render + abs formatter helper + test.
- `components/findings/FindingRow.tsx` *(new, lifted from CoverageDetail)*.
- `components/findings/FindingMetrics.tsx` *(new)*.
- `pages/Findings.tsx` *(new)*.
- `pages/CoverageDetail.tsx` *(modify)* — use shared FindingRow + FindingMetrics.
- `lib/api.ts` *(modify)* — `getFindings` + `FindingsResponse` / `FindingsSummary` / `FindingRow` types.
- `lib/sidebarNav.ts` *(modify)* — Findings nav leaf.
- `App.tsx` *(modify)* — `/findings` lazy route.

## Data flow

```
findings.jsonl (per target)  ──read_findings──┐
discover_targets(workspace)  ──────────────────┤→ GET /api/findings → { findings[], summary }
                                               │        │
                                               │        ├→ Findings.tsx: FindingMetrics + FindingRow[]
                                               │        └→ (click tile → client-side severity filter)
CoverageDetail (per-target findings, existing) ┴────────→ FindingMetrics + FindingRow (shared)

usage-timeline endpoints (existing) → RunUsageTimeline (diverging: in↑ / out↓ / cached on right axis)
RunGraphResponse (existing) → buildRunGraphModel (+ terminal-run reconciliation) → Graph view
```

## Error handling
- `GET /api/findings`: an unreadable/absent `findings.jsonl` for a target is skipped with a `tracing::warn!` (matches `read_findings`'s tolerant behavior); the endpoint never 500s on a single bad target. No targets at all → `{ findings: [], summary: { total:0, … } }` (200).
- Frontend: `getFindings` failure → the page shows the standard error state (same pattern as Coverage); an empty result shows an empty-state message, not a blank dashboard.

## Testing
- Backend: `cargo test -p rupu-cp` (new `findings` handler tests: empty, sort, summary). `cargo clippy -p rupu-cp --all-targets` clean.
- Frontend: `npm run build` (strict, exit 0), `npm test -- --run` green incl. new `runGraphModel` reconciliation test, the timeline abs-formatter test, and a `FindingMetrics` render test. No `any`; static Tailwind; main `index-*.js` chunk stays lean (no new deps; `grep -c recharts dist/assets/index-*.js` = 0).
- Visual validation by matt before merge (GUI rendering is the non-automatable gate): completed run shows no awaiting units; timeline diverges with cached legible; Findings page lists severity-ordered with working tiles; Coverage detail matches.
