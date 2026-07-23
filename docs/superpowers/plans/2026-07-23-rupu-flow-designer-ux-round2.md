# Flow Designer — UX round 2 (edge expressiveness, card chrome fix, autocomplete popup) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox syntax.

**Goal:** Fix three issues matt found in-browser on v0.62.0-beta: (1) plain connectors show no color/animation — only branch arms were styled; (2) the node card's top accent bar renders outside the rounded corners and the selected state stacks clashing chrome; (3) the expression autocomplete popup is clipped by the rail's scroll container, and some expression-capable fields lack completions.

**Architecture:** Pure frontend (`crates/rupu-cp/web`), zero Rust. Workflow-editor visual changes gated on `workflowEditorUi === 'next'` (classic byte-identical). Exception, explicitly sanctioned: the CodeMirror tooltip parent/theming fix in `ExpressionFieldImpl.tsx` is an unconditional BUGFIX (the clipping bug exists in classic too); new ExpressionField wirings for previously-plain inputs are next-gated.

**Tech Stack:** React 18 + TS + Vitest/jsdom, @xyflow/react, @codemirror/autocomplete + @codemirror/view (`tooltips`), lucide-react, existing `kindVisuals.ts`.

## Global Constraints
- Frontend-only; no `*.rs`. Classic byte-identical except the sanctioned tooltip bugfix above.
- CSS: `.wfx-*` namespaced additions, `rgb(var(--c-*))` existing tokens only, dark-ready, reduced-motion guards LIVE (any new animation must stop under `prefers-reduced-motion: reduce`).
- Tests: vitest jsdom, structural markers + emitted-shape assertions; `npx vitest run` + `npx tsc -b` clean from `crates/rupu-cp/web`. matt eyeballs pixels.

---

## Task 1: Every edge expresses meaning (next only)

**Files:** Modify `WorkflowEditorGraph.tsx` (edges memo), `src/styles.css`; Test `WorkflowEditorGraph.test.tsx`.

When `next`: every edge's stroke + arrow tint from the SOURCE node's kind accent (`KIND_ACCENT[sourceKind]` via kindVisuals + useThemeColors; build a `Map(node.id → kind)` inside the memo), strokeWidth 2; branch-arm edges keep their existing ✓/✕ chips, colors and 2.5 stroke (unchanged); all next edges get `animated: true` (xyflow built-in dash-flow). styles.css: ensure the dash animation is visible/tasteful over the grid and add a LIVE `@media (prefers-reduced-motion: reduce)` rule killing `.react-flow__edge-path` animation. Classic edge emission byte-identical (all inside `next` conditionals — extend the existing classic-unchanged tests).

- [ ] Failing tests: next plain edge carries source-kind accent stroke + `animated: true`; branch edges unchanged from current next shape except animated; classic edges exactly today's shape (no `animated`).
- [ ] Fail → implement → pass (`npx vitest run src/components/workflow-editor`; `tsc -b`).
- [ ] Commit `-m "feat(cp-web): kind-colored animated edges behind flag"`.

## Task 2: Node card chrome fix (next only)

**Files:** Modify `nodes/EditableStepNode.tsx`, `src/styles.css`; Test `EditableStepNode.test.tsx`.

Problems (from screenshot + code): `.wfx-bar` (absolute, 3px, `border-radius: 12px 12px 0 0`) juts past the card's 12px rounded corners (radius collapses on a 3px-tall element; no clipping); selected state sets `borderColor: <kind accent>` AND `.wfx-sel`'s brand-purple ring → clashing double chrome. Fix:
1. Clip the bar inside the radius: wrap bar+head+body in an inner `.wfx-clip` div (`border-radius: inherit; overflow: hidden`) so xyflow `Handle`s (siblings, on the border) are NOT clipped — or equivalently render the accent as a clipped pseudo-element. Bar stays 3px kind-colored.
2. One coherent selection signal: selected → ring + border BOTH from the kind accent (`box-shadow: 0 0 0 2px alpha(accent,.35)` via inline style or CSS var `--wfx-accent` set inline), drop the brand-purple ring for nodes. Unselected unchanged.
- [ ] Failing tests: next card renders `.wfx-clip` containing `.wfx-bar`; handles remain direct children of `.wfx-node` (not inside the clip); selected next card carries the accent ring marker (assert inline style/CSS var), no brand-purple ring. Classic untouched.
- [ ] Fail → implement → pass. Commit `-m "fix(cp-web): clip node accent bar inside card radius + coherent selection ring"`.

## Task 3: Autocomplete popup rendering + coverage

**Files:** Modify `ExpressionFieldImpl.tsx`, `src/styles.css` (tooltip theme), `StepForm.tsx` (wire missing fields, next-gated); Test `ExpressionField.test.tsx` + `StepForm.test.tsx`.

1. **Popup fix (unconditional bugfix):** add `tooltips({ parent: document.body, position: 'fixed' })` (from `@codemirror/view`) to the extensions so the completion tooltip escapes the rail's `overflow-y-auto` clipping. Theme `.cm-tooltip`/`.cm-tooltip-autocomplete` (background `rgb(var(--c-panel))`, border `--c-border`, selected row `--c-brand-500` alpha, mono 12-13px, max-height + scroll, z-index above the editor shell) — both themes (tokens flip automatically), guard SSR (`typeof document !== 'undefined'`).
2. **Coverage audit (next-gated):** enumerate StepForm text inputs that accept template expressions but are plain `<input>`s (known suspect: panel "Approval prompt" ~:636; check gate/fix_with etc.). Wire each through `ExpressionField` (single-line) when `workflowEditorUi === 'next'`; classic keeps the plain input byte-identical.
- [ ] Failing tests: the CM tooltip config includes the body parent (assert via rendered tooltip parenting where jsdom allows, else unit-assert the extensions builder output/flag); next Approval-prompt renders the ExpressionField shell marker; classic renders today's plain input.
- [ ] Fail → implement → pass. Commit `-m "fix(cp-web): autocomplete tooltip escapes scroll clipping; expression completions on missed fields"`.

## Task 4: Verify + PR
- [ ] `npm run build` + full `npx vitest run` + `tsc -b`; final whole-branch review (most capable model); draft PR with in-browser check request.
