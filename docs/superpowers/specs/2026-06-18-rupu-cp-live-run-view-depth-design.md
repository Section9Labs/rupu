# rupu Control Plane — Live Run View depth (Phase 1.5) — Design

**Date:** 2026-06-18
**Author:** matt + Claude
**Status:** Design draft (visual design validated via brainstorm companion)
**Builds on:** `docs/superpowers/specs/2026-06-18-rupu-control-plane-design.md` (Phase 1 Observe, PR #319). Stacks on the `rupu-cp` crate + `crates/rupu-cp/web` app.

## Summary

Phase 1 shipped an Observe Control Plane whose Runs view is shallow: it lists only orchestrator workflow runs, and the run graph paints only steps that have *already produced a result* (no pending/awaiting states, flat linear chain, no fan-out/loop structure, minimal motion). This slice deepens the live run view into a professional workflow-run graph and broadens what "Runs" surfaces — addressing four pieces of operator feedback:

1. **Broaden the activity surface.** Distinct sidebar destinations per activity domain (Runs / Autoflows / Sessions / Workflows), not just workflow runs under one "Runs" page.
2. **Show every step, always, with correct per-step state** (pending / running / awaiting / done / failed / skipped) — not only completed steps.
3. **Tasteful live animation** of the active frontier (running node + inbound edge).
4. **Render step *types* distinctly** — `for_each` (data fan-out), `parallel` (named sub-steps), `panel + gate` fix-loops, and approval gates each get their own visual treatment.

The design language was researched against Dagster, Prefect, Temporal, Argo Workflows, Airflow, GitHub Actions, n8n, Kestra, and Conductor, and the React Flow (xyflow) layout/animation ecosystem, then validated visually with matt.

**Out of scope (later phases):** any *control* action (approve/reject/cancel/dispatch/send-input) — that is CP Phase 2; this slice stays read-only/Observe. Multi-host, RBAC, and the authoring editor remain Phases 3–4.

---

## Part 1 — Activity surface (sidebar IA)

### Decision
Per-domain sidebar destinations rather than one merged feed, with unified-feed-style filtering *within* each destination. Revised nav:

```
Dashboard
── Observe ──
  Runs          (all orchestrator runs; filter by trigger: manual / cron / event)
  Autoflows     (NEW — autoflow cycle history; each cycle links to its run)
  Live Events
  Coverage
── Build ──
  Workflows     (definitions)
  Agents
── Run ──
  Sessions
  Workers
Settings
```

### Rationale & data sources
- **Runs** stays backed by `RunStore` (orchestrator `RunRecord`s). `RunRecord` already carries the trigger payload (`event: Option<serde_json::Value>`); the list gains **trigger-type filter chips** (manual / cron / event) so triggered (autoflow/webhook/cron) runs are visible and distinguishable instead of unlabeled. No new store.
- **Autoflows** (new) is backed by `rupu_runtime::autoflow_history::AutoflowHistoryStore` (`AutoflowCycleRecord` / `AutoflowCycleEvent`, each carrying an optional `run_id`). This is the higher-level lens — *which autoflow definition fired, when, and how each cycle resolved* — cross-linking into the corresponding Run. A new read endpoint surfaces it.
- **Sessions / Workflows / Workers** already exist as Phase-1 pages; only nav grouping changes.

### Explicitly deferred
One-shot `rupu run` agent invocations are **not** persisted as `RunRecord`s (the `RunStore` is orchestrator-only); they produce transcripts under the session/transcript stores. Surfacing them as "agent runs" would require a new persistence path and is **out of scope** here — noted so the absence is intentional, not an oversight. Sessions already cover persistent agent activity.

---

## Part 2 — The run graph (centerpiece)

A left→right DAG that always shows the full workflow structure with live per-step state.

### 2.1 Graph data model
The graph is assembled from four layers, merged client-side into a `RunGraphModel`:

1. **Skeleton (all steps, always):** the run's **own persisted `workflow.yaml`** (`RunStore` writes `run_dir/workflow.yaml` at `create`). Parsing it yields every step and its *kind* — `step` / `for_each` / `parallel` / `panel`(+`gate`) — even for ad-hoc or cloned workflows whose definition isn't in the global store. This is the fix for "only completed steps show": the skeleton enumerates pending steps that have produced no result yet.
2. **Terminal step state:** `step_results.jsonl` (`StepResultRecord` — `success`/`skipped`/`finished_at`/`kind`) overlays done/failed/skipped onto skeleton steps.
3. **Per-unit fan-out state:** `unit_checkpoints.jsonl` (`UnitCheckpoint`) provides per-`for_each`-unit terminal state for completed/resumed runs.
4. **Live overlay:** the existing SSE event stream (`StepStarted`/`StepWorking`/`StepAwaitingApproval`/`StepCompleted`/`StepFailed`/`StepSkipped`/`UnitStarted`/`UnitCompleted`/`RunCompleted`/`RunFailed`) drives running/awaiting transitions and per-unit progress in real time.

Merge precedence per node: **live SSE > checkpoints/results > skeleton-default(pending)**. A completed run (no SSE) renders correctly from skeleton ⊕ results ⊕ checkpoints; an in-flight run layers SSE on top.

### 2.2 State model
Finite state set with **color + shape** (so state reads without relying on color — GitHub Actions' accessibility win):

| State | Color (Prefect-derived) | Glyph |
|---|---|---|
| pending | `#cbd5e1` slate | `•` |
| running | `#1860f2` blue (distinct from done) | `⟳` (spinner) |
| awaiting (gate/approval) | `#f59e0b` amber | `⏸` |
| done | `#2ac769` green | `✓` |
| failed | `#fb4e4e` red | `✕` |
| skipped / upstream-skipped | muted grey | `⤼` |

Every node paints from its **own** state, so pending + running + awaiting + done + failed coexist on one canvas. Failed (red) stays distinct from skipped/upstream (grey) — the single most useful run-diagnosis distinction (Airflow/GitHub).

### 2.3 Node types
1. **Step node** — rounded-rect card: leading status glyph · name · duration · agent chip. Hover/click → the step's transcript (reuse the existing Phase-1 transcript drill-down).
2. **`parallel` container** — bordered box (brand-tinted) with a header (`parallel · <name>`, aggregate roll-up `k/n`) wrapping the named sub-step cards side-by-side (shared layout rank).
3. **`for_each` fan-out** — collapses on size:
   - **Small N (≤ threshold, default 12):** the unit squares render inline as a grid, each one node colored by its own state; click a unit → its transcript.
   - **Large N:** a collapsed card leading with **`X / N` + a single % progress bar**, with **failed broken out in red** (`X / N` · `2 failed`) so failures never hide behind a green bar. A density grid previews the units; **"expand" opens a virtualized, filterable drill-in list** (by state) where each row is one unit → its transcript (Airflow `map_index` drill-in pattern). Threshold is tunable.
4. **`panel + gate` fix-loop** — the panel renders as a container of panelist cards; the **gate** renders as a decision node (`until_no_findings_at_severity_or_above`, `max_iterations`). On "findings remain," an **animated back-edge** routes through the fixer and re-enters the panel; the panel header shows a **round counter (`2/5`)**. Exits green on `clear`, red if it exhausts `max_iterations` still dirty.
5. **Approval gate** — a first-class **awaiting** node (amber, dashed inbound edge). Approve/Reject buttons render **display-only** here (the mutation is CP Phase 2); the node shows the awaiting reason / `approval_prompt`.

### 2.4 Layout
**dagre, `rankdir: 'LR'`** (synchronous, small, xyflow's own recommendation; matches the L→R convention of Dagster/Airflow/GitHub Actions/n8n). Layout computed off the render path; cached by a hash of the workflow structure so re-renders during live updates don't relayout. Escalation to `elkjs` (`layered`, edge routing/ports for deeply nested containers) is a future option, not Phase 1.5. Pan/zoom + `fitView` + a minimap for large graphs.

### 2.5 Animation (active frontier only)
- Running node: a soft **pulse** ring (blue).
- Active inbound edge: **marching-ants** (React Flow `animated: true`) into the currently-running node; **amber** dashed variant into an awaiting gate (Temporal's pending-edge cue).
- Optional **dot-along-edge** (`<animateMotion>`) on a `for_each`'s inbound edge to convey data fan-out.
- Done and pending edges are **static**. No whole-canvas motion. Motion is confined to the active frontier.

### 2.6 Performance (large graphs / large N)
Memoized node + edge components (`React.memo`); layout off the render path + cached; for_each large-N collapses by default (units live behind the drill-in, virtualized); React Flow `onlyRenderVisibleElements` considered only past hundreds of nodes (benchmark first — it can hurt small graphs). Any cap on rendered units is shown explicitly ("+140 more"), never silent.

---

## Part 3 — Backend additions

Small, additive, read-only. No change to existing endpoints' shapes beyond noted additions.

### 3.1 `GET /api/runs/{id}/graph` (new)
Returns the assembled graph inputs so the client doesn't re-derive structure from three files:
```json
{
  "run": { ...RunRecord... },
  "workflow": { "steps": [ { "id": "...", "kind": "step|for_each|parallel|panel", "agent": "...",
                             "for_each": "...", "parallel": [ {"id","agent"} ],
                             "panel": {...}, "gate": {"max_iterations","until_no_findings_at_severity_or_above"} } ] },
  "step_results": [ ...StepResultRecord... ],
  "units": [ { "step_id": "...", "index": N, "unit_key": "...", "status": "done|failed|...", "transcript_path": "..." } ]
}
```
- `workflow` is parsed from the run's `run_dir/workflow.yaml` (the snapshot), NOT the global workflow store — so it reflects exactly what ran. Map the parsed `Workflow.steps` into the slim DTO above (kind + the structural fields the graph needs); do not dump the full step internals.
- `units` is assembled from `unit_checkpoints.jsonl` (terminal per-unit state) — the live deltas still come from the SSE stream.
- A new dedicated endpoint (rather than fattening `GET /api/runs/{id}`) keeps the graph model explicit and the existing detail endpoint lean.

### 3.2 `GET /api/autoflows` (new)
Lists recent autoflow cycles from `AutoflowHistoryStore::list_recent` → `[{ autoflow, cycle_id, status, run_id?, started_at, finished_at, outcome }]` (slim DTO over `AutoflowCycleRecord`). Detail (`GET /api/autoflows/{id}` or the cycle events via `list_recent_events`) is a follow-up; the list + cross-link to the run is the Phase-1.5 bar. Tolerate a missing store dir → `[]`.

### 3.3 Runs list — trigger-type
`GET /api/runs` gains nothing structural; the existing `RunRecord.event`/trigger is exposed in the slim list DTO (a `trigger: "manual" | "cron" | "event"` derivation) so the UI can filter. Derive trigger type from the run record (presence/shape of `event`) — no new store.

### 3.4 Panel-round visibility (scoped, flagged)
There is **no dedicated panel-round Event variant** today; panel-loop iterations surface only through generic step events. To drive a crisp live **round counter** and per-round state, add a minimal signal — preferred: a new `Event::PanelRound { run_id, step_id, round, max_iterations, max_severity_remaining }` emitted by the runner's gate loop (mirrors `UnitStarted`). This is the one backend change beyond pure read-adapters; it is small and isolated. **Fallback if descoped:** render the panel/gate construct structurally and show round state only from `step_results`/notes (coarser, post-hoc), with the live counter deferred. The plan will treat the event addition as its own task so it can be cut without blocking the rest.

---

## Part 4 — Frontend

Rework `crates/rupu-cp/web` Runs/RunDetail:
- **`lib/runGraphModel.ts`** — pure builder: `(graphResponse, liveEvents) => RunGraphModel` (nodes with kind+state, edges, fan-out units, panel rounds). Pure + unit-testable; this is where merge precedence (§2.1) lives.
- **`lib/graphLayout.ts`** — dagre LR layout + structure-hash cache → node positions.
- **`components/RunGraph.tsx`** — React Flow canvas; consumes the model + positions; one custom node component per type (`StepNode`, `ParallelContainerNode`, `FanoutNode`, `PanelLoopNode`/gate). Animated edges on the active frontier.
- **`components/FanoutDrill.tsx`** — the virtualized, filterable unit drill-in list.
- **`pages/Runs.tsx`** — trigger-type filter chips.
- **`pages/Autoflows.tsx`** (new) + nav entry; rows cross-link to `/runs/:id`.
- Keep the existing single-SSE-subscription pattern in `RunDetail` feeding both the graph model and the event feed.

---

## Testing

- **Backend:** fixture a run dir (`workflow.yaml` + `step_results.jsonl` + `unit_checkpoints.jsonl`) and assert `GET /api/runs/{id}/graph` returns the expected skeleton + unit states; assert a workflow with `for_each`/`parallel`/`panel` maps to the right `kind`s. Assert `GET /api/autoflows` over a seeded `AutoflowHistoryStore`. If the `PanelRound` event lands, a round-trip + emission test.
- **Frontend:** unit-test `runGraphModel` merge precedence (skeleton-only → all pending; ⊕ results → terminal; ⊕ checkpoints → per-unit; ⊕ live events → running/awaiting; failed≠skipped). Layout determinism test (same structure → same positions). `tsc -b && vite build` green. Rendering itself is validated by matt (same rule as the TUI / Phase 1).

---

## Open decisions for review
1. **Sidebar placement of Autoflows** — under Observe (next to Runs, as drafted) vs. under Build (next to Workflows, since an autoflow is a workflow+trigger). Drafted under Observe.
2. **`PanelRound` event** — include the small runner change (crisp live round counter) in this slice, or descope to the coarse post-hoc fallback? Drafted as a separate, cuttable task.
3. **Small-N fan-out threshold** — default 12 inline before collapsing. Tunable.
