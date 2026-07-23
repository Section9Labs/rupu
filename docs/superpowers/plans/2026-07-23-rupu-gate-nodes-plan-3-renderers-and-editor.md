# Gate Nodes & Action Steps — Plan 3: Renderers & Editor

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** PR ③ of the arc (`docs/superpowers/specs/2026-07-23-rupu-workflow-gate-and-action-nodes-design.md` §5) — approval gate nodes and action steps become *visible* in every renderer (app-canvas git-graph, CP run viewer) and *authorable* in the visual workflow editor, including connector cards generated from the live MCP tool catalog and a synthesized gate node for legacy inline approvals.

**Architecture:** There are three independent "kind" enumerations, none sharing a source — extend all three. (a) Rust `StepKind` + the shape-based mappers in app-canvas `render_rows` and CP `map_step` (both currently collapse `action`/gate into linear). (b) The run-viewer DTO/TS model. (c) The editor's TS `StepKind` union + its ~7 maps. New cards live in the `next` editor variant only (classic stays byte-stable). A new `GET /api/tools` endpoint serves `tool_catalog()` so connector cards are catalog-driven.

**Tech Stack:** Rust (rupu-app-canvas insta snapshots, rupu-cp Axum + serde), TypeScript/React (React Flow, vitest), existing `useWorkflowEditorUi` classic/next flag.

## Global Constraints

- Workspace deps only; `#![deny(clippy::all)]`; thiserror (lib) / anyhow-style in CP handlers matching the crate's existing error pattern.
- **Classic editor + classic run-viewer output must stay byte-stable** — new cards/nodes are `next`-gated exactly like the existing `BRANCH_ITEM` (`NodePalette.tsx:76`: `ui === 'next' ? [...ITEMS, BRANCH_ITEM] : ITEMS`). A gate/action run *node*, however, must render in BOTH variants of the run viewer (a run's shape isn't a UI preference) — only the *editor palette cards* are next-gated.
- No new `StepKind` Rust variants (Action/ApprovalGate exist since Plan 1). No new executor `Event` variants.
- Never package-wide cargo fmt (per-file only). Web: `npm run test`, `tsc -b`, `npm run build` must all pass; theme-aware (light+dark) per existing token usage.
- **GUI rendering can't be validated by subagents** (CLAUDE.md rupu-app rule 4 — though that's the GPUI app; the CP web UI is browser-validatable). Web component tasks require a render/interaction test AND are flagged for matt's visual check before the PR merges.
- Baseline: 4 `linear_runner.rs` flakes + rupu-cli ANSI/session redness + rupu-mcp `schema_snapshot` drift are all pre-existing; don't chase them.
- Line refs are v0.65.2 main (`567c94e3`); re-locate by quoted code if drifted.

---

### Task 1: app-canvas — gate & action graph rows

**Files:**
- Modify: `crates/rupu-app-canvas/src/git_graph.rs` (branch chain `:110-122`; add `emit_gate_step`/`emit_action_step` alongside `emit_linear_step` `:252`)
- Test: `crates/rupu-app-canvas/tests/git_graph_snapshots.rs` (+ new `.snap` files under `tests/snapshots/`)

**Interfaces:**
- Consumes: `rupu_orchestrator::workflow::{is_approval_gate, Step}`, `GraphRow`/`GraphCell`/`NodeStatus`. (Check app-canvas already deps rupu-orchestrator — it does, `render_rows` takes `&Workflow`.)
- Produces: two new emit fns; the branch chain distinguishes, IN THIS ORDER (most-specific first): `is_approval_gate(step)` → gate; `step.action.is_some()` → action; `step.branch.is_some()` → (branch currently falls to linear — leave branch as-is unless trivially addable, NOT this plan's scope); else existing panel/parallel/for_each/linear.

- [ ] **Step 1: Write failing snapshot tests**

In `git_graph_snapshots.rs`, model on `snapshot_panel_with_3_panelists` (`:41`). Two tests: a workflow with a standalone `approval:` gate step (with a prompt), and one with an `action: scm.prs.create` step. Use `insta::assert_yaml_snapshot!(render_rows(&wf, &lookup))`. Build the `Workflow` via `Workflow::parse` on inline YAML (gate: `approval:` + prompt, no agent; action: `action:` + `with:`). Run with `cargo insta test` OR `INSTA_UPDATE=no cargo test` (pending snapshots fail).

- [ ] **Step 2: Run to confirm FAIL** — `cargo test -p rupu-app-canvas 2>&1 | tail -5`. Expect: gate/action steps currently render as a linear bullet with `step.agent` empty — the snapshot either doesn't exist (fails pending) or shows the wrong (linear) shape.

- [ ] **Step 3: Implement**

Clone `emit_linear_step` (single row: `Bullet` + `Label` + `Meta`). For the gate:

```rust
/// A standalone approval gate node — a diamond-ish bullet, the gate id as
/// label, and a meta tag showing it's a human gate (+ auto-approve hint if set).
fn emit_gate_step(rows: &mut Vec<GraphRow>, step: &Step, status: NodeStatus, /* + the same params emit_linear_step takes — match its signature exactly */) {
    // Bullet cell (reuse the linear glyph choice), Label = step.id,
    // Meta = "gate" (+ " · auto" when step.approval.as_ref().and_then(|a| a.auto_approve.as_ref()).is_some()).
    // anchor = Some((step.id, status)) exactly like emit_linear_step.
}
```

Action mirror: `Meta = format!("action · {}", step.action.as_deref().unwrap_or("?"))`. Read `emit_linear_step`'s real signature and the `GraphCell::Meta` construction in the file — match them (do not invent cell variants). Wire both into the `:110-122` chain before the linear `else`.

- [ ] **Step 4: Accept snapshots + full suite** — review the `.snap` diffs (gate row shows a gate meta, action row shows the tool), `cargo insta accept` (or move `.snap.new`→`.snap`), then `cargo test -p rupu-app-canvas` green. Commit the `.snap` files.
- [ ] **Step 5: Commit** — `feat(app-canvas): render approval gate + action step graph rows`

---

### Task 2: CP run-viewer — DTO + TS model + state styling

**Files:**
- Modify: `crates/rupu-cp/src/api/graph.rs` (`StepNodeDto` `:265`/`:270`; `map_step` `:352`; the ~3 literal `StepNodeDto` construction sites — `agent_run_dag` `:310`, `unpersisted_run_dag` `:330`, and any inside `map_step`), `crates/rupu-cp/web/src/lib/api.ts` (`StepNodeDto` `:342`), `crates/rupu-cp/web/src/lib/runGraphModel.ts` (`GraphNode.kind`)
- Test: a Rust test near the existing `graph.rs` tests (grep `mod tests` in graph.rs); `runGraphModel.test.ts`

**Interfaces:**
- Produces: `StepNodeDto.kind` gains `"branch" | "action" | "gate"`; new optional fields `action: Option<String>` (the tool name) and `gate: Option<GateNodeDto>` where `GateNodeDto { auto_approve: bool, has_on_reject: bool, timeout_seconds: Option<u64> }` (NOT the existing panel `GateDto` — name it distinctly, e.g. `ApprovalGateDto`, to avoid the `:294` collision the recon flagged). TS `StepNodeDto` mirrors. `map_step` gains arms: `is_approval_gate(step)` → `kind:"gate"` + populated `gate`; `step.action.is_some()` → `kind:"action"` + `action`; keep precedence most-specific-first (gate/action before the linear fallback).

- [ ] **Step 1: Failing tests**

Rust: a `map_step` test that a parsed gate step yields `kind == "gate"` with `gate.auto_approve` reflecting the YAML, and an action step yields `kind == "action"` with `action == Some("scm.prs.create")`. TS: extend `runGraphModel.test.ts` — a DTO with `kind:"gate"` produces a `GraphNode` with `kind:"gate"` (folding still assigns `awaiting_approval` state from the event, unchanged).

- [ ] **Step 2: RED** — `cargo test -p rupu-cp map_step 2>&1 | tail`; `cd crates/rupu-cp/web && npm run test -- runGraphModel 2>&1 | tail`.

- [ ] **Step 3: Implement** — add the DTO fields (serde: `#[serde(skip_serializing_if = "Option::is_none")]`, and `default` on deserialize for forward-compat), the `map_step` arms, and the literal construction sites (new fields `None`). TS union + optional fields. `runGraphModel.ts`: `GraphNode.kind` is typed `StepNodeDto['kind']` already (`:46`), so it widens automatically; confirm no exhaustive switch on kind breaks (grep the file).

- [ ] **Step 4: GREEN** — Rust + web tests; `cargo build -p rupu-cp`, `tsc -b`.
- [ ] **Step 5: Commit** — `feat(cp): run-graph DTO carries gate + action step kinds`

---

### Task 3: CP run-viewer — GateNode & ActionNode React components

**Files:**
- Modify: `crates/rupu-cp/web/src/components/RunGraph.tsx` (`FlowKind` `:37`, `flowKind()` `:39`, `NODE_TYPES` `:54`), `crates/rupu-cp/web/src/components/graph/stepStyle.ts` (optional kind glyphs)
- Create: `crates/rupu-cp/web/src/components/graph/GateNode.tsx`, `crates/rupu-cp/web/src/components/graph/ActionNode.tsx`
- Test: `crates/rupu-cp/web/src/components/graph/GateNode.test.tsx`, `ActionNode.test.tsx`

**Interfaces:**
- Consumes: Task 2's `GraphNode.kind` (`'gate'|'action'`) + node data (`gate`, `action`, plus the existing `state` for status). Model both components on `StepNode.tsx` (same handles, same `state`-driven glyph via `stepStyle.ts`, same React Flow `NodeProps` shape — read StepNode fully first).
- Produces: `flowKind()` returns `'gate'` / `'action'` for those kinds; `NODE_TYPES` gains `{ gate: GateNode, action: ActionNode }`.

- GateNode: diamond-ish silhouette (CSS transform or a bordered pill with a ◇ glyph), the gate id, and the awaiting/approved/rejected state from `state` (`awaiting_approval` → ⏸ + "awaiting", done → ✓, failed/rejected → ✕). When `data.gate.auto_approve`, show a small "auto" tag. When `data.gate.has_on_reject`, a subtle reject-branch affordance (a second bottom handle or a "↳ on reject" caption — keep it declarative, no new edges in this task).
- ActionNode: compact card, the tool name (`data.action`) prominent with a monospace treatment, a small "connector" tag and a Write/Read badge if derivable (the DTO doesn't carry kind Read/Write yet — omit the badge unless Task 4's catalog is already wired; a follow-up can add it). State glyph as usual.

- [ ] **Step 1: Failing render tests** — GateNode renders the gate id + "awaiting" when `state:'awaiting_approval'` and an "auto" tag when `gate.auto_approve`; ActionNode renders the tool name. Model on any existing `components/graph/*.test.tsx` (grep — if none, model on a `components/*.test.tsx` that renders a node with mocked React Flow props).
- [ ] **Step 2: RED**, **Step 3: implement**, **Step 4: GREEN** (`npm run test`, `tsc -b`, `npm run build`).
- [ ] **Step 5: Commit** — `feat(cp): GateNode + ActionNode in the run graph`
- [ ] **Visual-check flag:** this task changes rendered run graphs — matt validates in the browser before PR merge.

---

### Task 4: `GET /api/tools` — MCP catalog endpoint

**Files:**
- Modify: `crates/rupu-mcp/src/tools/mod.rs` (ensure `ToolSpec` + `ToolKind` derive `Serialize` — recon says `input_schema` is already a `Value`; add derives if missing), `crates/rupu-cp/src/api/` (new `tools.rs` module + route registration — mirror an existing simple GET handler, e.g. how `api/coverage.rs` or a small list route registers), `crates/rupu-cp/web/src/lib/api.ts` (client fn + `ToolSpec` TS type)
- Test: a Rust handler test (mirror an existing `api/*.rs` test) + the TS `api.ts` fetch has a light test if the file has an api test harness (grep `api.test`)

**Interfaces:**
- Produces: `GET /api/tools` → `{ tools: [{ name, description, input_schema, kind: "read"|"write" }] }` from `rupu_mcp::tools::tool_catalog()`. TS: `api.getTools(): Promise<ToolSpec[]>` with `ToolSpec = { name: string; description: string; input_schema: unknown; kind: 'read'|'write' }`. `kind` currently `#[serde(skip)]` on `ToolSpec` (recon: `tools/mod.rs:34-39`) — for THIS serialization emit it explicitly (add a small response DTO that maps `ToolSpec` → serializable shape incl. kind, rather than un-skipping the internal field, to avoid disturbing the MCP snapshot which the schema_snapshot test pins).

- [ ] **Step 1: Failing test** — the handler returns 200 with a body containing `scm.prs.create` and its `kind:"write"`. RED: route not registered.
- [ ] **Step 2: RED**, **Step 3: implement** (response DTO + handler + route registration; wire the router the same place other `/api/*` routes register — grep the router builder in crates/rupu-cp/src), **Step 4: GREEN** (`cargo test -p rupu-cp`, `cargo build -p rupu-cp`; verify the rupu-mcp `schema_snapshot` test is UNCHANGED — if adding a `Serialize` derive to `ToolSpec` altered it, that's a real regression to resolve, not the pre-existing drift).
- [ ] **Step 5: Commit** — `feat(cp): GET /api/tools serves the MCP catalog for the editor`

---

### Task 5: Editor — gate & action kinds (schema, palette, nodes, forms, connector cards)

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/workflowGraph.ts` (union `:16`, `StepNodeData` `:42`, parse precedence `:172-176`, `MODELLED_STEP_KEYS` `:225`, serializer `nodeToStepObject` `:379`), `components/workflow-editor/NodePalette.tsx` (`ITEMS` `:42`, next-gating `:76`), `components/workflow-editor/kindVisuals.ts` (`KIND_ACCENT` `:17`, `KIND_ICON` `:24`), `components/workflow-editor/StepForm.tsx` (`KIND_LABELS` `:67`, `kindOptions` `:89`, `switchKind` `:101`, per-kind bodies, approval gating `:191`), `components/workflow-editor/nodes/EditableStepNode.tsx` (kind chip/body switches), `components/workflow-editor/WorkflowEditorGraph.tsx` (`newNodeData` `:162`)
- Test: `lib/workflowGraph.test.ts` (round-trip), `StepForm.test.tsx`, `NodePalette.test.tsx`

**Interfaces:**
- Consumes: Task 4's `api.getTools()` for connector cards.
- Produces: editor `StepKind` union gains `'approval_gate' | 'action'`; parse detects gate (`approval` present, no agent/for_each/parallel/panel/branch/action) and action (`o.action` present); serializer emits the gate's `approval:` block (auto_approve/on_timeout/notify/on_reject) and the action's `action:`+`with:`; `newNodeData` seeds a gate (`approvalRequired:true` + empty on_reject) and an action (empty `action`/`with`). Palette: a "Gate" card (Flow group) + connector cards (from the catalog, grouped) — all next-gated.

Split guidance (this is the largest task — the implementer MAY commit in two logical steps within the one task branch, but it's reviewed as one): **(5a) schema+palette+node**: union, parse, serialize, `newNodeData`, kindVisuals accent/icon, one static "Gate" palette card + one generic "Action" card, EditableStepNode gate/action chips. Round-trip test: a YAML with a gate node and an action step parses→graph→serializes back to equivalent YAML (model on the existing branch round-trip test in `workflowGraph.test.ts`). **(5b) forms+connector cards**: StepForm gets a dedicated gate body (prompt textarea, auto_approve input, timeout, on_timeout select, an on_reject mini-list — reuse the existing steps-list patterns) shown when `kind === 'approval_gate'` (and the shared "Require approval" checkbox stays for legacy inline agent steps, `:191` gating extended to also exclude `approval_gate`); an action body (a tool `<select>` populated from `api.getTools()` + a `with:` key/value editor driven by the selected tool's `input_schema` properties). Connector palette cards generated by grouping `getTools()` by prefix (`scm.prs.*`, `issues.*`, `github.*`/`gitlab.*`) — each card drops an action node pre-seeded with that tool name.

- [ ] **Step 1: Failing round-trip test** (5a) — gate + action YAML survives parse→serialize; the editor union/parse currently collapses them to `step`, so the serialized output loses `approval:`/`action:` (or the passthrough keeps them but kind is wrong). Assert the node `kind` is `'approval_gate'`/`'action'` and the re-serialized YAML contains the `approval:`/`action:` blocks.
- [ ] **Step 2: RED**, **Step 3: implement 5a**, **Step 4: 5a green** (`npm run test -- workflowGraph`, `tsc -b`).
- [ ] **Step 6: Failing form/card tests** (5b) — StepForm shows the gate body for a gate node (a "prompt" field, an "auto approve" field) and not the plain agent fields; a mocked `api.getTools()` yields grouped connector cards in the palette; selecting a tool in the action body renders its schema keys. **Step 7: RED**, **Step 8: implement 5b**, **Step 9: green** (`npm run test`, `tsc -b`, `npm run build`).
- [ ] **Step 10: Commit** — `feat(cp): author gate nodes + connector action steps in the workflow editor`
- [ ] **Visual-check flag:** editor is a rich UI surface — matt validates authoring both kinds in the browser before merge.

---

### Task 6: Legacy inline-approval synthesized gate + sample + verification

**Files:**
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/` (parse: a legacy inline approval — agent+prompt+`approval.required`— renders as its normal agent node PLUS a visual "gate" affordance; the spec's "synthesized gate node + one-click extract to gate node" — SCOPE DECISION below), `lib/workflowGraph.ts`
- Create/verify: `.rupu/workflows/` already has `gate-demo.yaml` (Plan 1) and `action-demo.yaml` (Plan 2) — this task verifies they render correctly in canvas + run viewer + open cleanly in the editor.
- Modify: `CLAUDE.md` (renderers/editor note); `docs/.../plan-3` mark done.

**Scope decision (keep it small):** the full "synthesized standalone gate node between two steps" for legacy inline approvals is a graph-topology change (inserting a node the YAML doesn't have) that risks the editor's round-trip fidelity. For Plan 3, ship the *lighter* version the spec allows: a legacy inline-approval agent node shows a **dashed gate badge/ring** (a visual marker that this step has a human gate) plus a StepForm **"Convert to gate node"** button that rewrites the YAML — moving the `approval:` block onto a new standalone gate step inserted before the agent step and stripping it from the agent step. The button's rewrite is a pure `workflowGraph`-level transform with a unit test (in → out YAML). No synthesized phantom node in the graph. Note the deferral of full auto-synthesis in the plan doc.

- [ ] **Step 1: Failing test** — the convert transform: input YAML with an inline `approval: {required, prompt}` on an agent step → output YAML with a new gate step (carrying the prompt) immediately before it and the agent step's `approval:` removed. Unit-test the transform fn directly.
- [ ] **Step 2: RED**, **Step 3: implement** the transform + the dashed-gate badge on inline-approval nodes + the StepForm button, **Step 4: green**.
- [ ] **Step 5: Manual render verification** — `cargo run -p rupu-cli -- workflow show gate-demo --view full` and eyeball the canvas rows include a gate row; open both sample workflows in the editor (matt, at visual-check time). Full suite: `cargo test -p rupu-app-canvas -p rupu-cp`, `npm run test`, `tsc -b`, `npm run build`, `cargo clippy -p rupu-app-canvas -p rupu-cp 2>&1 | grep -c "^error"`.
- [ ] **Step 6: Commit** — `feat(cp): dashed-gate badge + convert-to-gate for legacy inline approvals; docs`

---

## Deferred to Plan 4 (unchanged)
`notify:` execution (calls `execute_action_step`), cp-serve gate sweep + orphan reaper, unattended cleanup permission mode, `[scm.default]` config wiring in `Registry::default_platform`. Also deferred from here: full auto-synthesized phantom gate node for legacy inline approvals (Plan 3 ships the dashed badge + convert button instead); Read/Write badge on ActionNode (needs the catalog kind threaded to the run DTO).
