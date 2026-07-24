# rupu — Run-graph "next" visuals (kind color + status overlay + animated connectors)

**Date:** 2026-07-23
**Status:** approved (matt, 2026-07-23)
**Scope:** frontend only — `crates/rupu-cp/web`. No Rust/API changes.

## 1. Problem

The run-detail graph (`/runs/:id` → `RunDetail` → `components/RunGraph.tsx` + `components/graph/*`) colors every node by run **status** only (running/done/failed via `graph/stepStyle.ts`), with a fixed violet tint on parallel/panel that doesn't even match the editor. The workflow **editor** graph, by contrast, uses a rich per-**kind** visual language (`workflow-editor/kindVisuals.ts` `KIND_ACCENT`/`KIND_ICON`, colored source-kind edges, `animated` dash-flow) gated behind `workflow_editor_ui = 'next'`. The two views of the same workflow look unrelated. We want the run graph to adopt the same visual language — kind-colored nodes and connectors with animation — while keeping the live run status legible.

## 2. Decision summary

Behind the **existing `workflow_editor_ui = 'next'` flag** (reused — one switch for "the new visual language" across author + run views; classic keeps today's look byte-for-byte), give the run graph a **two-channel** paint:

- **Kind channel (identity):** node top-bar + an icon+label kindpill take the step's kind color from `KIND_ACCENT`/`KIND_ICON`. Edges take their **source step's** kind color.
- **Status channel (overlay):** the glyph badge, label color, and the live ring/marching-ants animation stay **state-colored** — running pulses, awaiting glows amber, failed shows a red ✕. Status is the secondary signal layered on the kind identity.

Both channels resolve through the already-shared `lib/useThemeColors.ts` token space (light+dark handled). No Rust, no API, no new color tokens.

Rejected: (a) status-primary with only small kind accents — doesn't deliver "the new UI graph"; (b) kind-only (drop status color) — a failed step no longer reads as failed; (c) a new `run_viewer_ui` flag — extra config for what is conceptually the same "new visual language" feature; (d) unconditional replacement — no classic fallback during dogfooding.

## 3. The run-kind → editor-kind adapter

The run model's `StepNodeDto['kind']` vocabulary differs from the editor's `StepKind`: run emits `'step' | 'parallel' | 'fanout' | 'panel' | 'gate' | 'action'` (no `branch`); editor has `'for_each'` and `'approval_gate'`. A pure adapter `runKindToStepKind(k): StepKind` maps `fanout→for_each`, `gate→approval_gate`, and the rest 1:1, so the run graph can index `KIND_ACCENT`/`KIND_ICON` from `workflow-editor/kindVisuals.ts`. `kindVisuals` is imported into the run graph (both live under `crates/rupu-cp/web/src/components/`; a shared import is fine — if lint/boundary objects, lift `kindVisuals.ts` to a neutral location, but a direct import is the default).

## 4. Nodes — two-channel paint (next only)

Each run node component (`StepNode`, `GateNode`, `ActionNode`, and the containers `ParallelNode`/`PanelLoopNode`/`FanoutNode`) gains a `next` branch. Classic branch = today's code unchanged.

- **Kind channel:** top-bar background = `colors.get(KIND_ACCENT[stepKind])`; a kindpill (icon from `KIND_ICON` + kind label) tinted like `EditableStepNode`'s `kindChipStyle` (`alpha(accent, .14)` bg + accent text). Containers (`ParallelNode`/`PanelLoopNode`) replace the hardcoded `brand.500` tint with `KIND_ACCENT['parallel']` / `KIND_ACCENT['panel']`.
- **Status channel (unchanged mechanism):** the circular glyph badge + label keep `stateStyle(colors, node.state).color`; the live ring stays `rg-pulse-run` / `rg-pulse-await`. Terminal states (done/failed/skipped) read from the glyph badge + label alone (git-graph convention). GateNode's dashed border + `◇` and ActionNode's `connector` chip stay as kind-distinguishing affordances but now also carry the kind color.
- The node's selection/focus ring (when `onSelectNode` highlights) uses the kind accent in next (matching `EditableStepNode`), state color in classic.

**Status legibility guard:** because kind now owns the top-bar, a terminal-failed node must still be unmistakable — the glyph badge (red ✕) + label ("failed") in `status.failed` is the required signal; do not let the kindpill or bar visually swamp it (the badge sits in the node's leading position at full state color).

## 5. Connectors — kind-colored, active-animated (next only)

`RunGraph.tsx`'s edge memo (currently `:155-181`) gains, in next mode, a source-node-kind lookup (mirror `WorkflowEditorGraph.tsx`'s `kindById` map) and:

- **stroke color** = `colors.get(KIND_ACCENT[runKindToStepKind(sourceKind)])`; markerEnd tinted to match.
- **target running** → the live-frontier animation: marching-ants in the **kind color**.
- **target awaiting** → marching-ants in **amber** (`status.awaiting`) — status wins for "needs you", regardless of source kind.
- **target done** (edge traversed) → solid kind color, full alpha.
- **target pending / not reached** → muted kind color (low alpha, e.g. `alpha(accent, .35)`) or `inkMute` — muted so the active path stands out.
- Classic mode: the existing flat `inkMute` + blue/amber `rg-edge-active`/`rg-edge-await` behavior, unchanged.

**CSS (additive — classic untouched):** the existing `.rg-edge-active` / `.rg-edge-await` classes (which hardcode blue/amber stroke inside the keyframe rule) stay exactly as they are and continue to serve **classic**. Add ONE new color-agnostic class `.rg-edge-flow` that carries only `stroke-dasharray` + the `rg-march` animation (no `stroke`); **next**-mode edges use `rg-edge-flow` and set the stroke color via the React Flow edge `style.stroke` (JS) — kind color for a running target, amber for awaiting. This keeps classic's CSS byte-stable while letting next's marching-ants render in any kind color. The global `prefers-reduced-motion` guard (`styles.css:547-549`) targets `.react-flow__edge-path` broadly, so it already covers `.rg-edge-flow` too (verify the selector; widen if it's class-specific).

Do **not** additionally set xyflow's `animated` prop — the run graph intentionally avoids it (RunGraph.tsx comment) because `rg-march` owns the animation; combining the two double-animates. Keep `rg-march` as the single animation source.

## 6. Flag threading

`RunGraph` (and the `components/graph/*` node components it renders) currently never read `useWorkflowEditorUi`. Thread the resolved `ui: 'classic' | 'next'` from `useWorkflowEditorUi()` — read it once in `RunGraph` (or `RunDetail`) and pass it down to node components via React Flow node `data` (the node components already receive `data.node`; add `data.ui`) and to the edge memo. Node components branch on it. No prop-drilling through unrelated components. The hook already resolves localStorage override → `[cp].workflow_editor_ui` → default `'classic'`, so classic is the default and the run graph is unchanged until a dogfooder flips the same flag they use for the editor.

## 7. Testing

- `runKindToStepKind` unit test: every run kind maps to a valid `StepKind` (`fanout→for_each`, `gate→approval_gate`, others identity).
- RunGraph edge memo: in next, an edge's stroke = source kind accent; edge into a running target carries the flow class; edge into awaiting carries amber; classic mode edges unchanged (regression test asserting the flat/blue/amber behavior still holds when `ui==='classic'`).
- Node components (`StepNode`/`GateNode`/`ActionNode`/`ParallelNode`/`PanelLoopNode`): render tests asserting (a) classic renders the state-colored bar as today, (b) next renders the kind bar + kindpill (icon + kind label present) AND still renders the state glyph/label (status overlay present). Use the existing `graph/*.test.tsx` harness (ReactFlowProvider wrapper).
- Snapshot/DOM: a terminal-failed node in next still exposes the failed glyph + "failed" label (status-legibility guard).
- Full: `npm run test`, `tsc -b`, `npm run build`. Classic byte-stability isn't asserted by a golden file, but the classic branch of each component must be the pre-change code path (reviewers verify the diff adds a `next` branch rather than rewriting classic).

## 8. Visual validation (required before merge)

Web rendering can't be subagent-validated. matt validates on the beta, in the browser, light + dark, with `rupu.cp.workflowEditorUi = next`: a workflow run mid-flight (running node pulsing, active edge marching in the kind color), a completed run (solid kind edges, kind-colored nodes with ✓ badges), a failed step (red ✕ still obvious), and an awaiting-approval gate (amber edge + glow). And with the flag off/`classic`: the run graph is visually identical to today.

## 9. Rollout

Single PR (frontend-only, one flag, ~6 focused component edits + one CSS refactor + the adapter). Ships to the next beta; matt's visual check gates the "recommend as default" decision (flipping `workflow_editor_ui` default to `next`, or leaving it opt-in, is a separate later call — out of scope here).
