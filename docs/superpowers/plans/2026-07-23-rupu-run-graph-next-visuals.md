# Run-graph "next" Visuals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Behind the existing `workflow_editor_ui = 'next'` flag, repaint the run-detail graph (`/runs/:id`) with the editor's per-kind color language — kind-colored nodes and connectors, with live run status kept as an overlay (glyph badge + label + pulse/marching-ants animation).

**Architecture:** A two-channel paint. One new run-graph-local bridge module (`graph/kindBridge.ts`) is the ONLY file importing the editor's `kindVisuals`, exposing `runKindAccent`/`runKindIcon`/`runKindLabel` keyed by the run model's own kind union. `RunGraph` reads `useWorkflowEditorUi()` once and threads `ui` into the edge memo and every node's `data`; each node component gains a `next` branch while its classic branch stays the existing code. Edge animation colors move from CSS to JS via one additive color-agnostic `.rg-edge-flow` class.

**Tech Stack:** TypeScript/React, @xyflow/react, vitest + @testing-library/react, existing `lib/useThemeColors` token system, Tailwind + `styles.css`.

## Global Constraints

- **Frontend only** — `crates/rupu-cp/web`. No Rust, no API, no new color tokens.
- **Classic must stay visually identical.** Every change is additive behind `ui === 'next'`; each component keeps its existing markup as the classic branch. Reviewers verify the diff *adds* a next branch rather than rewriting classic.
- **Status legibility guard:** in next, the status glyph badge (`stateStyle(...).color`) and the state label stay state-colored at full strength — a failed step must still read as failed. Kind color owns the top-bar/pill only.
- Do **not** set xyflow's `animated` edge prop — `rg-march` is the single animation source (doubling them double-animates; see the existing comment in `RunGraph.tsx:172-175`).
- The `prefers-reduced-motion` guard at `styles.css:547-549` targets `.react-flow__edge-path` broadly, so it already covers the new class — do not add a second guard.
- Verify with `npm run test`, `npx tsc -b`, `npm run build` (cwd `crates/rupu-cp/web`). Never package-wide `cargo fmt`.
- Line refs are main `79b89b2a`; re-locate by the quoted code if drifted.

---

### Task 1: `graph/kindBridge.ts` — run-kind → editor-kind visual bridge

**Files:**
- Create: `crates/rupu-cp/web/src/components/graph/kindBridge.ts`
- Test: `crates/rupu-cp/web/src/components/graph/kindBridge.test.ts`

**Interfaces:**
- Consumes: `KIND_ACCENT`, `KIND_ICON` from `../workflow-editor/kindVisuals`; `StepKind` from `../../lib/workflowGraph`; `ColorKey` from `../../lib/useThemeColors`; `StepNodeDto` from `../../lib/api`.
- Produces (every later task uses these exact names):
  - `runKindToStepKind(kind: StepNodeDto['kind']): StepKind`
  - `runKindAccent(kind: StepNodeDto['kind']): ColorKey`
  - `runKindIcon(kind: StepNodeDto['kind']): LucideIcon`
  - `runKindLabel(kind: StepNodeDto['kind']): string`

The run model's kind union is `'step' | 'for_each' | 'parallel' | 'panel' | 'gate' | 'action'` (confirm against `StepNodeDto` in `lib/api.ts`). Only `gate` differs from the editor's `StepKind` (`approval_gate`); the rest are identity. The editor's `branch` has no run-graph counterpart.

- [ ] **Step 1: Write the failing test**

```ts
// kindBridge — the run graph's single import boundary onto the editor's
// per-kind visual language.
import { describe, it, expect } from 'vitest';
import { runKindToStepKind, runKindAccent, runKindIcon, runKindLabel } from './kindBridge';
import { KIND_ACCENT, KIND_ICON } from '../workflow-editor/kindVisuals';

describe('runKindToStepKind', () => {
  it('maps gate onto the editor approval_gate kind', () => {
    expect(runKindToStepKind('gate')).toBe('approval_gate');
  });

  it('passes every other run kind through unchanged', () => {
    expect(runKindToStepKind('step')).toBe('step');
    expect(runKindToStepKind('for_each')).toBe('for_each');
    expect(runKindToStepKind('parallel')).toBe('parallel');
    expect(runKindToStepKind('panel')).toBe('panel');
    expect(runKindToStepKind('action')).toBe('action');
  });
});

describe('runKindAccent / runKindIcon', () => {
  it('resolves through the editor palette so both graphs share one source', () => {
    expect(runKindAccent('gate')).toBe(KIND_ACCENT.approval_gate);
    expect(runKindAccent('parallel')).toBe(KIND_ACCENT.parallel);
    expect(runKindAccent('for_each')).toBe(KIND_ACCENT.for_each);
    expect(runKindIcon('action')).toBe(KIND_ICON.action);
    expect(runKindIcon('step')).toBe(KIND_ICON.step);
  });
});

describe('runKindLabel', () => {
  it('gives each kind a short human label for the node pill', () => {
    expect(runKindLabel('step')).toBe('step');
    expect(runKindLabel('for_each')).toBe('for each');
    expect(runKindLabel('gate')).toBe('gate');
    expect(runKindLabel('action')).toBe('action');
    expect(runKindLabel('parallel')).toBe('parallel');
    expect(runKindLabel('panel')).toBe('panel');
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run (cwd `crates/rupu-cp/web`): `npx vitest run src/components/graph/kindBridge.test.ts`
Expected: FAIL — cannot resolve `./kindBridge`.

- [ ] **Step 3: Write the implementation**

```ts
// kindBridge — the run graph's SINGLE import boundary onto the workflow
// editor's per-kind visual language (`workflow-editor/kindVisuals`). The run
// model and the editor use different kind unions: the run model emits `gate`
// where the editor's `StepKind` says `approval_gate`, and the run model has no
// `branch`. Everything else is identity. Keeping the mapping here means only
// one run-graph file reaches across into the editor's module.

import type { LucideIcon } from 'lucide-react';
import { KIND_ACCENT, KIND_ICON } from '../workflow-editor/kindVisuals';
import type { StepKind } from '../../lib/workflowGraph';
import type { ColorKey } from '../../lib/useThemeColors';
import type { StepNodeDto } from '../../lib/api';

export type RunKind = StepNodeDto['kind'];

/** Map a run-model step kind onto the editor's `StepKind` vocabulary. */
export function runKindToStepKind(kind: RunKind): StepKind {
  return kind === 'gate' ? 'approval_gate' : (kind as StepKind);
}

/** The themed accent token for a run step's kind (same palette as the editor). */
export function runKindAccent(kind: RunKind): ColorKey {
  return KIND_ACCENT[runKindToStepKind(kind)];
}

/** The lucide icon for a run step's kind (same icons as the editor). */
export function runKindIcon(kind: RunKind): LucideIcon {
  return KIND_ICON[runKindToStepKind(kind)];
}

const LABELS: Record<RunKind, string> = {
  step: 'step',
  for_each: 'for each',
  parallel: 'parallel',
  panel: 'panel',
  gate: 'gate',
  action: 'action',
};

/** Short human label rendered in a node's kind pill. */
export function runKindLabel(kind: RunKind): string {
  return LABELS[kind];
}
```

If `StepNodeDto['kind']` turns out to include values beyond the six above, extend `LABELS` to match (it is a total `Record`, so TypeScript will fail the build until it does) — do not add an index signature.

- [ ] **Step 4: Run test to verify it passes**

Run: `npx vitest run src/components/graph/kindBridge.test.ts` → PASS (3 suites).
Then: `npx tsc -b` → clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/graph/kindBridge.ts crates/rupu-cp/web/src/components/graph/kindBridge.test.ts
git commit -m "feat(cp): kindBridge maps run step kinds onto the editor's kind palette"
```

---

### Task 2: Flag threading + kind-colored animated connectors

**Files:**
- Modify: `crates/rupu-cp/web/src/components/RunGraph.tsx` (`NodeData` `:70-74`, `RunGraphInner` `:100-101`, nodes memo `:141-153`, edges memo `:155-181`)
- Modify: `crates/rupu-cp/web/src/styles.css` (add one class next to `.rg-edge-await`, after `styles.css:230`)
- Test: `crates/rupu-cp/web/src/components/RunGraph.edges.test.tsx` (new)

**Interfaces:**
- Consumes: Task 1's `runKindAccent`; `useWorkflowEditorUi()` from `../hooks/useWorkflowEditorUi` (returns `WorkflowEditorUi = 'classic' | 'next'` — verify the exact export names/path before importing).
- Produces: `NodeData` gains `ui: WorkflowEditorUi` (threaded into every node's `data`, consumed by Tasks 3-4); the CSS class `rg-edge-flow` (color-agnostic marching-ants).

Edge rules in **next** (classic unchanged):
| target state | stroke | animation |
|---|---|---|
| running | source kind color (full) | `rg-edge-flow` |
| awaiting_approval | `status.awaiting` amber | `rg-edge-flow` |
| done | source kind color (full) | none |
| anything else (pending/…) | source kind color at alpha 0.35 | none |

- [ ] **Step 1: Write the failing test**

```tsx
// @vitest-environment jsdom
// RunGraph edges — classic keeps the flat/blue/amber status styling; next
// colors each edge by its SOURCE step's kind and animates only the live
// frontier. We assert on the Edge objects React Flow is handed, by mocking
// @xyflow/react's ReactFlow to capture its props.
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, cleanup } from '@testing-library/react';
import type { Edge } from '@xyflow/react';

let captured: Edge[] = [];
vi.mock('@xyflow/react', async () => {
  const actual = await vi.importActual<typeof import('@xyflow/react')>('@xyflow/react');
  return {
    ...actual,
    ReactFlow: (props: { edges: Edge[] }) => {
      captured = props.edges;
      return <div data-testid="rf" />;
    },
  };
});

const uiMock = vi.fn(() => 'classic');
vi.mock('../hooks/useWorkflowEditorUi', () => ({
  useWorkflowEditorUi: () => uiMock(),
}));

import RunGraph from './RunGraph';
import type { RunGraphModel, GraphNode } from '../lib/runGraphModel';

function node(id: string, kind: GraphNode['kind'], state: GraphNode['state']): GraphNode {
  return { id, kind, state } as GraphNode;
}

function modelWith(nodes: GraphNode[], edges: { from: string; to: string }[]): RunGraphModel {
  return {
    nodes,
    edges,
    nodeById: (id: string) => nodes.find((n) => n.id === id),
  } as unknown as RunGraphModel;
}

afterEach(() => {
  cleanup();
  captured = [];
  uiMock.mockReturnValue('classic');
});

const POS = new Map([
  ['a', { x: 0, y: 0 }],
  ['b', { x: 100, y: 0 }],
]);

describe('RunGraph edges', () => {
  it('classic: the edge into a running step keeps the rg-edge-active class', () => {
    const model = modelWith(
      [node('a', 'action', 'done'), node('b', 'step', 'running')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={model} positions={POS} />);
    expect(captured[0].className).toBe('rg-edge-active');
  });

  it('next: an edge is colored by its SOURCE kind and the live one flows', () => {
    uiMock.mockReturnValue('next');
    const model = modelWith(
      [node('a', 'action', 'done'), node('b', 'step', 'running')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={model} positions={POS} />);
    const edge = captured[0];
    // live frontier animates via the color-agnostic class...
    expect(edge.className).toBe('rg-edge-flow');
    // ...and its stroke comes from JS (the SOURCE step is an action step, so
    // the stroke must NOT be the running-blue the classic path would use).
    expect(edge.style?.stroke).toBeTruthy();
    expect(edge.style?.stroke).not.toBe('');
  });

  it('next: an edge into an awaiting gate flows in amber, not the source kind color', () => {
    uiMock.mockReturnValue('next');
    const model = modelWith(
      [node('a', 'action', 'done'), node('b', 'gate', 'awaiting_approval')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={model} positions={POS} />);
    const awaitEdge = captured[0];
    expect(awaitEdge.className).toBe('rg-edge-flow');

    // Compare against the same topology with a non-awaiting target: the
    // awaiting edge's stroke must differ (amber status wins over kind).
    cleanup();
    captured = [];
    const model2 = modelWith(
      [node('a', 'action', 'done'), node('b', 'gate', 'done')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={model2} positions={POS} />);
    expect(captured[0].style?.stroke).not.toBe(awaitEdge.style?.stroke);
  });

  it('next: a not-yet-reached edge is muted relative to a traversed one', () => {
    uiMock.mockReturnValue('next');
    const done = modelWith(
      [node('a', 'step', 'done'), node('b', 'step', 'done')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={done} positions={POS} />);
    const traversed = captured[0].style?.stroke;

    cleanup();
    captured = [];
    const pending = modelWith(
      [node('a', 'step', 'done'), node('b', 'step', 'pending')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={pending} positions={POS} />);
    expect(captured[0].style?.stroke).not.toBe(traversed);
  });
});
```

If `RunGraphModel`/`GraphNode` shapes make the casts above awkward, build the model with the real `buildRunGraphModel` from `lib/runGraphModel` seeded by DTOs instead — read `runGraphModel.test.ts` for the established construction pattern and follow it. Keep the assertions identical.

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/RunGraph.edges.test.tsx`
Expected: FAIL — the `next` cases fail because every edge is still classic-styled (`rg-edge-active`, `colors.inkMute`), and `useWorkflowEditorUi` isn't imported by `RunGraph` yet.

- [ ] **Step 3: Write the implementation**

In `RunGraph.tsx`, add the imports:

```tsx
import { useWorkflowEditorUi, type WorkflowEditorUi } from '../hooks/useWorkflowEditorUi';
import { runKindAccent } from './graph/kindBridge';
```

Extend `NodeData` (`:70-74`):

```tsx
interface NodeData extends Record<string, unknown> {
  node: GraphNode;
  /** Resolved visual language — 'next' turns on the kind-colored paint. */
  ui: WorkflowEditorUi;
  onOpenUnit?: (stepId: string, index: number) => void;
  onExpandFanout?: (stepId: string) => void;
}
```

In `RunGraphInner`, read the flag next to `colors` (`:101`):

```tsx
  const colors = useThemeColors();
  const ui = useWorkflowEditorUi();
```

Thread it in the nodes memo (`:148`) and add `ui` to the dep array (`:153`):

```tsx
        data: { node, ui, onOpenUnit: handleOpenUnit, onExpandFanout },
```
```tsx
  }, [model, positions, ui, handleOpenUnit, onExpandFanout]);
```

Replace the edges memo body (`:155-181`) with:

```tsx
  const edges = useMemo<Edge[]>(() => {
    return model.edges.map((e) => {
      const target = model.nodeById(e.to);
      const targetState = target?.state;
      const active = targetState === 'running';
      const awaiting = targetState === 'awaiting_approval';

      if (ui !== 'next') {
        // Classic — unchanged: flat inkMute, with the blue/amber marching-ants
        // classes owning both color and animation for the live frontier.
        const stroke = active
          ? colors.status.running
          : awaiting
            ? colors.status.awaiting
            : colors.inkMute;
        return {
          id: `${e.from}->${e.to}`,
          source: e.from,
          target: e.to,
          type: 'smoothstep',
          className: active ? 'rg-edge-active' : awaiting ? 'rg-edge-await' : undefined,
          markerEnd: { type: MarkerType.ArrowClosed, color: stroke },
          style: active || awaiting ? undefined : { stroke, strokeWidth: 2 },
        };
      }

      // Next — the connector takes its SOURCE step's kind color; the live
      // frontier animates via the color-agnostic `rg-edge-flow` class (stroke
      // supplied here in JS so the ants march in ANY kind color). An awaiting
      // gate is the one place status wins over kind: amber reads as "needs
      // you" regardless of what kind of step precedes it.
      const source = model.nodeById(e.from);
      const accent = source ? runKindAccent(source.kind) : 'inkMute';
      const traversed = active || targetState === 'done';
      const stroke = awaiting
        ? colors.status.awaiting
        : traversed
          ? colors.get(accent)
          : colors.alpha(accent, 0.35);
      return {
        id: `${e.from}->${e.to}`,
        source: e.from,
        target: e.to,
        type: 'smoothstep',
        className: active || awaiting ? 'rg-edge-flow' : undefined,
        markerEnd: { type: MarkerType.ArrowClosed, color: stroke },
        style: { stroke, strokeWidth: 2 },
      };
    });
  }, [model, colors, ui]);
```

Add the CSS class in `styles.css` immediately after the `.rg-edge-await` rule (which ends at `:230`) — note it deliberately sets **no** `stroke`:

```css
/* `next` run-graph flow: animation only, no color. The stroke is supplied by
 * the edge's inline style (RunGraph's edge memo) so the marching-ants can
 * render in ANY per-kind accent. Classic keeps .rg-edge-active/.rg-edge-await
 * above, which bake their own blue/amber stroke. */
.react-flow__edge.rg-edge-flow .react-flow__edge-path {
  stroke-width: 2;
  stroke-dasharray: 7 7;
  animation: rg-march 0.7s linear infinite;
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `npx vitest run src/components/RunGraph.edges.test.tsx` → PASS (4 tests).
Run: `npx vitest run src/components` → no regressions in the other graph tests.
Run: `npx tsc -b` → clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/RunGraph.tsx crates/rupu-cp/web/src/components/RunGraph.edges.test.tsx crates/rupu-cp/web/src/styles.css
git commit -m "feat(cp): kind-colored, active-animated run-graph connectors behind next"
```

---

### Task 3: Leaf nodes — StepNode, GateNode, ActionNode

**Files:**
- Modify: `crates/rupu-cp/web/src/components/graph/StepNode.tsx`, `GateNode.tsx`, `ActionNode.tsx`
- Test: `crates/rupu-cp/web/src/components/graph/StepNode.test.tsx` (new), extend `GateNode.test.tsx` + `ActionNode.test.tsx`

**Interfaces:**
- Consumes: Task 1's `runKindAccent`/`runKindIcon`/`runKindLabel`; Task 2's `data.ui`.
- Produces: each component's `*NodeData` interface gains `ui?: WorkflowEditorUi` (optional so existing tests that build data without it keep compiling and exercise classic).

**The two-channel rule (identical in all three):** in next, the top-bar background and the kind pill use `colors.get(runKindAccent(node.kind))`; the status glyph badge and the state label keep `stateStyle(colors, node.state).color`; the `rg-pulse-run`/`rg-pulse-await` ring classes stay exactly as they are.

- [ ] **Step 1: Write the failing tests**

New `StepNode.test.tsx` (model the harness on the existing `GateNode.test.tsx` — it wraps in `ReactFlowProvider` because `Handle` requires it; read it first):

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { ReactFlowProvider } from '@xyflow/react';
import StepNode from './StepNode';
import type { GraphNode } from '../../lib/runGraphModel';

function renderNode(node: Partial<GraphNode>, ui?: 'classic' | 'next') {
  const data = { node: { id: 'build', kind: 'step', state: 'running', ...node } as GraphNode, ui };
  return render(
    <ReactFlowProvider>
      {/* React Flow node components are called with (props); the harness in
          GateNode.test.tsx shows the exact prop shape — mirror it. */}
      <StepNode data={data} id="build" type="step" selected={false} zIndex={0} isConnectable={false}
        positionAbsoluteX={0} positionAbsoluteY={0} dragging={false} />
    </ReactFlowProvider>,
  );
}

afterEach(cleanup);

describe('StepNode', () => {
  it('classic: renders the step id and the neutral kind chip', () => {
    renderNode({}, 'classic');
    expect(screen.getByText('build')).toBeInTheDocument();
    expect(screen.getByText('step')).toBeInTheDocument();
  });

  it('next: keeps the status overlay legible on a FAILED step', () => {
    renderNode({ state: 'failed' }, 'next');
    // the status glyph + label must survive the kind repaint
    expect(screen.getByText('✕')).toBeInTheDocument();
    expect(screen.getByText('failed')).toBeInTheDocument();
    // and the kind identity is present
    expect(screen.getByText('step')).toBeInTheDocument();
  });

  it('next: renders a kind pill for the step kind', () => {
    renderNode({ kind: 'step' }, 'next');
    expect(screen.getByTestId('rg-kindpill')).toHaveTextContent('step');
  });
});
```

Check the real glyph/label strings against `graph/stepStyle.ts` (`GLYPH_LABEL`) and use those exact values — `✕`/`failed` above must match the table.

For `GateNode.test.tsx` and `ActionNode.test.tsx`, add one case each:

```tsx
  it('next: renders a kind pill alongside the existing status treatment', () => {
    // …render the node with data.ui = 'next' using the file's existing helper…
    expect(screen.getByTestId('rg-kindpill')).toBeInTheDocument();
    // and the status signal the file already asserts (awaiting/⏸, tool name, …)
    // must still be asserted in the same test.
  });
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `npx vitest run src/components/graph` → FAIL: `rg-kindpill` not found; StepNode.test.tsx is new.

- [ ] **Step 3: Write the implementation**

In each of the three components, add the imports and the branch. For `StepNode.tsx`:

```tsx
import { runKindAccent, runKindIcon, runKindLabel } from './kindBridge';
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';
```

```tsx
export interface StepNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** 'next' turns on the kind-colored paint; absent/'classic' keeps today's. */
  ui?: WorkflowEditorUi;
}
```

Inside the view, after `const s = stateStyle(colors, node.state);`:

```tsx
  const next = data.ui === 'next';
  // Kind channel (identity) vs status channel (overlay): in next the top-bar
  // and pill carry the step's KIND color while the glyph badge + label keep
  // the STATE color, so a failed step still reads as failed.
  const accent = runKindAccent(node.kind);
  const barColor = next ? colors.get(accent) : s.color;
  const KindIcon = runKindIcon(node.kind);
```

Top-bar (`:41-44`) uses `barColor`:

```tsx
      <div
        className="absolute left-0 right-0 top-0 h-[3px] rounded-t-[10px]"
        style={{ background: barColor }}
      />
```

The glyph badge (`:47-53`) is **unchanged** (`background: s.color`) — that is the status channel.

Replace the neutral kind chip (`:60-63`) with a branch:

```tsx
      <div className="mt-1.5 flex items-center gap-1.5">
        {next ? (
          <span
            data-testid="rg-kindpill"
            className="inline-flex items-center gap-1 rounded px-1.5 py-px text-meta font-medium"
            style={{ background: colors.alpha(accent, 0.14), color: colors.get(accent) }}
          >
            <KindIcon size={10} aria-hidden />
            {runKindLabel(node.kind)}
          </span>
        ) : (
          <span className="rounded bg-surface px-1.5 py-px text-meta text-ink-dim">
            {node.kind === 'panel' ? 'panel' : 'step'}
          </span>
        )}
        {node.agent && (
          <span className="truncate rounded bg-surface px-1.5 py-px text-meta text-ink-dim">
            {node.agent}
          </span>
        )}
      </div>
```

Apply the identical pattern to `GateNode.tsx` and `ActionNode.tsx`: read each file, switch its top-bar/accent source to `barColor`, and replace its existing neutral kind chip (`◇ gate`, `action`/`connector`) with the same `data-testid="rg-kindpill"` accent pill in next while leaving its classic markup untouched. Keep GateNode's dashed border and its auto/on-reject captions, and ActionNode's tool name, in both modes.

- [ ] **Step 4: Run tests to verify they pass**

Run: `npx vitest run src/components/graph` → PASS.
Run: `npx tsc -b` → clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/graph/StepNode.tsx crates/rupu-cp/web/src/components/graph/StepNode.test.tsx crates/rupu-cp/web/src/components/graph/GateNode.tsx crates/rupu-cp/web/src/components/graph/GateNode.test.tsx crates/rupu-cp/web/src/components/graph/ActionNode.tsx crates/rupu-cp/web/src/components/graph/ActionNode.test.tsx
git commit -m "feat(cp): kind-colored leaf run nodes with status overlay behind next"
```

---

### Task 4: Container nodes — ParallelNode, PanelLoopNode, FanoutNode

**Files:**
- Modify: `crates/rupu-cp/web/src/components/graph/ParallelNode.tsx` (container tint `:37-46`), `PanelLoopNode.tsx` (`:44-53`), `FanoutNode.tsx` (`:107-110,151`)
- Test: `crates/rupu-cp/web/src/components/graph/ContainerNodes.test.tsx` (new — one file covering all three)

**Interfaces:**
- Consumes: Task 1's `runKindAccent`; Task 2's `data.ui`.
- Produces: each container's `*NodeData` gains `ui?: WorkflowEditorUi` (same optional shape as Task 3).

Today all three containers hardcode a tint that does **not** match the editor: Parallel and Panel use `brand.500` (violet) and Fanout uses `status.running`. In next, each takes its real kind accent: parallel → `runKindAccent('parallel')` (`sev.critical`), panel → `runKindAccent('panel')` (`status.awaiting`), for_each → `runKindAccent('for_each')` (`brand.500`, i.e. Fanout's tint *changes* from running-blue to violet). Per-unit and per-sub-step chips keep their state colors (status channel) untouched.

- [ ] **Step 1: Write the failing test**

```tsx
// @vitest-environment jsdom
// Container run nodes take their KIND accent in next; the per-sub-step /
// per-unit chips stay state-colored. We assert the container tint CHANGES
// between classic and next rather than pinning a hex (the token resolves
// through CSS vars that jsdom does not compute) — the point is that kind now
// drives it, and that the status channel survives.
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { ReactFlowProvider, type NodeProps } from '@xyflow/react';
import ParallelNode from './ParallelNode';
import type { GraphNode } from '../../lib/runGraphModel';

afterEach(cleanup);

// Same harness shape as GateNode.test.tsx: `Handle` needs a provider ancestor.
function renderParallel(node: GraphNode, ui?: 'classic' | 'next') {
  const props = {
    id: node.id,
    data: { node, ui },
    type: 'parallel',
    dragging: false,
    zIndex: 0,
    selectable: true,
    deletable: true,
    selected: false,
    draggable: false,
    isConnectable: true,
    positionAbsoluteX: 0,
    positionAbsoluteY: 0,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  } as unknown as NodeProps<any>;
  return render(
    <ReactFlowProvider>
      <ParallelNode {...props} />
    </ReactFlowProvider>,
  );
}

const PARALLEL: GraphNode = {
  id: 'fanwork',
  kind: 'parallel',
  state: 'running',
  parallel: [
    { id: 'lint', state: 'done' },
    { id: 'test', state: 'running' },
  ],
} as unknown as GraphNode;

describe('ParallelNode', () => {
  it('next: keeps the status channel — every sub-step chip still renders', () => {
    renderParallel(PARALLEL, 'next');
    expect(screen.getByText('lint')).toBeInTheDocument();
    expect(screen.getByText('test')).toBeInTheDocument();
    expect(screen.getByText(/parallel · fanwork/)).toBeInTheDocument();
  });

  it('next: the container tint switches from the legacy brand violet to the parallel kind accent', () => {
    const classic = renderParallel(PARALLEL, 'classic');
    const classicTint = (classic.container.querySelector('[data-testid="rg-container"]') as HTMLElement)
      .style.borderColor;
    cleanup();
    const next = renderParallel(PARALLEL, 'next');
    const nextTint = (next.container.querySelector('[data-testid="rg-container"]') as HTMLElement)
      .style.borderColor;
    expect(nextTint).not.toBe(classicTint);
  });
});
```

Then add the analogous pair for `PanelLoopNode` and `FanoutNode` in the same file, built from each component's real props (read all three components first — `ParallelNode` takes `node.parallel`, `FanoutNode` takes `node.fanout.units`, `PanelLoopNode` takes the panel/gate fields). Assertions, per component: (a) with `ui:'next'` its per-unit/sub-step state chips still render (status channel preserved), and (b) its `[data-testid="rg-container"]` tint differs between classic and next — **for `PanelLoopNode` skip assertion (b)**: panel's kind accent (`status.awaiting`) and its legacy tint may resolve to different tokens but the same computed string in jsdom; assert only (a) plus that the loop cue still renders. Step 3 adds `data-testid="rg-container"` to each container's root element.

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/graph/ContainerNodes.test.tsx` → FAIL (tint identical across modes / testid missing).

- [ ] **Step 3: Write the implementation**

In each container, add `ui?: WorkflowEditorUi` to its data interface, import `runKindAccent` from `./kindBridge`, and compute the tint token once:

```tsx
  const next = data.ui === 'next';
  // Kind identity for the container: in next this is the SAME accent the
  // editor paints this kind with; classic keeps the legacy brand/running tint.
  const accentKey = next ? runKindAccent(node.kind) : 'brand.500'; // FanoutNode: 'status.running'
```

then replace the hardcoded token in that component's existing `colors.alpha('brand.500', …)` / `colors.get('brand.500')` / `text-brand-500` usages with `colors.alpha(accentKey, …)` / `colors.get(accentKey)` (a Tailwind class like `text-brand-500` becomes an inline `style={{ color: colors.get(accentKey) }}` in next; keep the class in classic). Add `data-testid="rg-container"` to each container's root element if the test needs it.

Leave every state-colored element alone: `ParallelNode`'s sub-step chips, `FanoutNode`'s `glyphBg(colors, u.state)` unit squares and its running→done progress gradient, `PanelLoopNode`'s `status.awaiting` gate block and `rg-loop-spin` cue.

- [ ] **Step 4: Run tests to verify they pass**

Run: `npx vitest run src/components/graph` → PASS.
Run: `npx tsc -b` → clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/graph/ParallelNode.tsx crates/rupu-cp/web/src/components/graph/PanelLoopNode.tsx crates/rupu-cp/web/src/components/graph/FanoutNode.tsx crates/rupu-cp/web/src/components/graph/ContainerNodes.test.tsx
git commit -m "feat(cp): container run nodes take their kind accent behind next"
```

---

### Task 5: Full verification + docs

**Files:**
- Modify: `CLAUDE.md` (the `rupu-cp` / web bullet)
- No new source files.

- [ ] **Step 1: Run the full web suite**

Run (cwd `crates/rupu-cp/web`):
```bash
npm run test
npx tsc -b
npm run build
```
Expected: all tests pass, tsc clean, build clean (the chunk-size advisory is pre-existing).

- [ ] **Step 2: Confirm classic is untouched**

Run: `git diff main -- crates/rupu-cp/web/src/components/graph crates/rupu-cp/web/src/components/RunGraph.tsx`
Read the diff and confirm every change is additive behind a `next` branch — no classic markup rewritten, no classic CSS rule modified (`.rg-edge-active`/`.rg-edge-await` untouched). Report any classic-path change in your report.

- [ ] **Step 3: Update CLAUDE.md**

Add to the rupu-cp bullet: the run-detail graph shares the editor's per-kind palette behind `[cp].workflow_editor_ui = "next"` (kind-colored nodes + connectors, run status as a glyph/label/animation overlay) via `components/graph/kindBridge.ts`; landed per `docs/superpowers/plans/2026-07-23-rupu-run-graph-next-visuals.md`.

- [ ] **Step 4: Write the visual-check checklist into the report**

The implementer cannot validate rendering. Write into the task report the exact checklist for matt (browser, light + dark, `localStorage.setItem('rupu.cp.workflowEditorUi','next')`):
1. Run mid-flight — the running node pulses and the edge into it marches in the *source step's* kind color.
2. Completed run — solid kind-colored edges, kind-colored bars, ✓ badges still obvious.
3. Failed step — the red ✕ badge and "failed" label still read at a glance against the kind-colored bar.
4. Awaiting gate — amber marching edge + amber glow.
5. A parallel and a for_each container — tints now match the editor's palette (red / violet).
6. Flag off (`classic`) — the graph is pixel-identical to today.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: note the next run-graph visual language in CLAUDE.md"
```

---

## Out of scope (deliberately)

- Flipping `workflow_editor_ui`'s default to `next` — a separate call after matt's visual check.
- The editor's canvas wash / `Background Lines` treatment for the run graph (the run graph keeps `Background Dots`); revisit only if the visual check asks for it.
- Any change to `graph/stepStyle.ts` — it remains the status channel, untouched.
- `branch` steps — the run model has no `branch` kind; nothing to paint.
