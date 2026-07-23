# Flow Designer — UX fixes round 1 (palette-in-rail, source toggle, semantics, editing comfort) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Four UX fixes matt requested after eyeballing v0.61.0-beta's Flow Designer `next` UI: (1) the node-palette cards move off the canvas into the inspector rail so the graph is unobstructed; (2) the YAML source pane can be hidden/shown; (3) connectors and nodes (and Agent Builder cards) carry semantic color/icons; (4) long-form text fields get room and the inspector rail becomes drag-resizable.

**Architecture:** Pure frontend (`crates/rupu-cp/web`), zero Rust. Everything workflow-editor-side is gated on `workflowEditorUi === 'next'` (classic byte-identical, per firm no-regression rule). Agent Builder icon work touches only the `.ab-*` card UI, which exists solely under the `agent_authoring_ui = "next"` flag (no classic surface). A new shared `kindVisuals.ts` module unifies per-kind accent + icon (also resolving the KIND_ACCENT/KIND_KEY duplication deferred from the previous pass).

**Tech Stack:** React 18 + TypeScript + Tailwind + Vitest (jsdom), @xyflow/react, lucide-react ^0.468 (already a dependency).

## Global Constraints

- **Frontend-only. No `*.rs` edits.**
- **Classic path byte-identical** for every workflow-editor component touched: new behavior is added under `workflowEditorUi === 'next'` branches (or props defaulting to classic behavior), never by mutating classic markup/props. `WorkflowEditor.tsx` currently renders: SplitPane(graph over YAML) left + fixed `lg:w-80` inspector rail right with tabs Settings|Step|Reference.
- **Icons:** `lucide-react` only (installed, tree-shaken named imports). Before using an icon name, verify it exists in the installed version (`node_modules/lucide-react/dist/lucide-react.d.ts` or import-and-tsc); substitute the closest equivalent if absent.
- **CSS:** extend the existing namespaced `.wfx-*` (workflow) / `.ab-*` (agent-builder) blocks in `src/styles.css`; `rgb(var(--c-*))` existing tokens only (NO new `--c-*` custom properties); dark-ready; `@media (prefers-reduced-motion)` guard on any new transition/animation.
- **Persistence keys:** `localStorage['rupu.editor.sourceOpen']` ('1'/'0', default open) and `localStorage['rupu.editor.railWidth']` (px int, clamp [280, 640], default 320). Guard all localStorage access in try/catch (house pattern: see REFORMAT_NOTICE_KEY usage in WorkflowEditor.tsx:170-181).
- **Tests:** vitest + jsdom, house mocking patterns (see WorkflowEditorGraph.test.tsx / WorkflowSettingsForm.test.tsx). Structural markers + behavior via captured callbacks; jsdom cannot verify pixels — matt eyeballs in-browser before merge (call out in PR). Run `npx vitest run` + `npx tsc -b` from `crates/rupu-cp/web`.
- Reuse the pointer-capture + keyboard (role="separator") resize pattern from `SplitPane.tsx` for the rail resizer.

## File Structure

**Create:** `src/components/workflow-editor/kindVisuals.ts` (shared per-kind accent ColorKey + lucide icon + label).
**Modify:** `WorkflowEditor.tsx` (rail palette slot, source toggle, resizable rail), `WorkflowEditorGraph.tsx` (portal palette, edge semantics), `NodePalette.tsx` (rail variant, consume kindVisuals), `nodes/EditableStepNode.tsx` (icon in card, consume kindVisuals), `StepForm.tsx` + `settings/*.tsx` (larger long-text under next), `agentBuilder/AgentBuilder.tsx` + `agentBuilder/cards/types.ts` (card icons), `src/styles.css`.

---

## Task 1: Node palette moves into the inspector rail (next only)

**Files:** Modify `WorkflowEditor.tsx`, `WorkflowEditorGraph.tsx`, `NodePalette.tsx`, `src/styles.css`; Test `WorkflowEditor.test.tsx` + `NodePalette.test.tsx`.

**Interfaces:** `WorkflowEditorGraph` gains `paletteContainer?: HTMLElement | null` — when set AND `workflowEditorUi === 'next'`, the graph renders `createPortal(<NodePalette variant="rail" …/>, paletteContainer)` instead of the floating dock (drag-onto-canvas and click-to-add-at-center logic stay inside the graph component untouched). `NodePalette` gains `variant?: 'float' | 'rail'` (default `'float'`; rail = compact non-absolute grid using `.wfx-palette-rail` classes). `WorkflowEditor` hosts a slot div in the aside above the tabs (next only) and passes its element via ref state.

- [ ] **Step 1: Failing tests** — with flag `next` + a mounted rail container: the palette markers (`.wfx-pcard`) render INSIDE the aside/rail slot and NOT absolutely over the canvas; click-to-add from the rail still calls `onAdd`/adds a node. With `classic`: floating dock unchanged (existing markers/position), no rail slot rendered.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** (portal; keep classic byte-identical; rail variant compact: icon-dot + label rows or 2-col grid, `.wfx-palette-rail` CSS).
- [ ] **Step 4: Run → pass** (`npx vitest run src/components/workflow-editor`; `npx tsc -b` clean).
- [ ] **Step 5: Commit** `-m "feat(cp-web): node palette docks in the inspector rail behind flag"`.

---

## Task 2: Hide/show the YAML source pane (next only)

**Files:** Modify `WorkflowEditor.tsx`; Test `WorkflowEditor.test.tsx`.

**Interfaces:** local state `sourceOpen: boolean`, default from `localStorage['rupu.editor.sourceOpen']` (missing → open). Next only: when closed, the left pane renders the graph full-height (no SplitPane) plus a slim bottom bar carrying a "Show source" toggle AND the ValidityBadge (validity must stay visible while the editor is hidden); when open, the existing bottom bar (`⟳ synced from graph` + badge) gains a "Hide source" toggle. Classic: SplitPane always, markup byte-identical.

- [ ] **Step 1: Failing tests** — flag `next`: a source-toggle button exists; clicking it removes the YAML editor from the DOM and keeps the validity badge visible; clicking again restores it; the preference persists to localStorage. Flag `classic`: no toggle; YAML editor always present.
- [ ] **Step 2: Run → fail.**  **Step 3: Implement.**  **Step 4: Run → pass** + `tsc -b`.  **Step 5: Commit** `-m "feat(cp-web): collapsible YAML source pane behind flag"`.

---

## Task 3: Shared kind visuals (accent + icon) and semantic edges (next only)

**Files:** Create `kindVisuals.ts`; Modify `nodes/EditableStepNode.tsx`, `NodePalette.tsx`, `WorkflowEditorGraph.tsx`, `src/styles.css`; Test existing test files + a small `kindVisuals.test.ts`.

**Interfaces:** `kindVisuals.ts` exports `KIND_ACCENT: Record<StepKind, ColorKey>` (exact current mapping: step→'status.running', for_each→'brand.500', parallel→'sev.critical', panel→'status.awaiting', branch→'status.done'), `KIND_ICON: Record<StepKind, LucideIcon>` (suggested: step→Bot, for_each→Repeat, parallel→Columns3, panel→ShieldCheck, branch→GitBranch — verify availability), `KIND_LABEL`. `EditableStepNode` and `NodePalette` import from it (delete their local duplicate maps — this must NOT change classic rendering: classic uses the same accent values, and icons render in the NEXT branch only).

**Edges (WorkflowEditorGraph edges memo, next only):** branch-arm edges get filled label chips — `✓ then` on `status.done` green / `✕ else` on `status.failed` red (labelBgStyle filled with `colors.alpha(key, ~0.12)`, labelStyle colored, border-radius via labelBgBorderRadius), stroke 2.5 colored, tinted arrow marker (exists); plain order edges keep the muted stroke + gain a muted tinted arrow; any edge with a pre-existing `label` (non-branch) gets a neutral brand-tinted chip. Classic edge output byte-identical.

- [ ] **Step 1: Failing tests** — kindVisuals exports complete maps for all 5 kinds; next node card renders the kind icon (`.wfx-kindicon` marker); rail/float palette cards render icons; edges memo (next) emits labelBgStyle fill for branch arms + labels `✓ then`/`✕ else`; classic edges unchanged (existing assertions still pass).
- [ ] **Step 2: Run → fail.**  **Step 3: Implement.**  **Step 4: Run → pass** + `tsc -b`.  **Step 5: Commit** `-m "feat(cp-web): shared kind icons + semantic edge chips behind flag"`.

---

## Task 4: Agent Builder card icons

**Files:** Modify `agentBuilder/AgentBuilder.tsx`, `agentBuilder/cards/types.ts` (card registry — add an `icon` field to the card def type), `src/styles.css` (`.ab-*` icon sizing); Test `AgentBuilder.test.tsx`.

**Interfaces:** each card def gains `icon: LucideIcon` (suggested: identity→IdCard, model→Cpu, anthropic→Sparkles, tools→Wrench, permission→Shield, reasoning→Brain, context→Layers, output→FileJson, dispatch→Send, concerns→ListChecks, prompt→FileText — verify availability, substitute equivalents). Icons render in both the palette rows (`.ab-pcard`) and the canvas card heads (`.ab-card-head`), sized ~14px, colored `currentColor`/muted. The AB card UI exists only under its own `next` flag — no classic surface to protect, but do not alter any behavior/props beyond adding icons.

- [ ] **Step 1: Failing tests** — palette rows and card heads render an icon element (e.g. `svg.ab-cicon`).  **Step 2: fail.**  **Step 3: Implement.**  **Step 4: pass** + `tsc -b`.  **Step 5: Commit** `-m "feat(cp-web): agent builder card icons"`.

---

## Task 5: Long-text room + drag-resizable inspector rail (next only)

**Files:** Modify `WorkflowEditor.tsx` (rail resizer), `StepForm.tsx`, `settings/TriggerCard.tsx`/`InputsCard.tsx`/`AutoflowCard.tsx`, `WorkflowSettingsForm.tsx`, `ExpressionField`/`ExpressionFieldImpl` if that's where the multiline textarea lives, `src/styles.css`; Test `WorkflowEditor.test.tsx` + relevant form tests.

**Interfaces:**
- **Long text (next only):** the multiline prompt editor (ExpressionField `multiline`) and description textareas get a taller default (prompt min-height ≈ 10rem, descriptions ≈ 4 rows) and `resize: vertical`. Implement via a `.wfx-rail` class on the aside when next + targeted CSS (e.g. `.wfx-rail .wfx-ta-lg`), or a size prop threaded where cleaner — classic rendering byte-identical (flag-gated class/prop only).
- **Rail resize (next only, lg+):** aside width driven by state (default 320px, `localStorage['rupu.editor.railWidth']`, clamp [280, 640]); a 6px drag handle on the aside's left edge using SplitPane's pointer-capture pattern; keyboard-accessible (`role="separator"`, `aria-orientation="vertical"`, ArrowLeft/ArrowRight ±16px). Classic keeps literal `lg:w-80` markup.

- [ ] **Step 1: Failing tests** — flag `next`: aside carries the resizer handle (role=separator) and a style width; ArrowLeft/ArrowRight changes width within clamp; width persists to localStorage; prompt textarea carries the large-size marker class. Flag `classic`: no handle, `lg:w-80` class intact, no size marker.
- [ ] **Step 2: Run → fail.**  **Step 3: Implement.**  **Step 4: Run → pass** + `tsc -b`.  **Step 5: Commit** `-m "feat(cp-web): resizable inspector rail + roomier long-text fields behind flag"`.

---

## Task 6: Whole-branch verification + PR

- [ ] `npm run build` + full `npx vitest run` + `npx tsc -b` from `crates/rupu-cp/web`.
- [ ] Final whole-branch reviewer (most capable model); fix Critical/Important.
- [ ] Draft PR: summarize the four UX fixes, frontend-only behind flags, **explicit in-browser visual check request**.

## Out of scope
- Any Rust; Run Room; further engine primitives; touch/mobile drag for the resizer beyond pointer events.
