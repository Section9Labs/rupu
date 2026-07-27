# Non-linear Orchestration — Phase 3 (`depends_on` + bounded subgraph loops) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add the two remaining non-linear language constructs — `depends_on:` (predecessor edges) and bounded **subgraph loops** (a sub-DAG that re-runs together until a condition holds, hard iteration cap) — while every existing linear workflow produces byte-for-byte identical output.

**Architecture:** `depends_on` is a fourth control-edge source folded into the existing `workflow_edges` union — the scheduler already runs arbitrary DAGs, so it needs only parse + validate + edge-contribution + editor render. A loop is a **super-node** in the outer `run_scheduler`: when its external predecessors are done it dispatches a bounded iteration driver that runs the loop's member sub-DAG through a **recursive `run_scheduler` call** (reusing Phase 2's concurrent dispatch/join/prune), evaluates `until` after each iteration, and converges / re-runs / fails-on-exhaustion. Per-iteration state is latest-wins; a data ref to a non-upstream loop member reads its prior iteration (the loop's controlled feedback). Resume re-enters a loop at its recorded iteration.

**Tech Stack:** Rust 2021, `tokio`, `thiserror`, `minijinja`, `serde`/`BTreeMap`; `cargo test -p rupu-orchestrator`. Editor: TypeScript/React/Vite, existing tokens; `crates/rupu-cp/web`.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-25-rupu-nonlinear-phase3-depends-on-and-loops-design.md` (decisions locked in §8). Branch: `nonlinear-phase3`, stacked on `nonlinear-phase2`.
- **Byte-for-byte legacy equality is the primary safety invariant.** A workflow with no `depends_on` and no `loops` must produce identical `step_results.jsonl` / `events.jsonl` / `run.json` as today. `depends_on` and `loops` are `#[serde(default, skip_serializing_if …)]`; the router still sends `is_nonlinear == false` workflows to `run_steps_inner`. The `dag_scheduler_golden` test (all 15 samples + the partial-edges regression) guards every task.
- **Reuse, do not fork:** `run_scheduler` / `run_node` / `workflow_edges` / `validate_graph` / Phase-2 per-node resume / the minijinja `when:`-condition evaluator. A loop iteration is a recursive `run_scheduler` over a node subset — not a new engine.
- `#![deny(clippy::all)]`; new errors via `thiserror`. `loops: BTreeMap<String, LoopDef>` (match `Workflow.inputs`/`outputs`; NO new dep — `IndexMap` is not a workspace dep).
- Frontend: `next` editor path only (`[cp].workflow_editor_ui = 'next'`); classic untouched; tokens only; no new dep; `tsc` + vitest + build clean.
- The 4 pre-existing `tests/linear_runner.rs` mock-provider flaky failures + the known `rupu-cli` toolchain-mismatch noise are baseline — verify against base, don't chase.

## File Structure

- `crates/rupu-orchestrator/src/workflow.rs` — `Step.depends_on`; `Workflow.loops`; `LoopDef`/`OnMax`; `workflow_edges` union gains `depends_on`; `validate_graph` gains `depends_on` + loop validation; `is_nonlinear` accounts for loops; helpers `loop_of_step`, `collapsed_graph_edges`, `loop_internal_edges`.
- `crates/rupu-orchestrator/src/runner.rs` — `run_scheduler` recognizes a loop super-node; new `run_loop_node` (the bounded iteration driver, recursive `run_scheduler` over the member subset); `until`/`on_max` evaluation; `loop.<name>.iteration`/`converged` in the render context; the cross-iteration feedback read; `RunWorkflowError::LoopExhausted`; loop resume (iteration re-entry).
- `crates/rupu-orchestrator/src/runs.rs` — additive `RunRecord` loop-progress field; optional `loop_iteration` discriminator on a loop member's `StepResult` (legacy shape unchanged when absent).
- `crates/rupu-cp/web/src/lib/workflowGraph.ts` — `deriveEdges` reads `depends_on`; loop-group model (`StepNodeData` loop membership; a `WorkflowLoop` type).
- `crates/rupu-cp/web/src/components/workflow-editor/*` — loop group rendering (dashed boundary + `until`/`max` badge), "Group into loop" action, loop form; `depends_on` delete-clears.

---

### Task 1: `depends_on:` — predecessor edges (language + graph + validation + editor render)

**Files:** `crates/rupu-orchestrator/src/workflow.rs`; `crates/rupu-cp/web/src/lib/workflowGraph.ts` + the editor edge-render/delete path; tests in-crate + a web test.

**Interfaces:**
- Consumes: Phase-1 `workflow_edges(&Workflow) -> Vec<(String,String)>`, `validate_graph`, `is_nonlinear`, the editor `deriveEdges`.
- Produces: `Step.depends_on: Vec<String>`; `workflow_edges` now includes `p → s` for each `p ∈ s.depends_on`.

- [ ] **Step 1: Failing test — parse + skip_serializing_if.** In `workflow.rs` tests: a step with `depends_on: [a]` parses into `Step.depends_on == vec!["a"]`; a step without it serializes with NO `depends_on` key (round-trip a legacy step, assert the key is absent). Run: `cargo test -p rupu-orchestrator --lib depends_on` — expect FAIL (field missing).
- [ ] **Step 2: Add the field.** `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub depends_on: Vec<String>` on `Step`. Run the test — PASS.
- [ ] **Step 3: Failing test — edge contribution.** `workflow_edges` on a 2-step workflow where `s2.depends_on = [s1]` returns the edge `("s1","s2")`; and it dedups if `s1.next = [s2]` also present (one edge, not two). FAIL first.
- [ ] **Step 4: Fold `depends_on` into `workflow_edges`.** In the control-edge accumulation, for each step `s` and each `p ∈ s.depends_on`, insert `(p, s.id)` into the same BTreeSet the `next`/`split`/branch edges go into (self-ref filtered). PASS.
- [ ] **Step 5: Failing test — validation.** A `depends_on` naming a non-existent id → `EdgeTargetUnknown`; a `depends_on: [own_id]` → `EdgeSelfLoop`; a `depends_on` forming a cycle with a `next` (`a next:[b]; b depends_on:[a]`… actually `a→b` twice — construct a real cycle: `a depends_on:[b]; b depends_on:[a]`) → `WorkflowCycle` with the path. FAIL first.
- [ ] **Step 6: Validate.** Confirm `validate_graph` already routes `depends_on` edges through the existing target-exists / self-loop / Kahn cycle checks (it will, since they read `workflow_edges`); add any missing wiring so `depends_on` participates. PASS.
- [ ] **Step 7: Failing test — is_nonlinear + byte-equality.** A `depends_on`-only linear chain in declaration order (`a; b depends_on:[a]; c depends_on:[b]`) is `is_nonlinear == false` (declaration order is topological) → would route to `run_steps_inner`. A `depends_on` that creates a fork/reconverge is `is_nonlinear == true`. And: every `.rupu/workflows/*.yaml` stays `is_nonlinear == false` + round-trips unchanged. FAIL/PASS as appropriate.
- [ ] **Step 8: Editor — read `depends_on` as inbound edges.** In `workflowGraph.ts` `deriveEdges`, for each node with `depends_on: [p]`, add an edge `p → thisNode` (alongside the existing `next`/`split`/branch/data derivation). The draw-action still writes `next` (unchanged). Web test: a workflow with `s2.depends_on=[s1]` renders the `s1→s2` edge; round-trips.
- [ ] **Step 9: Editor — delete a `depends_on` edge clears the entry.** Deleting a rendered edge that came from `depends_on` removes that id from the target's `depends_on` (mirror the existing `next`-clear on delete). Web test.
- [ ] **Step 10: Full suites + commit.** `cargo test -p rupu-orchestrator`; web `tsc`+vitest+build. Golden green. Commit `feat(orch): depends_on predecessor edges (language + graph + validation + editor render)`.

---

### Task 2: `loops:` — language + validation (no runtime; honesty gate)

**Files:** `crates/rupu-orchestrator/src/workflow.rs`; tests in-crate.

**Interfaces:**
- Consumes: `workflow_edges`, `validate_graph`, `is_nonlinear`.
- Produces: `Workflow.loops: BTreeMap<String, LoopDef>`; `LoopDef { nodes: Vec<String>, until: String, max_iterations: u32, on_max: OnMax }`; `enum OnMax { Fail (default), Proceed }`; helpers `pub fn loop_of_step(&Workflow, step_id) -> Option<&str>`, `pub fn loop_internal_edges(&Workflow, loop_name) -> Vec<(String,String)>` (edges of `workflow_edges` with both endpoints in the loop), `pub fn collapsed_graph_edges(&Workflow) -> Vec<(String,String)>` (each loop replaced by a super-node id `"loop:<name>"`, external edges rewired). `is_nonlinear` returns true if `!loops.is_empty()`.

- [ ] **Step 1: Failing test — parse.** A workflow with the spec §2a `loops:` block parses into `Workflow.loops["refine"]` with `nodes == [gen,test,critique]`, `until == "{{ steps.critique.approved }}"`, `max_iterations == 5`, `on_max == OnMax::Fail` (default when omitted); `on_max: proceed` parses to `Proceed`. A workflow with no `loops` serializes with NO `loops` key. FAIL first.
- [ ] **Step 2: Add the types + field.** `LoopDef`, `OnMax` (`#[derive(Default)]` → `Fail`), `Workflow.loops: BTreeMap<String, LoopDef>` with `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]`. PASS.
- [ ] **Step 3: Failing test — validation matrix.** Each rejected with its own error: unknown member id (`LoopNodeUnknown`); a step in two loops (`LoopNodeOverlap`); a loop with <2 nodes (`LoopTooSmall`); `max_iterations == 0` (`LoopMaxIterationsInvalid`); an empty `until` (`LoopUntilEmpty`); members whose internal edges are cyclic (`LoopSubgraphCyclic` with the path); a loop member with a `split`/`next`/branch-arm targeting a NON-member (`LoopMemberEscapes`) — note a member's external INBOUND edge (entry) and external OUTBOUND edge authored on an OUTSIDE node via `depends_on`/`next` are allowed; only a member's OWN control edge escaping is rejected; the collapsed outer graph having a cycle between loops/nodes (`WorkflowCycle`). FAIL first (errors don't exist).
- [ ] **Step 4: Add the errors + validation.** Add the `thiserror` variants to the workflow validation error enum. Implement in `validate_graph` (or a new `validate_loops` it calls): iterate `loops`, run each check, build `loop_of_step`/`loop_internal_edges`/`collapsed_graph_edges` helpers, run Kahn on the collapsed graph. PASS the whole matrix.
- [ ] **Step 5: Failing test — is_nonlinear + honesty gate + byte-equality.** A workflow with a `loops` block is `is_nonlinear == true`. Every `.rupu/workflows/*.yaml` (none have loops) stays `is_nonlinear == false` + round-trips. AND: calling `run_workflow` on a loop workflow returns a clear "loop execution requires the loop driver (lands next task)" error — reuse or add a gate variant (`LoopRuntimeNotYetWired { name }`) so a loop workflow does NOT silently mis-run through the scheduler before Task 3 wires execution. (This gate is REMOVED in Task 3.) FAIL/PASS.
- [ ] **Step 6: Wire is_nonlinear + the temporary honesty gate.** `is_nonlinear` returns true for `!loops.is_empty()`. In `run_workflow`'s router, if the workflow has loops, return `LoopRuntimeNotYetWired` (before any run.json side effects) — Task 3 replaces this with real execution. PASS.
- [ ] **Step 7: Full suite + commit.** `cargo test -p rupu-orchestrator`; golden green. Commit `feat(orch): loops language + validation (parse, matrix, collapsed-DAG check, honesty gate)`.

*(Lands NO loop execution — proves the language + all validation, with a fail-loud runtime gate so nothing mis-runs.)*

---

### Task 3: Loop execution — the super-node + recursive iteration driver

**Files:** `crates/rupu-orchestrator/src/runner.rs`; tests in-crate.

**Interfaces:**
- Consumes: `run_scheduler`, `run_node`, `workflow_edges`, `loop_of_step`, `loop_internal_edges`, `collapsed_graph_edges`, the minijinja `when:` evaluator, `base_context_for_step`.
- Produces: `async fn run_loop_node(loop_name, &LoopDef, …) -> Result<NodeOutcome, RunWorkflowError>` (the bounded iteration driver); `RunWorkflowError::LoopExhausted { name, max_iterations }`; `loop.<name>.iteration`/`converged` in the render context; the cross-iteration feedback read. Removes the Task-2 `LoopRuntimeNotYetWired` gate.

- [ ] **Step 1: Failing test — a refine loop converges + proceeds.** MockProvider harness: `seed → (loop refine: gen→test→critique, until "{{ steps.critique.approved }}") → ship`. Mock `critique` returns `approved=false` for iterations 0..K-1 and `true` at K. Assert: the loop runs exactly K+1 iterations; `ship` runs once after convergence and reads the converged `critique`/`gen` outputs; run status Completed. FAIL first (loop not executed — Task-2 gate).
- [ ] **Step 2: Build the outer super-node integration.** In `run_scheduler`, build the outer graph over `collapsed_graph_edges` (each loop = super-node `loop:<name>`). When a loop super-node becomes ready (all external predecessors done), dispatch `run_loop_node` instead of `run_node`. Its successors (external out-edges) become ready only on the loop node's completion. Reuse the existing Kahn/indegree machinery over the collapsed graph.
- [ ] **Step 3: Implement `run_loop_node` (the iteration driver).** For `i in 0..max_iterations`: set `loop.<name>.iteration = i`; run the member sub-DAG via a **recursive `run_scheduler`** restricted to `LoopDef.nodes` with `loop_internal_edges(name)`; after it completes, evaluate `until` (minijinja over the current `steps` context); if true → set `converged=true`, break, return `Completed`; if false and `i == max-1` → apply `on_max`. Persist each iteration's member `StepResult`s (Task 4 adds the iteration discriminator; here just persist latest-wins so `steps.<member>` = latest). PASS Step 1.
- [ ] **Step 4: Failing test — the cross-iteration feedback read.** A loop `gen→critique` where `gen`'s prompt references `{{ steps.critique.output }}` (a non-upstream member). Assert: on iteration 0 the ref renders empty; on iteration 1 `gen` sees iteration-0's `critique` output (the prior value), NOT a cycle error and NOT the current iteration's not-yet-run value. FAIL first.
- [ ] **Step 5: Implement the feedback resolution.** In the loop iteration's context build, a member's data ref to a loop sibling that is NOT an ancestor in `loop_internal_edges` resolves to that sibling's **prior-iteration** `StepResult` (kept from the last iteration; empty on iteration 0). A ref to an upstream sibling uses the current iteration (normal). Confirm the internal data-edge inference does NOT add an edge that would make the sub-DAG cyclic (a non-ancestor ref is a prior-iteration read, not an edge). PASS.
- [ ] **Step 6: Failing test — `on_max`.** With `until` never satisfied: `on_max: fail` (default) → run fails with `LoopExhausted { name:"refine", max_iterations:3 }` and a message "loop 'refine' exhausted 3 iterations, until never held"; `on_max: proceed` → run continues to `ship`, `ship` reads the last iteration's output, and `{{ loop.refine.converged }}` is `false`. FAIL first.
- [ ] **Step 7: Implement `on_max` + the `loop.*` surface.** Add `LoopExhausted` (thiserror); on cap-without-convergence apply `on_max`; expose `loop.<name>.iteration` (last index) + `converged` in the render context for members, successors, and `until`/`when`. Remove the Task-2 `LoopRuntimeNotYetWired` gate. PASS.
- [ ] **Step 8: Failing test — concurrency within an iteration.** A loop whose members include a `split → [a,b] → join` runs `a` and `b` concurrently within each iteration (assert overlap via the atomic high-water pattern from Phase-2's `scheduler_concurrency` test). Confirms the recursive `run_scheduler` keeps Phase-2 concurrency. Implement (should already hold) + PASS.
- [ ] **Step 9: Byte-equality + full suite + commit.** `dag_scheduler_golden` green (no sample has loops → untouched); `cargo test -p rupu-orchestrator` — only the 4 known flakes. Commit `feat(orch): bounded loop execution — super-node + recursive iteration driver + until/on_max + feedback`.

---

### Task 4: Loop resume / checkpoint

**Files:** `crates/rupu-orchestrator/src/runner.rs`, `crates/rupu-orchestrator/src/runs.rs`; tests in-crate.

**Interfaces:**
- Consumes: `run_loop_node`, Phase-2 per-node resume (`already_done` rebuild), `step_results.jsonl` persistence, `RunRecord`.
- Produces: an optional `loop_iteration: Option<u32>` discriminator on a loop member's persisted `StepResult` record (absent for non-loop steps → legacy shape unchanged); an additive `RunRecord` loop-progress field (`#[serde(default)] loop_progress: BTreeMap<String, u32>` — loop name → current iteration); loop re-entry on resume.

- [ ] **Step 1: Failing test — resume mid-loop.** Run the refine loop; simulate a pause at iteration 2 with `gen`+`test` done but `critique` not (persist those, set `loop_progress["refine"]=2`), then resume with a fresh scheduler over the same run dir. Assert: resume re-enters iteration 2, re-runs ONLY `critique` (not `gen`/`test` of iteration 2), converges, proceeds to `ship`; `gen`/`test` of iteration 2 are NOT re-dispatched. FAIL first.
- [ ] **Step 2: Persist per-iteration member results + the loop counter.** Add `loop_iteration: Option<u32>` to the `StepResult` persistence for a loop member (via the emit path in `run_loop_node`); a non-loop step persists it as absent (`skip_serializing_if = "Option::is_none"` → legacy `step_results.jsonl` byte-identical). Persist `RunRecord.loop_progress[name]` at each iteration boundary. PASS the resume test's re-entry.
- [ ] **Step 3: Failing test — a converged loop is not re-run on resume.** Resume a run where `refine` already converged (its members + `loop_progress` recorded, `ship` not yet run) → the loop is NOT re-entered; `ship` runs once. FAIL/PASS.
- [ ] **Step 4: Rebuild loop state on resume.** On resume, rebuild `loop_progress` + per-iteration `already_done` from disk; a loop whose successors are ready (converged) is skipped; a loop mid-iteration re-enters at `loop_progress[name]` and re-runs only that iteration's not-done members. Reconstruct a member's prior-iteration feedback value from the recorded `(name, iteration-1, member)` result. PASS.
- [ ] **Step 5: Failing test — cancel/restart a loop.** Cancel a run mid-loop (Phase-2 cancel) → in-flight iteration members aborted; restart re-enters at the recorded iteration. Assert no member of a not-yet-started iteration ran. FAIL/PASS (reuse Phase-2 cancel).
- [ ] **Step 6: Byte-equality + full suite + commit.** A no-loop workflow writes no `loop_iteration`/`loop_progress` → `step_results.jsonl`/`run.json` byte-identical; golden green. Commit `feat(orch): loop resume + checkpoint (per-iteration keying, re-enter-at-iteration)`.

---

### Task 5: Editor — loop authoring + rendering + `depends_on` polish

**Files:** `crates/rupu-cp/web/src/lib/workflowGraph.ts`, `crates/rupu-cp/web/src/components/workflow-editor/*`; web tests.

**Interfaces:**
- Consumes: the Phase-1 `next` editor, `deriveEdges` (+ Task-1 `depends_on` read), the run-graph status overlay (from the gate/action-node work).
- Produces: a `WorkflowLoop` model (`{ name, nodes: string[], until, maxIterations, onMax }`) surfaced from/to `loops:`; loop group rendering; a "Group into loop" action + form.

- [ ] **Step 1: Failing web test — loop renders as a dashed group.** Given a workflow with a `loops.refine` over `[gen,test,critique]`, the editor renders a dashed boundary enclosing those three nodes with a `refine` label + an `until`/`max_iterations` badge. FAIL first.
- [ ] **Step 2: Model + render the loop group.** Parse `loops:` into `WorkflowLoop[]`; render the dashed boundary + badge (tokens only; reuse the group-boundary style if one exists, else a bordered container behind the member nodes). PASS.
- [ ] **Step 3: Failing web test — "Group into loop" authors `loops:`.** Selecting ≥2 nodes + invoking "Group into loop" + filling name/`until`/`max_iterations`/`on_max` produces a `loops.<name>` entry with those `nodes`; the YAML round-trips. A <2-node selection disables the action. FAIL first.
- [ ] **Step 4: Implement the action + form.** Add the "Group into loop" action (on a multi-select) + a form (name / until / max_iterations / on_max picker); write the `loops:` entry; editing/removing updates it. PASS.
- [ ] **Step 5: Failing web test — inline loop validation.** A loop with overlapping members / a cyclic sub-DAG / a member-escape shows the backend validation error inline (reuse the Phase-1 inline validation surface). FAIL/PASS.
- [ ] **Step 6: Runtime overlay + `depends_on` delete polish.** At runtime the loop group shows `iteration N/max` + converged/failed (reuse the run-graph status overlay). Confirm Task-1's `depends_on` delete-clears works from the group context too. Web test.
- [ ] **Step 7: Full web suite + commit.** `tsc` + vitest + build clean. Commit `feat(cp-web): loop authoring + rendering + depends_on polish`.

## Operator gate (before merge, with Phases 1+2)
matt authors + runs a refine loop end to end — confirms it iterates, the feedback improves each pass, it converges + proceeds to the successor, exhaustion fails loudly (and `on_max: proceed` continues with `converged=false`), and resume mid-loop re-enters at the right iteration; `depends_on` authors an edge from the target side and renders; **every existing workflow still runs identically** (the primary risk).

## Self-review notes
- Spec coverage: §1 depends_on → Task 1; §2a-2c loop language+super-node+driver → Tasks 2-3; §2d feedback → Task 3 Steps 4-5; §2e state/template surface → Task 3 Step 7; §2f validation → Task 2 Step 3-4; §2g honesty → Task 2 Step 5-6 (gate) removed Task 3 Step 7; §3 resume → Task 4; §4 byte-equality → every task's golden step; §5 editor → Task 5 (+ Task 1 depends_on render). Nothing unmapped.
- Type consistency: `LoopDef`/`OnMax`/`loop_of_step`/`loop_internal_edges`/`collapsed_graph_edges` defined in Task 2, consumed in Tasks 3-4; `run_loop_node`/`LoopExhausted`/`loop_progress`/`loop_iteration` defined in Tasks 3-4; `WorkflowLoop` in Task 5. Consistent.
- Decisions map (spec §8): subgraph loop → Task 3; on_max fail-default → Task 3 Step 6-7; top-level loops map + BTreeMap → Task 2; latest-wins + loop.* → Task 3; v1 flat/disjoint/≥2 → Task 2 validation; depends_on → Task 1. Out-of-scope items (error/guarded edges, sub-workflows, single-node/nested loops) appear in NO task.
