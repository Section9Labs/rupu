# Flow Designer Node Shapes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give each Flow Designer node kind its own flowchart silhouette (diamond, parallelogram, trapezoid, hexagon, subroutine, stacked) so a node's kind survives zoom-out and reads without colour.

**Architecture:** A new pure geometry module (`nodeShapes.ts`) declares, per shape, the silhouette polygon, the *safe rectangle* content may occupy, the text alignment, and where handles anchor. `EditableStepNode`'s `next` render path paints that polygon as an **SVG layer** (not `clip-path`, which slices borders and cannot clip the outward selection glow) and positions its content inside the safe rect. `editorNodeSize()` gains matching per-kind boxes in the same change so dagre's reservation still equals what renders.

**Tech Stack:** React 18 + TypeScript, `@xyflow/react`, `@dagrejs/dagre`, Vitest + Testing Library, plain CSS custom properties (no Tailwind in `.wfx-*`).

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-23-rupu-flow-designer-node-shapes-design.md`. Approved visual reference: https://claude.ai/code/artifact/98bac423-71f0-4408-bf4f-1a2d8e2d0df4
- **Branch:** `flow-designer-shapes`, off main `79b89b2a` (v0.68.0).
- **All work is in `crates/rupu-cp/web/`.** Run every command from that directory.
- **`next` path only.** `EditableStepNode`'s `classic` render path (the `return` after the `if (ui === 'next')` block, ~line 404 onward) must stay **byte-identical**. Never edit it.
- **No backend change, no schema change, no new npm dependency.**
- **Tokens only** — colours come from `rgb(var(--c-*))` custom properties or `useThemeColors()`. No colour literals in new code.
- **Handle ids `'then'` / `'else'` are a model contract.** `applyConnect` (`WorkflowEditorGraph.tsx:90`) reads `sourceHandle` to write `thenTargets`/`elseTargets`. Their *position* may change; their `id` may never change.
- **The `render == reservation` invariant** (documented at `lib/workflowLayout.ts:8-11`): whatever `editorNodeSize()` returns for a kind is exactly what the node renders as `width` / `minHeight`. Any size change touches both in one commit.
- Test command: `npx vitest run <path>` from `crates/rupu-cp/web`.
- Typecheck command: `npx tsc -b --noEmit` from `crates/rupu-cp/web`.

## File Structure

| File | Responsibility |
|---|---|
| `src/components/workflow-editor/nodeShapes.ts` | **Create.** Pure geometry: shape name → polygon, SVG path, safe rect, alignment, handle anchors. No React, no DOM, no colour. |
| `src/components/workflow-editor/nodeShapes.test.ts` | **Create.** Geometry tests incl. a point-in-polygon check that every safe rect really is inside its silhouette. |
| `src/components/workflow-editor/kindVisuals.ts` | **Modify.** Add `KIND_SHAPE: Record<StepKind, ShapeName>` beside the existing accent/icon maps. |
| `src/lib/workflowLayout.ts` | **Modify.** Per-kind size constants + `editorNodeSize()` cases for `branch`/`action`/`approval_gate`/`for_each`. |
| `src/lib/workflowLayout.test.ts` | **Modify.** Assert the new per-kind boxes. |
| `src/components/workflow-editor/nodes/EditableStepNode.tsx` | **Modify (`next` path only).** SVG silhouette layer, safe-rect content box, `.wfx-bar` retirement, shape-aware alignment and handle anchors. |
| `src/components/workflow-editor/nodes/EditableStepNode.test.tsx` | **Modify.** Update the DOM-locking assertions at lines 260-293 to the new contract. |
| `src/styles.css` | **Modify.** `.wfx-node` loses its border/background/shadow (the SVG paints them); add `.wfx-sil`, `.wfx-safe`; delete `.wfx-bar`. |
| `src/components/workflow-editor/NodePalette.tsx` | **Modify.** Block chips render the silhouette in miniature. |
| `src/components/workflow-editor/NodePalette.test.tsx` | **Modify.** Assert the chip renders its shape. |
| `src/components/workflow-editor/WorkflowEditorGraph.tsx` | **Modify.** `applyAddConnectedNext` offsets by the *source* node's width. |

---

### Task 1: The geometry module

**Files:**
- Create: `crates/rupu-cp/web/src/components/workflow-editor/nodeShapes.ts`
- Test: `crates/rupu-cp/web/src/components/workflow-editor/nodeShapes.test.ts`

**Interfaces:**
- Consumes: `StepKind` from `../../lib/workflowGraph` (type only).
- Produces: `type ShapeName`, `interface SafeRect`, `interface HandleAnchor`, `interface SourceAnchor`, `interface NodeShape`, and `shapeFor(shape: ShapeName, w: number, h: number): NodeShape`. Tasks 2, 3, 4 and 5 all depend on these exact names.

- [ ] **Step 1: Write the failing test**

Create `src/components/workflow-editor/nodeShapes.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { shapeFor, type ShapeName } from './nodeShapes';

/** Ray-casting point-in-polygon. Points exactly ON an edge may report either
 *  way, so callers test corners that should be strictly inside. */
function inside(pt: [number, number], poly: [number, number][]): boolean {
  const [x, y] = pt;
  let hit = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i];
    const [xj, yj] = poly[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) hit = !hit;
  }
  return hit;
}

const ALL: ShapeName[] = [
  'rect',
  'diamond',
  'parallelogram',
  'trapezoid',
  'hexagon',
  'subroutine',
  'stacked',
];

describe('shapeFor', () => {
  it.each(ALL)('%s: every corner of the safe rect lies inside the silhouette', (name) => {
    const s = shapeFor(name, 220, 130);
    const { x, y, w, h } = s.safe;
    const corners: [number, number][] = [
      [x, y],
      [x + w, y],
      [x, y + h],
      [x + w, y + h],
    ];
    for (const c of corners) {
      expect(inside(c, s.points), `${name} corner ${c.join(',')} escaped the shape`).toBe(true);
    }
  });

  it.each(ALL)('%s: the path is closed and starts with a move', (name) => {
    const s = shapeFor(name, 220, 130);
    expect(s.path.startsWith('M ')).toBe(true);
    expect(s.path.trimEnd().endsWith('Z')).toBe(true);
  });

  it('a diamond has its four vertices at the box mid-points', () => {
    const s = shapeFor('diamond', 200, 124);
    expect(s.points).toEqual([
      [100, 2],
      [198, 62],
      [100, 122],
      [2, 62],
    ]);
  });

  it('a diamond centres its text — every other shape aligns to the start', () => {
    expect(shapeFor('diamond', 200, 124).align).toBe('center');
    for (const name of ALL.filter((n) => n !== 'diamond')) {
      expect(shapeFor(name, 220, 130).align, name).toBe('start');
    }
  });

  it('a diamond anchors then on the right vertex and else on the bottom vertex', () => {
    const s = shapeFor('diamond', 200, 124);
    expect(s.target).toEqual({ side: 'left', offset: '50%' });
    expect(s.sources).toEqual([
      { id: 'then', anchor: { side: 'right', offset: '50%' } },
      { id: 'else', anchor: { side: 'bottom', offset: '50%' } },
    ]);
    // both anchor points are real vertices of the polygon, not mid-slope
    expect(s.points).toContainEqual([198, 62]); // right vertex  == then
    expect(s.points).toContainEqual([100, 122]); // bottom vertex == else
  });

  it('every non-diamond shape has one unlabelled source on the right edge', () => {
    for (const name of ALL.filter((n) => n !== 'diamond')) {
      const s = shapeFor(name, 220, 130);
      expect(s.target, name).toEqual({ side: 'left', offset: '50%' });
      expect(s.sources, name).toEqual([{ anchor: { side: 'right', offset: '50%' } }]);
    }
  });

  it('a subroutine adds its two vertical bars as extra strokes', () => {
    const s = shapeFor('subroutine', 220, 130);
    expect(s.extra).toHaveLength(2);
    expect(s.extra[0]).toContain('M 11 2');
    expect(s.extra[1]).toContain('M 209 2');
  });

  it('a stacked shape adds two offset layer strokes behind its body', () => {
    expect(shapeFor('stacked', 220, 130).extra).toHaveLength(2);
  });

  it('shapes that steal horizontal room inset their safe rect past the slope', () => {
    // parallelogram shears by 20px per side; the safe rect must clear both.
    const p = shapeFor('parallelogram', 214, 80);
    expect(p.safe.x).toBeGreaterThanOrEqual(20);
    expect(p.safe.x + p.safe.w).toBeLessThanOrEqual(214 - 20);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/nodeShapes.test.ts`
Expected: FAIL — `Failed to resolve import "./nodeShapes"`.

- [ ] **Step 3: Write the implementation**

Create `src/components/workflow-editor/nodeShapes.ts`:

```ts
// nodeShapes — pure silhouette geometry for the Flow Designer's `next` nodes.
//
// Each node KIND paints a flowchart symbol (see kindVisuals.KIND_SHAPE):
// step→rect, branch→diamond, action→parallelogram, approval_gate→trapezoid,
// for_each→hexagon, parallel→subroutine, panel→stacked. This module owns the
// geometry only — no React, no DOM, no colour. The component paints `path`
// (plus `extra`) into an SVG layer and positions its content inside `safe`.
//
// Two rules encoded here, both from the approved design:
//  1. `safe` is inscribed at the shape's NARROWEST row, so text can never
//     overrun the outline (truncation is bounded by the safe rect, not the
//     bounding box).
//  2. `align` is part of the shape. A silhouette whose width varies across the
//     text band (the diamond) CENTRES its content — left-aligned text there
//     starts on the slope and reads as spilling outside the outline.

/** Stroke inset, in px — keeps the 1.5px silhouette stroke off the box edge so
 *  it is never half-clipped by the SVG viewBox. */
const I = 2;
/** Corner radius of the plain `rect` silhouette (matches the old `.wfx-node`). */
const R = 12;
/** Horizontal shear of a parallelogram, per side. */
const SHEAR = 20;
/** How far a trapezoid's top edge is inset, per side. */
const TAPER = 26;
/** How far a hexagon's left/right points reach in from the box edge. */
const POINT = 22;
/** Inset of a subroutine's two vertical rails from the box edge. */
const RAIL = 11;
/** Offset of a stacked shape's layers behind its body. */
const LAYER = 9;

export type ShapeName =
  | 'rect'
  | 'diamond'
  | 'parallelogram'
  | 'trapezoid'
  | 'hexagon'
  | 'subroutine'
  | 'stacked';

/** Where content may live, in box coordinates. */
export interface SafeRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A handle position expressed against the box, not a hardcoded percentage of
 *  a rectangle. `offset` is a CSS length applied along `side` (`top` for
 *  left/right, `left` for bottom). */
export interface HandleAnchor {
  side: 'left' | 'right' | 'bottom';
  offset: string;
}

/** A source handle. `id` is omitted for the single default source; `branch`
 *  reports the two arms, whose ids are a MODEL CONTRACT (applyConnect reads
 *  them) even though their positions are shape-derived. */
export interface SourceAnchor {
  id?: 'then' | 'else';
  anchor: HandleAnchor;
}

export interface NodeShape {
  /** Silhouette vertices. `rect` reports its un-rounded corners. */
  points: [number, number][];
  /** SVG `d` for the filled+stroked silhouette. */
  path: string;
  /** Extra `d` strings stroked (never filled) on top — rails, stack layers. */
  extra: string[];
  safe: SafeRect;
  align: 'start' | 'center';
  target: HandleAnchor;
  sources: SourceAnchor[];
}

function toPath(points: [number, number][]): string {
  return `M ${points.map(([x, y]) => `${x} ${y}`).join(' L ')} Z`;
}

/** Rounded rectangle — the only silhouette whose painted path differs from its
 *  polygon (the polygon is the un-rounded box, used for geometry tests). */
function roundedRectPath(w: number, h: number): string {
  const l = I;
  const t = I;
  const r = w - I;
  const b = h - I;
  return (
    `M ${l + R} ${t} L ${r - R} ${t} Q ${r} ${t} ${r} ${t + R} ` +
    `L ${r} ${b - R} Q ${r} ${b} ${r - R} ${b} ` +
    `L ${l + R} ${b} Q ${l} ${b} ${l} ${b - R} ` +
    `L ${l} ${t + R} Q ${l} ${t} ${l + R} ${t} Z`
  );
}

const LEFT_TARGET: HandleAnchor = { side: 'left', offset: '50%' };
const RIGHT_SOURCE: SourceAnchor[] = [{ anchor: { side: 'right', offset: '50%' } }];

/** Geometry for one silhouette at a given box size. Pure. */
export function shapeFor(shape: ShapeName, w: number, h: number): NodeShape {
  // Shared defaults. `extra` is deliberately NOT here: `as const` would make it
  // a readonly tuple, which is not assignable to `string[]`.
  const base = { align: 'start', target: LEFT_TARGET, sources: RIGHT_SOURCE } as const;

  switch (shape) {
    case 'diamond': {
      const points: [number, number][] = [
        [w / 2, I],
        [w - I, h / 2],
        [w / 2, h - I],
        [I, h / 2],
      ];
      return {
        points,
        path: toPath(points),
        extra: [],
        // inscribed at the band's narrowest rows (y = .32h and .68h)
        safe: { x: w * 0.23, y: h * 0.32, w: w * 0.54, h: h * 0.36 },
        align: 'center',
        target: LEFT_TARGET,
        sources: [
          { id: 'then', anchor: { side: 'right', offset: '50%' } },
          { id: 'else', anchor: { side: 'bottom', offset: '50%' } },
        ],
      };
    }

    case 'parallelogram': {
      const points: [number, number][] = [
        [SHEAR, I],
        [w - I, I],
        [w - SHEAR, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: SHEAR + 8, y: 11, w: w - 2 * SHEAR - 16, h: h - 22 },
      };
    }

    case 'trapezoid': {
      const points: [number, number][] = [
        [TAPER, I],
        [w - TAPER, I],
        [w - I, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: TAPER + 7, y: 13, w: w - 2 * TAPER - 14, h: h - 26 },
      };
    }

    case 'hexagon': {
      const points: [number, number][] = [
        [POINT, I],
        [w - POINT, I],
        [w - I, h / 2],
        [w - POINT, h - I],
        [POINT, h - I],
        [I, h / 2],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: POINT + 7, y: 11, w: w - 2 * POINT - 14, h: h - 22 },
      };
    }

    case 'subroutine': {
      const points: [number, number][] = [
        [I, I],
        [w - I, I],
        [w - I, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [`M ${RAIL} ${I} L ${RAIL} ${h - I}`, `M ${w - RAIL} ${I} L ${w - RAIL} ${h - I}`],
        safe: { x: RAIL + 8, y: 11, w: w - 2 * RAIL - 16, h: h - 22 },
      };
    }

    case 'stacked': {
      // body sits down-left; the layers peek out up-right.
      const points: [number, number][] = [
        [I, LAYER + I],
        [w - LAYER - I, LAYER + I],
        [w - LAYER - I, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [
          `M ${LAYER} ${I + 3} L ${w - I - 3} ${I + 3} L ${w - I - 3} ${h - LAYER}`,
          `M ${LAYER - 3} ${I + 6} L ${w - I - 6} ${I + 6} L ${w - I - 6} ${h - LAYER - 3}`,
        ],
        safe: { x: 13, y: LAYER + 10, w: w - LAYER - 24, h: h - LAYER - 21 },
      };
    }

    case 'rect':
    default: {
      const points: [number, number][] = [
        [I, I],
        [w - I, I],
        [w - I, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: roundedRectPath(w, h),
        extra: [],
        safe: { x: 15, y: 11, w: w - 30, h: h - 22 },
      };
    }
  }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/nodeShapes.test.ts`
Expected: PASS, 9 tests.

If the point-in-polygon test fails for `stacked` or `trapezoid`, the safe rect is wrong — widen the inset until every corner is strictly inside. Do **not** loosen the test.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/components/workflow-editor/nodeShapes.ts src/components/workflow-editor/nodeShapes.test.ts
git commit -m "feat(cp): pure silhouette geometry for Flow Designer nodes"
```

---

### Task 2: Per-kind boxes and the kind→shape map

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/workflowLayout.ts:25-64`
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/kindVisuals.ts`
- Test: `crates/rupu-cp/web/src/lib/workflowLayout.test.ts`

**Interfaces:**
- Consumes: `ShapeName` from `./nodeShapes` (Task 1).
- Produces: `KIND_SHAPE: Record<StepKind, ShapeName>` from `kindVisuals.ts`; new size exports `BRANCH_W`, `BRANCH_H`, `ACTION_W`, `GATE_W`, `FOR_EACH_W` from `lib/workflowLayout.ts`. Task 3 uses both.

- [ ] **Step 1: Write the failing tests**

Append to `src/lib/workflowLayout.test.ts` (inside the existing top-level `describe`, or as a new `describe` at the end of the file):

```ts
describe('editorNodeSize — per-kind shape boxes', () => {
  it('a branch reserves a taller, narrower box for its diamond', () => {
    expect(editorNodeSize({ id: 'b', kind: 'branch', condition: 'x' })).toEqual({
      width: 200,
      height: 124,
    });
  });

  it('action and approval_gate reserve extra width for their slanted sides', () => {
    expect(editorNodeSize({ id: 'a', kind: 'action', action: 'scm.prs.create' })).toEqual({
      width: 214,
      height: 80,
    });
    expect(editorNodeSize({ id: 'g', kind: 'approval_gate' })).toEqual({ width: 214, height: 80 });
  });

  it('for_each reserves extra width for its hexagon points, keeping its height', () => {
    expect(editorNodeSize({ id: 'f', kind: 'for_each', forEach: 'items' })).toEqual({
      width: 214,
      height: 100,
    });
  });

  it('a plain step is unchanged', () => {
    expect(editorNodeSize({ id: 's', kind: 'step', agent: 'a' })).toEqual({ width: 210, height: 80 });
  });
});
```

Create `src/components/workflow-editor/kindVisuals.test.ts` additions — append to the existing file:

```ts
import { KIND_SHAPE } from './kindVisuals';

describe('KIND_SHAPE', () => {
  it('maps every kind to its flowchart symbol', () => {
    expect(KIND_SHAPE).toEqual({
      step: 'rect',
      for_each: 'hexagon',
      parallel: 'subroutine',
      panel: 'stacked',
      branch: 'diamond',
      approval_gate: 'trapezoid',
      action: 'parallelogram',
    });
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/workflowLayout.test.ts src/components/workflow-editor/kindVisuals.test.ts`
Expected: FAIL — `editorNodeSize` returns `{width:210,height:80}` for branch; `KIND_SHAPE` is not exported.

- [ ] **Step 3: Write the implementation**

In `src/lib/workflowLayout.ts`, after the `FOR_EACH_H` declaration (line 30), add:

```ts
/** branch paints a diamond — taller, and narrower than a step because a
 *  diamond's usable width collapses toward its tips (it shows only its
 *  condition, which is all a branch has). */
export const BRANCH_W = 200;
export const BRANCH_H = 124;

/** action (parallelogram) and approval_gate (trapezoid) both lose horizontal
 *  room to slanted sides; the box grows so the text band stays step-sized. */
export const ACTION_W = 214;
export const GATE_W = 214;

/** for_each (hexagon) loses room to its left/right points. Height unchanged. */
export const FOR_EACH_W = 214;
```

Then replace the `editorNodeSize` switch body (lines 52-63) — keep the existing `parallel`/`panel` cases exactly as they are and add four new ones:

```ts
export function editorNodeSize(d: StepNodeData): NodeBox {
  switch (d.kind) {
    case 'parallel': {
      const rows = Math.max(d.parallel?.length ?? 0, 1);
      return { width: PARALLEL_W, height: PARALLEL_HEADER_H + rows * PARALLEL_SUBROW_H + PARALLEL_PAD_V };
    }
    case 'panel':
      return { width: PANEL_W, height: PANEL_BASE_H + (d.panel?.gate ? PANEL_GATE_H : 0) };
    case 'branch':
      return { width: BRANCH_W, height: BRANCH_H };
    case 'action':
      return { width: ACTION_W, height: NODE_H };
    case 'approval_gate':
      return { width: GATE_W, height: NODE_H };
    case 'for_each':
      return { width: FOR_EACH_W, height: FOR_EACH_H };
    default:
      return { width: NODE_W, height: NODE_H };
  }
}
```

In `src/components/workflow-editor/kindVisuals.ts`, add the import and the map after `KIND_ACCENT`:

```ts
import type { ShapeName } from './nodeShapes';

/** Which flowchart symbol each kind paints (see nodeShapes.ts). `parallel` and
 *  `panel` keep a rectangular body deliberately — they are the only kinds whose
 *  height grows with content, so the subroutine/stacked idioms (which grow)
 *  are the right flowchart forms rather than a fixed silhouette. */
export const KIND_SHAPE: Record<StepKind, ShapeName> = {
  step: 'rect',
  for_each: 'hexagon',
  parallel: 'subroutine',
  panel: 'stacked',
  branch: 'diamond',
  approval_gate: 'trapezoid',
  action: 'parallelogram',
};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/workflowLayout.test.ts src/components/workflow-editor/kindVisuals.test.ts`
Expected: PASS.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/lib/workflowLayout.ts src/lib/workflowLayout.test.ts src/components/workflow-editor/kindVisuals.ts src/components/workflow-editor/kindVisuals.test.ts
git commit -m "feat(cp): per-kind node boxes + kind->shape map"
```

---

### Task 3: Paint the silhouette

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/nodes/EditableStepNode.tsx` (`next` path only, ~lines 323-401)
- Modify: `crates/rupu-cp/web/src/styles.css:405-447`
- Test: `crates/rupu-cp/web/src/components/workflow-editor/nodes/EditableStepNode.test.tsx:259-293`

**Interfaces:**
- Consumes: `shapeFor` + `NodeShape` (Task 1), `KIND_SHAPE` (Task 2), existing `editorNodeSize` (Task 2).
- Produces: the `next` node DOM contract — `.wfx-node > .wfx-sil` (SVG silhouette) and `.wfx-node > .wfx-safe` (content box, `.wfx-safe-mid` when centred), containing exactly `.wfx-head` and `.wfx-body`. `.wfx-bar` no longer exists. Task 4 edits the same render block.

- [ ] **Step 1: Update the DOM-locking tests to the new contract**

In `src/components/workflow-editor/nodes/EditableStepNode.test.tsx`, replace the two tests at lines 260-293 (the `'wraps .wfx-bar, .wfx-head, and .wfx-body inside a .wfx-clip…'` test and the `'keeps the Handles outside .wfx-clip…'` test) with:

```ts
      it('wraps .wfx-head and .wfx-body inside .wfx-safe — the shape-derived content box', () => {
        const { container } = renderNode({ id: 'build', kind: 'step', agent: 'coder' }, [], false, 'next');
        const safe = container.querySelector('.wfx-safe');
        expect(safe).toBeInTheDocument();

        const head = safe?.querySelector('.wfx-head');
        const body = safe?.querySelector('.wfx-body');
        expect(head).toBeInTheDocument();
        expect(body).toBeInTheDocument();

        // exactly head/body live in the safe box — the accent bar is gone (the
        // silhouette is the kind signal now).
        expect(safe?.children).toHaveLength(2);
        expect(safe?.children[0]).toBe(head);
        expect(safe?.children[1]).toBe(body);
        expect(container.querySelector('.wfx-bar')).not.toBeInTheDocument();
      });

      it('paints the silhouette as an SVG layer, a direct child of .wfx-node', () => {
        const { container } = renderNode({ id: 'build', kind: 'step', agent: 'coder' }, [], false, 'next');
        const node = container.querySelector('.wfx-node');
        expect(node).toBeInTheDocument();

        const sil = node?.querySelector(':scope > .wfx-sil');
        expect(sil).toBeInTheDocument();
        expect(sil?.tagName.toLowerCase()).toBe('svg');
        expect(sil?.querySelector('path')?.getAttribute('d')).toMatch(/^M /);

        // Handle is mocked to `() => null`, so assert structurally: .wfx-node's
        // element children are the silhouette + the safe box, nothing else.
        expect(node?.querySelector(':scope > .wfx-safe')).toBeInTheDocument();
        expect(node?.querySelector(':scope > .wfx-head')).not.toBeInTheDocument();
        expect(node?.querySelector(':scope > .wfx-body')).not.toBeInTheDocument();
      });

      it('positions the safe box at the shape safe rect, centring a branch only', () => {
        const { container: step } = renderNode({ id: 's', kind: 'step', agent: 'a' }, [], false, 'next');
        const stepSafe = step.querySelector('.wfx-safe') as HTMLElement;
        expect(stepSafe.className).not.toContain('wfx-safe-mid');
        expect(stepSafe.style.left).toBe('15px');

        const { container: br } = renderNode(
          { id: 'route', kind: 'branch', condition: 'x == 1' },
          [],
          false,
          'next',
        );
        const brSafe = br.querySelector('.wfx-safe') as HTMLElement;
        expect(brSafe.className).toContain('wfx-safe-mid');
        // 200 * 0.23 = 46
        expect(brSafe.style.left).toBe('46px');
      });

      it('strokes the silhouette with the kind accent when selected', () => {
        const { container: idle } = renderNode({ id: 's', kind: 'step', agent: 'a' }, [], false, 'next');
        const { container: sel } = renderNode({ id: 's', kind: 'step', agent: 'a' }, [], true, 'next');
        const strokeOf = (c: HTMLElement) =>
          c.querySelector('.wfx-sil path:last-of-type')?.getAttribute('stroke');
        expect(strokeOf(idle as HTMLElement)).not.toBe(strokeOf(sel as HTMLElement));
      });
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/nodes/EditableStepNode.test.tsx`
Expected: FAIL — `.wfx-safe` and `.wfx-sil` are not in the document.

- [ ] **Step 3: Write the implementation**

In `EditableStepNode.tsx`, add the imports at the top of the file (beside the existing `kindVisuals` import at line 20):

```ts
import { KIND_ACCENT, KIND_ICON, KIND_SHAPE } from '../kindVisuals';
import { shapeFor } from '../nodeShapes';
```

Then replace the whole `if (ui === 'next') { … }` block (from line 323 `if (ui === 'next') {` through the `}` that closes it just before `return (` at line 403) with:

```tsx
  if (ui === 'next') {
    const shape = shapeFor(KIND_SHAPE[d.kind], box.width, box.height);
    const stroke = selected ? color : 'rgb(var(--c-border))';
    return (
      <div
        data-ui={ui}
        className="wfx-node"
        style={{ width: box.width, minHeight: box.height }}
      >
        <Handle type="target" position={Position.Left} style={handleStyle} />

        {/* The silhouette is painted in SVG rather than clipped with
            `clip-path`: a clip slices the 1px border at the clip boundary and
            cannot clip an outward selection glow, and it would leave the empty
            corners outside the shape still catching drags. The path is also
            the pointer target (see .wfx-sil in styles.css), so a diamond stops
            being grabbable in its corners. */}
        <svg className="wfx-sil" viewBox={`0 0 ${box.width} ${box.height}`} aria-hidden>
          {selected && (
            <path
              d={shape.path}
              fill="none"
              stroke={color}
              strokeWidth={5}
              strokeLinejoin="round"
              opacity={0.25}
            />
          )}
          {shape.extra.map((d2, i) => (
            <path key={i} d={d2} fill="none" stroke="rgb(var(--c-border))" strokeWidth={1.5} />
          ))}
          <path
            d={shape.path}
            fill="rgb(var(--c-panel))"
            stroke={stroke}
            strokeWidth={1.5}
            strokeLinejoin="round"
          />
        </svg>

        {/* Content lives in the shape's safe rect, inscribed at the silhouette's
            narrowest row — truncation is bounded by THIS box, not the bounding
            box. `align` is part of the shape: a diamond centres, because
            left-aligned text there starts on the slope and reads as spilling
            outside the outline. */}
        <div
          className={shape.align === 'center' ? 'wfx-safe wfx-safe-mid' : 'wfx-safe'}
          style={{
            left: shape.safe.x,
            top: shape.safe.y,
            width: shape.safe.w,
            minHeight: shape.safe.h,
          }}
        >
          <div className="wfx-head">
            <span className="wfx-kindpill" style={kindChipStyle(colors, d.kind)}>
              <KindIcon className="wfx-kindicon" size={12} strokeWidth={2} aria-hidden />
              {d.kind}
            </span>
            <span className="wfx-nid">{d.id}</span>
            {hasProblems && (
              <span className="wfx-problem" title={problems.join('\n')} aria-label="has problems" />
            )}
          </div>

          <div className="wfx-body">
            {d.kind === 'parallel' ? (
              <ParallelBodyNext d={d} />
            ) : d.kind === 'panel' ? (
              <PanelBodyNext d={d} />
            ) : d.kind === 'branch' ? (
              <BranchBodyNext d={d} />
            ) : d.kind === 'action' ? (
              <ActionBodyNext d={d} />
            ) : d.kind === 'approval_gate' ? (
              <GateBodyNext d={d} />
            ) : (
              <StepBodyNext d={d} />
            )}
          </div>
        </div>

        {d.kind === 'branch' ? (
          <>
            <Handle
              type="source"
              position={Position.Right}
              id="then"
              style={{ ...handleStyle, top: '38%', background: colors.status.done }}
            />
            <Handle
              type="source"
              position={Position.Right}
              id="else"
              style={{ ...handleStyle, top: '68%', background: colors.status.failed }}
            />
          </>
        ) : (
          <Handle type="source" position={Position.Right} style={handleStyle} />
        )}
      </div>
    );
  }
```

Note: the handles are deliberately left untouched here — Task 4 makes them shape-aware. `selBoxShadow` is now unused; delete its `const selBoxShadow = …` declaration (lines 327-330) to keep the file lint-clean.

In `src/styles.css`, replace the `.wfx-node`, `.wfx-clip` and `.wfx-bar` rules (lines 405-447 — from the `/* ── node card shell ── */` comment through the `.wfx-bar { … }` block) with:

```css
/* ── node card shell ── */
/* Width/min-height come from `editorNodeSize()` inline (shared with the dagre
 * layout reservation — see workflowLayout.ts) so this card never grows past
 * what the canvas reserved for it; nothing below sets `width`.
 * The shell paints NOTHING: border, fill and selection glow all live on the
 * SVG silhouette (.wfx-sil) so non-rectangular kinds don't show a stray
 * rectangle behind their shape. */
.wfx-node {
  position: relative;
  text-align: left;
  cursor: grab;
  /* the silhouette path is the hit area, not the bounding box — a diamond must
   * not be grabbable in its empty corners. Children opt back in below. */
  pointer-events: none;
}

/* ── silhouette: the per-kind flowchart symbol (see nodeShapes.ts) ── */
.wfx-sil {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  /* the selection glow strokes outside the box */
  overflow: visible;
  transition: none;
}
.wfx-sil path { pointer-events: auto; }

/* ── safe box: where content may live, inscribed in the silhouette ── */
.wfx-safe {
  position: absolute;
  display: flex;
  flex-direction: column;
  min-width: 0;
  /* text is never interactive; clicks belong to the silhouette beneath */
  pointer-events: none;
}
/* shapes whose width varies across the text band centre their content */
.wfx-safe-mid { align-items: center; text-align: center; }
.wfx-safe-mid .wfx-head { justify-content: center; }
.wfx-safe-mid .wfx-nid { flex: 0 1 auto; }
```

Then adjust the two rules that assumed the old padded card — `.wfx-head` and `.wfx-body` (immediately below the block you replaced) now sit inside the safe rect, which already provides the inset:

```css
.wfx-head { display: flex; align-items: center; gap: 7px; padding: 0 0 4px; }
```

```css
.wfx-body {
  padding: 0; font-size: 10.5px; color: rgb(var(--c-ink-dim));
  display: flex; flex-direction: column; gap: 5px;
  min-width: 0;
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/`
Expected: PASS. Every other `next`-path test (kind pill, per-kind bodies, branch ports) must still pass untouched — if one fails, the body components were changed, which is out of scope; revert that part.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/components/workflow-editor/nodes/EditableStepNode.tsx src/components/workflow-editor/nodes/EditableStepNode.test.tsx src/styles.css
git commit -m "feat(cp): paint per-kind flowchart silhouettes on editor nodes"
```

---

### Task 4: Shape-aware handle anchors

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/nodes/EditableStepNode.tsx` (the `next` path's `Handle` elements only)
- Test: `crates/rupu-cp/web/src/components/workflow-editor/nodes/EditableStepNode.test.tsx`

**Interfaces:**
- Consumes: `shape.target` / `shape.sources` from `shapeFor` (Task 1), already computed in the render body by Task 3.
- Produces: no new exports. The `next` path's handles are positioned from shape geometry; ids unchanged.

- [ ] **Step 1: Write the failing test**

The `@xyflow/react` mock renders `Handle` as `() => null`, so the position can't be read from the DOM. Change the mock at the top of `EditableStepNode.test.tsx` (lines 12-15) to record its props:

```ts
vi.mock('@xyflow/react', () => ({
  Handle: (props: Record<string, unknown>) => (
    <i
      data-testid="handle"
      data-type={String(props.type)}
      data-position={String(props.position)}
      data-handleid={props.id === undefined ? '' : String(props.id)}
    />
  ),
  Position: { Top: 'top', Bottom: 'bottom', Left: 'left', Right: 'right' },
}));
```

Then add this test inside the `describe('card chrome …')` block:

```ts
      it('anchors a branch on its diamond vertices — then right, else bottom', () => {
        const { container } = renderNode(
          { id: 'route', kind: 'branch', condition: 'x == 1' },
          [],
          false,
          'next',
        );
        const handles = [...container.querySelectorAll('[data-testid="handle"]')].map((h) => ({
          type: h.getAttribute('data-type'),
          position: h.getAttribute('data-position'),
          id: h.getAttribute('data-handleid'),
        }));
        expect(handles).toEqual([
          { type: 'target', position: 'left', id: '' },
          { type: 'source', position: 'right', id: 'then' },
          // else drops to the BOTTOM vertex — at the old top:68% it floated
          // mid-slope, visibly detached from the diamond's outline.
          { type: 'source', position: 'bottom', id: 'else' },
        ]);
      });

      it('every other kind keeps a single right-edge source', () => {
        const { container } = renderNode({ id: 's', kind: 'step', agent: 'a' }, [], false, 'next');
        const handles = [...container.querySelectorAll('[data-testid="handle"]')].map((h) => ({
          type: h.getAttribute('data-type'),
          position: h.getAttribute('data-position'),
          id: h.getAttribute('data-handleid'),
        }));
        expect(handles).toEqual([
          { type: 'target', position: 'left', id: '' },
          { type: 'source', position: 'right', id: '' },
        ]);
      });
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/nodes/EditableStepNode.test.tsx`
Expected: FAIL — `else` reports `position: 'right'`, not `'bottom'`.

Note: the changed mock now renders DOM where it previously rendered nothing. If the pre-existing test `'keeps the Handles outside .wfx-clip…'` (now `'paints the silhouette as an SVG layer…'`, Task 3) asserts on `.wfx-node`'s children, confirm it still passes — it asserts by selector, not by child count, so it should.

- [ ] **Step 3: Write the implementation**

Add this helper above `function EditableStepNode` in `EditableStepNode.tsx`:

```tsx
const HANDLE_SIDE = { left: Position.Left, right: Position.Right, bottom: Position.Bottom } as const;

/** Turn a shape's HandleAnchor into xyflow's (position, style) pair. `offset`
 *  runs along the anchored side: `top` for a left/right edge, `left` for the
 *  bottom edge. */
function anchorProps(
  anchor: HandleAnchor,
  base: React.CSSProperties,
): { position: Position; style: React.CSSProperties } {
  return {
    position: HANDLE_SIDE[anchor.side],
    style: anchor.side === 'bottom' ? { ...base, left: anchor.offset } : { ...base, top: anchor.offset },
  };
}
```

and extend the `nodeShapes` import:

```ts
import { shapeFor, type HandleAnchor } from '../nodeShapes';
```

Then, in the `next` path, replace the target handle:

```tsx
        <Handle type="target" {...anchorProps(shape.target, handleStyle)} />
```

and replace the whole source-handle block (the `{d.kind === 'branch' ? … : …}` expression) with:

```tsx
        {shape.sources.map((s) => {
          // arm colour is a UI cue; the handle ID is a MODEL CONTRACT
          // (applyConnect reads 'then'/'else' to write thenTargets/elseTargets).
          const tint =
            s.id === 'then' ? colors.status.done : s.id === 'else' ? colors.status.failed : undefined;
          const { position, style } = anchorProps(s.anchor, handleStyle);
          return (
            <Handle
              key={s.id ?? 'source'}
              type="source"
              id={s.id}
              position={position}
              style={tint ? { ...style, background: tint } : style}
            />
          );
        })}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/`
Expected: PASS.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/components/workflow-editor/nodes/EditableStepNode.tsx src/components/workflow-editor/nodes/EditableStepNode.test.tsx
git commit -m "feat(cp): anchor node handles to the silhouette, not the bounding box"
```

---

### Task 5: Teach the palette the same shapes

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/NodePalette.tsx:405-425` (the rail `variant === 'rail'` block chips)
- Modify: `crates/rupu-cp/web/src/styles.css` (add `.wfx-pshape`)
- Test: `crates/rupu-cp/web/src/components/workflow-editor/NodePalette.test.tsx`

**Interfaces:**
- Consumes: `shapeFor` (Task 1), `KIND_SHAPE` (Task 2).
- Produces: no new exports. Each rail block chip renders a `.wfx-pshape` SVG miniature of its kind's silhouette.

- [ ] **Step 1: Write the failing test**

Append inside the existing `describe('variant="rail" (Task 1: inspector-rail dock)', …)` block at `src/components/workflow-editor/NodePalette.test.tsx:150`. That file has no shared render helper — every test renders inline, and `branch`/`approval_gate` chips only appear when `workflowEditorUi="next"` is passed (see the existing test at line 194), so this test must pass it too:

```ts
  it('each block chip previews its kind silhouette, so the shape is learned at pick time', () => {
    const { container } = render(
      <NodePalette onAdd={() => {}} onDragStartKind={() => {}} variant="rail" workflowEditorUi="next" />,
    );
    const branchChip = container.querySelector('[aria-label="Add branch node"]');
    expect(branchChip).toBeInTheDocument();

    const shape = branchChip?.querySelector('.wfx-pshape');
    expect(shape).toBeInTheDocument();
    expect(shape?.tagName.toLowerCase()).toBe('svg');
    // a diamond: four vertices, no curves
    const d = shape?.querySelector('path')?.getAttribute('d') ?? '';
    expect(d).toMatch(/^M /);
    expect(d).not.toContain('Q');
    expect(d.match(/L/g) ?? []).toHaveLength(3);

    // a step chip previews the rounded rect instead
    const stepD = container
      .querySelector('[aria-label="Add step node"] .wfx-pshape path')
      ?.getAttribute('d');
    expect(stepD).toContain('Q');
  });
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/NodePalette.test.tsx`
Expected: FAIL — `.wfx-pshape` is not in the document.

- [ ] **Step 3: Write the implementation**

Add to the imports in `NodePalette.tsx`:

```ts
import { KIND_ACCENT, KIND_ICON, KIND_SHAPE } from './kindVisuals';
import { shapeFor } from './nodeShapes';
```

Add this component above the `NodePalette` function:

```tsx
/** Miniature silhouette for a palette chip — the same geometry the canvas
 *  paints, so a shape is learned where you PICK the block rather than first
 *  met where it lands. Drawn at a fixed 34x20 viewBox and scaled by CSS. */
function ShapePreview({ kind, color }: { kind: StepKind; color: string }) {
  const shape = shapeFor(KIND_SHAPE[kind], 34, 20);
  return (
    <svg className="wfx-pshape" viewBox="0 0 34 20" aria-hidden>
      {shape.extra.map((d, i) => (
        <path key={i} d={d} fill="none" stroke={color} strokeWidth={1} opacity={0.5} />
      ))}
      <path d={shape.path} fill="none" stroke={color} strokeWidth={1.25} strokeLinejoin="round" />
    </svg>
  );
}
```

In the rail block-chip `map` (line ~405-425), replace the icon line:

```tsx
                <Icon className="wfx-picon" size={14} strokeWidth={2} style={{ color }} aria-hidden />
```

with the shape preview followed by the icon:

```tsx
                <ShapePreview kind={item.kind} color={color} />
                <Icon className="wfx-picon" size={14} strokeWidth={2} style={{ color }} aria-hidden />
```

Add to `src/styles.css`, next to the existing `.wfx-picon` rule:

```css
.wfx-pshape { flex: none; width: 24px; height: 14px; }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/NodePalette.test.tsx`
Expected: PASS.

- [ ] **Step 5: Typecheck and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit
git add src/components/workflow-editor/NodePalette.tsx src/components/workflow-editor/NodePalette.test.tsx src/styles.css
git commit -m "feat(cp): preview each block's silhouette in the palette"
```

---

### Task 6: Offset "⊕ next" by the source node's real width

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditorGraph.tsx:41,229`
- Test: `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditorGraph.test.tsx`

**Interfaces:**
- Consumes: `editorNodeSize` from `../../lib/workflowLayout` (Task 2).
- Produces: no new exports. `applyAddConnectedNext`'s placement is now source-width-aware.

`applyAddConnectedNext` hardcodes `NODE_W` (210) for the horizontal gap. That was already wrong for `parallel`/`panel` (220 wide) and is now wrong for four more kinds. In scope because per-shape widths make the overlap materially more visible.

- [ ] **Step 1: Write the failing test**

Append to `src/components/workflow-editor/WorkflowEditorGraph.test.tsx` (the file already imports `applyAddConnectedNext`; if it does not, add it to the existing import from `./WorkflowEditorGraph`):

```ts
describe('applyAddConnectedNext placement', () => {
  it('offsets by the SOURCE node width, so a wide container never overlaps its next node', () => {
    const graph = {
      nodes: [
        {
          id: 'fan',
          data: {
            id: 'fan',
            kind: 'parallel' as const,
            parallel: [{ id: 'a', agent: 'x', prompt: 'p' }],
          },
          position: { x: 100, y: 40 },
        },
      ],
      edges: [],
      settings: {},
    } as unknown as Parameters<typeof applyAddConnectedNext>[0];

    const { graph: next, id } = applyAddConnectedNext(graph, 'fan');
    const added = next.nodes.find((n) => n.id === id)!;
    // parallel is PARALLEL_W (220) wide, + the 64px gap — NOT NODE_W (210).
    expect(added.position).toEqual({ x: 100 + 220 + 64, y: 40 });
  });

  it('still offsets a plain step by the step width', () => {
    const graph = {
      nodes: [
        { id: 's', data: { id: 's', kind: 'step' as const, agent: 'a' }, position: { x: 0, y: 0 } },
      ],
      edges: [],
      settings: {},
    } as unknown as Parameters<typeof applyAddConnectedNext>[0];

    const { graph: next, id } = applyAddConnectedNext(graph, 's');
    expect(next.nodes.find((n) => n.id === id)!.position).toEqual({ x: 210 + 64, y: 0 });
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/WorkflowEditorGraph.test.tsx`
Expected: FAIL — the parallel case gets `x: 374` (100 + 210 + 64) instead of `384`.

- [ ] **Step 3: Write the implementation**

In `WorkflowEditorGraph.tsx`, change the import at line 41:

```ts
import { autoLayout, editorNodeSize, NODE_W } from '../../lib/workflowLayout';
```

and replace the `position` line at 229:

```ts
    position: { x: base.x + (source ? editorNodeSize(source.data).width : NODE_W) + gap, y: base.y },
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/workflow-editor/`
Expected: PASS.

- [ ] **Step 5: Typecheck, full suite, and commit**

```bash
cd crates/rupu-cp/web && npx tsc -b --noEmit && npx vitest run
git add src/components/workflow-editor/WorkflowEditorGraph.tsx src/components/workflow-editor/WorkflowEditorGraph.test.tsx
git commit -m "fix(cp): place the next node by the source's real width"
```

---

## Operator gate (required before merge)

Silhouette rendering cannot be unit-verified. Before the PR leaves draft, matt must check in the running app — `make cp-web`, restart `rupu cp serve`, with `[cp].workflow_editor_ui = 'next'`:

1. All seven kinds render their shape, **light and dark**, with no stray rectangle behind any shape (that would mean `.wfx-node` kept a border or background).
2. Text sits inside every outline at every kind — especially the branch, which must read centred.
3. Edges meet the silhouette; a branch's `else` edge leaves from the bottom vertex.
4. A diamond is not draggable in its empty corners; clicking the shape still selects it.
5. Zoom out to ~30%: kinds remain distinguishable.
6. Sizes are the tuning knob — §4c of the spec calls the values a starting point. If a shape crowds its text, change the constants in `workflowLayout.ts` *and* re-run the `render == reservation` tests.
