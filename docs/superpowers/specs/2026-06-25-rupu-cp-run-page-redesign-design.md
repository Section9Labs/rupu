# rupu-cp — Run detail page redesign (always-on graph + scoped tabs + for_each file-browser) — Design

**Date:** 2026-06-25
**Surface:** `crates/rupu-cp` (one backend fix) + `crates/rupu-cp/web` (the redesign)
**Status:** approved by matt (design), pending spec review

## Goal
Redesign the workflow **run detail** page (`/runs/:id`, `RunDetail.tsx`) per matt's direction:
1. **Keep the current graph** (xyflow) exactly as-is — do NOT redesign it.
2. Make the **graph + the "Token usage by turn" chart persistent chrome** (always visible at the top).
3. Below them, a **tab panel (Transcript · Events · Findings)** that **follows the selected step** in the graph.
4. Replace the right-side `for_each` slide-over (`FanoutDrill`) with a **file-browser**: the fan-out's units listed on the **left**, the selected unit's **transcript on the right**.
5. Fix the bug where the Findings tab is empty even though the transcript shows findings.

## The bug fix (findings empty on the run page)
Two layered causes (confirmed in code):
- **Primary:** `report_finding` only persists to the durable `findings.jsonl` ledger when the run executes under a coverage target (a workflow with a `concerns:` block). A run without `concerns:` shows a finding *card* in the transcript (parsed from the tool call) but writes nothing the Findings tab can read. **This is expected** — a non-coverage run genuinely produced no durable findings. (Possible future enhancement, NOT in this slice: fall back to transcript-parsed findings when the ledger is empty. Flagged, not built.)
- **Operative cause for assessor-style runs:** findings reported *inside a `for_each` unit* are attributed to that **unit's sub-run id**, not the parent run id. The per-run Findings filter (`GET /api/findings?run_id=<id>`) matches `declared_by.run_id == id` exactly, so for_each-unit findings are dropped from the parent run's view.

**Fix:** `GET /api/findings?run_id=<id>` matches findings whose `declared_by.run_id` is the run id **OR any of that run's fan-out unit sub-run ids**. The backend resolves the sub-run id set by reading the run's unit checkpoints (each `UnitCheckpoint` carries its own `run_id`). So "a run's findings" = parent-run findings ∪ all its fan-out units' findings. The per-run Findings tab (and the run page) then populate correctly.

## Layout

```
┌─ HEADER: workflow name · status · run id · started/finished · usage row · awaiting banner ─┐
├────────────────────────────────────────────────────────────────────────────────────────────┤
│  GRAPH  (current xyflow RunGraph, UNCHANGED)                         ← ALWAYS shown          │
├────────────────────────────────────────────────────────────────────────────────────────────┤
│  Token usage by turn  (RunUsageTimeline)                            ← ALWAYS shown           │
├────────────────────────────────────────────────────────────────────────────────────────────┤
│  selected: <step or "whole run">      [ Transcript ] [ Events ] [ Findings ⑫ ]   ← scoped    │
│  ┌──────────────────────────────────────────────────────────────────────────────────────┐  │
│  │  <active tab body, scoped to the selection>                                           │  │
│  └──────────────────────────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Persistent chrome (always visible)
- The existing header (unchanged).
- The existing **`RunGraph`** (xyflow), unchanged — clicking a node sets the selection.
- The existing **`RunUsageTimeline`** ("Token usage by turn"), unchanged, moved to always-visible (it already is in the header today; keep it visible above/with the tab panel).

### Selection model
A single selection cursor: `selected = { stepId } | { stepId, unitIndex } | null`. The graph drives it (click a step → `{stepId}`; click/expand a for_each unit → `{stepId, unitIndex}`; nothing selected → `null` = whole run). The tab panel reads the cursor. This unifies today's separate `sel` + `drillStepId` state.

### Tab panel — Transcript · Events · Findings (below the chrome)
- **Transcript** — *follows the selection*:
  - normal step selected → that step's transcript (`TranscriptPanel path=stepResult.transcript_path`).
  - `for_each` step selected → the **file-browser** (see below).
  - nothing selected → empty-state: "Select a step in the graph to view its transcript."
- **Events** — *follows the selection*: the SSE event feed filtered to the selected step's `step_id` (including its `unit_*` events); nothing selected → the full run event feed. (Client-side filter over the already-subscribed stream; no backend change.)
- **Findings** — **run-wide** (always): the run's complete findings (parent run ∪ fan-out sub-runs, via the bug fix), severity-ordered `FindingMetrics` + `FindingRow` list, each row carrying a provenance chip (step/unit it came from). NOT scoped to the selected step — a finding doesn't record which step emitted it (top-level findings all share the parent run id), so per-step findings scoping isn't cleanly possible; run-wide with provenance is the honest behavior. The tab badge shows the run's finding count.

### The `for_each` file-browser (replaces `FanoutDrill`)
When the selected step is a `for_each`, the **Transcript tab** renders a two-column master-detail:

```
┌─ scan-deps · 40 units ───────────────────────────────────────────────────┐
│ [ all 40 ][ running 4 ][ done 34 ][ failed 2 ]   ← state filter pills      │
├──────────────────────────┬────────────────────────────────────────────────┤
│ UNITS (left, ~34%)       │  TRANSCRIPT (right, ~66%)                       │
│ ▸ #12 users.rb     ✓     │  [transcript of the selected unit, live]        │
│ ▸ #14 auth.rs      ● sel │                                                 │
│ ▸ #15 tokio        ●     │                                                 │
│   …all 40, scroll        │                                                 │
└──────────────────────────┴────────────────────────────────────────────────┘
```

- **Left column:** the fan-out's units (`model.nodeById(stepId).fanout.units`), each row = state glyph + unit `key` + state label; reuse `FanoutDrill`'s filter pills (all/running/done/failed with counts) and `STATE_STYLE` glyphs + the `ROW_CAP`/"+N more" cap. Clicking a unit sets `selected = { stepId, unitIndex }`.
- **Right column:** `TranscriptPanel` for the selected unit's `transcriptPath` (live). Empty-state until a unit is picked (auto-select the first unit is acceptable).
- `FanoutDrill.tsx` (the fixed right slide-over) is **deleted**; its pill/list/glyph logic is reused inside the file-browser's left column.

## Components & files
**Backend:**
- `crates/rupu-cp/src/api/findings.rs` *(modify)* — `?run_id=` matches the run id ∪ its fan-out unit sub-run ids (resolve the sub-run set from the run's unit checkpoints via `RunStore`); summary over the matched set. Tests.

**Frontend:**
- `crates/rupu-cp/web/src/pages/RunDetail.tsx` *(rewrite)* — persistent graph + usage chart chrome; the unified selection cursor; the Transcript/Events/Findings tab panel scoped to the selection; mount the file-browser for for_each. Remove the old Graph/Events/Findings TabBar-of-the-whole-page and the `FanoutDrill` overlay.
- `crates/rupu-cp/web/src/components/FanoutDrill.tsx` *(delete)* — logic absorbed into the file-browser.
- `crates/rupu-cp/web/src/components/run/StepTranscriptBrowser.tsx` *(new)* — the for_each file-browser (units left / transcript right) with the state-filter pills.
- `crates/rupu-cp/web/src/components/run/StepEventsTab.tsx` *(new, or inline)* — the event feed filtered by selected step_id (reuse `RunEventFeed`/`RunEventFeed`-equivalent with a step filter).
- The Findings tab reuses `FindingMetrics` + `FindingRow` (with provenance chips, already supported).

## Data flow
```
getRunGraph(id) ─ graph model (steps, for_each units w/ sub-run ids + transcript paths)
subscribeRunLog(id) ─ SSE events (filtered client-side by selected step_id for the Events tab)
getRunUsageTimeline(id) ─ the always-on usage chart
getFindings({ runId: id }) ─ run-wide findings (parent ∪ fan-out sub-runs, via the bug fix)

selection cursor (from graph click):
  null            → Transcript: prompt · Events: all run events · Findings: run-wide
  { stepId }      → Transcript: step transcript (or file-browser if for_each) · Events: step's events · Findings: run-wide
  { stepId, unit }→ Transcript: that unit's transcript (file-browser right pane) · Events: step's events · Findings: run-wide
```

## Error / empty / loading
- Findings empty (non-coverage run) → "No findings recorded for this run." (with a hint that findings require a coverage target) — not a blank tab.
- Transcript with no selection → the "select a step" prompt.
- Each tab keeps its own loading/error state; the persistent graph + usage chart render independent of the tab.

## Testing
- Backend: `cargo test -p rupu-cp` — `?run_id=` returns parent + fan-out-unit findings; a finding under a unit sub-run id is included for the parent run; summary covers the union. clippy clean.
- Frontend: `npm test -- --run` — selection drives the tab panel (select a step → Transcript shows its transcript; select a for_each → file-browser appears; Events filters by step); the file-browser unit-click swaps the right transcript; Findings renders run-wide. Existing suite green; `npm run build` strict; recharts out of main chunk; no `any`; static Tailwind.
- Visual validation by matt: graph + usage always visible; tabs follow the selected step; for_each opens the left/right file-browser (no slide-over); findings populate for a coverage/assessor run.

## Non-goals
- No change to the graph component itself.
- No transcript-parsed findings fallback for non-coverage runs (flagged as a possible follow-up).
- No per-normal-step findings scoping (data doesn't support it; Findings is run-wide).
