# Flow Designer 3.1 — if/branch conditional routing (engine + designer) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

## Context

rupu workflows execute as a **strict linear walk** of `workflow.steps` (`crates/rupu-orchestrator/src/runner.rs:934` — `for step in &opts.workflow.steps`). There is no DAG, no jumps: the only run/skip decision today is a per-step `when:` gate, and the graph in the CP editor is a *view*, not the execution model. So there is no way to author real **conditional routing** — "if X, run arm A; else run arm B; then rejoin" — except by hand-writing mutually-exclusive `when:` on every step. This is pass 2 of the CP redesign (Agent Builder → **Flow Designer** → Run Room); matt chose the **if/branch engine** slice: add true conditional routing + reconvergence to the orchestrator AND its designer blocks, end-to-end, behind a UI flag.

**The key enabling insight:** `when:`-skipped steps are already **first-class** — they're persisted (`StepResult{skipped:true, output:"", success:false}`, `runner.rs:991-1003`), stay in the template context (`steps.<id>.output` renders as `""`, not undefined — `runner.rs:1237-1253`), and reconvergence therefore *already works* as long as steps keep declaration order (forward-refs are illegal — `validate_template_refs`, `workflow.rs:1191`). So branching is a **low-risk generalization of the existing skip machinery**, not a runner rewrite.

**Goal:** A `branch` step kind — `{ condition, then: [step ids], else: [step ids] }` — that, when reached, evaluates `condition` and marks the *not-taken* arm's steps to be skipped as the linear loop passes them; plus the visual designer (branch node with condition + true/false target edges) behind a `[cp].workflow_editor_ui` flag defaulting to the current editor.

**Architecture:** Additive-only. A new `Branch` struct + `Step.branch` field (existing workflows omit it → unaffected by `#[serde(deny_unknown_fields)]`). The runner gains a run-scoped `branch_skipped: BTreeSet<String>`; the branch arm computes which arm to skip; a check at the top of the loop skips any step in that set (before its own `when:`/approval/dispatch). Not-taken steps reuse the existing `StepSkipped` event with a distinct `reason`. The web editor models the branch kind through the existing `workflowGraph.ts` extension points; the palette card + branch inspector are gated on the flag (existing branch steps always render, for correctness).

**Tech Stack:** Rust (rupu-orchestrator, rupu-config), React 19 + TS + Tailwind + Vitest (crates/rupu-cp/web), @xyflow, minijinja, js-yaml.

## Global Constraints

- **Additive & non-breaking:** `Step` is `#[serde(deny_unknown_fields)]` (`workflow.rs:662`) — the `branch` field must be added to `Step`; existing workflows (no `branch:`) must still parse and run identically. No change to the linear execution order of non-branch workflows.
- **Branch YAML shape (exact):**
  ```yaml
  - id: <branch-step-id>
    branch:
      condition: "<minijinja expr; truthy → then-arm runs, else-arm skipped>"
      then: [<step ids in the then-arm>]   # default []
      else: [<step ids in the else-arm>]   # default []
  ```
  `Branch` is its own `#[serde(deny_unknown_fields)]` struct (mirror `Panel` `workflow.rs:535`).
- **Branch semantics:** on reaching the branch step, render `condition` via the existing `render_when_expression` (`templates.rs:432`, truthy rules identical to `when:`). Truthy → add all `else` ids to the run's `branch_skipped` set; falsy → add all `then` ids. The branch step itself produces `StepResult{ kind: Branch, output: "then"|"else" (taken arm), success: true, skipped: false }` and emits `StepStarted{kind:branch}` + `StepCompleted{success:true}`. A step whose id is in `branch_skipped` is skipped when the loop reaches it — **checked before its own `when:`/approval/dispatch** — emitting `StepSkipped{ reason: "not taken by branch <branch-id>" }` and persisting `StepResult{ skipped:true, output:"", success:false }` (same as a `when:`-skip). Reconvergence: steps in neither arm run normally; they read `steps.<arm>.output` = `""` for the not-taken arm.
- **Arm lists are the COMPLETE (flattened) membership of each arm**, including any nested branch's steps — so skipping an arm skips everything in it (handles nested branches correctly). The designer computes this from graph reachability; hand-authors list every id.
- **Validation (parse-time):** shape mutual-exclusion (branch excludes agent/prompt/for_each/parallel/panel); `condition` non-empty; every `then`/`else` id exists; `then ∩ else = ∅`; no arm id equals the branch's own id; each arm id appears **after** the branch step in declaration order (routing is forward). The `condition` is template-linted automatically (backward refs / unknown fields) by adding it to `collect_templates_for_step` (`workflow.rs:1237`).
- **Feature flag defaults to classic:** `[cp].workflow_editor_ui = "classic" | "next"` (default `classic`); per-browser `localStorage['rupu.cp.workflowEditorUi']` override; resolver falls back to classic. The flag gates only **authoring** (the branch palette card + the branch option in the kind picker); rendering an existing branch step in the editor is always on (correctness).
- **Reuse, don't reinvent:** `render_when_expression` (`templates.rs:432`) for the condition; the `validate_contracts` id-lookup pattern (`workflow.rs:1129`) for `validate_branch_targets`; the `useAgentAuthoringUi` hook + `CpConfig.agent_authoring_ui` pattern for the flag; the `workflowGraph.ts` `raw_passthrough` + `MODELLED_STEP_KEYS` machinery for the node.
- **Rust:** workspace deps only; `#![deny(clippy::all)]`; `thiserror` for lib errors. Do NOT run `cargo fmt` package-wide — only `cargo fmt -- <file>` on touched files.
- **Tests:** Rust `cargo test -p rupu-orchestrator` (+ `-p rupu-config`); web `npm test`/`npx vitest run` from `crates/rupu-cp/web`. Existing suites must stay green.
- **Contract-on-branch behavior (documented, not blocked):** if a `contracts.outputs.*.from_step` sits on a not-taken arm, it resolves to an empty output at runtime — identical to today's behavior for a `when:`-skipped emitting step. No new validation added for it in this pass.

## File Structure

**Rust — engine (`crates/rupu-orchestrator/src/`):**
- `workflow.rs` — `Branch` struct + `Step.branch` field; `validate_step_shape` arm; new `WorkflowParseError` variants; `collect_templates_for_step` (push `branch.condition`); new `validate_branch_targets` wired into `Workflow::parse`.
- `runs.rs` — `StepKind::Branch` variant.
- `runner.rs` — `branch_skipped` set; branch-skip check at loop top; branch evaluation + `StepResult`/events; `step_kind_for_run_record` arm.
- `tests/workflow_parse.rs` — parse/validation tests; a runner integration test (mock provider) proving arm-skip + reconvergence.

**Rust — flag:** `crates/rupu-config/src/policy_config.rs` — `CpConfig.workflow_editor_ui`.

**Web (`crates/rupu-cp/web/src/`):**
- `hooks/useWorkflowEditorUi.ts` — flag hook (mirror `useAgentAuthoringUi.ts`).
- `lib/workflowGraph.ts` — `'branch'` kind, `StepNodeData` fields, `MODELLED_STEP_KEYS`, `parseStepData`, edge pass (labeled edges), `nodeToStepObject`, `GraphEdge` label, `validateGraph`.
- `components/workflow-editor/` — `WorkflowEditorGraph.tsx` (`asStepKind`, `newNodeData`, edge label, 2nd source handle), `NodePalette.tsx` (`KIND_COLOR`+`ITEMS`, gated), `nodes/EditableStepNode.tsx` (`KIND_KEY`+`BranchBody`+handle), `StepForm.tsx` (`KIND_LABELS`+`BranchFields`).

---

## Task 1: Config flag `[cp].workflow_editor_ui`

**Files:** Modify `crates/rupu-config/src/policy_config.rs` (`CpConfig`); Test in the crate's existing test location (mirror `agent_authoring_ui`'s tests).

**Interfaces:** Produces `CpConfig.workflow_editor_ui: String` (default `"classic"`), auto-surfaced via `/api/config` `cp`.

- [ ] **Step 1:** Read how `agent_authoring_ui` was added (field + `#[serde(default="...")]` + `default_workflow_editor_ui()` fn + the manual `impl Default` line + tests) — mirror it exactly.
- [ ] **Step 2: Failing tests** (mirror the agent-authoring-ui tests): empty `[cp]` → `workflow_editor_ui == "classic"`; `workflow_editor_ui = "next"` parses.
- [ ] **Step 3: Run → fail.** `cargo test -p rupu-config`.
- [ ] **Step 4: Add the field + default fn + `impl Default` line.**
- [ ] **Step 5: Run → pass.** `cargo test -p rupu-config`.
- [ ] **Step 6: Commit.** `git commit -m "feat(config): add [cp].workflow_editor_ui flag (default classic)"`

---

## Task 2: `Branch` struct + `Step.branch` field + `StepKind::Branch`

**Files:** Modify `workflow.rs` (Step struct ~730, add `Branch`); `runs.rs` (`StepKind`); Test `tests/workflow_parse.rs`.

**Interfaces:** Produces:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    pub condition: String,
    #[serde(default)] pub then: Vec<String>,
    #[serde(default)] pub r#else: Vec<String>,   // YAML key `else` (reserved word → raw ident)
}
```
`Step.branch: Option<Branch>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`); `StepKind::Branch` (`runs.rs`, `#[serde(rename_all="snake_case")]` → `"branch"`).

- [ ] **Step 1: Failing test** (parse round-trip): a workflow with a branch step parses and `step.branch` is `Some` with the expected condition/then/else. NOTE: this test will FAIL validation until Task 3 adds the shape arm — so for THIS task assert only that serde deserializes the `Branch` struct in isolation (`serde_yaml::from_str::<Branch>(...)`), not the full `Workflow::parse`.
```rust
#[test]
fn branch_struct_parses() {
    let b: Branch = serde_yaml::from_str("condition: \"{{ steps.a.output }}\"\nthen: [x, y]\nelse: [z]\n").unwrap();
    assert_eq!(b.condition, "{{ steps.a.output }}");
    assert_eq!(b.then, vec!["x", "y"]);
    assert_eq!(b.r#else, vec!["z"]);
}
```
- [ ] **Step 2: Run → fail** (`Branch` undefined). `cargo test -p rupu-orchestrator branch_struct_parses`.
- [ ] **Step 3: Add `Branch` struct + `Step.branch` field + `StepKind::Branch`.** (Add the field near `panel`, mirror its serde attrs. Add the enum variant.)
- [ ] **Step 4: Run → pass.** Also `cargo build -p rupu-orchestrator` clean (the `if/else` dispatch chain and `validate_step_shape` still compile — an unhandled `branch` shape currently falls through to the linear arm; Task 3 fixes validation and Task 5 the dispatch).
- [ ] **Step 5: Commit.** `-m "feat(orchestrator): Branch struct + Step.branch field + StepKind::Branch"`

---

## Task 3: `validate_step_shape` branch arm + error variants

**Files:** Modify `workflow.rs` (`validate_step_shape` ~972, `WorkflowParseError` enum ~40-160); Test `tests/workflow_parse.rs`.

**Interfaces:** New `WorkflowParseError` variants: `BranchMutuallyExclusive { step }`, `BranchEmptyCondition { step }`, `BranchArmsOverlap { step, id }`, `BranchTargetsSelf { step }`. (Target-existence/forward errors are Task 4.)

- [ ] **Step 1: Failing tests** (mirror `rejects_parallel_combined_with_for_each_or_agent`): a branch step with `agent`/`prompt`/`for_each`/`parallel`/`panel` also set → error contains "mutually exclusive"; empty `condition` → error; a step id in both `then` and `else` → error; an arm listing the branch's own id → error. A valid branch step (condition + disjoint arms, no agent) passes `validate_step_shape`.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** — add `else if let Some(branch) = &step.branch { ... }` as the FIRST arm of the chain in `validate_step_shape` (before the panel/parallel/linear arms, since a branch has no agent/prompt and must not hit the linear fallthrough). Reject co-set agent/prompt/for_each/parallel/panel; reject empty `condition`; reject `then ∩ else != ∅`; reject an arm containing `step.id`. Add the error variants (mirror `PanelMutuallyExclusive`/`PanelEmpty`).
- [ ] **Step 4: Run → pass.** Full `cargo test -p rupu-orchestrator` (no regressions).
- [ ] **Step 5: Commit.** `-m "feat(orchestrator): validate branch step shape"`

---

## Task 4: Branch condition template-lint + `validate_branch_targets`

**Files:** Modify `workflow.rs` (`collect_templates_for_step` ~1237, new `validate_branch_targets`, wire into `Workflow::parse` ~850, error variants); Test `tests/workflow_parse.rs`.

**Interfaces:** New `WorkflowParseError`: `BranchTargetUnknown { step, target }`, `BranchTargetNotForward { step, target }`. New `fn validate_branch_targets(wf: &Workflow) -> Result<(), WorkflowParseError>`.

- [ ] **Step 1: Failing tests:**
  - Condition lint (mirror `lint_validates_when_template`): a branch `condition` referencing `steps.a.<typo-field>` → the same unknown-field lint fires; referencing a forward step → forward-ref error.
  - Targets: a `then`/`else` id that doesn't exist → `BranchTargetUnknown`; a target that appears *before* the branch step in declaration order → `BranchTargetNotForward`; a well-formed branch (targets exist and are declared after) → `Workflow::parse` Ok.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** — (a) in `collect_templates_for_step`, `if let Some(b) = &step.branch { out.push(("branch.condition", b.condition.clone())); }` (this alone makes the condition get the existing forward-ref/unknown-field lint). (b) Write `validate_branch_targets` modeled on `validate_contracts` (`workflow.rs:1129`): for each branch step, for each id in `then`/`else`, require it exists (`wf.steps.iter().any(|s| s.id == id)`) and its index > the branch step's index. Call it in `Workflow::parse` after `validate_template_refs`.
- [ ] **Step 4: Run → pass.** Full `cargo test -p rupu-orchestrator`.
- [ ] **Step 5: Commit.** `-m "feat(orchestrator): lint branch condition + validate branch targets"`

---

## Task 5: Runner — branch evaluation, skip-set, arm-skip (the crux)

**Files:** Modify `runner.rs` (`run_steps_inner` ~914, `step_kind_for_run_record` ~1286); Test `tests/workflow_parse.rs` or a runner test module (use the mock-provider harness — find how existing runner tests construct a run, e.g. `MockProvider`/`BypassDecider` in `rupu-agent`'s runner tests or orchestrator integration tests).

**Interfaces:** No new public API. Internal: a `branch_skipped: BTreeSet<String>` local in `run_steps_inner`.

- [ ] **Step 1: Read the loop** `runner.rs:914-1206` — the resume-skip check (`936`), the `when:` block (`973-1006`), the dispatch chain (`1086`), `persist_step_result` (`1208`), `base_context_for_step` (`1227`). Confirm where to insert the branch-skip check and the branch arm.
- [ ] **Step 2: Failing runner test** — a workflow: `classify` (mock agent) → `route` (branch, condition true) → `arm_a` (in `then`) → `arm_b` (in `else`) → `join` (reads `steps.arm_a.output` + `steps.arm_b.output`). Assert after run: `arm_b` result has `skipped==true, output==""` and reason contains "not taken by branch"; `arm_a` ran (`skipped==false`); `join` ran and its rendered prompt saw `arm_b`'s output as empty. Add a second case with condition false (arm_a skipped, arm_b runs).
- [ ] **Step 3: Run → fail.**
- [ ] **Step 4: Implement:**
  - Declare `let mut branch_skipped: BTreeSet<String> = BTreeSet::new();` before the loop.
  - **Branch-skip check** — at the loop top, right after the resume-skip `continue` (`~936`) and BEFORE the `when:` block: `if branch_skipped.contains(&step.id) { emit StepSkipped{ reason: format!("not taken by branch") }; persist StepResult{ skipped:true, success:false, output:"", kind: step_kind_for_run_record(step) }; step_results.push(...); continue; }`. (Reuse the exact skip-persist code shape at `runner.rs:991-1003`.)
  - **Branch arm** — handle `step.branch.is_some()` as a dedicated block (NOT via the agent dispatch chain, since it mutates loop state): emit `StepStarted{kind:Branch}`; render `branch.condition` via `render_when_expression`; `let taken = if truthy { "then" } else { "else" };` add the *other* arm's ids to `branch_skipped`; build `StepResult{ step_id, output: taken.into(), success:true, skipped:false, kind: StepKind::Branch, ..default }`; emit `StepCompleted{success:true}`; persist + push; `continue`. (Place this block before the existing `agent/prompt` dispatch chain.)
  - Add a `StepKind::Branch` arm to `step_kind_for_run_record` (`runner.rs:1286`).
- [ ] **Step 5: Run → pass.** Full `cargo test -p rupu-orchestrator`.
- [ ] **Step 6: Commit.** `-m "feat(orchestrator): execute branch steps — skip the not-taken arm"`

---

## Task 6: `useWorkflowEditorUi` flag hook (web)

**Files:** Create `crates/rupu-cp/web/src/hooks/useWorkflowEditorUi.ts` (+ `.test.ts`). Mirror `useAgentAuthoringUi.ts` exactly.

**Interfaces:** `useWorkflowEditorUi(): 'classic' | 'next'`; `resolveWorkflowEditorUi(cp, override)` (pure). Storage key `rupu.cp.workflowEditorUi`; config key `cp.workflow_editor_ui`.

- [ ] **Step 1: Failing test** — copy the three `resolveAgentUi` cases (override wins / config fallback / default classic) for `resolveWorkflowEditorUi`.
- [ ] **Step 2: Run → fail.** **Step 3:** Implement by mirroring `useAgentAuthoringUi.ts` (change the two key strings). **Step 4:** Run → pass. **Step 5: Commit** `-m "feat(cp-web): useWorkflowEditorUi flag hook"`.

---

## Task 7: `workflowGraph.ts` — model the branch kind

**Files:** Modify `crates/rupu-cp/web/src/lib/workflowGraph.ts`; Test `src/lib/workflowGraph.test.ts`.

**Interfaces:** `StepKind` gains `'branch'`; `StepNodeData` gains `condition?: string; thenTargets?: string[]; elseTargets?: string[]`; `GraphEdge` gains `label?: string; branch?: 'then'|'else'`.

- [ ] **Step 1: Failing tests** (mirror `expectRoundTrip` + edge assertions): a workflow with a branch step round-trips (`graphToWorkflowObject(yamlToGraph(x))` deep-equals `x`), `kind==='branch'`, condition/then/else parsed; `yamlToGraph` emits labeled edges `branchId→thenTarget` (label 'true'/'then') and `branchId→elseTarget` (label 'false'/'else'); `raw_passthrough` of an unrelated key still preserved.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** the seven edit points from research: add `'branch'` to `StepKind`; add fields to `StepNodeData`; add `branch` to `MODELLED_STEP_KEYS`; detect the shape in `parseStepData` (set `kind`, parse `branch.condition`/`then`/`else`); in the `yamlToGraph` edge pass emit labeled then/else edges (extend `GraphEdge` + `addEdge` to carry `label`/`branch`, and include the label in the dedupe key so two edges to overlapping targets don't collapse); add a `d.kind==='branch'` arm to `nodeToStepObject` (emit `branch: { condition, then, else }`); add a `branch` arm to `validateGraph` (condition required, ≥1 target, targets must be known node ids); include then/else in `extractStepRefs` if you want them dependency-tracked.
- [ ] **Step 4: Run → pass.** `npx vitest run src/lib/workflowGraph.test.ts` then full `npx vitest run`; `npx tsc -b` clean.
- [ ] **Step 5: Commit.** `-m "feat(cp-web): model branch step kind in workflowGraph"`

---

## Task 8: Editor components — branch node, palette, inspector (behind flag)

**Files:** Modify `WorkflowEditorGraph.tsx`, `NodePalette.tsx`, `nodes/EditableStepNode.tsx`, `StepForm.tsx` (+ `WorkflowEditor.tsx`/`WorkflowDetail.tsx` to thread the flag); Tests: the respective `*.test.tsx`.

**Interfaces:** consumes `useWorkflowEditorUi()`.

- [ ] **Step 1: Failing tests** — `WorkflowEditorGraph.test.tsx`: `asStepKind('branch')==='branch'`; `newNodeData('branch')` seeds `{condition:'', thenTargets:[], elseTargets:[]}`; a branch `GraphEdge` with a label renders (assert the label passes into the xyflow edge). `StepForm.test.tsx`: selecting a branch node shows the condition field + then/else pickers and edits flow to node data. `NodePalette.test.tsx`: with flag `'next'` the branch palette card renders; with flag classic it does NOT.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** the research-mapped edits: `WorkflowEditorGraph` — `asStepKind` add `'branch'`, `newNodeData` branch defaults, edge render `label`/`labelStyle` (green "true"/red "false"), a 2nd source `Handle` id `then`/`else` on the branch node, handle-aware connect. `NodePalette` — `KIND_COLOR.branch` + an `ITEMS` entry `{kind:'branch', label:'branch', sub:'if / then / else'}`, rendered only when `useWorkflowEditorUi()==='next'`. `EditableStepNode` — `KIND_KEY.branch` (a status/sev color), a `BranchBody` (condition + then/else summary) in the body switch, the 2nd source handle. `StepForm` — `KIND_LABELS.branch='Branch (if)'` (the kind option only offered when flag `'next'`) + a `BranchFields` component (condition `ExpressionField` + then/else multi-select of node ids). Thread the flag from `WorkflowDetail`/`WorkflowEditor` down to palette + StepForm. **Rendering an existing branch node is always on; only the palette card + the "branch" kind option are flag-gated.**
- [ ] **Step 4: Run → pass.** Full `npx vitest run`; `npx tsc -b` clean.
- [ ] **Step 5: Commit.** `-m "feat(cp-web): branch node designer (palette/node/inspector) behind flag"`

---

## Task 9: Whole-branch verification + PR

- [ ] **Step 1:** `cargo test -p rupu-orchestrator` + `cargo test -p rupu-config` green.
- [ ] **Step 2:** `npm run build` + `npx vitest run` (from `crates/rupu-cp/web`) green.
- [ ] **Step 3:** Dispatch the final whole-branch reviewer; fix Critical/Important in one pass.
- [ ] **Step 4:** Open a draft PR summarizing the branch semantics, the additive-safety, and the flag (defaults classic — a no-op for existing workflows/editor until flipped).

## Verification (end-to-end)

1. **Engine, flag-independent:** author a workflow YAML with a `branch` step (condition + then/else) and run it via the mock harness / `rupu run` — confirm the not-taken arm's steps are `skipped` with reason "not taken by branch", the taken arm runs, and a join step reading `steps.<arm>.output` sees `""` for the skipped arm. Existing (non-branch) workflows run byte-identically.
2. **Parse validation:** malformed branches (unknown target, backward target, overlapping arms, condition referencing a forward/typo step) are rejected by `Workflow::parse` with the specific errors (proven by `tests/workflow_parse.rs`).
3. **Designer, flag off (default):** `/workflows/:name` editor is unchanged; no branch palette card; a workflow that already contains a branch step still renders its branch node + labeled edges correctly (round-trip preserved).
4. **Designer, flag on:** `localStorage['rupu.cp.workflowEditorUi']='next'` (or `[cp] workflow_editor_ui = "next"`) → branch palette card appears; drag a branch node, set its condition + then/else targets, see true/false edges; the emitted YAML has a valid `branch:` block that `validateWorkflow` accepts.

## Out of scope (later passes)
- Other engine primitives (switch/case, loops, try/catch, model-auto, sub-workflow, wait) and the broader "unlock the unauthorable" Flow Designer surface (trigger/inputs/autoflow authoring, full canvas restyle).
- A dedicated runtime event distinguishing "branch-not-taken" from "when-skipped" in the CP **run** graph (Run Room, pass 3) — this pass reuses `StepSkipped` with a reason string.
- Contract-reachability validation for a contract output whose emitting step is on a conditional arm (documented as resolving empty, same as today's `when:`-skip).
