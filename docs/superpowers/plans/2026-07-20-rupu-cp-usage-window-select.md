# Usage: arbitrary date-window select (drag on graph) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Let the operator drag-select a section of the usage graph to set an arbitrary `{since, until}` date window — "7/30/All but any dates." The whole page (graph, breakdown table, headline, outliers) then reflects that window. Reused on `/usage` and the Projects graph via the shared `UsageTimeline` component.

**Design (approved 2026-07-20):** The page's range becomes a **window** `{since, until}` instead of a 3-value preset. 7/30/All buttons set presets ending "now"; a drag-select on the chart sets a custom window. Everything already funnels through a server-side since/until window — the client just picks a custom one and refetches (fast, local endpoints). One code path: a window is a window.

**Tech Stack:** Rust (axum, chrono), React + TS + Recharts (no new deps), Vitest.

## Global Constraints

- Workspace deps only; NEVER a version literal in a crate Cargo.toml. rupu-cp READ-ONLY. `#![deny(clippy::all)]`; `unsafe_code` forbidden. `thiserror` for libs.
- **NEVER run `cargo fmt`.** `rustfmt --edition 2021 <path>` one file at a time; never a crate root or `mod.rs`. `git diff --stat` after; revert stray files.
- **NO new npm deps.** **NEVER hardcode a color literal** — `useThemeColors()` / `--c-*` tokens only.
- Reports MUST paste literal command output (this project has had a subagent claim a command clean without running it).
- rustc resolves 1.95 vs pinned 1.88. Implementer FIRST records the `cargo clippy -p rupu-cp --all-targets` error count at the task base, then verifies unchanged.
- Do NOT `git push` / `git checkout <commit>` / detach HEAD / `git stash` (push.default=matching; controller pushes with an explicit refspec).

---

### Task W1: add `until` to the two since-only usage endpoints

**Files:** `crates/rupu-cp/src/api/usage.rs` (`UsageRunsQuery` + `get_usage_runs`), `crates/rupu-cp/src/api/usage_outliers.rs` (`OutliersQuery` + handler + `resolve_since`), tests.

`/api/usage` and `/api/usage/timeline` already accept `since`+`until` via `resolve_window`. Only `/api/usage/runs` and `/api/usage/outliers` are since-only. Add `until`:
- **`UsageRunsQuery`**: add `until: Option<String>`. Replace the since-only bounding with `resolve_window(since, until, now)` (the helper already exists in `usage.rs`) and keep only runs whose `started_at` is within `[start, end]`.
- **`OutliersQuery`**: add `until: Option<String>`. Change `resolve_since` → a window bound (reuse/mirror `resolve_window`; bound the runs feeding `find_outliers` to `[since, until]`). NOTE the baseline is a per-workflow median over the windowed runs — narrowing the window correctly changes which runs are outliers; that is intended.
- Absent `until` → `now` (same default the other endpoints use). Present-but-unparseable `until` → 400 (mirror the existing `since`/`resolve_window` error handling).

- [ ] **Step 1: record clippy baseline** (`cargo clippy -p rupu-cp --all-targets -- -D warnings 2>&1 | grep -cE '^error'` → N).
- [ ] **Step 2: failing tests** in `tests/usage.rs`: `GET /api/usage/runs?since=&until=` excludes a run outside the window (present but before `since`, and one after `until`); `GET /api/usage/outliers?until=` excludes an outlier outside the window; unparseable `until` → 400.
- [ ] **Step 3: fail → Step 4: implement → Step 5: verify** (`cargo test -p rupu-cp usage outlier`; `cargo test -p rupu-cp` no new failures; clippy still N). Manual: curl `/api/usage/runs?until=<a-past-iso>` and confirm fewer rows than unbounded. → **Step 6: commit.**

---

### Task W2: web — range becomes a `{since, until}` window

**Files:** `crates/rupu-cp/web/src/lib/api.ts` (client methods + a window type), the pages/components that hold range state (`src/pages/Usage.tsx`, `src/components/usage/UsageTimeline.tsx`, `src/components/project/ProjectUsageTimeline.tsx`), tests.

Generalize the range plumbing so a custom window is possible. Presets keep working unchanged visually.
- Introduce `interface UsageWindow { since: string; until: string }` (RFC-3339). Add a helper `presetWindow(range: DashboardRange, now?): UsageWindow` — `since = usageRangeSince(range)`, `until = new Date(now).toISOString()` (for `'all'`, `until` = now, `since` = epoch, unchanged).
- Update the client methods to accept a window (or `since`+`until`) and send BOTH params: `getUsage`, `getUsageRuns`, `getUsageOutliers` (and pass `until` to `getUsageTimeline` if that path is still used). Keep a backward-compatible overload or migrate all call sites — your call, but every usage fetch must send `until`.
- The pages/components hold `window: UsageWindow` state instead of (or alongside) the preset. The 7/30/All buttons set `presetWindow(preset)`. This task does NOT add the drag UI (W3 does) — it just makes the window the source of truth and confirms presets still drive the whole page correctly through it.

- [ ] **Steps:** failing tests (a client method sends both `since` and `until`; `presetWindow('7d')` yields a since ~7 days before until; pages fetch with a window) → fail → implement → pass → `npx vitest run` green, `npx tsc --noEmit` clean, `npm run build` succeeds → commit.

---

### Task W3: drag-select a window on the graph

**Files:** `src/components/dashboard/UsageTimelineStacked.tsx` (the drag interaction), the shared `src/components/usage/UsageTimeline.tsx` + `src/components/project/ProjectUsageTimeline.tsx` (wire the selection → window), tests.

Add marquee selection to the timeline chart and wire it to the window state so BOTH `/usage` and the project graph get it (do it in the shared path, not per-page).
- **In `UsageTimelineStacked`:** add Recharts mouse handlers on the chart — `onMouseDown` captures the start day-bucket (`e.activeLabel`), `onMouseMove` (while dragging) updates a `[startLabel, currentLabel]` band drawn with a `<ReferenceArea x1= x2=>` using a themed translucent fill (`useThemeColors()` alpha — NO color literal), `onMouseUp` finalizes and calls a new prop `onSelectRange?(startDay: string, endDay: string)`. Handle: drag direction (start may be after end → order them), a click with no drag (start===end → treat as no selection / ignore, do NOT collapse the window to one day by accident), and pointer-up outside the plot. Keep the graph fully functional when `onSelectRange` is not provided (the dashboard's other consumers of this component must be unaffected — the handlers are inert without the prop).
- **In the shared component (`UsageTimeline`) / its parents:** pass `onSelectRange`; convert the two day-bucket strings to a `{since, until}` window (start of `startDay` → end of `endDay`, RFC-3339) and set the window state (W2) → the page refetches for that window, exactly like a preset. Add a visible **"custom range · × clear"** affordance near the range buttons that appears when a custom window is active; clicking a preset or the × returns to that preset's window.
- Optional polish (only if it doesn't complicate): narrow the already-loaded graph client-side instantly on mouseup while the refetch lands. Not required.

- [ ] **Steps:** failing tests (dragging from day A to day B calls `onSelectRange('A','B')`; a reversed drag orders them; the shared component converts a selection to a window and the "clear" restores the preset; a consumer WITHOUT `onSelectRange` still renders and is unaffected) → fail → implement → pass → `npx vitest run` green, `npx tsc --noEmit` clean, `npm run build` succeeds → commit. **Controller browser-validates the drag on both pages.**

---

## Definition of Done
- `/api/usage/runs` and `/api/usage/outliers` accept `until`; unparseable → 400; windowing verified on real data.
- `/usage` and the Projects graph: drag-selecting a section sets an arbitrary date window that narrows the WHOLE view (graph + table + headline + outliers), consistent with the 7/30/All presets; a "clear" returns to the preset.
- The drag interaction is inert for the dashboard's other `UsageTimelineStacked` consumers (no `onSelectRange`).
- Backend green + clippy at recorded baseline; web `vitest` + `tsc --noEmit` + `npm run build` all clean; no new deps; no color literals.
- Controller browser-validates the drag-select on both pages, light + dark.
