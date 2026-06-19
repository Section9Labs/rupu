# rupu Control Plane — Live Run View depth (Phase 1.5) — Design

**Date:** 2026-06-18
**Author:** matt + Claude
**Status:** Design draft (visual design validated via brainstorm companion)
**Builds on:** `docs/superpowers/specs/2026-06-18-rupu-control-plane-design.md` (Phase 1 Observe, PR #319). Stacks on the `rupu-cp` crate + `crates/rupu-cp/web` app.

## Summary

Phase 1 shipped an Observe Control Plane whose Runs view is shallow: it lists only orchestrator workflow runs, and the run graph paints only steps that have *already produced a result* (no pending/awaiting states, flat linear chain, no fan-out/loop structure, minimal motion). This slice deepens the live run view into a professional workflow-run graph and broadens what "Runs" surfaces — addressing four pieces of operator feedback:

1. **Broaden the activity surface.** Separate the authored *definitions* (Build) from their *executions* (Runs), each grouped by domain — Agents / Workflows / Autoflows — so "Runs" surfaces agent and autoflow executions, not just direct workflow runs.
2. **Show every step, always, with correct per-step state** (pending / running / awaiting / done / failed / skipped) — not only completed steps.
3. **Tasteful live animation** of the active frontier (running node + inbound edge).
4. **Render step *types* distinctly** — `for_each` (data fan-out), `parallel` (named sub-steps), `panel + gate` fix-loops, and approval gates each get their own visual treatment.

The design language was researched against Dagster, Prefect, Temporal, Argo Workflows, Airflow, GitHub Actions, n8n, Kestra, and Conductor, and the React Flow (xyflow) layout/animation ecosystem, then validated visually with matt.

**Out of scope (later phases):** any *control* action (approve/reject/cancel/dispatch/send-input) — that is CP Phase 2; this slice stays read-only/Observe. Multi-host, RBAC, and the authoring editor remain Phases 3–4.

---

## Part 1 — Activity surface (sidebar IA)

### Decision
The organizing principle: **Build holds the authored *definitions*; Runs holds their *executions*.** Both are grouped by the same three domains — Agents, Workflows, Autoflows — so an operator can move from "the thing I wrote" to "every time it ran" and back. Revised nav:

```
Dashboard
── Runs ──            (executions, grouped by what ran)
  Agents              agent runs — standalone `rupu run` + per-session invocations
  Workflows           orchestrator runs launched directly
  Autoflows           trigger-fired runs / autoflow cycles
── Observe ──
  Live Events
  Coverage
── Build ──           (authored definitions)
  Workflows
  Agents
  Autoflows
── Fleet ──           (renamed from "Run" to avoid colliding with the Runs group)
  Sessions
  Workers
Settings
```

Each Runs sub-item is a unified list for that domain with in-list filters (e.g. status, trigger). Routes: `/runs/agents`, `/runs/workflows`, `/runs/autoflows` (executions) vs `/workflows`, `/agents`, `/autoflows` (definitions).

### Data sources (all grounded — no new persistence)
- **Runs › Workflows** — `RunStore` `RunRecord`s launched directly (no trigger `event`). Existing store.
- **Runs › Autoflows** — trigger-fired runs. Two complementary sources: `RunRecord`s carrying a trigger payload (`event: Option<serde_json::Value>`), and the higher-level cycle history in `rupu_runtime::autoflow_history::AutoflowHistoryStore` (`AutoflowCycleRecord`, each carrying an optional `run_id` that cross-links to the run). The Autoflows list shows cycles → drill into the run graph.
- **Runs › Agents** — agent-level executions, from two real sources: (1) **standalone `rupu run`** writes a `StandaloneRunMetadata` sidecar `<transcripts>/<run_id>.meta.json` (`run_id`, `session_id?`, `trigger_source`, workspace/repo/issue refs) per run — scan the transcripts dir for `*.meta.json`; (2) **session invocations** — each session's `SessionRunRecord`s (`run_id`, `prompt`, `transcript_path`, `started_at`/`completed_at`, `status`, token counts). Both are read as the on-disk contract (the same black-box approach Phase-1 uses for sessions), so `rupu-cp` stays a read adapter and doesn't depend on `rupu-cli` internals. Each agent run links to its transcript (and, for sub-agent dispatch, to the parent workflow run via `RunStore::create_sub_run`'s `<runs>/<parent>/sub/<id>/`).
- **Build › Workflows / Agents** — already Phase-1 pages (definitions). **Build › Autoflows** (new) — the autoflow definitions; surfaced via the autoflow definition source the CLI's autoflow listing uses (a small read adapter; exact reader pinned in the plan).
- **Sessions / Workers** — existing Phase-1 pages; only the nav group label changes (Run → Fleet).

The previously-deferred "agent runs" are therefore **in scope**: the `.meta.json` sidecars + `SessionRunRecord`s make them queryable without new persistence. Only the *run-graph* (Part 2) is workflow/autoflow-specific; an agent run's "view" is its transcript + metadata (no multi-step DAG), so Runs › Agents is a list → transcript, not a graph.

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

### 3.2 Runs, by domain (new + extended)
Three list endpoints backing the Runs sub-items:
- `GET /api/runs/workflows` — `RunStore` runs launched directly (no trigger `event`). (The existing `GET /api/runs` is kept as the unfiltered superset; the slim DTO gains `trigger: "manual" | "cron" | "event"` derived from the record so the UI can filter.)
- `GET /api/runs/autoflows` — autoflow cycles from `AutoflowHistoryStore::list_recent` → `[{ autoflow, cycle_id, status, run_id?, started_at, finished_at, outcome }]` (slim DTO over `AutoflowCycleRecord`), each cross-linking to its run; plus trigger-fired `RunRecord`s. Tolerate a missing store dir → `[]`.
- `GET /api/runs/agents` — agent runs merged from (a) `<transcripts>/*.meta.json` (`StandaloneRunMetadata`, parsed via a minimal CP-side DTO) and (b) `SessionRunRecord`s scanned from the session stores. Slim DTO: `[{ run_id, agent?, session_id?, trigger_source, status?, started_at, transcript_path }]`. Each row links to its transcript.

### 3.3 Autoflow definitions (Build)
`GET /api/autoflows` (definitions) lists authored autoflows for the Build › Autoflows page, via the same source the CLI's autoflow listing uses (read adapter; exact reader pinned in the plan). Distinct from `GET /api/runs/autoflows` (executions).

### 3.4 Panel-round visibility (scoped, flagged)
There is **no dedicated panel-round Event variant** today; panel-loop iterations surface only through generic step events. To drive a crisp live **round counter** and per-round state, add a minimal signal — preferred: a new `Event::PanelRound { run_id, step_id, round, max_iterations, max_severity_remaining }` emitted by the runner's gate loop (mirrors `UnitStarted`). This is the one backend change beyond pure read-adapters; it is small and isolated. **Fallback if descoped:** render the panel/gate construct structurally and show round state only from `step_results`/notes (coarser, post-hoc), with the live counter deferred. The plan will treat the event addition as its own task so it can be cut without blocking the rest.

---

## Part 4 — Frontend

Rework `crates/rupu-cp/web` Runs/RunDetail:
- **`lib/runGraphModel.ts`** — pure builder: `(graphResponse, liveEvents) => RunGraphModel` (nodes with kind+state, edges, fan-out units, panel rounds). Pure + unit-testable; this is where merge precedence (§2.1) lives.
- **`lib/graphLayout.ts`** — dagre LR layout + structure-hash cache → node positions.
- **`components/RunGraph.tsx`** — React Flow canvas; consumes the model + positions; one custom node component per type (`StepNode`, `ParallelContainerNode`, `FanoutNode`, `PanelLoopNode`/gate). Animated edges on the active frontier.
- **`components/FanoutDrill.tsx`** — the virtualized, filterable unit drill-in list.
- **Nav restructure** (`lib/sidebarNav.ts`) — add the **Runs** group (Agents / Workflows / Autoflows executions), add **Build › Autoflows**, rename **Run → Fleet**.
- **Runs execution pages** — `pages/runs/WorkflowRuns.tsx` (+ filters; today's `Runs.tsx` becomes this), `pages/runs/AutoflowRuns.tsx` (cycles, cross-link to `/runs/:id` graph), `pages/runs/AgentRuns.tsx` (list → transcript; no graph). Shared list primitives reused.
- **Build › Autoflows** — `pages/AutoflowsDefs.tsx` (definitions list).
- `RunDetail` (the graph) is reached from Workflow/Autoflow run rows; keep the existing single-SSE-subscription pattern feeding both the graph model and the event feed.

---

## Testing

- **Backend:** fixture a run dir (`workflow.yaml` + `step_results.jsonl` + `unit_checkpoints.jsonl`) and assert `GET /api/runs/{id}/graph` returns the expected skeleton + unit states; assert a workflow with `for_each`/`parallel`/`panel` maps to the right `kind`s. Assert `GET /api/autoflows` over a seeded `AutoflowHistoryStore`. If the `PanelRound` event lands, a round-trip + emission test.
- **Frontend:** unit-test `runGraphModel` merge precedence (skeleton-only → all pending; ⊕ results → terminal; ⊕ checkpoints → per-unit; ⊕ live events → running/awaiting; failed≠skipped). Layout determinism test (same structure → same positions). `tsc -b && vite build` green. Rendering itself is validated by matt (same rule as the TUI / Phase 1).

---

## Resolved decisions
1. **Sidebar IA** — Build = definitions (Workflows / Agents / Autoflows); Runs = executions grouped the same (Agents / Workflows / Autoflows); "Run" group renamed "Fleet" (Sessions / Workers). (matt)
2. **`PanelRound` event** — included, as its own isolated/cuttable task, to drive a crisp live round counter (matt deferred to recommendation; revisit if it adds friction).
3. **Small-N fan-out threshold** — 12 inline before collapsing to X/N + % bar. (matt)
