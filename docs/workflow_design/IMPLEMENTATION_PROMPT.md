# Implementation prompt — rupu.app visual Workflow Builder

Copy-paste this prompt (plus the screenshots in `screenshots/` and the mockup
`Workflow Builder.dc.html`) to the implementing engineer / coding agent.

---

## PROMPT

Build a visual, drag-and-drop **Workflow Builder** into the rupu macOS app
(`apps/rupu-macos`), reached from the Workflow detail screen
(`RupuKit/Sources/RupuLibrary/WorkflowDetailScreen.swift`, route
`.workflowDefinition(name:)`). It is the native port of the CP web editor at
`crates/rupu-cp/web/src/components/workflow-editor/` — read that folder first;
it is the behavioral contract. The interactive HTML mockup
(`Workflow Builder.dc.html`) and screenshots show the approved design.

### 1. Layout (see screenshots)

Window content, left to right, below the existing app chrome (Sidebar.swift
stays untouched; the builder replaces the detail screen's body):

- **Header row (46pt)**: back chevron + `Library ▸ <name>` breadcrumb
  (`.leadText`), a `running` StatusPill while a run overlay is live, spacer,
  `valid` dot (workflow parses), a Design/Run segmented control, a YAML
  source-pane toggle (code icon), and a brand `Launch` button
  (`Color.rupuBrand600`, white text, radius 5).
- **Canvas (flex)**: pannable/scrollable node graph, dot-grid background
  (1px `rupuBorder` dots on `rupuBg`, 22pt pitch). Canvas logical size grows
  with content; scrolls inside its pane.
- **YAML pane (192pt, collapsible)** under the canvas: mono 11pt read-only
  view of the canonical YAML, header `WORKFLOW.YAML` eyebrow + `canonical ·
  round-trips with the canvas`. Persist open/closed (CP uses
  `rupu.editor.sourceOpen`; use `@AppStorage`).
- **Inspector rail (320pt, right)**: tabs `Blocks | Step | Settings`
  (2px brand underline on the active tab).

### 2. Design tokens — use RupuDesign, do not invent values

Everything comes from `RupuKit/Sources/RupuDesign/Tokens.swift` +
`Typography.swift`: `rupuBg/rupuPanel/rupuSurface/rupuBorder/rupuBorderStrong/
rupuInk/rupuDim/rupuMute`, `Color.status(_:)`, `Color.severity(_:)`,
`ChromeShape.pill` (radius 4), `Eyebrow` (mono 10 uppercase kern 1.2),
`panelStyle(.innerCard)`. Both appearances come free via `dynamicColor`.

### 3. Node kinds — port `kindVisuals.ts` + `nodeShapes.ts` exactly

Ten `StepKind`s, each with accent, lucide icon, flowchart silhouette, and
palette family (verbatim from `kindVisuals.ts`):

| kind | accent token | icon | shape | family |
|---|---|---|---|---|
| step | status.running | Bot | rect | work |
| for_each | brand.500 | Repeat | hexagon | work |
| parallel | sev.critical | Columns3 | subroutine (rails) | work |
| panel | status.awaiting | ShieldCheck | stacked | work |
| action | sev.info | Zap | parallelogram | work |
| run | sev.medium | Terminal | rect | work |
| branch | status.done | GitBranch | vhex (vertical hexagon) | orchestration |
| split | brand.600 | Split | fanout | orchestration |
| join | brand.700 | Merge | fanin | orchestration |
| approval_gate | status.paused | UserCheck | trapezoid | orchestration |

Node = 176×68 silhouette (SwiftUI `Shape` per symbol; port the inset
constants and the small-box clamp from `nodeShapes.ts` — SHEAR 20, TAPER 26,
POINT 22, RAIL 11, LAYER 9, FAN 30, clamp fraction 0.3), stroked 1.4pt in the
kind accent, filled `rupuPanel`. Content inside the shape's safe rect: kind
chip (mono 9 uppercase, accent) + icon, step id (12.5 semibold ink), sub-line
(10.5 dim; mono for run/action). Selected: brand stroke 2pt + soft brand
drop shadow. A `nodeStyle` build flag also supports the "cards" variant:
plain rounded rect + 3.5pt accent top bar (see mockup tweaks).

### 4. Palette (Blocks tab) — port `NodePalette.tsx`

Filter field; `WORK` / `ORCHESTRATION` eyebrow sections; one card per kind:
mini silhouette preview (34×20, same Shape code), mono label in accent, one-
line tagline (`one agent`, `over a list`, `N at once`, `review+gate`,
`connector call`, `command`, `if / then / else`, `fan out`, `barrier`,
`human approval`). Click selects → detail card below (what-it-is blurb,
`*`-marked required fields, YAML example, **Add to canvas** button); drag
onto the canvas drops the node at the pointer (`NSItemProvider` drag or a
custom pointer-tracked ghost). Field names in examples must mirror
`crates/rupu-orchestrator/src/workflow.rs`. An alternate `dock` placement
(floating chip strip, top-left of canvas) exists in the mockup; ship rail
first.

### 5. Canvas interactions

- Drag node body → move (clamped to canvas). Click → select + rail jumps to
  Step tab. Esc clears; ⌫ deletes selection (never while a text field has
  focus).
- Ports: source dot on the right edge (branch gets two, `then`/`else`,
  labeled); drag from a port draws a dashed brand bezier; release on another
  node adds the edge. Edges: cubic bezier, 1.5pt `rupuBorderStrong`,
  arrowhead at target, mono uppercase `then`/`else` labels near the source.
- Branch/split/join routing is edge-derived (single source of truth), same
  as `withDerivedEdges` in `lib/workflowGraph.ts`.

### 6. YAML round-trip

Same architecture as `WorkflowEditor.tsx`: the draft YAML is the single
source of truth; graph edits serialize graph → workflow object → YAML
(canonical dump) and the pane re-renders; every mutation (add/remove/connect/
rename/field edit) round-trips. Reject cycles before committing. Reuse the
step schema from `workflow.rs` (`agent/prompt`, `for_each/max_parallel`,
`parallel:`, `panel: panelists/subject`, `branch: condition/then/else`,
`split: [targets]`, `join: wait`, `approval: prompt/timeout_seconds/
on_timeout`, `action/with`, `run`).

### 7. Step form (rail, Step tab)

Header: silhouette preview + id + kind chip. Fields per kind (eyebrow labels,
`rupuBg` inputs, 1px `rupuBorder`, radius 5, brand focus ring): STEP ID
(slugified rename that rewrites edges), AGENT (picker from the agent
catalog), PROMPT, FOR EACH / MAX PARALLEL, SUB-STEP IDS, PANELISTS / SUBJECT,
CONDITION, WAIT POLICY (all/any/count), APPROVAL PROMPT / TIMEOUT /
ON TIMEOUT, TOOL (MCP catalog via `GET /api/tools`) / WITH, COMMAND. Below:
derived read-only rows (`then →`, `else →`, `targets →`, `waits on ←`) and a
destructive **Remove step**. Settings tab: NAME, DESCRIPTION, TRIGGER card
(cron pill + expression), INPUTS card.

### 8. Run overlay

Design/Run segmented control (Launch also flips to Run). In Run mode, per
`StepGraphView.swift`'s glyph language: done = green check badge, running =
pulsing blue dot (respect `accessibilityReduceMotion`), pending = dashed ring
+ dimmed node, skipped = mute ring + 0.45 opacity; node stroke takes the
status color; for_each shows `n/m items`. Edges: done→done = green; into a
running node = blue animated dashes; skipped path = dashed dim. Statuses come
from the existing run stream (`RunDetailStore` / SSE), not new endpoints.

### 9. Acceptance checklist

- [ ] All 10 kinds placeable by drag AND by Add-to-canvas; correct shape/
      accent/icon in both light and dark
- [ ] Connect, rename, delete, field edits all round-trip into canonical YAML
- [ ] branch then/else + split targets derive from edges
- [ ] Run overlay matches StepGraphView glyph semantics; reduce-motion safe
- [ ] Rail width/tab + YAML-pane state persist across launches
- [ ] No ad-hoc colors/fonts — RupuDesign tokens only
