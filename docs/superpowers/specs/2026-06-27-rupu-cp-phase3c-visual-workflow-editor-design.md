# rupu-cp Phase 3c — Visual workflow DAG editor — Design

**Date:** 2026-06-27
**Surfaces:** `rupu-cp/web` (the editor — the bulk), `rupu-cp` (one tiny validate endpoint)
**Status:** approved direction (matt: freeform drag-and-connect canvas; reject invalid connections with a clear reason; goal = author workflows without knowing the YAML)

## Goal
A freeform drag-and-drop DAG editor for workflow definitions, so a user can build/modify a workflow visually without learning the YAML. The canvas is the third slice of CP Phase 3 (Authoring), on top of 3a (agent `.md` editor) and 3b (workflow `.yaml` editor).

## Core principle (honest semantics — no mock behavior)
rupu has **no `depends`/`needs` field**. The runtime executes `steps:` as an **ordered list**; data flows by minijinja reference (`{{ steps.<id>.output }}`) to any *earlier* step (a forward reference is a parse error). Concurrency exists only inside container step-kinds (`parallel:`, `for_each:`, `panel:`). The editor must reflect this truthfully:
- **An edge A→B means "B runs after A"** (ordering + lets B reference A's output). It is NOT a promise of parallelism.
- On **Save**, the graph is **topologically sorted** into the linear `steps:` list. A diamond (A→B, A→C, B→D, C→D) is legal and serializes to `A,B,C,D`.
- **Real concurrency is explicit**: a `parallel` / `for_each` / `panel` **container node**. The editor never makes two side-by-side edges silently run sequential-but-look-parallel.
- **Invalid connections are rejected at draw time with a clear reason** (see §Connection rules). Configurations that can't serialize are prevented or flagged before Save, not after.

## Source of truth & data flow
- The workflow page gets two tabs: **Graph** ⇄ **YAML** (the 3b CodeMirror editor). They edit the same definition.
- The in-memory model is the **parsed workflow object** (via `js-yaml`, already a dependency). The canvas owns the `steps` array; a **Settings** panel owns the top-level fields (name, description, inputs, trigger, defaults). Fields the canvas doesn't model (autoflow, contracts, notifyIssue, …) are **preserved untouched** on the object across edits.
- **Save** = `js-yaml.dump(object)` → the existing 3b `PUT/POST /api/workflows` (so every write is still validated by `Workflow::parse` server-side; nothing new in the write path). Switching Graph→YAML regenerates the YAML text from the object; YAML→Graph re-parses.
- **Known limitation (flagged):** serializing through the object **drops YAML comments and custom formatting** — inherent to a structural/visual editor. The YAML tab remains for anyone who wants hand-authored YAML with comments; using the Graph tab and saving rewrites the file canonically.

## Node types (canvas)
Each top-level step is one node. Reuse the existing read-only visuals (`StepNode`, `ParallelNode`, `FanoutNode`, `PanelLoopNode`) adapted for editing. Container internals (parallel sub-steps, panelists) are **edited in the node's side-panel form**, not as separate canvas nodes — this keeps the canvas = the top-level step DAG and matches the flat `steps` data model.

| Node kind | YAML shape | Side-panel form fields |
|---|---|---|
| **step** (linear) | `id, agent, prompt, when?, continue_on_error?, actions?, approval?` | id, agent (dropdown of known agents), prompt (textarea), when, continue_on_error, approval.required |
| **for_each** | linear + `for_each: <expr>, max_parallel?` | + for_each expression, max_parallel |
| **parallel** | `id, parallel: [{id, agent, prompt}], max_parallel?` | id, max_parallel, repeatable sub-step rows (id/agent/prompt) |
| **panel** | `id, panel: {panelists[], subject, prompt?, max_parallel?, gate?}` | id, panelists (multi-select agents), subject, prompt, max_parallel, gate (until_severity / max_iterations / fixer) |

The node "kind" is chosen when adding from the palette; switching kind is allowed via the form but warns it will drop kind-specific fields (mutual exclusivity is structural — exactly one kind per node, so the YAML mutual-exclusivity errors can't be produced).

## Connection rules (`canConnect(source, target, graph)`)
Wired to React Flow's `isValidConnection` (prevents the drop) AND surfaced as an inline reason (toast / banner) when a user attempts an invalid one:
1. **No self-loop** — "A step can't depend on itself."
2. **No cycle** — "This would create a cycle (… → … → …); steps must form a DAG." (Run a reachability check from `target` to `source` before adding.)
3. **No duplicate edge** — silently ignored (or "Already connected.").
Multiple inputs/outputs (fan-in / fan-out / diamonds) are allowed — they topo-sort fine.

## Other validation surfaced before Save (no surprises)
- **Required fields:** linear/for_each need `agent` + `prompt`; parallel needs ≥1 sub-step; panel needs ≥1 panelist + subject. Empty → the node shows a red "incomplete" badge and Save is blocked with a list.
- **Forward template references:** detect `steps.<id>` references in a node's templates; if the referenced node topo-sorts *after* this one, flag on the node ("references `steps.x` which runs later"). (The server `Workflow::parse` also rejects this; the client flag is the early, friendly version.)
- **Live YAML validity:** a small **`POST /api/workflows/validate { raw } → 200 {ok:true} | 400 {error}`** endpoint (reuses `Workflow::parse`, writes nothing) drives a live "valid ✓ / invalid ✕ <reason>" badge in both the Graph and YAML tabs. This is the only backend addition.

## Layout
- On load, positions come from **dagre** (already a dependency) — top-to-bottom layered layout.
- The user's manual arrangement is **persisted in `localStorage`** keyed by workflow name (browser-local; honest — it's not in the file). Cleared positions fall back to dagre. (Persisting layout into the repo is a deferred follow-up; the YAML has no place for it.)
- Topo-sort tiebreak among ready nodes = vertical-then-horizontal position, so the visual top-to-bottom order matches the emitted step order intuitively.

## Files
**Backend (tiny):**
- `crates/rupu-cp/src/api/workflows.rs` — add `POST /api/workflows/validate` (`validate_workflow`: `Workflow::parse(&raw)` → `{ok:true}` or `ApiError::bad_request`). Tests.

**Frontend (the bulk):**
- `crates/rupu-cp/web/src/lib/workflowGraph.ts` — **pure** core: `yamlToGraph(obj) → {nodes, edges, workflowMeta}`, `graphToWorkflowObject(nodes, edges, meta) → object` (topological sort + per-kind YAML mapping), `canConnect(...)`, forward-ref detection, required-field checks. Heavily unit-tested (no React/DOM).
- `crates/rupu-cp/web/src/lib/workflowLayout.ts` — dagre auto-layout + localStorage position persistence.
- `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditorGraph.tsx` — the editable `@xyflow/react` canvas: palette (add step/for_each/parallel/panel), draw/delete edges via `isValidConnection`, select/delete nodes, node validity badges.
- `crates/rupu-cp/web/src/components/workflow-editor/nodes/*` — editable node renderers (extend or wrap the existing graph node components).
- `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` (+ sub-forms) — the side-panel per-kind form.
- `crates/rupu-cp/web/src/components/workflow-editor/WorkflowSettingsForm.tsx` — name/description/inputs/trigger.
- `crates/rupu-cp/web/src/lib/api.ts` — `validateWorkflow(raw)`.
- `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx` — Graph ⇄ YAML tabs; Save routes through existing `saveWorkflow`/`createWorkflow`; live-validity badge.
- The editable canvas + forms live in their own lazy route chunk (xyflow already chunks separately); keep the main bundle lean (recharts/codemirror/xyflow all stay out of `index-*.js`).

## Testing
- **Pure core (`workflowGraph.ts`)** — the priority. Round-trip: `yamlToGraph` ∘ `graphToWorkflowObject` is stable for representative workflows (linear, diamond, for_each, parallel, panel-with-gate). Topo-sort produces a valid linear order; tiebreak is deterministic. `canConnect` rejects self-loop/cycle, accepts diamonds. Forward-ref + required-field detectors. Top-level fields (autoflow/contracts) survive a round-trip untouched.
- **Backend** — `/api/workflows/validate`: valid → 200 `{ok:true}`; unparseable → 400 with message; nothing written. clippy clean.
- **Frontend components** — palette adds a node; an invalid connection is rejected and surfaces the reason; editing a node's form updates the model; Save serializes + calls `saveWorkflow`; Graph⇄YAML tab switch round-trips. Mock `@xyflow/react` where needed.
- **Gates** — `npm test -- --run` green; `npm run build` strict; recharts + codemirror + xyflow all absent from `index-*.js`.
- **Visual validation by matt** — build a workflow from scratch on the canvas (linear + a parallel container + a for_each), connect them, watch an invalid connection get rejected with a reason, Save, then confirm the YAML tab shows correct YAML and the CLI can run it.

## Non-goals / deferred (TODO)
- Persisting node layout into the repo (localStorage only in v1).
- Editing parallel sub-steps / panelists as first-class draggable canvas nodes (form-managed in v1).
- Preserving YAML comments/formatting through a Graph-tab save (structural editor rewrites canonically).
- Project-scoped workflows (global only, matching 3a/3b).
- A node for raw/unknown step shapes beyond the four kinds (if a loaded step doesn't match, show it read-only with a "edit in YAML" hint rather than dropping it).
