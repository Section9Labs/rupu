# Flow Designer — one rail, four tabs (design)

**Date:** 2026-07-23
**Status:** Approved (operator signed off on the interactive artifact "Flow Designer — One Rail, Four Tabs", incl. the Blocks-tab + detail-card decisions).
**Scope:** `crates/rupu-cp/web/src/components/workflow-editor/` — behind the existing `[cp].workflow_editor_ui = 'next'` flag. No backend change (verified: `/api/tools` already returns `input_schema`). Baseline: main `a54eaa9b` (v0.67.0).

## 1. Problem
In the `next` Flow Designer, the right rail is a single column: the block/connector palette is portaled into a slot at the TOP, then a Settings/Step/Reference sub-tab bar, then the editor pane. The palette (6 blocks + a server-driven connector catalog, ~20 cards, 2-wide) eats the rail height, crushing the editor to a few lines ("Select a node to edit its step" with nothing below). See operator screenshot.

## 2. Design (approved)
**The palette becomes a tab.** One tab bar at the top of the rail: **Blocks · Step · Settings · Reference**. Palette and editor stop competing — each tab owns the full rail height.

- **Blocks** (default tab): the palette, made dense + filterable + self-documenting.
  - Blocks in a tight 3-up grid of compact chips; connector actions as compact mono chips grouped by service (SCM / Issues / GitHub / GitLab / …), a filter field at top.
  - **Detail card** — clicking a block/action SELECTS it (does not instantly add) and shows a detail card: title + kind, a "what it does" blurb, **required fields** (`*`-marked), a short example, and a primary **"Add to canvas"** button. Drag-to-place still works directly (unchanged). "Add to canvas" (or drag) commits → auto-selects the new node → flips to **Step**.
  - Block blurbs are authored (a small per-kind catalog: `{ what, requiredFields[], example }`). Connector detail is generated from the tool: `ToolSpec.description` + required params from `input_schema.required` / `.properties[name].{type,description}` (already on `/api/tools`).
- **Step**: the selected-node editor (`StepForm`) — now with the full rail height. Selecting a canvas node auto-switches here (**already implemented** — `handleSelect` sets `panelTab='step'` on select; "Add to canvas" selects the new node so it jumps too). Empty state when nothing selected, with a hint to pick a block.
- **Settings** / **Reference**: unchanged content (`WorkflowSettingsForm` / `ExpressionReference`), just moved into the shared tab bar.
- Default tab: **Blocks** when nothing is selected (today it defaults to Settings).

Unchanged: the canvas toolbar (Re-layout / ⊕ next / Find step) + zoom/minimap; the rail's drag-to-resize handle; the sync/source/validity footer.

## 3. Structure (from investigation, v0.67.0)
- Rail shell: `components/workflow-editor/WorkflowEditor.tsx` (`<aside>`, lines ~546-637): palette slot (`paletteSlotRef`, ~555-557) → resize handle → tab bar (3 `PanelTabButton`, ~572-599) → editor pane (`flex-1 overflow-y-auto`, ~601-636). `type PanelTab = 'step'|'settings'|'reference'` (73), `panelTab` state default `'settings'` (155).
- Palette: `NodePalette.tsx` — portaled into the rail slot by `WorkflowEditorGraph.tsx` (~707-728, `variant="rail"`) because it needs the graph's `addNode`/drop wiring. Block `ITEMS` (51-61) + connector groups from `tools: ToolSpec[]` via `groupConnectors` (74-88). Cards = `.wfx-pcard`; rail grid `.wfx-palette-rail-grid` (styles.css ~651). `kindVisuals.ts` = `KIND_ACCENT`/`KIND_ICON`.
- Selection: `selectedId` state (154), `handleSelect` (342-345) `setSelectedId(id); if(id) setPanelTab('step')`. Add/drop call `onSelect(newId)`.
- Palette CSS: `.wfx-palette-*` in `styles.css` (~574-656).

## 4. Implementation
1. `PanelTab` union gains `'blocks'`; a 4th `PanelTabButton`; order Blocks · Step · Settings · Reference; default `'blocks'`.
2. Relocate the palette portal target: move `wfx-rail-palette-slot` from the always-mounted top position into the **Blocks** tabpanel (mount the slot only when `panelTab === 'blocks'`; the portal from the graph targets it there). The graph keeps ownership of `addNode`/drop — only the slot's DOM location changes.
3. Detail card: a `NodeDetail` sub-component in the Blocks panel. Selecting a palette chip sets a `selectedPaletteKey` (block kind or tool name) and renders the detail. Block catalog authored inline (6 kinds). Connector detail parses `ToolSpec.input_schema` (JSON Schema: `required[]`, `properties`). "Add to canvas" calls the same `onAdd(kind, seed)` the chip click used to; drag unchanged. Chip click no longer instant-adds (learn-first) — flag this as the one deliberate interaction change.
4. Palette density: CSS pass on `.wfx-palette-rail-grid` (2→3 col), compact chips, tighter connector chips, add the filter `<input>` (client-side substring over block labels + tool names, reusing the existing `Find step` idiom's feel).
5. All gated on `workflowEditorUi === 'next'`; classic path untouched.

## 5. Constraints & testing
- Tokens/`.wfx-*` classes only; both themes; no new deps; no backend change; `#![deny(clippy::all)]` n/a (web-only).
- Tests: tab bar has 4 tabs, default Blocks; selecting a canvas node switches to Step (existing behavior preserved); clicking a palette block shows its detail with required fields + example; "Add to canvas" adds a node + switches to Step; connector detail renders required params from a mocked `input_schema`; drag path still sets the DnD mime.
- Operator gate: matt validates in the running app (light+dark) before merge — GPUI/DOM rendering can't be unit-verified. Visual reference: artifact https://claude.ai/code/artifact/dbf8a196-b910-4914-9f5a-2fd1986d2034.
