# rupu-cp Phase 3c — Visual workflow DAG editor — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** A freeform drag-and-connect DAG editor for workflow definitions, so users author workflows without writing YAML. Graph ⇄ YAML tabs on the workflow page; edges = run-after ordering; Save topo-sorts to the linear `steps:` list and goes through the existing 3b validated write path.

**Spec:** `docs/superpowers/specs/2026-06-27-rupu-cp-phase3c-visual-workflow-editor-design.md` (read §Core principle, §Connection rules, §Node types).

**Architecture:** A **pure core** (`workflowGraph.ts`) converts a parsed workflow object (via `js-yaml`, already a dep) ⇄ a `{nodes, edges, meta}` graph, topo-sorting on serialize. The canvas (`@xyflow/react`, already a dep + chunked) renders/edits it; side-panel forms edit each node. Source of truth = YAML; Save = `js-yaml.dump` → existing `PUT/POST /api/workflows` (`Workflow::parse`-validated). One tiny new backend endpoint `POST /api/workflows/validate` powers a live valid/invalid badge.

**Constraints:** no `any` (TS); static Tailwind; recharts + codemirror + **xyflow** all stay OUT of `index-*.js` (the editor is lazy-loaded); stage only specific files; never `-A`/`.rupu/*`; never package-wide `cargo fmt`.

## Reference code (read before building)
- `crates/rupu-cp/src/api/workflows.rs` — has `WorkflowWriteBody`, `Workflow::parse` usage, the test idiom (Task 1 mirrors it).
- `crates/rupu-cp/web/src/components/RunGraph.tsx` + `components/graph/{StepNode,ParallelNode,FanoutNode,PanelLoopNode}.tsx` — the read-only canvas + node visuals to adapt.
- `crates/rupu-cp/web/src/lib/runGraphModel.ts`, `lib/graphLayout.ts` — existing model + dagre layout patterns.
- `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx` — gets the Graph⇄YAML tabs (currently YAML-only via CodeEditor from 3b).
- `crates/rupu-cp/web/src/lib/api.ts` — `saveWorkflow`/`createWorkflow` (3b), `getAgents()` (agent dropdown), `WorkflowDetail` type.
- Step model (orchestrator `workflow.rs`): `Step { id, agent?, prompt?, actions[], when?, continue_on_error?, for_each?, parallel?: [SubStep{id,agent,prompt}], max_parallel?, approval?, panel?: Panel{panelists[], subject, prompt?, max_parallel?, gate?} }`. Workflow top-level: `{ name, description?, trigger, inputs{}, defaults, autoflow?, contracts, notify_issue, ... }`.

---

### Task 1: Backend — `POST /api/workflows/validate`

**Files:** Modify `crates/rupu-cp/src/api/workflows.rs`.

- [ ] **Step 1: Write failing tests** (mirror the existing workflows.rs test idiom; tempdir AppState not even needed — validate is stateless, but keep the idiom):
  - valid yaml (`"name: demo\nsteps:\n  - id: one\n    agent: x\n    prompt: hi\n"`) → handler returns `Json` with `ok == true`.
  - unparseable yaml (`"steps: []"` → empty workflow, or duplicate ids) → returns `Err` whose `ApiError` is a 400 carrying the parse message.
  - confirm it writes NOTHING (stateless — no fs access at all).
- [ ] **Step 2: Run `cargo test -p rupu-cp workflows` — confirm failure.**
- [ ] **Step 3: Implement.** Reuse `WorkflowWriteBody { raw }` (or a local `ValidateBody { raw }`). Handler:
  ```rust
  async fn validate_workflow(Json(body): Json<WorkflowWriteBody>) -> ApiResult<Json<serde_json::Value>> {
      Workflow::parse(&body.raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
      Ok(Json(serde_json::json!({ "ok": true })))
  }
  ```
  Route: add `.route("/api/workflows/validate", post(validate_workflow))` to `routes()`. (Place BEFORE `/api/workflows/:name` is fine — axum matches the literal first; verify `validate` isn't captured as a `:name`. If precedence is an issue, the literal static route wins in axum 0.7; keep it as its own line.)
- [ ] **Step 4:** `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` green/clean.
- [ ] **Step 5: Commit** `git add crates/rupu-cp/src/api/workflows.rs` → `feat(cp): POST /api/workflows/validate (parse-only, writes nothing)`.

---

### Task 2: Pure core — `workflowGraph.ts` (the keystone; TDD-heavy)

**Files:** Create `crates/rupu-cp/web/src/lib/workflowGraph.ts` + `crates/rupu-cp/web/src/lib/workflowGraph.test.ts`. No React/DOM imports — pure functions over plain objects.

**Types (define at top):**
```ts
export type StepKind = 'step' | 'for_each' | 'parallel' | 'panel';
export interface SubStep { id: string; agent: string; prompt: string; }
export interface PanelGate { until_severity?: string; max_iterations?: number; fixer?: string; }
export interface PanelCfg { panelists: string[]; subject: string; prompt?: string; max_parallel?: number; gate?: PanelGate; }
export interface StepNodeData {
  id: string; kind: StepKind;
  agent?: string; prompt?: string; when?: string; continue_on_error?: boolean; actions?: string[];
  for_each?: string; max_parallel?: number;
  parallel?: SubStep[]; panel?: PanelCfg;
  approvalRequired?: boolean;
}
export interface GraphNode { id: string; data: StepNodeData; position: { x: number; y: number }; }
export interface GraphEdge { id: string; source: string; target: string; }
export interface WorkflowMeta { name: string; description?: string; rest: Record<string, unknown>; } // rest = untouched top-level keys (trigger/inputs/defaults/autoflow/contracts/...)
export interface WorkflowGraph { nodes: GraphNode[]; edges: GraphEdge[]; meta: WorkflowMeta; }
```

- [ ] **Step 1: `yamlToGraph(obj: Record<string, unknown>): WorkflowGraph`.** From a parsed workflow object:
  - `meta.name`, `meta.description`; `meta.rest` = a shallow copy of `obj` with `name`/`description`/`steps` removed (these survive untouched on serialize).
  - For each entry in `obj.steps` (array): derive `kind` (`panel` if `panel` present; `parallel` if `parallel` present; `for_each` if `for_each` present; else `step`), map fields into `StepNodeData`. Defensively narrow every field (steps come from arbitrary YAML). A step shape matching none cleanly still becomes a `step` node carrying whatever scalar fields it has (don't drop it).
  - **Edges from order + data refs:** create a chain edge `steps[i] → steps[i+1]` for the base ordering, PLUS an edge `X → Y` when step Y's templates reference `steps.X` (dedupe; skip if it would duplicate the chain edge). Edge id = `${source}->${target}`.
  - `position`: `{x:0,y:0}` placeholder (Task 3 lays out). 
  - Write tests: a linear 3-step workflow → 3 nodes, 2 chain edges; a step whose prompt references an earlier step → the extra data-ref edge present (deduped); for_each/parallel/panel steps map to the right kind with their fields; `meta.rest` carries `trigger`/`inputs`/`autoflow` verbatim.
- [ ] **Step 2: `extractStepRefs(data: StepNodeData): string[]`** — scan all template strings in the node (prompt, for_each, when, sub-step prompts, panel subject/prompt) for `steps.<id>` occurrences (regex `/steps\.([A-Za-z0-9_-]+)/g`), return unique ids. Used by yamlToGraph edges + forward-ref detection. Test it.
- [ ] **Step 3: `topoSort(nodes, edges): { order: GraphNode[] } | { cycle: string[] }`.** Kahn's algorithm. **Tiebreak** among ready (in-degree 0) nodes: smallest `position.y`, then smallest `position.x`, then `id` (stable). On a remaining cycle, return the cycle node ids. Tests: linear → identity order; diamond A→B,A→C,B→D,C→D → A first, D last, B/C ordered by y/x; a cycle → `{cycle}` non-empty.
- [ ] **Step 4: `graphToWorkflowObject(g: WorkflowGraph): { obj: Record<string, unknown> } | { error: string }`.** Topo-sort; if cycle → `{error: 'cycle: ...'}`. Build `steps: []` in sorted order, each node → its YAML mapping (only include fields that are set; omit empties; `parallel` step omits top-level agent/prompt; panel emits the `panel:` block; approvalRequired → `approval: { required: true }`). Reassemble `{ name, ...(description?), ...meta.rest, steps }` — **order keys** so `name` is first and `steps` last for readable YAML. Tests: round-trip `graphToWorkflowObject(yamlToGraph(obj))` deep-equals `obj` (modulo key order + dropped-empty fields) for: linear, diamond, for_each, parallel (2 sub-steps), panel-with-gate, and a workflow with `trigger`+`inputs`+`autoflow` (rest preserved).
- [ ] **Step 5: `canConnect(source: string, target: string, g: { edges: GraphEdge[] }): { ok: true } | { ok: false; reason: string }`.**
  - source === target → `{ok:false, reason:"A step can't depend on itself."}`.
  - existing edge source→target → `{ok:false, reason:"These steps are already connected."}`.
  - adding source→target creates a cycle (i.e. target can already reach source) → `{ok:false, reason:"This would create a cycle — steps must form a DAG."}` (reachability DFS from target over existing edges; if it hits source, reject).
  - else `{ok:true}`. Tests: self-loop, duplicate, back-edge that closes a cycle, and a valid fan-in all behave.
- [ ] **Step 6: `validateGraph(g: WorkflowGraph): string[]`** — returns human-readable problems (empty = ok): each node missing required fields (linear/for_each need agent+prompt; parallel needs ≥1 sub-step each with agent+prompt; panel needs ≥1 panelist + subject); duplicate node ids; forward refs (a node referencing `steps.X` where X topo-sorts after it). Tests for each.
- [ ] **Step 7:** `npm test -- --run workflowGraph` green. (This task ships no UI; it's the tested foundation.)
- [ ] **Step 8: Commit** the two files → `feat(cp/web): workflowGraph pure core (yaml⇄graph, topo-sort, canConnect)`.

---

### Task 3: Layout — `workflowLayout.ts`

**Files:** Create `crates/rupu-cp/web/src/lib/workflowLayout.ts` + test.

- [ ] **Step 1:** `autoLayout(nodes: GraphNode[], edges: GraphEdge[]): GraphNode[]` using `@dagrejs/dagre` (already a dep; see `lib/graphLayout.ts` for the exact import + usage idiom). Top-to-bottom (`rankdir: 'TB'`), reasonable nodesep/ranksep, node size ~ 220×80. Returns nodes with updated `position`. Pure (no DOM). 
- [ ] **Step 2: Test** — a linear chain lays out with strictly increasing `y`; a diamond places D below B and C. (Assert relative ordering, not exact pixels.)
- [ ] **Step 3: Commit** → `feat(cp/web): dagre auto-layout for the workflow editor`.

---

### Task 4: Editable canvas — `WorkflowEditorGraph.tsx` + editable nodes

**Files:** Create `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditorGraph.tsx` and `crates/rupu-cp/web/src/components/workflow-editor/nodes/EditableStepNode.tsx` (one renderer parameterized by kind, reusing the visual language of the existing `components/graph/*` nodes — colored top-bar, kind chip, validity dot). Test as feasible.

- [ ] **Step 1:** The canvas component props:
  ```ts
  interface Props {
    graph: WorkflowGraph;
    onChange: (g: WorkflowGraph) => void;     // node move / add / delete / edge add / edge delete
    selectedId: string | null;
    onSelect: (id: string | null) => void;
    problemsById: Record<string, string[]>;   // from validateGraph, for the per-node red dot
  }
  ```
  Wrap in `ReactFlowProvider` (mirror RunGraph). Use controlled `nodes`/`edges` derived from `graph`. Wire `onNodesChange`/`onEdgesChange` (position + selection + removal) back through `onChange`.
- [ ] **Step 2: Connections.** Set React Flow's `isValidConnection={(c) => canConnect(c.source, c.target, graph).ok}` so invalid drops are refused. On `onConnect`, run `canConnect`; if `ok` add the edge via `onChange`; if not, call a passed-in `onInvalidConnection(reason)` (the page shows a toast/banner). 
- [ ] **Step 3: Palette + delete.** A small toolbar (add **Step / For-each / Parallel / Panel** → appends a node with a unique id like `step-N`, no edges, auto-positioned near the viewport center; selecting it opens the form). Delete = React Flow's built-in (Backspace) AND a delete button on the selected node; deleting a node drops its edges. Re-layout button calls `autoLayout`.
- [ ] **Step 4: Node renderer** — colored bar by kind, id, agent/summary line (e.g. `parallel · N sub-steps`, `for_each: <expr>`, `panel · N panelists`), a red dot when `problemsById[id]` is non-empty, click selects. Reuse Tailwind classes from the existing nodes.
- [ ] **Step 5:** `'@xyflow/react/dist/style.css'` import (already used by RunGraph). Ensure this whole subtree is only reached via the lazy editor route (Task 6 lazy-loads it) so xyflow stays out of the main chunk.
- [ ] **Step 6: Test** (mock `@xyflow/react` to a div that exposes the handlers, like existing graph tests if any; otherwise a lightweight render test): adding from the palette calls `onChange` with a new node; an invalid `onConnect` triggers `onInvalidConnection` and does NOT add an edge; a valid one adds it.
- [ ] **Step 7: Commit** → `feat(cp/web): editable workflow canvas (palette, validated connections)`.

---

### Task 5: Side-panel forms — `StepForm.tsx` (+ sub-forms) and `WorkflowSettingsForm.tsx`

**Files:** Create `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx`, `WorkflowSettingsForm.tsx` + tests.

- [ ] **Step 1: `StepForm`** props `{ node: GraphNode; agents: AgentSummary[]; onChange: (data: StepNodeData) => void; problems: string[] }`. Renders fields per `node.data.kind`:
  - common: `id` (text), kind switcher (changing kind warns it clears kind-specific fields).
  - step / for_each: `agent` (`<select>` of `agents`), `prompt` (textarea), `when` (text), `continue_on_error` (checkbox), `approvalRequired` (checkbox); for_each adds `for_each` expr + `max_parallel`.
  - parallel: `max_parallel` + a repeatable list of sub-step rows (`id`/`agent`/`prompt`, add/remove).
  - panel: `panelists` (multi-select of `agents`), `subject` (textarea), `prompt` (textarea), `max_parallel`, gate (`until_severity` text, `max_iterations` number, `fixer` agent select).
  - Show `problems` inline (`role="alert"`). No `any`; controlled inputs; every edit calls `onChange` with the updated `StepNodeData`.
- [ ] **Step 2: `WorkflowSettingsForm`** props `{ meta: WorkflowMeta; onChange: (meta: WorkflowMeta) => void }` — `name`, `description`. (Inputs/trigger editing is a stretch goal; if time-boxed, render `meta.rest`'s trigger/inputs read-only with an "edit in YAML" note rather than dropping them — they're preserved either way.)
- [ ] **Step 3: Test** — editing the agent select calls `onChange` with the new agent; switching kind to `parallel` surfaces the sub-step editor; adding a sub-step row updates the data; panelists multi-select updates the panel cfg.
- [ ] **Step 4: Commit** → `feat(cp/web): workflow editor side-panel forms`.

---

### Task 6: Wire into `WorkflowDetail` — Graph ⇄ YAML tabs, Save, live validity

**Files:** Modify `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx`, `crates/rupu-cp/web/src/lib/api.ts`; Create `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditor.tsx` (the lazy-loaded composition of canvas + form + settings); test.

- [ ] **Step 1: `api.ts`** — `validateWorkflow(raw: string): Promise<{ ok: boolean; error?: string }>` → `POST /api/workflows/validate`; resolve `{ok:true}` on 200, and on a 400 `ApiError` resolve `{ok:false, error: message}` (catch and convert — this is a validity check, not an exception). No `any`.
- [ ] **Step 2: `WorkflowEditor.tsx`** — composes `WorkflowEditorGraph` + `StepForm` (for `selectedId`) + `WorkflowSettingsForm`, owns the `WorkflowGraph` state, recomputes `validateGraph` → `problemsById`, shows an invalid-connection toast. Props `{ initialYaml: string; agents: AgentSummary[]; onYamlChange: (yaml: string) => void }`: on any graph change, serialize via `graphToWorkflowObject` + `js-yaml.dump` and call `onYamlChange` (keeping the YAML tab in sync). Seed state by `js-yaml.load(initialYaml)` → `yamlToGraph` → `autoLayout`.
- [ ] **Step 3: `WorkflowDetail.tsx` tabs.** Add a `view: 'graph' | 'yaml'` toggle in the YAML/Graph section. Lazy-load the editor: `const WorkflowEditor = lazy(() => import('../components/workflow-editor/WorkflowEditor'))` with a Suspense fallback — keeps xyflow out of the main chunk. Maintain a single `draftYaml` string: the YAML tab (CodeEditor from 3b) edits it directly; the Graph tab edits via `WorkflowEditor.onYamlChange`. Switching tabs just changes which view renders the same `draftYaml` (Graph re-parses it on mount). 
  - **Live validity badge:** debounce `draftYaml` → `validateWorkflow` → show `✓ valid` / `✕ <reason>`.
  - **Save** (one button for both tabs): `saveWorkflow(name, draftYaml)`; on success refresh; on 400 show the parse error inline. Disable Save when `draftYaml === detail.yaml` or while saving or when the live badge says invalid.
  - Fetch `agents` via `getAgents()` for the forms.
- [ ] **Step 4: Test** (`WorkflowDetail.test.tsx` additions; mock `WorkflowEditor` to a stub that calls `onYamlChange('...')`, mock `validateWorkflow`): switching to the Graph tab renders the editor; an `onYamlChange` from it updates the draft + enables Save; Save calls `saveWorkflow` with the serialized yaml; an invalid live-validate disables Save and shows the reason.
- [ ] **Step 5: Gates** (from `crates/rupu-cp/web`): `npm test -- --run` green; `npm run build` exit 0; `grep -c recharts dist/assets/index-*.js` → 0; confirm `@xyflow`, `@codemirror` do NOT appear in `dist/assets/index-*.js` (own chunks); report main chunk size.
- [ ] **Step 6: Commit** → `feat(cp/web): Graph⇄YAML tabs + visual workflow editor wiring`.

---

### Final verification
- `cargo test -p rupu-cp` green; clippy clean. `npm test -- --run` green; `npm run build` strict; recharts + codemirror + **xyflow** all out of the main chunk.
- Final review: honest edge semantics (topo-sort, no faked parallelism); `canConnect` rejects self-loop/cycle with clear reasons; round-trip preserves top-level fields (autoflow/contracts/trigger/inputs); Save always goes through `Workflow::parse`; the lazy chunking holds.
- matt visual-validates: build a workflow from scratch (linear + parallel container + for_each), connect nodes, see an invalid connection rejected with a reason, Save, confirm the YAML tab + CLI agree.
- TODO: note v1 scope (global workflows; sub-steps form-managed; no layout persistence; Graph-save normalizes YAML/drops comments).
