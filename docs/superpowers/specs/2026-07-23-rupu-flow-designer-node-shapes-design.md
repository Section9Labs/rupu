# Flow Designer — a shape language for nodes (design)

**Date:** 2026-07-23
**Status:** Approved (operator signed off on the artifact "Flow Designer — A Shape Language", https://claude.ai/code/artifact/98bac423-71f0-4408-bf4f-1a2d8e2d0df4, plus the shape-aware text-alignment correction in §5).
**Scope:** `crates/rupu-cp/web/src/components/workflow-editor/` + `src/lib/workflowLayout.ts` — behind the existing `[cp].workflow_editor_ui = 'next'` flag. No backend change, no schema change, no new dependency. Baseline: main `79b89b2a` (v0.68.0).

## 1. Problem

Every node on the editor canvas is the same rounded card. Kind is carried by exactly two signals, both of which fail at low zoom:

- a 3px accent bar (`.wfx-bar`) — sub-pixel below ~40% zoom;
- a 11px lucide glyph in the kind pill — illegible at the same point.

So a workflow zoomed out far enough to see whole is a column of identical rectangles. Colour is also the *only* differentiator, which excludes colourblind operators from the one cue that does survive to mid-zoom.

Shape is the channel the canvas isn't using. It survives zoom, it is colour-independent, and for this particular domain it is already a solved visual language: rupu's node kinds map almost one-to-one onto classical flowchart symbols.

## 2. The vocabulary (approved)

| kind | form | rationale |
|---|---|---|
| `step` | rounded rectangle — *process* | the default unit of work; unchanged from today |
| `branch` | **diamond** — *decision* | evaluates a condition, takes one arm |
| `action` | **parallelogram** — *input/output* | calls an external system; data crosses the boundary |
| `approval_gate` | **trapezoid** — *manual operation* | the ANSI symbol for human intervention |
| `for_each` | **hexagon** — *preparation* | iteration |
| `parallel` | **double-edged rectangle** — *predefined process* | contains concurrent child steps |
| `panel` | **stacked rectangle** — *multi-review* | N panelists, then a gate |

`parallel` and `panel` keep a rectangular body deliberately: they are the only kinds whose height grows with content (sub-step rows / gate block), so a fixed decorative silhouette would fight them. The subroutine and stacked forms are the flowchart idioms that *do* grow.

## 3. Structure (from investigation, v0.68.0)

- **One node component.** `WorkflowEditorGraph.tsx:53` registers `NODE_TYPES = { editable: EditableStepNode }`; every graph node is projected with `type: 'editable'` (~354-364). `EditableStepNode.tsx` (471 lines) holds **two full render paths** selected by `data.workflowEditorUi`: `next` (~331-401, `.wfx-*` classes) and `classic` (~404-467, Tailwind). Per-kind bodies are a ternary chain in both (`next` ~363-377).
- **Containers are atomic nodes.** `yamlToGraph` emits one node per top-level step (`workflowGraph.ts:308-312`); `parallel` sub-steps live in `d.parallel: SubStep[]` and `panel` panelists in `d.panel.panelists` — rendered as rows *inside* the card. There is **zero use of xyflow parenting** (`parentId` / `extent`) anywhere in `web/src`.
- **Layout already reserves per-kind boxes.** `workflowLayout.ts:25-64` `editorNodeSize(d)` returns `{width,height}` per kind (`NODE_W=210`/`NODE_H=80`, `FOR_EACH_H=100`, `PARALLEL_*` row formula, `PANEL_*`); `autoLayout` (69-91) feeds those to dagre (`rankdir: 'LR'`, `nodesep: 36`, `ranksep: 72`). The documented invariant (comment at 8-11) is **render box == reservation box** — the component applies the same numbers inline as `width` / `minHeight`.
- **Handles.** Target `Position.Left` (`EditableStepNode.tsx:342`), source `Position.Right` (~380-399); `branch` gets two labelled sources positioned by *percentage of bounding-box height* (`top:'38%'` / `'68%'`). `handleStyle` at :316. Handle ids `'then'`/`'else'` are a **model contract** — `applyConnect` (`WorkflowEditorGraph.tsx:75-115`) reads `sourceHandle` to write `thenTargets`/`elseTargets`, and the edges memo re-derives `sourceHandle = e.branch`. Handles are mounted *outside* `.wfx-clip` so `overflow:hidden` can't cut them.
- **Chrome.** `.wfx-node` (styles.css ~405-445) is a bordered rounded div; selection glow is computed in JS (`selBoxShadow`, `EditableStepNode.tsx:327-330`). `.wfx-clip` exists solely so `.wfx-bar` (the 3px accent) gets rounded corners. No `clip-path` or `polygon()` exists anywhere in the codebase.
- **`kindVisuals.ts`** (35 lines) is colour + icon only: `KIND_ACCENT`, `KIND_ICON`. Consumers: `EditableStepNode.tsx:20`, `NodePalette.tsx:18`, `WorkflowEditorGraph.tsx:46` (edge stroke derives from the source node's accent).

## 4. Design

### 4a. Silhouettes are SVG, not `clip-path`

`clip-path` is rejected: it slices the 1px CSS border at the clip boundary, cannot clip the outward `box-shadow` selection glow, and leaves the corners outside the shape still catching drags.

Instead each node renders an **SVG layer** as the first child of `.wfx-node`, absolutely filling it: one `<path>` carrying `fill` (panel), `stroke` (border, or the kind accent when selected), and the selection glow as a second soft outer stroke. Because the path is the paint *and* the pointer target, a diamond stops being grabbable in its empty corners — the node finally feels like its shape.

### 4b. A shape is geometry + a safe rect + an alignment + anchors

New module `components/workflow-editor/nodeShapes.ts` — pure, no React, unit-testable:

```ts
shapeFor(kind: StepKind, w: number, h: number): {
  path: string;                       // SVG path data for the silhouette
  extra?: string;                     // subroutine bars / stacked layers
  safe: { x; y; w; h };               // where content may live
  align: 'start' | 'center';          // §5
  anchors: { target: Anchor; sources: Anchor[] };  // §4d
}
```

`kindVisuals.ts` gains `KIND_SHAPE: Record<StepKind, ShapeName>` beside the existing accent/icon maps — the one place a kind's visual identity is declared.

### 4c. Sizing: grow the box by what the shape steals

The `render == reservation` invariant is preserved by adding cases to `editorNodeSize()` in the same change. **Rule:** the safe rect is inscribed at the shape's *narrowest row*, and the bounding box grows to keep the safe width comparable to a plain step where it reasonably can.

Starting values (from the approved artifact; final values tuned at the operator gate, §7):

| kind | box | note |
|---|---|---|
| `step` | 210 × 80 | unchanged (`NODE_W`/`NODE_H`) |
| `branch` | 200 × 124 | taller; shows only its condition, which is all a branch has |
| `action` | 214 × 80 | 20px skew each side |
| `approval_gate` | 214 × 80 | 26px top inset |
| `for_each` | 214 × 100 | keeps `FOR_EACH_H`; 22px points |
| `parallel` | existing formula | header + rows × 26 + pad, unchanged |
| `panel` | existing formula | base + gate, unchanged |

**Shipped values differ from the starting table above** (tuned during implementation, plus a
final-review fix):

| kind | box | note |
|---|---|---|
| `branch` | **280 × 200** | widened/heightened from 200×124 — the diamond's safe rect is inscribed at its narrowest band (28%–72% of height), so headroom for `BranchBodyNext`'s realistic header+condition+two port-pill content needed more than the starting guess. |
| `panel` | **`PANEL_HEADER_H`(31) + rows × `PANEL_PORT_ROW_H`(17) + `PANEL_PAD_V`(30) + gate** | NOT "unchanged" as this table originally said. A final-review pass (F2) found the old fixed `PANEL_BASE_H`(84) reserved the same height regardless of panelist count, so a 3-panelist node (each panelist wraps to its own port-pill row in the fixed-width safe rect) clipped its 3rd panelist. `rows = Math.max(panelists.length, 1)`, mirroring `parallel`'s own per-row scaling; constants measured in headless Chrome against `PanelBodyNext`'s real CSS — see `workflowLayout.ts`'s doc comment for the full derivation. |

`step` / `action` / `approval_gate` / `for_each` / `parallel` shipped exactly as the starting table specified.

### 4d. Handle anchors become shape-aware

Anchors move from hardcoded percentages to the shape's own geometry:

- **Default** — target on the left edge at mid-height, source on the right edge at mid-height. On a diamond the left/right *vertices* are exactly at mid-height, so this needs no special case.
- **`branch`** — `then` on the right vertex (`Position.Right`); `else` moves to the **bottom vertex** (`Position.Bottom`, 50% width). Today's `top:38%/68%` would land mid-slope on a diamond and visibly float off the outline. Routing an `else` downward is also the flowchart convention.

The handle **ids stay `'then'` / `'else'`** — only their position changes. `applyConnect` and the edges memo are untouched.

### 4e. The accent bar retires

`.wfx-bar` needs a flat top edge that most of these shapes do not have. In the `next` path it is removed for every kind (including `step`, for consistency): the **silhouette is the kind signal**, the accent lives in the kind pill and in the selected outline. This drops `.wfx-clip` to two children — see §7.

### 4f. The palette learns the same shapes

`NodePalette.tsx` block chips render the silhouette in miniature (reusing `shapeFor`), so a shape is learned where you *pick* the block, not first met where it lands.

### 4g. Fix `applyAddConnectedNext`'s hardcoded width

`WorkflowEditorGraph.tsx:229` offsets the "⊕ next" node by a literal `NODE_W`, which is already wrong for `parallel`/`panel` (220) and gets worse with per-shape widths. It becomes `editorNodeSize(source.data).width`. In scope because this change makes the existing bug materially more visible.

## 5. Text placement is part of the shape (operator correction)

Left-aligning text inside a shape whose width varies by row starts the text on the slope and reads as spilling outside the outline — the operator flagged exactly this on the diamond.

**Rule:** shapes with **varying width across the text band** (`branch`) centre their content; **constant-width** shapes (`step`, `action` — a shear is constant-width, just shifted — `approval_gate`, `for_each`, `parallel`, `panel`) stay left-aligned like every other card in the CP. Centring a decision is also the flowchart convention.

Truncation is unchanged in mechanism (`overflow:hidden` + `text-overflow:ellipsis` per field) but now bounded by the shape's safe rect rather than the full bounding box.

## 6. Decisions carried from the design review

Both open questions resolve to **exactly what the approved artifact draws** — the artifact is the contract:

1. **Icons stay, and so does the kind word.** (My lean was to drop the word; the artifact shows it, and that is what was signed off. Revisit only if it crowds a shape in the running app.)
2. **Neutral outline, accent on selection.** Every node strokes in `--c-border`; the kind accent appears in the kind pill and takes over the outline on hover/selection, so a forty-node graph doesn't turn into a rainbow.

## 7. Constraints & testing

- **`next` only.** The `classic` render path is untouched and its DOM-locking tests stay green.
- **`next`-path tests must be updated deliberately, not incidentally.** `EditableStepNode.test.tsx:259-293` asserts `.wfx-clip` has exactly 3 element children and `.wfx-node` has only `.wfx-clip` as an element child. Both change (bar removed → 2 children; SVG layer added as a sibling of `.wfx-clip`). Update the assertions to the new contract; do not delete them.
- Tokens only (no colour literals); both themes; no new npm dep; no backend change.
- **Tests:** `shapeFor` geometry per kind (path is closed, safe rect lies inside the polygon, anchors lie *on* the outline); the `render == reservation` invariant (rendered `width`/`minHeight` equals `editorNodeSize` for all seven kinds); branch `else` anchor is `Position.Bottom` while its handle id stays `'else'`; alignment is `center` for `branch` and `start` for the rest; `applyAddConnectedNext` offsets by the *source* node's width (regression: a `parallel` source at 220).
- **Out of scope:** the MiniMap (`WorkflowEditorGraph.tsx:765-772`) draws flat rects from measured bounding boxes and is single-colour today; a custom minimap node is a clean follow-up.
- **Operator gate:** matt validates in the running app (light + dark, several zoom levels) before merge — silhouette rendering cannot be unit-verified. Visual reference: the approved artifact.
