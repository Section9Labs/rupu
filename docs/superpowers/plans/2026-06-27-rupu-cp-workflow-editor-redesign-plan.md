# rupu-cp Workflow Editor Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Replace the tab-based workflow editor with a single unified screen — editable LR graph on top, live YAML below, inspector rail on the right — with a graphical palette and context-aware expression autocomplete. Ships as **6 phased PRs**, validated (binary) between each.

**Design:** `docs/superpowers/specs/2026-06-27-rupu-cp-workflow-editor-redesign-design.md` (read §1 layout, §2 wireframes, §4 live sync, §10 component plan, §11 Decisions).

**Decisions (locked):** top/bottom split (resizable); YAML = source of truth, canonical rewrite accepted + `⟳ synced` badge; **mirror** the Runs node visuals as separate components with shared tokens (do NOT refactor the Runs cards); CodeMirror `ExpressionField` (lazy, focused-field only); autocomplete offers input *names* only; Save gated on ≥1 complete step; no position persistence (auto-layout on load).

**Constraints:** no `any`; static Tailwind w/ existing tokens (`ink`/`panel`/`brand`/`border`); **xyflow + codemirror stay OUT of the main `index-*.js`** (the whole editor shell is lazy-loaded); stage only specific files; never `-A`/`.rupu/*`; never package-wide `cargo fmt`. The pure core `lib/workflowGraph.ts` and `lib/workflowLayout.ts` are load-bearing — don't change their behavior except where a phase explicitly says so (e.g. LR layout in P3).

## Phase roadmap (each = one PR; expand the next phase's tasks after the prior merges + matt validates)
- **P1 — Unify the shell (no new capability).** Tabs → one screen (graph top / YAML bottom / inspector right), resizable splitter, YAML always editable, unified Save/Revert, validity badge in header, remove the read-only steps spine. Graph→YAML already works; this just co-locates it. **(Detailed below.)**
- **P2 — Live YAML→graph reconcile.** Debounced parse, id-diff patch-in-place, pause+dim on invalid YAML (don't nuke the graph), preserve selection. The true bidirectional promise.
- **P3 — Runs-graph parity.** Switch editor layout to `rankdir:'LR'`; re-skin editable nodes to mirror the Runs cards (StepNode/ParallelNode/FanoutNode/PanelLoopNode look) as separate `workflow-editor/nodes/*` components sharing style tokens; smoothstep edges; selected/hover/⚠ states.
- **P4 — Graphical palette + canvas authoring.** Draggable palette cards (shrunk real node previews), drop-on-canvas + drop-on-edge insert, inline ⊕-next, green/red connection feedback, find-step.
- **P5 — Expression intelligence.** `lib/workflowExpressions.ts` (typed vocabulary + `completionsFor(context)`) + `ExpressionField` (CodeMirror highlight + context-aware autocomplete), wired into `StepForm` expression fields.
- **P6 — Reference & polish.** `ExpressionReference` panel/overlay + ƒx affordance; a11y pass (tablist/separator/keyboard for canvas + autocomplete); first-time reformat confirm; dirty-navigation guard.

---

## PHASE 1 — Unify the shell

**Outcome:** the workflow page shows graph + YAML simultaneously (no tabs), with an inspector rail. Behaviorally identical to today otherwise (graph edits → YAML live; YAML edits → graph on the existing reseed; Save through `saveWorkflow`). Pure layout/IA refactor.

### Reference (read first)
- `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx` — current page: header (name/scope/Autoflow/Delete/Run), the `view: 'graph'|'yaml'` tabs + `ViewTabButton`, the lazy `WorkflowEditor`, the `CodeEditor` YAML edit mode (with `editing` toggle, Cancel/Save), the read-only steps spine (`readSteps`/`StepRow`), `draftYaml`/`dirty`/`saving`/`saveError`/`validity` state + the debounced `validateWorkflow` effect, `revertDraft`, `remove`, launcher.
- `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditor.tsx` — composes `WorkflowEditorGraph` + `StepForm` + `WorkflowSettingsForm`; props `{ initialYaml, agents, onYamlChange }`; owns graph state + the `lastSeenYaml` echo-guard + `validateGraph` → `problemsById`.
- `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditorGraph.tsx`, `StepForm.tsx`, `WorkflowSettingsForm.tsx` — reused as-is.
- `crates/rupu-cp/web/src/pages/AgentDetail.tsx` — Suspense/lazy + styling conventions.
- Design §1 (layout), §2(a) wireframe (full editor at rest).

### Task 1: Editor shell composition (graph top / YAML bottom / inspector right)

**Files:** Modify `crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditor.tsx`; create `crates/rupu-cp/web/src/components/workflow-editor/SplitPane.tsx` (a tiny vertical resizable splitter) + test.

- [ ] **Step 1: `SplitPane.tsx`** — a controlled two-pane vertical splitter: props `{ top: ReactNode; bottom: ReactNode; defaultRatio?: number }`. A draggable horizontal divider (pointer events; keyboard: ↑/↓ adjust, with `role="separator"` + `aria-orientation="horizontal"` + `aria-valuenow`). Ratio in local state, clamped (e.g. 25–80%). Static Tailwind. No `any`. Pure presentational.
- [ ] **Step 2: Test `SplitPane`** — renders top + bottom; dragging/keyboard updates the ratio (assert the style/flex-basis changes or the aria-valuenow). Keep it light.
- [ ] **Step 3: Restructure `WorkflowEditor.tsx`** into the unified shell. New props:
  ```ts
  interface WorkflowEditorProps {
    draftYaml: string;                 // controlled by the page (single source)
    onYamlChange: (yaml: string) => void;
    agents: AgentSummary[];
    validity: { ok: boolean; error?: string } | null;  // from the page's live validate
  }
  ```
  Layout (design §2a): a `SplitPane` whose **top** is the existing `WorkflowEditorGraph` (with palette/toolbar) and **bottom** is the `CodeEditor` (`language="yaml"`, `value={draftYaml}`, `onChange={onYamlChange}`, always editable — no Edit toggle). To the right of the split, an **inspector rail** with a small tablist `[Settings] [Step]` (Reference tab arrives in P6): `Step` shows `StepForm` for the selected node (disabled/hint when none selected); `Settings` shows `WorkflowSettingsForm`. Auto-select the `Step` tab when a node is selected. Keep the graph-state ownership + `lastSeenYaml` echo-guard + `problemsById` exactly as today, but seed/sync from the `draftYaml` prop instead of `initialYaml` (same reseed semantics for P1 — live reconcile is P2). The `⟳ synced` / validity micro-text sits on the YAML pane footer (badge text from `validity`).
  - Move the invalid-connection toast here (unchanged).
- [ ] **Step 4:** ensure the component remains the lazy-loaded boundary (xyflow + codemirror both pulled in here) — the PAGE lazy-imports it.
- [ ] **Step 5: Commit** `git add crates/rupu-cp/web/src/components/workflow-editor/WorkflowEditor.tsx crates/rupu-cp/web/src/components/workflow-editor/SplitPane.tsx <test>` → `feat(cp/web): unified workflow editor shell (graph+YAML+inspector)`.

### Task 2: Page wiring — drop the tabs, unify Save/Revert, header validity

**Files:** Modify `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx` + its test.

- [ ] **Step 1: Remove the tab UI** (`view` state, `ViewTabButton`, the `view==='graph'?…:editing?…:` branching) and the **read-only steps spine** (`readSteps`, `StepRow`, `StepChip`, the Steps `<section>`). Remove the YAML `editing`/`startEdit`/`cancelEdit` toggle — YAML is always editable inside the shell now.
- [ ] **Step 2: Render the unified shell.** Keep the header (BackLink, name, ScopeChip, Autoflow chip, Delete, Run/launcher) and ADD a **validity badge** in the header (`✓ valid` green / `✕ <error>` red, from the existing `validity` state). Below the header, render `<Suspense fallback={…}><WorkflowEditor draftYaml={draftYaml} onYamlChange={setDraftYaml} agents={agents} validity={validity} /></Suspense>` (keep `WorkflowEditor` lazy). Keep the single **Save** + **Revert** (already unified) — place them on the shell's YAML footer or the header; `saveDisabled = saving || !dirty || validity?.ok === false`. Preserve `save`/`revertDraft`/`remove`/the debounced `validateWorkflow` effect/`getAgents` fetch exactly.
- [ ] **Step 3: Update `WorkflowDetail.test.tsx`** — remove/replace the tab-switch tests; keep: load renders the (stubbed) editor; an editor `onYamlChange` enables Save → `saveWorkflow` called; invalid validate disables Save + shows reason; Delete confirm→`deleteWorkflow`→navigate. Mock `WorkflowEditor` to a stub exposing an `onYamlChange` trigger (as today).
- [ ] **Step 4: Gates** (from `crates/rupu-cp/web`): `npm test -- --run` green; `npm run build` exit 0; `grep -c recharts dist/assets/index-*.js` → 0; `grep -l "@xyflow\|@codemirror\|ReactFlow" dist/assets/index-*.js` → empty (both still lazy); report main chunk size (should stay ~50 KB).
- [ ] **Step 5: Commit** `git add crates/rupu-cp/web/src/pages/WorkflowDetail.tsx crates/rupu-cp/web/src/pages/WorkflowDetail.test.tsx` → `feat(cp/web): single-screen workflow editor — drop Graph/YAML tabs`.

### Phase 1 final verification
- `npm test -- --run` green; `npm run build` strict; xyflow + codemirror out of main chunk; main ~50 KB.
- Review: no behavior regression vs. tabs (graph→YAML still live; YAML→graph still reseeds; Save/validate/delete/run intact); a11y on the splitter; lazy boundary preserved.
- matt visual-validates: one screen, graph above + YAML below, resizable, inspector shows Settings + the selected node's Step form; editing either still works; Save/validity behave.
- TODO note: P2 makes YAML→graph live (today it reseeds on foreign change only).
