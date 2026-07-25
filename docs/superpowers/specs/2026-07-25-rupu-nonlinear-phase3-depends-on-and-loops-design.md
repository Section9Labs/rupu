# Non-linear orchestration — Phase 3: `depends_on:` + bounded subgraph loops (design)

**Date:** 2026-07-25
**Status:** Design — approved by matt (loop scope = subgraph iteration; `on_max` = fail-loudly default). Ready to plan.
**Parent:** the proposal + Phase 1 + Phase 2 (`2026-07-24-rupu-nonlinear-orchestration-proposal.md`, `…-phase1-design.md`, `…-phase2-scheduler-design.md`). Phase 3 delivers the two decisions the proposal deferred as fast-follows: **D1** (`depends_on:` predecessor edges) and **D4** (bounded `loop`, core graph stays acyclic).
**Scope:** `crates/rupu-orchestrator` (language + validation + scheduler) and `crates/rupu-cp/web` (editor rendering + authoring). **Existing linear workflows must produce byte-for-byte identical output.**

Phase 1 made non-linear workflows authorable + valid; Phase 2 made them execute concurrently. Phase 3 adds the last two language constructs: `depends_on:` (author an edge from the target side) and bounded **subgraph loops** (a sub-DAG that re-runs together until a condition holds, with a hard iteration cap). Everything else the proposal floated (error edges, guarded edges, sub-workflows, subgraph-map) stays in the *later* deferred set — **out of scope for Phase 3**.

## 1. `depends_on:` — predecessor edges (the small piece)

### 1a. Language
Additive to `Step` (`crates/rupu-orchestrator/src/workflow.rs`): `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub depends_on: Vec<String>`. Each id `p` in a step `s`'s `depends_on` contributes a control edge `p → s` — the symmetric inverse of `next:` (where `s.next = [t]` contributes `s → t`).

### 1b. Graph model
`workflow_edges(&Workflow)` (Phase 1) already unions control edges (`next` / `split` / branch arms) with inferred `steps.X` data edges. Phase 3 adds `depends_on` as a fourth control-edge source in the same union (`p → s` for each `p` in `s.depends_on`). Dedup (BTreeSet) and self-ref filtering are unchanged. An explicit `depends_on` edge only *adds* ordering — it never removes an authored `next` edge; a `next` and a `depends_on` describing the same edge collapse to one after dedup.

### 1c. Validation
Reuse the existing edge checks in `validate_graph`: every `depends_on` id names a real step (`EdgeTargetUnknown`), no self-edge (`EdgeSelfLoop`), and the combined graph is still a DAG (Kahn cycle detection — a `depends_on` that forms a cycle is rejected with the offending path). `is_nonlinear` already reads `workflow_edges`, so a `depends_on`-only workflow correctly routes to the scheduler.

### 1d. Editor
`deriveEdges` (the `next` explicit-edge derivation from Phase 1) additionally reads `depends_on`: a step's `depends_on: [p]` renders an inbound edge from `p`. The **draw-action still writes `next`** on the source node (unchanged authoring), so the canvas never *emits* `depends_on` on its own — but a hand-authored `depends_on` in the YAML pane renders correctly and round-trips (serialize preserves it). Deleting a rendered `depends_on` edge clears that `depends_on` entry on the target.

### 1e. Legacy compatibility
`depends_on` is `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, so it never appears in a legacy workflow's serialized form; a workflow with no `depends_on` parses + runs exactly as today. `is_nonlinear == false` for a workflow whose only edges are a trivial linear chain expressed via `depends_on` in declaration order (the router still picks `run_steps_inner` — see §4 byte-equality).

## 2. Bounded subgraph loops (the real work)

### 2a. Language — the top-level `loops:` map
A loop is a **named group of existing step ids** that re-run together as a sub-DAG. New top-level field on `Workflow`: `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")] pub loops: BTreeMap<String, LoopDef>` (`BTreeMap`, matching the existing `inputs`/`outputs` maps on `Workflow` — no new dep; loop order is semantically irrelevant since each loop is independent and keyed by name). `LoopDef { nodes: Vec<String>, until: String, max_iterations: u32, #[serde(default)] on_max: OnMax }`, where `enum OnMax { #[default] Fail, Proceed }`.

```yaml
steps:
  - { id: seed, agent: seeder }
  - { id: gen,      agent: generator }
  - { id: test,     agent: tester,   depends_on: [gen] }
  - { id: critique, agent: critic,   depends_on: [test] }
  - { id: ship,     agent: shipper,  depends_on: [critique] }
loops:
  refine:
    nodes: [gen, test, critique]
    until: "{{ steps.critique.approved }}"
    max_iterations: 5
    on_max: fail          # default; or `proceed`
```

- `seed → (loop refine) → ship`: `seed` feeds the loop (external edge into `gen`), `ship` runs after the loop converges (external edge out of `critique`).
- The loop's `nodes` keep their own bodies (agent/action/etc.) and their internal control+data edges (`gen → test → critique`).

### 2b. The loop as a super-node
The outer scheduler (Phase 2's `run_scheduler`) treats a loop as **one node collapsed from its members**:
- **Entry:** the loop becomes *ready* when every external predecessor of every loop member is `done` (an external edge is one whose source is outside the loop). Internal edges do not gate entry.
- **Successors:** the loop's external successors (edges from a loop member to a non-member) become ready only after the loop **converges or exhausts** — never after a single iteration.
- **Indegree accounting:** a loop member's *internal* inbound edges are handled by the inner iteration scheduler, not the outer Kahn indegree. The outer graph is built over the **collapsed** graph (each loop replaced by its super-node); this collapsed graph must be acyclic (§2f).

### 2c. An iteration = a recursive `run_scheduler` over the node subset
Each iteration runs the loop's members through the **existing** `run_scheduler` machinery, restricted to the loop's node set and their internal edges (a recursive/nested invocation over a sub-DAG — the same concurrent dispatch, join, branch-pruning already built). After a full iteration completes:
1. Evaluate `until` (a minijinja condition, same engine as `when:`/branch) against the current `steps` context.
2. `true` → the loop converges; set `loop.<name>.converged = true`; its successors become ready.
3. `false` and `iteration + 1 < max_iterations` → run the next iteration.
4. `false` and the cap is reached → **`on_max`**: `fail` (default) fails the run with `LoopExhausted { name, max_iterations }` ("loop 'refine' exhausted 5 iterations, until never held"); `proceed` continues to the successors with the last iteration's outputs and `loop.<name>.converged = false`.

Iterations are **sequential** (iteration N+1's inputs depend on N's outputs + the `until` check). Concurrency *within* an iteration is exactly Phase 2's (members run concurrently per their internal edges).

### 2d. The feedback mechanic (what makes refine loops work)
Within an iteration, a member's templated fields are rendered against the `steps` context. A data ref to a member that is **upstream in this iteration's sub-DAG** (a real internal edge, e.g. `test` reads `gen`) uses **this iteration's** value. A data ref to a loop member that is **not upstream** (e.g. `gen` reads `critique`) resolves to that member's **prior iteration's** value — the loop's *controlled feedback* ("regenerate, improved by the last critique"). This back-reference is the ONLY cyclic data flow, and it never becomes a sub-DAG edge, so the inner graph stays acyclic. On **iteration 0**, any not-yet-produced ref renders empty (identical to today's forward-ref-before-run behavior — a substitution, not an error). Rationale: data-edge inference inside a loop adds an intra-iteration edge ONLY when the referenced member is already an ancestor in the internal control+data DAG; a ref that would create an internal cycle is reinterpreted as a prior-iteration (cross-iteration) read, not an edge.

### 2e. Per-iteration state + template surface
- **Latest-wins:** `steps.<member>` always holds the **latest** iteration's `StepResult` (output/success). Downstream (`ship`) and the `until` condition read the converged (or last) iteration's outputs.
- **Loop metadata exposed to templates:** `loop.<name>.iteration` (0-based index of the current/last iteration) and `loop.<name>.converged` (bool). Available inside the loop (to members, for feedback prompts like "attempt {{ loop.refine.iteration }}") and after it (to successors and other `until`/`when` conditions).
- No cross-iteration history accumulation in v1 (YAGNI); latest-wins only.

### 2f. Validation
In `validate_graph` (extended):
- Every `loops.<name>.nodes` id names a real step (`LoopNodeUnknown`).
- Loops are **disjoint** — a step belongs to at most one loop (`LoopNodeOverlap`); **loops do not nest** in v1.
- A loop has **≥ 2 nodes** (a 1-node "loop" is a node-level retry; if wanted, that's a trivial degenerate but we require ≥2 to keep the construct meaningful — a single-node bounded retry can be a fast-follow).
- The loop's members + their **internal** edges form an **acyclic sub-DAG** (`LoopSubgraphCyclic` with the offending path) — the feedback back-reference (§2d) is excluded from this check because it is not an internal edge.
- `max_iterations ≥ 1` (`LoopMaxIterationsInvalid`).
- `until` is a non-empty template (parse-checked like `when:`).
- The **collapsed** outer graph (each loop replaced by its super-node, external edges rewired to/from the super-node) is a **DAG** (`WorkflowCycle`) — i.e. no cycle *between* loops or between a loop and outer nodes; the only permitted "cycle" is the loop's own bounded iteration.
- A loop member may not carry `split`/`join` that reaches OUTSIDE the loop (a member's external control edges are entry/exit only — a `split` targeting a non-member from inside a loop is rejected, `LoopMemberEscapes`). Internal split/join between members is allowed.

### 2g. Runtime honesty
Consistent with Phase 2's router: a workflow using `loops:` is `is_nonlinear == true` (it has a loop super-node) and routes to `run_scheduler`. No silent linear mis-run; `on_max: fail` never silently passes an unconverged result (the whole point of the fail-loudly default).

## 3. Resume / checkpoint (the hard part, one level down)

Generalize Phase 2's per-node resume to loop iterations:
- **Persist:** each loop iteration's member `StepResult`s are recorded keyed by `(loop_name, iteration, step_id)` (the existing `step_results.jsonl` gains an optional `loop_iteration` discriminator on a loop member's record; a non-loop step omits it → legacy shape unchanged). The loop also persists its current `iteration` counter + `converged` in the run record (additive `RunRecord` field, `#[serde(default)]`).
- **Resume:** rebuild the outer ready-set (Phase 2). For a loop that was in-flight, re-enter at its recorded `iteration`; within that iteration, re-run only the not-done members (Phase 2's per-node resume, scoped to the loop's node subset for that iteration). A converged loop is not re-run; its successors resume normally. A member's prior-iteration feedback value is reconstructed from the recorded `(loop, iteration-1, member)` result.
- **Cancel/pause:** a loop in-flight participates in Phase 2's cancel/pause — an in-flight iteration's members are aborted; restart re-enters the loop at the recorded iteration.
- Byte-equality: a workflow with no loops writes no `loop_iteration` discriminator and no loop counter → `step_results.jsonl` / `run.json` byte-identical.

## 4. Byte-for-byte legacy equality (unchanged primary invariant)
- A workflow with no `loops` and no `depends_on` is untouched: `depends_on`/`loops` are `#[serde(default, skip_serializing_if)]`, the router still sends `is_nonlinear == false` workflows to `run_steps_inner`, and `dag_scheduler_golden` (all 15 samples + the partial-edges regression) stays green on every task.
- A linear chain expressed purely via `depends_on` in declaration order is `is_nonlinear == false` (declaration order is topological) → `run_steps_inner`, identical output.

## 5. Editor (`crates/rupu-cp/web`, behind `[cp].workflow_editor_ui = 'next'`)
- **`depends_on` rendering:** `deriveEdges` reads `depends_on` as inbound edges (§1d); draw still writes `next`; delete of a `depends_on`-derived edge clears the entry; YAML round-trips.
- **Loop rendering:** a loop draws as a **dashed group boundary** enclosing its member nodes, labelled with the loop name + an `until` / `max_iterations` badge; at runtime a live `iteration N/max` + converged/failed overlay (reuse the run-graph status overlay from the gate/action-node work).
- **Loop authoring:** select ≥2 nodes → a "Group into loop" action → a form for name / `until` / `max_iterations` / `on_max`. Editing/removing the loop group updates `loops:`. The inline validation surface (Phase 1) shows loop errors (overlap, cyclic sub-DAG, member-escapes) inline.
- Classic editor untouched; tokens only; no new dep.

## 6. Constraints & testing
- Backend: `#![deny(clippy::all)]`; new errors via `thiserror`; reuse `run_scheduler`/`run_node`/`workflow_edges`/Phase-2 resume — do NOT fork the scheduler. `IndexMap` for `loops` (ordered) is already a workspace dep (used elsewhere) — confirm before adding.
- Frontend: `next` path only; classic untouched; tokens; no new dep; `tsc` + vitest + build clean.
- **Tests:**
  - Rust language: `depends_on` parse + `skip_serializing_if`; `depends_on` edge in `workflow_edges`; `depends_on` cycle rejected; `loops` parse + validation (unknown node, overlap, <2 nodes, cyclic sub-DAG, max<1, member-escapes, collapsed-graph cycle); every `.rupu/workflows/*.yaml` still `is_nonlinear == false` and round-trips.
  - Rust runtime: a refine loop (`gen→test→critique`, `until` on critique) converges on iteration K and proceeds to `ship` with the converged output; the feedback ref (`gen` reads prior `critique`) reads the prior iteration's value (iteration 0 empty); exhaustion with `on_max: fail` fails with `LoopExhausted`; `on_max: proceed` continues with `converged=false`; `loop.<name>.iteration`/`converged` render; concurrency within an iteration (a `split` inside the loop) still fans out.
  - Resume: pause a run mid-loop (iteration 2, some members done) → resume re-enters iteration 2, re-runs only not-done members, converges, proceeds; a converged loop is not re-run.
  - Byte-equality: `dag_scheduler_golden` green; a `depends_on`-only linear workflow routes to `run_steps_inner`.
  - Editor: `depends_on` renders + round-trips; a loop group renders (dashed boundary + badge); "Group into loop" authors `loops:`; cyclic/overlap loop shows an inline error.
- **Operator gate (before merge, with Phases 1+2):** matt authors + runs a refine loop end to end — confirms it iterates, the feedback improves each pass, it converges + proceeds, exhaustion fails loudly (and `proceed` continues), and resume mid-loop re-enters correctly; `depends_on` authors an edge from the target side; **every existing workflow still runs identically**.

## 7. Build order (one PR, sequenced tasks — for the plan)
1. `depends_on` language + `workflow_edges` union + validation + editor render (small, self-contained; golden stays green).
2. `loops` language + validation (parse, `LoopDef`/`OnMax`, all §2f checks, collapsed-graph DAG check) — no runtime yet; `is_nonlinear` true for a loop workflow; a loop workflow rejected at runtime with a clear "loop execution lands in a later task" gate until task 3 (honesty).
3. Loop execution: the super-node in `run_scheduler` + the recursive per-iteration sub-scheduler + `until`/`on_max` + latest-wins state + `loop.*` template surface + the feedback mechanic.
4. Loop resume/checkpoint (`(loop, iteration, step)` keying + `RunRecord` loop counter + re-enter-at-iteration).
5. Editor loop authoring (dashed group boundary, badge, "Group into loop" action, runtime overlay) + `depends_on` delete-clears.

## 8. Decisions (locked, 2026-07-25)
- **Loop scope = subgraph iteration** (a sub-DAG re-runs together), not single-node retry.
- **`on_max = fail` (default), `proceed` escape hatch**; `loop.<name>.converged` exposed. Fail-loudly matches the no-silent-no-op ethos.
- **Top-level `loops:` map**; **latest-wins** per-iteration state + `loop.<name>.iteration`/`converged`; **v1 flat** (loops don't nest, members disjoint, ≥2 members).
- **`depends_on`**: additive predecessor edges; editor writes `next` on draw, reads/renders `depends_on`.
- **Out of scope (still deferred):** error edges, guarded edges, sub-workflows, subgraph-map, single-node loops, nested loops.
