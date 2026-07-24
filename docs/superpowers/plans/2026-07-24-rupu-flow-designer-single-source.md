# Flow Designer Single-Source-of-Truth Rebuild — Implementation Plan (Phase 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `next` Flow Designer's canvas edges a pure derived view of the ordered node list, with one all-or-nothing commit path, so the canvas and the YAML can never silently drift apart — fixing the vanishing-nodes, branch-no-line, action→step, panel-drop, and validate-flood bugs.

**Architecture:** The ordered `nodes` array (each with `StepNodeData`) is the single source of truth. `graph.edges` is kept on the type but is **always** recomputed by one pure function `deriveEdges(nodes)` — never mutated independently. Branch routing lives only in `thenTargets`/`elseTargets`; branch-arm edges are derived from them. Every op returns `withDerivedEdges(meta, nodes)`, enforcing the invariant `edges === deriveEdges(nodes)`.

**Tech Stack:** React 18 + TypeScript, `@xyflow/react`, Vitest + Testing Library, `js-yaml`.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-24-rupu-flow-designer-single-source-design.md`; findings: `docs/superpowers/specs/2026-07-24-rupu-flow-designer-correctness-findings.md`.
- **All work in `crates/rupu-cp/web/`.** Run every command from there. Test: `npx vitest run <path>`; typecheck: `npx tsc -b --noEmit`.
- **`next` render path only.** `EditableStepNode.tsx`'s `classic` render path stays byte-identical. The editor logic changes here are behind `[cp].workflow_editor_ui = 'next'` at the UI level; the pure `lib/` functions are shared but their outputs must stay identical for already-valid inputs (the equivalence tests enforce this).
- **No backend change, no schema change, no new npm dependency. Tokens only.**
- **The invariant:** after every graph-producing operation, `graph.edges` deep-equals `deriveEdges(graph.nodes)`. `graph.edges` is never mutated except by re-deriving.
- **Branch routing is single-source:** `thenTargets`/`elseTargets` on the node are the only store; edges tagged `branch:'then'|'else'` are derived from them, never the reverse.
- **Order is node-array order.** Linear flow = the order of `nodes`. `deriveEdges` builds the chain from consecutive array order.

## File Structure

| File | Responsibility / change |
|---|---|
| `src/lib/workflowGraph.ts` | Extract `deriveEdges(nodes)` + `withDerivedEdges(meta,nodes)` from `yamlToGraph`; `yamlToGraph` and `graphToWorkflowObject` consume them; panel emitter emits `when`/`continue_on_error`/`actions`. |
| `src/lib/workflowGraph.test.ts` | `deriveEdges` equivalence to current load-time edges (all samples); per-kind round-trip stability incl. fresh nodes; panel-`when` round-trip. |
| `src/components/workflow-editor/WorkflowEditorGraph.tsx` | Every `apply*` op returns `withDerivedEdges`; `applyConnect` plain→reorder / branch→arm arrays; `applyDelete` scrubs targets; `newNodeData` seeds `action:''`; `seedGraph`. |
| `src/components/workflow-editor/WorkflowEditorGraph.test.tsx` | Invariant + per-op tests (P0.2 branch edge appears; delete scrubs; reorder). |
| `src/components/workflow-editor/WorkflowEditor.tsx` | `commit` all-or-nothing + `onStepChange` rebuilds edges via `withDerivedEdges`; `seedGraph`. |
| `src/components/workflow-editor/WorkflowEditor.test.tsx` | commit rejects a cycle without mutating; form branch edit produces the edge. |
| `src/components/workflow-editor/StepForm.tsx` | `switchKind` seeds kind defaults + preserves agent/prompt; `BranchFields` cycle-guarded candidates. |
| `src/components/workflow-editor/StepForm.test.tsx` | cycle-forming target excluded; kind-switch preserves/seeds. |
| `src/pages/WorkflowDetail.tsx` | validate effect skips the server POST while the graph is structurally incomplete. |
| `src/pages/WorkflowDetail.test.tsx` | incomplete graph → no server call, inline validity; complete → server call. |

---

### Task 1: `deriveEdges` + `withDerivedEdges` (the one edge producer)

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/workflowGraph.ts` (extract from `yamlToGraph`, ~308-352)
- Test: `crates/rupu-cp/web/src/lib/workflowGraph.test.ts`

**Interfaces:**
- Produces: `export function deriveEdges(nodes: GraphNode[]): GraphEdge[]` and `export function withDerivedEdges(meta: WorkflowMeta, nodes: GraphNode[]): WorkflowGraph`. Tasks 2-4 consume both. Existing `GraphNode`/`GraphEdge`/`WorkflowMeta`/`WorkflowGraph` types are unchanged.

- [ ] **Step 1: Write the failing test**

Append to `src/lib/workflowGraph.test.ts`:

```ts
import { deriveEdges, withDerivedEdges, yamlToGraph } from './workflowGraph';
import fs from 'node:fs';
import path from 'node:path';
import yaml from 'js-yaml';

const WF_DIR = path.resolve(__dirname, '../../../../../.rupu/workflows');

describe('deriveEdges', () => {
  it('reproduces yamlToGraph edges exactly for every real workflow', () => {
    for (const f of fs.readdirSync(WF_DIR).filter((n) => n.endsWith('.yaml'))) {
      const g = yamlToGraph(yaml.load(fs.readFileSync(path.join(WF_DIR, f), 'utf8')) as Record<string, unknown>);
      // deriveEdges(nodes) must equal what yamlToGraph itself produced.
      expect(deriveEdges(g.nodes), f).toEqual(g.edges);
    }
  });

  it('derives a branch arm edge from thenTargets/elseTargets', () => {
    const nodes = yamlToGraph({
      name: 'w',
      steps: [
        { id: 'b', branch: { condition: 'x', then: ['t'], else: ['e'] } },
        { id: 't', agent: 'a', prompt: 'p' },
        { id: 'e', agent: 'a', prompt: 'p' },
      ],
    }).nodes;
    const edges = deriveEdges(nodes);
    expect(edges).toContainEqual(expect.objectContaining({ source: 'b', target: 't', branch: 'then', label: 'true' }));
    expect(edges).toContainEqual(expect.objectContaining({ source: 'b', target: 'e', branch: 'else', label: 'false' }));
  });

  it('withDerivedEdges always satisfies edges === deriveEdges(nodes)', () => {
    const nodes = yamlToGraph({ name: 'w', steps: [{ id: 'a', agent: 'x', prompt: 'p' }] }).nodes;
    const g = withDerivedEdges({ name: 'w', rest: {} }, nodes);
    expect(g.edges).toEqual(deriveEdges(g.nodes));
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/workflowGraph.test.ts -t deriveEdges`
Expected: FAIL — `deriveEdges`/`withDerivedEdges` are not exported.

- [ ] **Step 3: Extract the implementation**

In `src/lib/workflowGraph.ts`, add (right before `yamlToGraph`) a pure `deriveEdges` holding the exact (a)+(b)+(c) logic currently inline in `yamlToGraph` (lines ~315-351), and `withDerivedEdges`:

```ts
/** The ONE producer of canvas edges. Pure function of the ordered node list:
 *  (a) a chain edge between each consecutive pair (declared order),
 *  (b) a data-ref edge X->Y whenever Y references steps.X,
 *  (c) a branch-arm edge per then/else target (labeled true/false).
 *  graph.edges is ALWAYS this — never stored independently. */
export function deriveEdges(nodes: GraphNode[]): GraphEdge[] {
  const ids = new Set(nodes.map((n) => n.id));
  const edges: GraphEdge[] = [];
  const seen = new Set<string>();
  const addEdge = (source: string, target: string, opts?: { label?: string; branch?: 'then' | 'else' }): void => {
    const label = opts?.label;
    const key = `${source}->${target}::${label ?? ''}`;
    if (source === target || seen.has(key)) return;
    seen.add(key);
    const id = opts?.branch ? `${source}->${target}:${opts.branch}` : `${source}->${target}`;
    const e: GraphEdge = { id, source, target };
    if (label !== undefined) e.label = label;
    if (opts?.branch !== undefined) e.branch = opts.branch;
    edges.push(e);
  };
  for (let i = 0; i < nodes.length - 1; i++) addEdge(nodes[i].id, nodes[i + 1].id);
  for (const n of nodes) {
    for (const ref of extractStepRefs(n.data)) if (ids.has(ref)) addEdge(ref, n.id);
  }
  for (const n of nodes) {
    if (n.data.kind !== 'branch') continue;
    for (const t of n.data.thenTargets ?? []) if (ids.has(t)) addEdge(n.id, t, { label: 'true', branch: 'then' });
    for (const t of n.data.elseTargets ?? []) if (ids.has(t)) addEdge(n.id, t, { label: 'false', branch: 'else' });
  }
  return edges;
}

/** Build a WorkflowGraph whose edges are derived from its nodes — the only
 *  correct way to construct/return a graph. */
export function withDerivedEdges(meta: WorkflowMeta, nodes: GraphNode[]): WorkflowGraph {
  return { meta, nodes, edges: deriveEdges(nodes) };
}
```

Then replace `yamlToGraph`'s inline edge-building (the block from `const ids = new Set(...)` at ~314 through the branch-arm loop ending ~351, i.e. everything between building `nodes` and `return { nodes, edges, meta }`) with:

```ts
  return { nodes, edges: deriveEdges(nodes), meta };
```

(Leave the `nodes`/`meta` construction above it and `extractStepRefs` unchanged.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/workflowGraph.test.ts`
Expected: PASS (the equivalence test proves the extraction is behavior-preserving for all 15 samples).

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/lib/workflowGraph.ts src/lib/workflowGraph.test.ts
git commit -m "refactor(cp): extract deriveEdges/withDerivedEdges as the one edge producer"
```

---

### Task 2: Total serialization (panel fields + edges-from-nodes) + round-trip net

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/workflowGraph.ts` — `nodeToStepObject` panel arm (~424-438); `graphToWorkflowObject` (~520-536)
- Test: `crates/rupu-cp/web/src/lib/workflowGraph.test.ts`

**Interfaces:**
- Consumes: `deriveEdges` (Task 1).
- Produces: no new exports. `graphToWorkflowObject` now topo-sorts on `deriveEdges(nodes)` rather than trusting `g.edges`.

- [ ] **Step 1: Write the failing tests**

Append to `src/lib/workflowGraph.test.ts`:

```ts
describe('serialization totality', () => {
  it('a panel step keeps its when/continue_on_error on round-trip (P0.4)', () => {
    const input = {
      name: 'w',
      steps: [{ id: 'p', when: 'inputs.go', continue_on_error: true, panel: { panelists: ['r'], subject: 's' } }],
    };
    const g = yamlToGraph(input);
    const out = graphToWorkflowObject(g) as { obj: Record<string, unknown> };
    const step = (out.obj.steps as Record<string, unknown>[])[0];
    expect(step.when).toBe('inputs.go');
    expect(step.continue_on_error).toBe(true);
  });

  it('graphToWorkflowObject orders by derived edges, not stored edges', () => {
    const g = yamlToGraph({ name: 'w', steps: [{ id: 'a', agent: 'x', prompt: 'p' }, { id: 'b', agent: 'x', prompt: 'p' }] });
    // corrupt the stored edges; serialization must ignore them and use deriveEdges(nodes)
    const corrupted = { ...g, edges: [] };
    const out = graphToWorkflowObject(corrupted) as { obj: Record<string, unknown> };
    expect((out.obj.steps as Record<string, unknown>[]).map((s) => s.id)).toEqual(['a', 'b']);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/workflowGraph.test.ts -t "serialization totality"`
Expected: FAIL — panel `when` is dropped; ordering test may pass or fail depending on stored edges.

- [ ] **Step 3: Implement**

In `nodeToStepObject`'s panel arm (the `} else if (d.kind === 'panel') {` block), after `o.panel = po;` add the shared linear fields the arm currently omits:

```ts
    o.panel = po;
    if (d.when) o.when = d.when;
    if (d.continue_on_error === true) o.continue_on_error = true;
    if (d.actions && d.actions.length > 0) o.actions = d.actions;
```

In `graphToWorkflowObject`, replace `const sorted = topoSort(g.nodes, g.edges);` with:

```ts
  const sorted = topoSort(g.nodes, deriveEdges(g.nodes));
```

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/workflowGraph.test.ts`
Expected: PASS.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/lib/workflowGraph.ts src/lib/workflowGraph.test.ts
git commit -m "fix(cp): panel emits when/continue_on_error; serialize orders by derived edges"
```

---

### Task 3: Every graph op returns `withDerivedEdges`; branch routing single-source

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditorGraph.tsx` — `applyConnect` (75-115), `applyDelete` (118-124), `applyRemoveEdges` (133-152), `applyAddNodeAt`/`applyAddNode`/`applyAddConnectedNext`/`applyInsertOnEdge` (186-260), `newNodeData` (~170-184); and `convertInlineApprovalToGate` (`lib/workflowGraph.ts:680-742`), `seedGraph` (`WorkflowEditor.tsx`), `reconcileGraph` (`lib/workflowLayout.ts:164-173`)
- Test: `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditorGraph.test.tsx`

**Interfaces:**
- Consumes: `withDerivedEdges`/`deriveEdges` (Task 1).
- Produces: every `apply*` returns a graph satisfying the invariant. `applyConnect` on a plain (non-branch) handle **reorders** the target to immediately follow the source (the honest single-source semantic — a stored plain edge that isn't consecutive-order or a data-ref does not round-trip today, so it is already lost; reorder makes the drawn intent durable).

- [ ] **Step 1: Write the failing tests**

Append to `src/components/workflow-editor/WorkflowEditorGraph.test.tsx`:

```ts
import { deriveEdges } from '../../lib/workflowGraph';

function invariantHolds(g: { nodes: any[]; edges: any[] }): boolean {
  return JSON.stringify(g.edges) === JSON.stringify(deriveEdges(g.nodes));
}

describe('single-source graph ops', () => {
  const base = () =>
    yamlToGraph({
      name: 'w',
      steps: [
        { id: 'b', branch: { condition: 'x', then: [], else: [] } },
        { id: 't', agent: 'a', prompt: 'p' },
      ],
    });

  it('a branch then-target set via applyConnect appears as a derived edge (P0.2)', () => {
    let g = base();
    applyConnect(g, { source: 'b', target: 't', sourceHandle: 'then' }, (ng) => (g = ng), () => {});
    expect(g.nodes.find((n) => n.id === 'b')!.data.thenTargets).toContain('t');
    expect(invariantHolds(g)).toBe(true);
    expect(deriveEdges(g.nodes)).toContainEqual(expect.objectContaining({ source: 'b', target: 't', branch: 'then' }));
  });

  it('applyDelete scrubs the deleted id from surviving branch targets (P1)', () => {
    let g = base();
    applyConnect(g, { source: 'b', target: 't', sourceHandle: 'then' }, (ng) => (g = ng), () => {});
    g = applyDelete(g, 't');
    expect(g.nodes.find((n) => n.id === 'b')!.data.thenTargets ?? []).not.toContain('t');
    expect(invariantHolds(g)).toBe(true);
  });

  it('every op leaves edges === deriveEdges(nodes)', () => {
    let g = base();
    expect(invariantHolds(g)).toBe(true);
    const r = applyAddNode(g, 'step');
    expect(invariantHolds(r.graph)).toBe(true);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/WorkflowEditorGraph.test.tsx -t "single-source"`
Expected: FAIL — `applyDelete` leaves the stale target; invariant breaks after ops that build edges directly.

- [ ] **Step 3: Implement**

Import at the top of `WorkflowEditorGraph.tsx`:

```ts
import { deriveEdges, withDerivedEdges } from '../../lib/workflowGraph';
```

Rewrite the ops to mutate **nodes only**, then return `withDerivedEdges`. Replace `applyConnect`'s arm/plain tail (from `if (arm) {` to the end) with:

```ts
  if (arm) {
    const nodes = graph.nodes.map((n) => {
      if (n.id !== source) return n;
      const key = arm === 'then' ? 'thenTargets' : 'elseTargets';
      const list = (n.data[key] as string[] | undefined) ?? [];
      if (list.includes(target)) return n;
      return { ...n, data: { ...n.data, [key]: [...list, target] } };
    });
    onChange(withDerivedEdges(graph.meta, nodes));
    return;
  }
  // plain connect = reorder: move target to immediately after source (linear
  // flow is node-array order; a free-floating stored edge doesn't round-trip).
  const src = graph.nodes.findIndex((n) => n.id === source);
  const tgt = graph.nodes.findIndex((n) => n.id === target);
  if (src < 0 || tgt < 0) return;
  const nodes = [...graph.nodes];
  const [moved] = nodes.splice(tgt, 1);
  nodes.splice(nodes.findIndex((n) => n.id === source) + 1, 0, moved);
  onChange(withDerivedEdges(graph.meta, nodes));
}
```

Replace `applyDelete`:

```ts
export function applyDelete(graph: WorkflowGraph, id: string): WorkflowGraph {
  const nodes = graph.nodes
    .filter((n) => n.id !== id)
    .map((n) => {
      const then = n.data.thenTargets?.filter((t) => t !== id);
      const els = n.data.elseTargets?.filter((t) => t !== id);
      if ((then?.length ?? 0) === (n.data.thenTargets?.length ?? 0) && (els?.length ?? 0) === (n.data.elseTargets?.length ?? 0)) return n;
      return { ...n, data: { ...n.data, ...(then ? { thenTargets: then } : {}), ...(els ? { elseTargets: els } : {}) } };
    });
  return withDerivedEdges(graph.meta, nodes);
}
```

Replace `applyRemoveEdges`'s body so a removed **branch** edge drops its target from the arm list (as today) and a removed **plain** edge is a no-op (chain order can't be individually deleted), then re-derive:

```ts
export function applyRemoveEdges(graph: WorkflowGraph, ids: ReadonlySet<string>): WorkflowGraph {
  const removed = deriveEdges(graph.nodes).filter((e) => ids.has(e.id) && e.branch);
  if (removed.length === 0) return graph;
  const nodes = graph.nodes.map((n) => {
    let data = n.data;
    for (const e of removed) {
      if (e.source !== n.id) continue;
      if (e.branch === 'then') data = { ...data, thenTargets: (data.thenTargets ?? []).filter((t) => t !== e.target) };
      else if (e.branch === 'else') data = { ...data, elseTargets: (data.elseTargets ?? []).filter((t) => t !== e.target) };
    }
    return data === n.data ? n : { ...n, data };
  });
  return withDerivedEdges(graph.meta, nodes);
}
```

In `newNodeData`, seed `action` so a fresh action node keeps its kind on round-trip (P0.3) — add alongside the other kind seeds:

```ts
  if (kind === 'action') data.action = '';
```

For `applyAddNodeAt`/`applyAddNode`/`applyAddConnectedNext`/`applyInsertOnEdge`: each currently builds `{ ...graph, nodes, edges }`. Replace each return with `withDerivedEdges(graph.meta, nodes)` (dropping any hand-built `edges` array — the added chain/branch edge re-derives from the new node's position/data). For `applyAddConnectedNext` the intent "new node follows source" is already satisfied by appending after the source index; ensure the new node is spliced in immediately after the source rather than pushed to the end.

In `lib/workflowGraph.ts` `convertInlineApprovalToGate` (the `const edges = g.edges.map(...)` + `edges.push(...)` block, ~734-739): replace with `return withDerivedEdges(g.meta, nodes)` after building `nodes` (the gate→step link re-derives from the new consecutive order — verify the gate node is inserted immediately before `stepId` so the chain edge is correct; the existing insert at `nodes.splice(idx, 0, gateNode)` already does this).

In `lib/workflowLayout.ts` `reconcileGraph`: it already returns `{ meta: next.meta, edges: next.edges, nodes }`. Change the return to `withDerivedEdges(next.meta, nodes)` so a reconcile also honors the invariant.

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/ src/lib/`
Expected: PASS. If a pre-existing test asserted a hand-built plain edge from `applyConnect`/`applyAddNode`, update it to assert the derived edge (a reorder produces the consecutive-order edge) — do not delete the assertion.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/components/workflow-editor/WorkflowEditorGraph.tsx src/components/workflow-editor/WorkflowEditorGraph.test.tsx src/lib/workflowGraph.ts src/lib/workflowLayout.ts
git commit -m "fix(cp): all graph ops derive edges; branch routing single-source; delete scrubs targets"
```

---

### Task 4: `commit` all-or-nothing + `onStepChange` re-derives + cycle guard

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditor.tsx` — `commit` (320-345), `onStepChange` (352-364), `seedGraph`
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` — `BranchFields` candidates (603)
- Test: `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditor.test.tsx`, `StepForm.test.tsx`

**Interfaces:**
- Consumes: `withDerivedEdges` (Task 1), `canConnect` (`workflowGraph.ts:544`).
- Produces: `commit` never half-applies; `onStepChange` re-derives edges so a form branch edit draws the line (P0.2); the branch picker cannot offer a cycle-forming target (P0.1).

- [ ] **Step 1: Write the failing tests**

Append to `WorkflowEditor.test.tsx`:

```ts
it('a branch then-target set through the step form draws the derived edge (P0.2)', () => {
  // render editor with a branch + target, edit thenTargets via onStepChange path,
  // assert the emitted YAML contains branch.then AND the graph edges include the arm.
  // (Use the existing render helper in this file; drive onStepChange with
  //  { ...branchData, thenTargets: ['t'] } and read the resulting graph.)
});

it('commit rejects a cycle-forming edit without mutating the canvas (P0.1)', () => {
  // Build a graph a->b where b is a branch; attempt to commit b.thenTargets=['a']
  // (a cycle). Assert onYamlChange was NOT called and the on-screen graph is unchanged.
});
```

Append to `StepForm.test.tsx`:

```ts
it('the branch Then/Else picker excludes targets that would form a cycle (P0.1)', () => {
  // Graph: a -> b(branch). Render StepForm for `b`. `a` is upstream, so choosing
  // it as a then-target would cycle; assert `a` is NOT offered as a candidate.
});
```

(Fill each test body against the file's existing render helpers — `WorkflowEditor.test.tsx` renders `<WorkflowEditor>`; `StepForm.test.tsx` renders `<StepForm>` with `allNodeIds`/`graph` props. Assert the observable: emitted YAML / `onYamlChange` calls / rendered candidate checkboxes.)

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/WorkflowEditor.test.tsx src/components/workflow-editor/StepForm.test.tsx`
Expected: FAIL — form edit doesn't draw the edge; cycle commit still emits; `a` is offered.

- [ ] **Step 3: Implement**

In `WorkflowEditor.tsx`, import `withDerivedEdges` and replace `commit` so a serialize failure rejects the edit (never `setGraph` a graph it can't emit):

```ts
  const commit = useCallback(
    (next: WorkflowGraph): void => {
      const res = graphToWorkflowObject(next);
      if (!('obj' in res)) {
        setCommitError(res.error); // surfaced near the editor (see setCommitError state)
        return; // reject — do NOT mutate graph or YAML
      }
      setCommitError(null);
      setGraph(next);
      try {
        if (typeof localStorage !== 'undefined' && !localStorage.getItem(REFORMAT_NOTICE_KEY) && hasYamlComments(draftYamlRef.current)) {
          localStorage.setItem(REFORMAT_NOTICE_KEY, '1');
          setReformatNotice(true);
        }
      } catch { /* localStorage unavailable */ }
      const dumped = yaml.dump(res.obj);
      lastSeenYaml.current = dumped;
      onYamlChange(dumped);
    },
    [onYamlChange],
  );
```

Add the `commitError` state near the other `useState`s and render it as a small inline banner (reuse the existing `ErrorBanner`/notice styling in this file):

```ts
  const [commitError, setCommitError] = useState<string | null>(null);
```

Replace `onStepChange` so the edited nodes are re-derived (this is the P0.2 fix at the form layer):

```ts
  const onStepChange = useCallback(
    (data: StepNodeData): void => {
      if (selectedId === null) return;
      const nodes = graph.nodes.map((n) => (n.id === selectedId ? { ...n, id: data.id, data } : n));
      commit(withDerivedEdges(graph.meta, nodes));
      setSelectedId(data.id);
    },
    [commit, graph, selectedId],
  );
```

In `StepForm.tsx`, cycle-guard the branch candidates. `BranchFields` receives the graph's node ids; give it the derived edges (or pass `graph`) so it can call `canConnect`. Minimal change: compute candidates excluding any target that would cycle:

```ts
  // Exclude self AND any target that would create a cycle (a step can't route
  // back to one of its own ancestors) — mirrors the drag-connect guard.
  const candidates = allNodeIds.filter(
    (id) => id !== d.id && canConnect(d.id, id, { edges }).ok,
  );
```

`BranchFields` must receive `edges` (the derived edges) as a prop from `StepForm`'s caller (`WorkflowEditor` already holds `graph`; thread `graph.edges` down to `StepForm` → `BranchFields`). Add `edges: GraphEdge[]` to the `BranchFields` prop type and to the `StepForm` props, passing `graph.edges` at the `WorkflowEditor` call site.

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/`
Expected: PASS.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/components/workflow-editor/WorkflowEditor.tsx src/components/workflow-editor/StepForm.tsx src/components/workflow-editor/WorkflowEditor.test.tsx src/components/workflow-editor/StepForm.test.tsx
git commit -m "fix(cp): commit is all-or-nothing; form edits re-derive edges; branch picker cycle-guarded"
```

---

### Task 5: Validate without the 400 flood

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx` — validate effect (~151-172)
- Test: `crates/rupu-cp/web/src/pages/WorkflowDetail.test.tsx`

**Interfaces:**
- Consumes: `yamlToGraph`, `validateGraph` (`lib/workflowGraph.ts`).
- Produces: the server `/validate` POST is skipped while the graph is structurally incomplete; inline client validity is shown instead.

- [ ] **Step 1: Write the failing test**

Append to `WorkflowDetail.test.tsx` (mock `api.validateWorkflow`):

```ts
it('does not POST to /validate while a node is structurally incomplete', async () => {
  const spy = vi.spyOn(api, 'validateWorkflow');
  // draftYaml with a branch missing its condition (structurally incomplete)
  renderWithDraft('name: w\nsteps:\n  - id: b\n    branch: {}\n');
  await advanceTimersAndFlush(500);
  expect(spy).not.toHaveBeenCalled(); // client shows the problem inline; no server flood
});

it('DOES POST once the graph is structurally complete', async () => {
  const spy = vi.spyOn(api, 'validateWorkflow').mockResolvedValue({ ok: true });
  renderWithDraft('name: w\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n');
  await advanceTimersAndFlush(500);
  expect(spy).toHaveBeenCalledTimes(1);
});
```

(Use the file's existing render/timer helpers; if none, render `<WorkflowDetail>` with the draft seeded and use `vi.useFakeTimers()`.)

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/WorkflowDetail.test.tsx -t validate`
Expected: FAIL — it POSTs for the incomplete branch.

- [ ] **Step 3: Implement**

Replace the validate effect's timeout body so it does a client-side structural check first:

```ts
    const t = setTimeout(() => {
      let loaded: unknown;
      try {
        loaded = yaml.load(draftYaml);
      } catch {
        setValidity({ ok: false, error: 'YAML does not parse yet.' });
        return;
      }
      if (typeof loaded !== 'object' || loaded === null) return;
      const problems = validateGraph(yamlToGraph(loaded as Record<string, unknown>));
      const count = Object.values(problems).reduce((a, l) => a + l.length, 0);
      if (count > 0) {
        // structurally incomplete — show inline, DON'T flood the server
        setValidity({ ok: false, error: `${count} unfinished ${count === 1 ? 'field' : 'fields'}` });
        return;
      }
      api.validateWorkflow(draftYaml).then((r) => { if (!cancelled) setValidity(r); }).catch(() => {});
    }, 400);
```

Add the imports `import yaml from 'js-yaml'; import { yamlToGraph, validateGraph } from '../lib/workflowGraph';` if not present.

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/WorkflowDetail.test.tsx`
Expected: PASS.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/pages/WorkflowDetail.tsx src/pages/WorkflowDetail.test.tsx
git commit -m "fix(cp): gate server validate on client-side completeness — no 400 flood"
```

---

### Task 6: Kind-switch seeds defaults + preserves shared fields

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` — `switchKind` (126-136)
- Test: `crates/rupu-cp/web/src/components/workflow-editor/StepForm.test.tsx`

**Interfaces:**
- Produces: switching to `branch` seeds `condition:''`/`then`/`else` (so it emits the friendly error, F.1); switching to `action` seeds `action:''` (keeps kind, P0.3); switching `step`↔`for_each` preserves `agent`/`prompt` (§8.3); an `approval*` block is cleared when switching to a kind whose form can't show it (§8.4).

- [ ] **Step 1: Write the failing tests**

Append to `StepForm.test.tsx`:

```ts
it('switching to branch seeds an empty condition (F.1)', () => {
  const out: StepNodeData[] = [];
  // render StepForm for a `step` node, invoke the kind <select> -> 'branch',
  // capture onChange payload
  // ...assert:
  expect(out.at(-1)!.kind).toBe('branch');
  expect(out.at(-1)!.condition).toBe('');
  expect(out.at(-1)!.thenTargets).toEqual([]);
});

it('switching step -> for_each preserves agent and prompt (§8.3)', () => {
  // start with { kind:'step', agent:'coder', prompt:'do it' }, switch to for_each
  // assert agent==='coder' && prompt==='do it' on the payload
});

it('switching a step that had approval into a branch clears the approval block (§8.4)', () => {
  // start { kind:'step', approvalRequired:true }, switch to branch
  // assert approvalRequired is undefined on the payload
});
```

(Drive via the Kind `<select>`'s `onChange` using the file's render helper.)

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/StepForm.test.tsx -t switch`
Expected: FAIL — no condition seeded; agent/prompt dropped; approval leaks.

- [ ] **Step 3: Implement**

Replace `switchKind`:

```ts
  // Kind switch — keep id; carry shared fields only where the DESTINATION kind
  // can hold them; seed the destination's required defaults so it round-trips
  // and validates with a friendly error rather than a raw parser one.
  function switchKind(kind: StepKind): void {
    const base: StepNodeData = { id: d.id, kind };
    if (d.when !== undefined) base.when = d.when;
    if (d.continue_on_error !== undefined) base.continue_on_error = d.continue_on_error;
    if (d.actions !== undefined) base.actions = d.actions;
    if (d.raw_passthrough !== undefined) base.raw_passthrough = d.raw_passthrough;
    // agent/prompt are shared by the step + for_each forms — preserve across those.
    if ((kind === 'step' || kind === 'for_each')) {
      if (d.agent !== undefined) base.agent = d.agent;
      if (d.prompt !== undefined) base.prompt = d.prompt;
    }
    // approval is only editable on step/for_each/approval_gate — carry it there, drop it elsewhere.
    if (kind === 'step' || kind === 'for_each' || kind === 'approval_gate') {
      if (d.approvalRequired !== undefined) base.approvalRequired = d.approvalRequired;
      if (d.approvalPrompt !== undefined) base.approvalPrompt = d.approvalPrompt;
      if (d.approvalTimeoutSeconds !== undefined) base.approvalTimeoutSeconds = d.approvalTimeoutSeconds;
    }
    // seed destination defaults (mirrors newNodeData)
    if (kind === 'parallel') base.parallel = [];
    if (kind === 'panel') base.panel = { panelists: [], subject: '' };
    if (kind === 'branch') { base.condition = ''; base.thenTargets = []; base.elseTargets = []; }
    if (kind === 'action') base.action = '';
    if (kind === 'approval_gate') { base.approvalRequired = true; base.approvalOnReject = []; }
    onChange(base);
  }
```

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/StepForm.test.tsx`
Expected: PASS.

- [ ] **Step 5: Typecheck, full suite, and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit && npx vitest run
git add src/components/workflow-editor/StepForm.tsx src/components/workflow-editor/StepForm.test.tsx
git commit -m "fix(cp): kind-switch seeds destination defaults and preserves shared fields"
```

---

## Operator gate (required before merge)

These bugs were all interaction-time and invisible to unit tests — matt validates in the running app (`make cp-web`, restart `cp serve`, `[cp].workflow_editor_ui='next'`), light + dark:
1. Add a branch, set a Then target in the settings → **the line appears immediately**.
2. Delete a node that a branch pointed at → no dangling reference; save is clean.
3. Try to route a branch back to an upstream step → the option isn't offered; nothing vanishes.
4. Add a fresh action/branch/gate node → an inline "finish this" badge, **no console 400 flood**.
5. Add an `action` node, set a tool → it stays a parallelogram across edits (doesn't revert to a step).
6. Switch a step's kind around → agent/prompt survive step↔for_each; no leftover invisible approval.

## Self-review notes
- Spec §2–§6 + §7 desync fixes all map to Tasks 1-6. Phase 2 authoring gaps (notify UI, typed `with:`, severity `<select>`, passthrough bags) are explicitly out of scope.
- The `deriveEdges` equivalence test (Task 1) is the safety net proving the extraction changed no behavior for valid inputs; the invariant test (Task 3) proves no op can reintroduce a stored/derived split.
