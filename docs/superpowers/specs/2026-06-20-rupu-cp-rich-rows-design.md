# rupu Control Plane — Rich list rows + usage graphs (Slice E) — Design

**Date:** 2026-06-20
**Author:** matt + Claude
**Status:** Design approved (visuals validated via the brainstorm companion)
**Builds on:** Slices A.3 (cost/tokens, #325) + A.4 (per-row usage + pagination, #326), merged to `main`.

## Summary

The CP list views are thin one-liners. matt wants each row to feel like "a proper row with information" — tokens consumed (in/out/cached), cost, duration, turns — and wants a **usage graph** derived from the transcript timelines. Validated visually (companion):

1. **Rows → a shared "metric strip" row** (mock B): identity + chips + status on top, a labeled stat strip beneath (in / out / cached / cost / duration / turns). One reusable component replaces the ~10 hand-rolled rows.
2. **One general graph per list** (mock A): a stacked bar per loaded run (in/out/cached tokens), sitting atop each list, bars aligned to the rows, hover → run + cost. **NOT** per-row.
3. **Per-turn graph on the run-detail page** (top): tokens in/out/cached across turns 0→N, animating live while the run is active.

## Key data facts (from the transcript + DTO investigation)

- **Per-row `usage`** (in/out/cached/cost) already exists on `RunListRow`, `AgentRunRow`, `AutoflowCycleRow`, `AutoflowEventRow`, `SessionSummary` (A.4). → the metric strip's token/cost columns and the **general bar graph are pure frontend** from already-loaded data.
- **Transcripts record tokens PER TURN, not per second.** `Event::TurnEnd { turn_idx, tokens_in, tokens_out }` + `Event::Usage { input/output/cached_tokens }` are emitted once per turn; there are **no per-event timestamps** (only `RunStart.started_at` + `RunComplete.duration_ms`). → the run-detail timeline's x-axis is **turn index**, and it's built from the transcript events the page already fetches/streams (live via the A.2 SSE tail). **No backend** for the timeline.
- **Duration** is computable from `started_at`/`finished_at` on `RunListRow` / `AutoflowCycleRow`, and `created_at`/`updated_at` on sessions — frontend-side. `AgentRunRow` has `started_at` but no `finished_at`.
- **Turns**: `SessionSummary.total_turns` exists; run rows have no turn count. → one small backend addition (count turns during the per-run transcript pass that already computes usage).
- **No shared Row component** — every page hand-rolls its row. → extracting one is a real DRY win.
- **Aggregate areas (projects / agents / workflows)** list rows have **no** usage/run-count today → showing rich rows + the bar graph there needs real per-entity **rollups** (heavier). This is why the work phases.

## Architecture

### Three reusable frontend units (`crates/rupu-cp/web/src/`)

1. **`components/lists/MetricRow.tsx`** — the shared row. Props: a `header` (identity node — name/id/chips + status), an array of `metrics` (`{ label, value }`), and an optional `to` (link) + `onClick`. Renders the B layout: header line + a labeled stat strip (`tabular-nums`, the existing `text-ink`/`ink-dim`/`ink-mute` tokens). A small `formatDuration(ms|start,end)` helper in `lib/`. The metric strip degrades gracefully — a metric with a `null` value is omitted (so rows missing duration/turns just show fewer stats). Replaces the per-page row markup incrementally.
2. **`components/charts/UsageBarChart.tsx`** — the general graph. Input: the loaded list rows' `usage` (`{ id, label, input_tokens, output_tokens, cached_tokens, cost_usd }[]`). Renders a recharts stacked `BarChart` (in #1860f2 / out #22c55e / cached #f59e0b), newest→oldest, hover tooltip → label + tokens + cost, click → the row's link. Empty/zero → a tidy empty state. Pure presentational; recompute on the loaded set (re-renders as pagination appends).
3. **`components/charts/RunUsageTimeline.tsx`** — the per-turn graph for run-detail. Input: a `TurnUsagePoint[]` (`{ turn, tokens_in, tokens_out, tokens_cached }`) derived **frontend-side** from the transcript events (a `buildTurnSeries(events)` helper in `components/transcript/`). Renders a stacked area/line over turn index. On a live run it grows as `turn_end`/`usage` events arrive on the existing SSE stream.

recharts is already a dep and split into its own lazy `charts` chunk (vite manualChunks) — adding these to list/detail routes keeps the **main entry ~48 KB**; recharts loads on-demand per route.

### Backend (minimal, Phase 1)

- **`turns` on the run-row DTOs.** Extend the per-run computation (the `crate::usage` path already reads each run's transcripts for usage) to also return a turn count. Add `turns: u64` to `RunListRow` and `AgentRunRow`, populated on the paginated page only (consistent with A.4's slice-before-usage). Sessions already carry `total_turns`.
- **`AgentRunRow` duration.** Add `duration_ms: Option<u64>` from the transcript's `RunComplete.duration_ms` (read in the same per-run pass), since agent runs lack `finished_at`. Other rows compute duration from timestamps frontend-side.
- A small `RunMetrics { usage, turns, duration_ms }` returned by one combined transcript pass keeps it DRY and avoids a second read.

### Backend (Phase 2 — aggregate areas)

Per-entity rollups for the list rows that have none today, computed on the paginated page:
- **Projects** (`ProjectRow`): `usage` rollup + `run_count` + `last_active` (sum/scan the project's runs).
- **Agents** (`AgentSummary`): `run_count` + `usage` rollup across runs by that agent (the `/api/usage` breakdown by agent from A.3 is a building block).
- **Workflows** (`WorkflowSummary`): `run_count` + `usage` rollup + `last_run` across runs of that workflow.

These are heavier (many transcript reads per row); pagination (20) bounds them. Caching is a future optimization (out of scope) — if a rollup is measurably slow, `log()` the cost rather than silently truncating.

## Data flow

- **List page:** fetch page (rows carry `usage` + new `turns`/`duration_ms`) → `UsageBarChart` renders from the rows → each row renders via `MetricRow` from its `usage` + timestamps + `turns`. Pagination append (A.4) feeds both.
- **Run detail:** the page already fetches the transcript (and SSE-tails a live one) → `buildTurnSeries(events)` → `RunUsageTimeline` at the top. No new endpoint.

## Components & boundaries

| Unit | Responsibility | Depends on |
|---|---|---|
| `MetricRow` | shared row layout (header + stat strip) | `lib/usage` formatters, `formatDuration` |
| `UsageBarChart` | per-run stacked-bar list graph | recharts, the rows' `usage` |
| `RunUsageTimeline` | per-turn in/out/cached timeline | recharts, `buildTurnSeries` |
| `buildTurnSeries` | events → per-turn points (pure) | transcript event types |
| `crate::usage` (extended) | `RunMetrics` (usage + turns + duration) in one pass | `rupu_transcript` |
| Phase-2 rollup handlers | per-entity usage/run-count | `crate::usage`, `RunStore` |

## Error handling

- **Missing metric** (no duration / no turns) → the stat is omitted from the strip; never show a fake `0`/`—` where the value is genuinely unknown vs. genuinely zero (zero tokens is a real `0`; absent duration is omitted).
- **Empty/short transcript** → `buildTurnSeries` yields `[]`; the timeline shows an empty state; `RunMetrics.turns = 0`.
- **Unreadable transcript** → existing tolerance (skip) carries through; usage/turns default.
- **Live timeline** → reuses the existing SSE error handling (the panel already degrades on stream error).

## Testing

- **Frontend:** `MetricRow` renders header + the provided metrics, omits null ones; `UsageBarChart` renders a bar per row + an empty state for zero; `buildTurnSeries` (pure) maps `turn_end`/`usage` events → ordered per-turn points (and coalesces the paired events of one turn); `formatDuration` edge cases. `npm run build` strict + `npm test -- --run`; main chunk ~48 KB (recharts lazy, `grep -c recharts` in main = 0).
- **Backend:** `RunMetrics` turn-count + duration from a fixture transcript; the row DTOs serialize `turns`/`duration_ms`; Phase-2 rollups sum per entity. `cargo test -p rupu-cp` + `clippy --all-targets`.
- Rendering validated by matt (the graphs especially).

## Scope & non-goals

- **In:** the shared `MetricRow`, the general `UsageBarChart` atop each list, the run-detail `RunUsageTimeline`, the small run-row metric additions (Phase 1), and the per-entity rollups for projects/agents/workflows (Phase 2).
- **Out (YAGNI):** wall-clock per-turn timing (data doesn't exist — x-axis is turn index); per-step sub-graphs; virtualized chart rendering; rollup caching (until measured slow); changing the pagination model (A.4 stands).

## Decomposition — one spec, two plans (each its own PR, merged in order)

1. **Plan 1 — Runs & Sessions (the fast, high-impact win):** `MetricRow` + `UsageBarChart` + `RunUsageTimeline` + `buildTurnSeries` + the `RunMetrics` backend addition (`turns`/`duration_ms` on run/agent-run rows); wire the Workflow/Agent/Autoflow run lists, Sessions, Project runs/sessions, and the run-detail timeline. Almost all data already exists.
2. **Plan 2 — Aggregate areas:** backend per-entity rollups (usage + run-count + last-active) for Projects / Agents / Workflows list rows; apply `MetricRow` + `UsageBarChart` there.
