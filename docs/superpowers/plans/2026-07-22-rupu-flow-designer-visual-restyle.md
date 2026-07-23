# Flow Designer — visual restyle + trigger/inputs/autoflow authoring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

## Context

The CP workflow editor works but (a) looks like the plain default @xyflow editor — matt's reaction after the if/branch pass was "I do not see any new UI" — and (b) leaves the most valuable authoring **raw-YAML-only**: `trigger`, typed `inputs`, and the entire `autoflow:` block are passthrough (`WorkflowMeta.rest`), surfaced in the Settings tab as read-only chips that say "edit these in the YAML tab." This is the **visual Flow Designer** pass (matt chose "Restyle + authoring cards"): apply the approved mockup's instrument design language to the editor AND add real authoring for trigger/inputs/autoflow — all behind the existing `[cp].workflow_editor_ui = "next"` flag (default `classic`), which already exists from the if/branch pass.

**This is a PURE FRONTEND pass — zero Rust.** The Rust schema (`Trigger`, `InputDef`, `Autoflow`, `AutoflowSelector`, …) and all validation (`validate_trigger`, `validate_input_def`, `validate_autoflow`) already exist in `crates/rupu-orchestrator/src/workflow.rs`. The authoring cards emit YAML those validators accept; `api.validateWorkflow` is the live check.

**Goal:** When `workflowEditorUi === 'next'`: (1) the editor renders in the mockup's instrument look — restyled kind-colored node cards, grid canvas, restyled edges + palette; (2) the Settings inspector gains real editors for `trigger`, `inputs`, and `autoflow` (enable toggle + selector/author-gate/claim/reconcile/workspace/outcome + a read-only lifecycle ribbon). When `classic`: the editor is byte-identical to today.

**Architecture:** Frontend only. The flag already threads `WorkflowDetail → WorkflowEditor → WorkflowEditorGraph/StepForm/NodePalette`; this pass also threads it onto the xyflow **node `data`** (the node component can't otherwise see it) and into `WorkflowSettingsForm`. Authoring edits `meta.rest.{trigger,inputs,autoflow}` through a pure, unit-tested `lib/workflowMeta.ts` helper (preserving all sibling `rest` keys + the name-first/steps-last key order). A namespaced `.wfx-*` CSS block (ported from the mockup, built on the existing `--c-*` tokens) carries the new look, mirroring the `.sr-*`/`.ab-*` precedent.

**Tech Stack:** React 19 + TypeScript + Tailwind + Vitest, @xyflow/react, js-yaml. (crates/rupu-cp/web)

## Global Constraints

- **Frontend-only. No Rust / no `*.rs` edits.** The flag, schema, and validation all already exist.
- **All new UI behind `workflowEditorUi === 'next'`; the classic path must be byte-identical.** Default every new flag prop to `'classic'`. `classic` renders exactly today's editor + today's read-only-chips Settings form.
- **Meta authoring edits `meta.rest.{trigger,inputs,autoflow}` WITHOUT dropping sibling keys.** `WorkflowMeta = { name; description?; rest: Record<string,unknown> }` (`lib/workflowGraph.ts:86`); `rest` holds every non-name/description/steps top-level key. `graphToWorkflowObject` spreads `rest` verbatim (name first, then description, then rest in existing order, then steps last). Patch immutably: `onChange({ ...meta, rest: { ...meta.rest, trigger: … } })`.
- **Emit ONLY valid keys/values** (every struct is `#[serde(deny_unknown_fields)]`; enums `snake_case`). Exact shapes (from `workflow.rs`):
  - `trigger`: `{ on: manual|cron|event, cron?, event?, filter? }`. Cross-field (`validate_trigger`): `manual`→no cron/event/filter; `cron`→`cron` required, 5-field cron (`min hour dom mon dow`), no event/filter; `event`→`event` required, no cron (filter allowed). The UI must show/hide these fields to stay valid.
  - `inputs`: a **MAP keyed by input name** (`BTreeMap<String, InputDef>`), NOT a list. Each `InputDef`: emit `type:` (string|int|bool, default string), `required:` (bool), `default:` (scalar), `enum:` (list), `description:`. (Serde renames: emit `type`/`enum`, never `ty`/`allowed`.) `validate_input_def`: a `default` must match `type`; if `enum` non-empty the default must be in it.
  - `autoflow`: `{ enabled, entity: issue|pull_request, source?, priority, selector, wake_on[], reconcile_every?, claim?, workspace?, outcome? }`. `selector`: `{ states[](open|closed), labels_all[], labels_any[], labels_none[], limit?, draft?(include|exclude|only), base?, authors[], authors_from?(collaborators|org_members), on_skip?(skip|label_needs_human) }`. `claim`: `{ key: issue|pr_head_sha, ttl? }`. `workspace`: `{ strategy: worktree|in_place, branch? }`. `outcome`: `{ output }` (required string). Cross-field (`validate_autoflow`): `reconcile_every` + `claim.ttl` are durations `<digits><s|m|h|d>`; `selector.draft`/`selector.base` allowed ONLY when `entity == pull_request` (gate those controls); `outcome.output` MUST be a key present in `contracts.outputs`.
  - **`outcome.output` is a `<select>` of existing `meta.rest.contracts.outputs` keys** (leave `contracts` as passthrough; disable the outcome field when there are none — do NOT co-author contracts in this pass).
- **Round-trip:** an existing workflow opened → edited via a card → saved must round-trip (`graphToWorkflowObject(yamlToGraph(x))` deep-equals x for untouched parts). `api.validateWorkflow(dumpedYaml)` must return ok for anything the cards produce.
- **Reuse:** `useThemeColors` tokens (`colors.get('status.done')` / `colors.alpha(...)`), the `.sr-*` class patterns, and add a new namespaced `.wfx-*` block to `src/styles.css` ported from the mockup `/Users/matt/.claude/jobs/8339450f/tmp/flow-designer.html`. No new `--c-*` tokens. Reuse the existing array-join/ternary className pattern (`EditableStepNode.tsx:170`, `WorkflowEditor.tsx:443`) for flag-conditional styling.
- **Tests:** vitest, `// @vitest-environment jsdom`; mock heavy children per house pattern (e.g. `ReactFlow`, `useThemeColors` where needed — see `WorkflowEditorGraph.test.tsx`). jsdom can assert **structural markers** (flag-on renders a restyled marker class / mono id; grid background variant; a trigger/inputs/autoflow control renders + its `onChange` emits the right YAML shape + `expectRoundTrip`) but CANNOT verify visual correctness. **matt must eyeball the restyle in a real browser** (flag on) before it's considered done — call this out in the PR. Run `npx vitest run` + `npx tsc -b` from `crates/rupu-cp/web`.

## File Structure

**Modify (restyle):** `src/components/workflow-editor/nodes/EditableStepNode.tsx` (node card), `WorkflowEditorGraph.tsx` (thread flag→node data; canvas `Background`; edges), `NodePalette.tsx` (dock), and `src/styles.css` (append `.wfx-*` block).
**Create (authoring logic):** `src/lib/workflowMeta.ts` (+ `.test.ts`) — typed read/write of trigger/inputs/autoflow over `meta.rest`.
**Modify (authoring UI):** `src/components/workflow-editor/WorkflowSettingsForm.tsx` — flag-gated trigger/inputs/autoflow editors (+ a lifecycle ribbon). Create small card components under `src/components/workflow-editor/settings/` (`TriggerCard.tsx`, `InputsCard.tsx`, `AutoflowCard.tsx`, `LifecycleRibbon.tsx`).

---

## Task 1: Thread the flag onto the node `data` (enabler)

**Files:** Modify `WorkflowEditorGraph.tsx` (node projection ~224-234), `nodes/EditableStepNode.tsx` (`NodeData` type ~22-25); Test `WorkflowEditorGraph.test.tsx`.

**Interfaces:** `NodeData` gains `workflowEditorUi?: WorkflowEditorUi` (default treated as `'classic'`). The node projection in `WorkflowEditorGraph` sets `data: { node, problems, workflowEditorUi }` and includes it in the `useMemo` deps.

- [ ] **Step 1: Failing test** — assert the projected node data carries `workflowEditorUi` (e.g. a small test that the node component, given `data.workflowEditorUi === 'next'`, renders a `data-ui="next"` marker; classic → `data-ui="classic"`).
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** — add `workflowEditorUi` to `NodeData`; in `WorkflowEditorGraph.tsx` node `.map` (the `workflowEditorUi` prop is already in scope at :221) set it on `data` + add to the memo deps; in `EditableStepNode` read `const ui = data.workflowEditorUi ?? 'classic'` and render a `data-ui={ui}` attribute on the outer div (used by later tasks + the test).
- [ ] **Step 4: Run → pass.** `npx vitest run src/components/workflow-editor` ; `npx tsc -b` clean.
- [ ] **Step 5: Commit.** `-m "feat(cp-web): thread workflow-editor-ui flag onto node data"`

---

## Task 2: Restyle the node card (behind flag)

**Files:** Modify `nodes/EditableStepNode.tsx`; Modify `src/styles.css` (start the `.wfx-*` block — node classes); Test `nodes/EditableStepNode.test.tsx` (create if absent, else extend).

**Interfaces:** consumes `data.workflowEditorUi` (Task 1). No prop-signature changes.

- [ ] **Step 1: Failing test** — with `data.workflowEditorUi === 'next'`, the node renders the restyled markers (e.g. a `.wfx-node` container + a `.wfx-kindpill` uppercase-mono kind label + a mono id); with `classic`, the current markup (`text-ui font-semibold` id, `kindChipStyle` chip) is unchanged.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** — in `EditableStepNode`, when `ui === 'next'` render the mockup card (from `flow-designer.html`: `.node` = 186px, radius 12, subtle shadow; top accent bar; `.kindpill` uppercase-mono; mono `.nid`; `.expr` mono chips; pill `.port`s) using new `.wfx-*` classes + `useThemeColors` per-kind accent (`KIND_KEY`). Restyle each body (`StepBody`/`ParallelBody`/`PanelBody`/`BranchBody`) to the mockup look under the flag; keep the existing markup for `classic`. Append the `.wfx-node*` / `.wfx-kindpill` / `.wfx-expr` / `.wfx-port` rules to `src/styles.css` (ported from the mockup, `rgb(var(--c-*))` tokens, dark-mode-ready, with a `@media (prefers-reduced-motion)` guard for any animation). Keep the two branch source handles + the problem dot working in both looks.
- [ ] **Step 4: Run → pass.** `npx vitest run src/components/workflow-editor` + `npx tsc -b` clean.
- [ ] **Step 5: Commit.** `-m "feat(cp-web): restyle workflow node cards behind flag"`

---

## Task 3: Restyle canvas + edges + palette (behind flag)

**Files:** Modify `WorkflowEditorGraph.tsx` (`Background`, edge styling), `NodePalette.tsx` (dock); Modify `src/styles.css` (`.wfx-*` canvas/palette classes); Test the two `*.test.tsx`.

- [ ] **Step 1: Failing tests** — flag `'next'`: `WorkflowEditorGraph` renders the grid background (`BackgroundVariant.Lines`/`Cross`, gap 28) instead of Dots (assert the variant passed to the mocked `Background`); the palette dock renders with the restyled `.wfx-palette` markers. Flag `classic`: Dots + current palette classes unchanged.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** — `WorkflowEditorGraph` (flag in scope at :221): when `next`, switch `<Background>` to the mockup grid (Lines/Cross, gap ~28, `colors.alpha('inkMute',…)`), restyle edges (thicker/marching-ants for the branch/active edges per the mockup's `.edge.flow`/`t-true`/`t-false`), and adjust the canvas container to the mockup backdrop (radial brand wash + grid). `NodePalette` (flag at :65): when `next`, render the restyled dock/cards (mockup `.pcard`/`.pdot`); also switch its hardcoded hex `KIND_COLOR` to `useThemeColors` tokens so it matches the node accents. Keep classic markup untouched.
- [ ] **Step 4: Run → pass.** Full `npx vitest run`; `npx tsc -b` clean.
- [ ] **Step 5: Commit.** `-m "feat(cp-web): restyle workflow canvas, edges, and palette behind flag"`

---

## Task 4: `lib/workflowMeta.ts` — typed read/write of trigger/inputs/autoflow

**Files:** Create `src/lib/workflowMeta.ts` (+ `.test.ts`).

**Interfaces:** Produces pure functions (the authoring UI's data layer):
- `readTrigger(rest) → TriggerModel`, `writeTrigger(rest, TriggerModel) → rest'`
- `readInputs(rest) → InputModel[]` (`{ name, type, required, default?, enumValues[], description? }`), `writeInputs(rest, InputModel[]) → rest'` (emits the `BTreeMap`-shaped object keyed by name; `type`/`enum` keys).
- `readAutoflow(rest) → AutoflowModel | null`, `writeAutoflow(rest, AutoflowModel|null) → rest'`.
- `contractOutputKeys(rest) → string[]` (keys of `rest.contracts.outputs`, for the outcome `<select>`).
Each `write*` returns a NEW `rest` preserving all other keys; deleting a block (empty trigger back to manual / autoflow disabled-and-cleared) omits the key rather than emitting an empty object.

- [ ] **Step 1: Failing tests** — round-trip each: `writeTrigger(rest, readTrigger(rest))` is stable; a trigger set to `event` emits `{ on: event, event, filter? }` and NOT `cron`; inputs round-trip preserves the map shape + emits `type:`/`enum:`; autoflow round-trips incl selector/claim/workspace/outcome; `writeInputs`/`writeAutoflow` preserve an unrelated `rest.contracts` key; `contractOutputKeys` returns the contract names.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** the pure functions per the exact schema in Global Constraints (emit only valid keys/enums; inputs as a name-keyed map; omit-when-empty).
- [ ] **Step 4: Run → pass.** `npx vitest run src/lib/workflowMeta.test.ts` + full; `npx tsc -b` clean.
- [ ] **Step 5: Commit.** `-m "feat(cp-web): typed read/write for workflow trigger/inputs/autoflow meta"`

---

## Task 5: Trigger + Inputs authoring cards (behind flag)

**Files:** Create `src/components/workflow-editor/settings/{TriggerCard,InputsCard}.tsx`; Modify `WorkflowSettingsForm.tsx` (render them when flag `next`); thread `workflowEditorUi` into `WorkflowSettingsForm` from `WorkflowEditor.tsx`; Modify `styles.css` (`.wfx-*` settings-card classes); Test `WorkflowSettingsForm.test.tsx` (greenfield, model on `StepForm.test.tsx`).

**Interfaces:** `WorkflowSettingsForm` gains `workflowEditorUi?: WorkflowEditorUi`. `TriggerCard`/`InputsCard`: `({ rest, onRest }: { rest; onRest: (rest') => void })` using `lib/workflowMeta` read/write.

- [ ] **Step 1: Failing tests** — flag `next`: the Settings form renders a Trigger card (on = manual/cron/event segmented; choosing `event` shows event+filter, hides cron) and an Inputs card (add an input → name/type/required/default/enum/description → `onChange` emits `meta.rest.inputs` as a name-keyed map with `type:`). Assert the emitted meta via `onChange`. Flag `classic`: the current read-only rest-chips form renders unchanged.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** the two cards (segmented controls / chips / add-remove rows styled with `.wfx-*` + the reusable controls, mirroring the mockup's `.card-sec`/`.seg`/`.chip`), each reading/writing via `lib/workflowMeta` and calling `patch({ rest })`. Gate them on `workflowEditorUi === 'next'` in `WorkflowSettingsForm`; keep the classic chips list for `classic`. Thread the flag from `WorkflowEditor.tsx` into `WorkflowSettingsForm`.
- [ ] **Step 4: Run → pass.** Full `npx vitest run`; `npx tsc -b` clean.
- [ ] **Step 5: Commit.** `-m "feat(cp-web): trigger + inputs authoring cards behind flag"`

---

## Task 6: Autoflow authoring cards + lifecycle ribbon (behind flag)

**Files:** Create `src/components/workflow-editor/settings/{AutoflowCard,LifecycleRibbon}.tsx`; Modify `WorkflowSettingsForm.tsx` (render when `next`); Test `WorkflowSettingsForm.test.tsx`.

**Interfaces:** `AutoflowCard`: `({ rest, onRest })` — an enable toggle + (when enabled) the sections: entity, selector (states/labels_all/limit; draft/base ONLY when entity=pull_request), author gate (authors_from/on_skip), reconcile (reconcile_every/claim.ttl/wake_on), workspace (strategy/branch), outcome (output = `<select>` of `contractOutputKeys(rest)`, disabled when empty). `LifecycleRibbon`: read-only viz of the autoflow (selector → author gate → claim → run → reconcile → outcome), mockup `.lc-*`.

- [ ] **Step 1: Failing tests** — flag `next`: toggling autoflow on emits `meta.rest.autoflow = { enabled: true }`; setting entity=pull_request reveals draft/base; setting reconcile_every=`10m` + claim.ttl=`3h` emits valid durations; the outcome `<select>` lists `contractOutputKeys`; the whole autoflow block round-trips via `expectRoundTrip` on the workflow object. Author-gate `authors_from` + `on_skip` emit the right enums.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** `AutoflowCard` (sections styled `.wfx-*` per the mockup autoflow cards) using `lib/workflowMeta.readAutoflow/writeAutoflow`; gate draft/base on entity; wire outcome to `contractOutputKeys`. Add the read-only `LifecycleRibbon`. Render both in `WorkflowSettingsForm` when `next`.
- [ ] **Step 4: Run → pass.** Full `npx vitest run`; `npx tsc -b` clean.
- [ ] **Step 5: Commit.** `-m "feat(cp-web): autoflow authoring cards + lifecycle ribbon behind flag"`

---

## Task 7: Whole-branch verification + PR

- [ ] **Step 1:** `npm run build` + `npx vitest run` (full suite) + `npx tsc -b` clean, from `crates/rupu-cp/web`.
- [ ] **Step 2:** Sanity: open a real workflow YAML with a trigger/inputs/autoflow block, load it through `yamlToGraph`/`graphToWorkflowObject` in a test, confirm round-trip; confirm `api.validateWorkflow` shape by asserting the emitted objects parse (a fixture test).
- [ ] **Step 3:** Dispatch the final whole-branch reviewer; fix Critical/Important in one pass.
- [ ] **Step 4:** Open a draft PR: summarize the restyle + the trigger/inputs/autoflow authoring, that it's frontend-only behind `[cp].workflow_editor_ui` (default classic), and **explicitly request an in-browser visual check with the flag on** (jsdom can't verify the restyle).

## Verification (end-to-end)

1. **Flag off (default):** the workflow editor + Settings tab are visually and behaviorally identical to today (proven by classic-path tests + a visual check).
2. **Flag on** (`localStorage['rupu.cp.workflowEditorUi']='next'` or `[cp] workflow_editor_ui="next"`): the editor renders the instrument look (restyled kind-colored node cards, grid canvas, restyled edges + palette); the Settings tab shows Trigger / Inputs / Autoflow editors (no more "edit these in the YAML tab" chips); editing them updates the YAML, and `validateWorkflow` stays green; existing `trigger`/`inputs`/`autoflow`/`contracts`/`concerns` keys are preserved on save.
3. **Round-trip:** a workflow with a full autoflow block opened and re-saved without edits is unchanged (key order + sibling keys preserved).

## Out of scope (later passes)
- Run Room (pass for the live single-run view).
- Co-authoring `contracts:` (outcome uses existing contract keys only); step-level `contract:`/`host:`/`distribute:` visual authoring (still `raw_passthrough`); the broader engine primitives (switch/loop/model-auto/try-catch/sub-workflow/wait).
- Any Rust change (none needed — schema + validation already exist).
