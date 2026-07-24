# Non-linear orchestration — Phase 1: language + editor (design)

**Date:** 2026-07-24
**Status:** Design; pending operator review.
**Parent:** `2026-07-24-rupu-nonlinear-orchestration-proposal.md` (decisions D1-D5 recorded there).
**Scope:** the workflow **language** (`crates/rupu-orchestrator/src/workflow.rs` — parse + validate + a graph model, **no scheduler**) and the **editor** (`crates/rupu-cp/web`, behind `[cp].workflow_editor_ui = 'next'`).
**Explicitly out of scope (Phase 2/3):** the DAG scheduler, per-node resume, `depends_on:`, bounded loops.

Phase 1 lets you **design, draw, and validate** non-linear workflows and get the authoring UX right. It does **not** execute them: per "no silent no-ops," the runner rejects a non-linear workflow at run time with a clear error until Phase 2 (§6).

## 1. The language additions

### 1a. `next:` — explicit successor edges
Add to `Step` (`workflow.rs`): `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub next: Vec<String>`. Each id is a successor. This is the explicit edge. Absent `next` on a step in a **legacy** (edge-free) workflow keeps meaning "the next step in the list" (§4); in a **graph** workflow it means "terminal (no successor)."

### 1b. `split` — the fork orchestration node
A step carrying `split: [ids]` and **no** agent/action/for_each/parallel/panel/branch/approval. It is an orchestration node: it fans the flow into N **independent concurrent tracks** (the ids). `split: [a, b, c]` is the fork made visible; semantically equivalent to a plain node with `next: [a, b, c]`, but it reads as "the flow splits here" and carries no work. (Distinct from `parallel`, which is one node running N agents on one subject and collapsing them — that stays exactly as it is.)

### 1c. `join` — the barrier orchestration node
A step carrying `join: { wait: <policy> }` and no agent/etc. Its **inbound set is derived from the edges pointing at it** (nodes whose `next`/`split`/branch-arm includes this id) — consistent with the successor model (D1: `next` first; predecessor `depends_on` is Phase 3). Its `next` is where flow continues after the barrier. Policy (D2):
- `wait: all` (default) — every inbound path must finish.
- `wait: any` — first inbound path to finish; the rest are cancelled.
- `wait: { count: k }` — k-of-n inbound paths.

A **regular** node with several inbound edges implicitly waits for **all** of them (D2) — so a plain reconverge needs no `join` node; `join` is for `any`/`count` or a barrier you want visible.

### 1d. `branch` — unchanged shape, now a real edge in the model
`branch: { condition, then, else }` keeps its exact YAML shape. In the **graph model** (validation + editor), `then`/`else` are conditional successor edges. Its runtime semantics don't change in Phase 1 (still the linear skip-set — see §6); Phase 2 makes the untaken subgraph a real prune. A `branch` step may not also carry `next` (its successors are its arms) — validation error if both.

### 1e. Data-edge inference (D3)
The dependency graph a validator/editor builds is the **union** of:
1. explicit control edges — `next`, `split`, branch `then`/`else`;
2. **inferred data edges** — if a step's templated fields reference `{{ steps.X.output }}`, add an edge `X → thisStep`. The language already tracks these refs (`validate_template_refs`, `workflow.rs:1596`) — Phase 1 promotes them from a forward-only lint to real edges. An explicit `next` edge always overrides/augments; inference only *adds* ordering, never removes an authored edge.

## 2. Validation (the real work of the Rust side)

Build the dependency graph, then:
- **DAG / cycle check.** A genuine topological sort; reject a cycle with the offending id path (`WorkflowCycle`). This replaces "cycles are impossible by construction" — with explicit edges they're possible, so they must be *detected*. (Loops are Phase 3; Phase 1 rejects all cycles.)
- **Edge targets exist** — every `next`/`split`/branch-arm/`join`-derived id names a real step; unknown → `EdgeTargetUnknown`. No self-edges.
- **Node-shape rules** — `split` and `join` are mutually exclusive with agent/action/for_each/parallel/panel/branch/approval (they're orchestration-only), mirroring the existing `validate_step_shape` mutual-exclusion style. `join.wait` count ≥ 1 and ≤ its inbound degree.
- **Reachability** — warn on a node unreachable from any entry (no inbound and not an entry), and on a `split`/`join` with degree < 2 (a pointless fork/barrier).
- **Legacy forward-only checks stay** for edge-free workflows; graph workflows use the DAG check instead.

## 3. The editor

### 3a. Edges become authored, not derived-from-order
`deriveEdges(nodes)` (from the earlier single-source work) **flips its source**: instead of "consecutive array order + branch arms + data refs," it becomes **"explicit `next` + `split` + branch `then`/`else` + inferred `steps.X` data edges."** The array order stops implying connection.

### 3b. Drawing, replacing, clearing (your asks)
- **Drop a node → it connects to nothing** (empty `next`). No auto-link to a neighbour.
- **Draw a line A → B → sets the edge.** On a **regular** work node (single control successor), a new line **replaces** the existing one (your "draw replaces the old"). On a **`split`** node, lines accumulate (that's its fan-out). Branch arms attach to the then/else handles as today.
- **Delete a line → clears that `next` entry.**
- **Edit in the YAML pane** — point a step at another by typing `next: [id]`; the canvas reconciles (already wired).

### 3c. New nodes + shapes
- `split` and `join` join the palette as **orchestration** nodes (the work/orchestration split from the proposal taxonomy — group them in the Blocks tab).
- Shapes: they get `KIND_SHAPE` entries and silhouettes consistent with the shape system (a fork/merge glyph — I'll bring you 2-3 silhouette options the way we did for the branch, since these are new and you'll want to approve them). `StepForm` bodies: `split` = its target list (or draw); `join` = the `wait` policy picker (all / any / count-k).
- The vhex branch and the rest render unchanged.

### 3d. Migration
A legacy (edge-free) workflow loads with the implicit-order interpretation; on the **first graph edit** the editor **materialises the implicit chain into explicit `next`** (each step → the next in the list) and from then on authors edges explicitly. One-way, lossless, and consistent with how the editor already re-emits YAML. Round-trip tested.

## 4. Legacy compatibility (non-negotiable)
- A workflow with **no** `next`/`split`/`join` parses and runs **exactly as today** — implicit list order, current branch skip behaviour. Byte-for-byte.
- `next`/`split`/`join` are all `#[serde(default, skip_serializing_if…)]`, so they never appear in a legacy workflow's serialized form.
- The `is_nonlinear(workflow)` predicate (§6) is false for every current `.rupu/workflows/*.yaml` (they have no edges) — verified by test.

## 5. The work/orchestration taxonomy (surfaced, not just internal)
Formalise the two families in `kindVisuals`/the palette: **work** (`step`, `action`, `for_each`, `parallel`, `panel`) and **orchestration** (`branch`, `split`, `join`, `approval_gate`). Purely a grouping + labeling change in Phase 1 (no behaviour), but it makes the language legible and is where `split`/`join` live in the Blocks tab.

## 6. The runtime gate (honesty, per "no silent no-ops")
Add `is_nonlinear(&Workflow) -> bool`: true iff the workflow uses `split`/`join`, or its explicit edges form anything other than the trivial linear chain the current runner already executes (a fork — a node with >1 non-branch successor — or a reconverge — a node with >1 inbound). When `run_workflow` is asked to execute a workflow where `is_nonlinear` is true, it **returns a clear error** (`NonlinearNotYetSupported { … }`, surfaced in the CLI and CP) — it never falls back to the linear loop and mis-runs it. Legacy/linear workflows (including today's branches) run unchanged. This gate is deleted in Phase 2 when the scheduler lands.

## 7. Constraints & testing
- Backend: `#![deny(clippy::all)]`; new errors via `thiserror`; no runtime scheduler in Phase 1.
- Frontend: `next` path only; classic untouched; tokens only; no new dep.
- Tests:
  - Rust: `next`/`split`/`join` parse + `skip_serializing_if` (absent from legacy output); DAG/cycle detection (a cycle is rejected with its path); edge-target + shape validation; **every `.rupu/workflows/*.yaml` is `is_nonlinear == false`** and round-trips unchanged; `run_workflow` on a `split`/`join`/fork workflow returns `NonlinearNotYetSupported` (and a linear-with-explicit-`next` workflow still runs).
  - Editor: `deriveEdges` from explicit edges (a fork draws two lines; a dropped node draws none); draw-sets-`next` / draw-replaces-on-regular / accumulates-on-split; delete clears; legacy→explicit migration round-trips; `split`/`join` author + validate; cycle shows an inline error.
- Operator gate: matt authors a fork/join graph in the editor (light + dark), confirms drop-is-disconnected and draw-replaces, and confirms running a non-linear workflow gives the clear Phase-2 error rather than mis-running.

## 8. Open sub-decision for you
- **Split/join shapes** — new silhouettes; I'll bring 2-3 options to approve (like the branch), or we ship a simple placeholder now and refine later. Which?
