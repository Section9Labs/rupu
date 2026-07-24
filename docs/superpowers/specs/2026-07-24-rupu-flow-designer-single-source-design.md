# Flow Designer (`next`) — single-source-of-truth rebuild (design)

**Date:** 2026-07-24
**Status:** Direction approved (operator chose "canonical model → YAML generated losslessly"). Spec pending operator review.
**Depends on:** findings in `2026-07-24-rupu-flow-designer-correctness-findings.md`.
**Scope:** `crates/rupu-cp/web/src/components/workflow-editor/**` + `src/lib/{workflowGraph,workflowLayout}.ts`, behind the existing `[cp].workflow_editor_ui = 'next'` flag. No backend change. The `classic` editor path is untouched.

## 1. The one insight this rests on

`yamlToGraph` (`workflowGraph.ts:315-352`) already builds **every** canvas edge purely from the node list:
- **(a)** a base chain edge between each consecutive pair of steps (declared order),
- **(b)** a data-ref edge `X→Y` whenever `Y` references `steps.X`,
- **(c)** a branch-arm edge per `then`/`else` target.

Nothing in `graph.edges` carries information that isn't recoverable from the ordered nodes. Therefore **edges must not be stored — they are a pure derived view.** (Corollary, and proof the current model is already broken: a plain edge drawn on the canvas that is neither consecutive-order nor a data-ref serializes to nothing and vanishes on the next reconcile. There is no such thing as a free-floating edge in a valid workflow.)

## 2. The canonical model

```ts
interface WorkflowModel {
  meta: WorkflowMeta;      // name/description/rest (+ typed trigger/inputs/autoflow via workflowMeta.ts)
  nodes: GraphNode[];      // ORDERED. GraphNode = { id, data: StepNodeData, position }
}
```

- **Stored (authoritative):** `meta` + the **ordered** `nodes` (each carrying its full `StepNodeData`, including `thenTargets`/`elseTargets`), plus per-node `position` (editor layout state; not serialized to YAML).
- **Not stored:** `edges`. Removed from state entirely.
- **Order is the node-array order.** Linear flow = array order; reordering flow = reordering the array. Branch routing = `then`/`else` arrays. These are the only two ways flow is expressed, matching the YAML.

`thenTargets`/`elseTargets` become the *single* home of branch routing — the second representation (edges tagged `branch:`) is deleted. This alone removes root cause RC1.

## 3. Derived views

Two pure functions, factored out of `yamlToGraph` so load-time and edit-time derivation are the **same code** (no second path to drift):

```ts
// factored from yamlToGraph (a)+(b)+(c); the ONLY producer of edges anywhere.
function deriveEdges(nodes: GraphNode[]): GraphEdge[];

// the canvas render input
function toReactFlow(model): { nodes: RFNode[]; edges: RFEdge[] }  // edges = deriveEdges(model.nodes)
```

`deriveEdges` runs on every render. `graph.edges` as stored state is gone; every reader (`topoSort`, the edges memo in `WorkflowEditorGraph.tsx`, serialization) consumes `deriveEdges(nodes)`.

## 4. One commit path, all-or-nothing

```ts
function commit(nextNodes, nextMeta): void {
  const res = graphToWorkflowObject({ meta: nextMeta, nodes: nextNodes }); // uses deriveEdges internally
  if ('error' in res) { surfaceError(res.error); return; }  // REJECT — model NOT mutated, canvas NOT mutated
  setModel({ meta: nextMeta, nodes: nextNodes });
  const dumped = yaml.dump(res.obj);
  lastSeenYaml.current = dumped;
  onYamlChange(dumped);
}
```

The current `setGraph(next)`-then-maybe-skip-`onYamlChange` split (RC2) is deleted: an edit that can't serialize (a cycle) is **rejected before it touches state**, with a visible reason. The canvas can never diverge from the YAML.

**Prevention over cure:** the branch Then/Else picker (`StepForm.tsx:603`) gains a cycle guard — it offers only targets that don't create a cycle (reuse `canConnect`'s reachability check, currently only applied to drag-connect). So P0.1 becomes unreachable, and the all-or-nothing commit is the backstop.

**RC3 dissolves naturally:** because edits mutate the model directly (not via a YAML round-trip), the echo guard no longer needs to gate edge re-derivation — edges are always fresh from `deriveEdges`. The `lastSeenYaml` guard stays only for its real job: ignoring our own YAML echo so a *hand* YAML edit still reconciles.

## 5. Total, round-trip-stable serialization

Serialization must satisfy: **`parse(dump(model)) ≡ model`** for every kind, and **`dump(model)` is accepted by `Workflow::parse` whenever the node is complete.** Concrete fixes:

- **Action kind preserved (P0.3):** an `action` node always emits an `action:` key (empty string when unset) so re-parse keeps kind `action` instead of collapsing to `step`. An empty tool is an *incomplete* state ("pick a tool"), not a kind change.
- **Panel emits `when`/`continue_on_error`/`actions` (P0.4)** like the other arms.
- **Branch condition (F.1):** emit `condition` (empty string when unset) so the friendly `BranchEmptyCondition` fires instead of a raw serde "missing field" error — or gate emit on the node (see §6).
- **Sub-step / panel-gate completeness (F.2/F.3):** these are *incomplete* states; handle via §6, not by emitting invalid shapes silently.
- **Regression net:** a per-kind round-trip test — `nodeToStepObject → dump → yamlToGraph` stable on kind + all modeled fields, for every kind and for the empty/fresh variant of each. This is the guard that keeps serialization total as the schema grows.

## 6. Validation without the flood (RC5)

Today every keystroke POSTs to `/api/workflows/validate`; in-progress nodes 400 constantly. New behavior:
- Client-side `validateGraph` already knows structural incompleteness (missing agent, empty condition, empty sub-step). Show those as **inline node badges** (they largely exist) and **do not POST to the server** while a node is structurally incomplete — the server round-trip is for the checks only it can do (action-catalog, cross-refs), which are meaningful only once the node is complete.
- Keep the server validate for complete graphs; map any raw serde error that slips through to a friendly message.
- Net: no console 400 flood; the badge tells you what to finish; the server confirms once you've finished.

## 7. The desync siblings (P1), fixed for free or cheaply

- **Dangling branch targets on delete:** with edges derived, deleting a node just makes its derived edges disappear; also scrub the deleted id from every surviving `thenTargets`/`elseTargets` in the delete op so serialization never references a ghost.
- **Orphaned edges on kind-switch:** gone by construction — no stored branch edges to orphan; `switchKind` away from `branch` clears `then`/`else` and the derived edges vanish.
- **Kind-switch `step`↔`for_each` drops agent/prompt:** preserve shared fields across compatible kind switches.
- **Invisible leftover `approval:` on branch/panel:** clear (or surface) approval fields when switching to a kind whose form can't show them.

## 8. Phasing

- **Phase 1 (this spec, shippable):** §2–§6 + the delete/kind-switch fixes in §7. Removes the reported bugs (vanish, no-line, action→step, panel drop, flood) and the whole desync class. Behind the flag; `classic` untouched.
- **Phase 2 (follow-on, separate spec):** authoring gaps — severity `<select>`, `max_parallel` min-guard, typed `with:` values, `Approval.notify` UI, on_reject kind-awareness, passthrough bags for `Panel`/`Branch`/`Approval`.

## 9. Constraints & testing
- `next` path only; `classic` byte-identical; behind `workflow_editor_ui = 'next'`. Tokens only; no backend change; no new dep.
- Tests: `deriveEdges` equivalence to the current load-time edges for all sample workflows; per-kind round-trip stability (incl. fresh/empty nodes); commit rejects a cycle without mutating; branch picker excludes cycle-forming targets; delete scrubs targets; the four real reported repros (P0.1–P0.4) become passing regression tests.
- Operator gate: matt validates in the running app (author a branch end-to-end, delete a targeted node, switch kinds, add an incomplete node) before merge — the failures were all interaction-time and invisible to unit tests.
