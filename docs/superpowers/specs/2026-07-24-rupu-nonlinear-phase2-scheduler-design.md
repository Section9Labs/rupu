# Non-linear orchestration — Phase 2: the DAG scheduler (design)

**Date:** 2026-07-24
**Status:** Design — all decisions locked (§11). Ready to plan.
**Parent:** the proposal + Phase 1 (`2026-07-24-rupu-nonlinear-orchestration-proposal.md`, `…-phase1-design.md`). Grounded in the orchestrator investigation.
**Scope:** `crates/rupu-orchestrator` (runner + run-store + resume), and the small CP surface for approve/reject of a specific gate. Removes the Phase 1 honesty gate. **Existing linear workflows must produce byte-for-byte identical output.**

Phase 1 made non-linear workflows *authorable + valid*; Phase 2 makes them *execute*. This is the engine rewrite: the single-cursor linear loop becomes a ready-set scheduler with real concurrency, join barriers, branch pruning, per-node resume, and multi-gate approval.

## 1. What's there today (verified)

- **One cursor.** `run_steps_inner` is `for step in &opts.workflow.steps` in list order (`runner.rs:991`). `branch`/`when` only skip-mark; nothing jumps.
- **Real concurrency exists, but only inside one node.** `parallel`/`panel`/`for_each` fan out with `tokio::spawn` + `Semaphore(max_parallel)` + a join loop (`runner.rs:3168-3226`) — a self-contained fork/join. **This is the primitive Phase 2 promotes to workflow scope.**
- **Single-cursor state.** Resume rebuilds one `already_done` set from the flat `step_results.jsonl`, with at most one `paused_step` and one `RunRecord.awaiting_step_id: Option<String>` (`runs.rs:134`). No multi-node/multi-path state.
- **Executor boundary is clean.** `InProcessExecutor` spawns one task per *run*; the runner owns sequencing; `dispatch_one`/`StepFactory` runs one node; `EventSink` fans events. Phase 2 changes only the sequencing + resume, not the executor or dispatch boundary.

## 2. The scheduler (replaces the linear loop)

A ready-set scheduler inside `run_workflow`/`run_steps_inner`:

1. **Build the graph** from `workflow_edges` (control ∪ inferred data edges — Phase 1). Entry nodes = in-degree 0.
2. **Ready** = a node whose inbound dependencies are satisfied: a regular node when *all* predecessors are `done` (implicit all-join, D2); a `join` when its policy (`all`/`any`/`count`) is met; a `branch`'s taken-arm successors become eligible on branch completion; the untaken arm is **pruned** (never becomes ready).
3. **Dispatch** every ready node **concurrently** via the existing `dispatch_one`/`StepFactory` boundary (unchanged), bounded by a **workflow-scope `Semaphore`** — the `parallel`-step pattern promoted to the whole graph.
4. **On a node completing:** persist its result, mark `done`, recompute which successors are now ready, and dispatch them. A `branch` completing evaluates its condition and prunes the untaken subgraph (reachability). Loop until no ready/running nodes remain.
5. A `parallel`/`panel`/`for_each` container node stays exactly as it is internally (its own spawn+join); it's just one node in the outer graph now.

Legacy linear workflow → the graph is a chain → the scheduler dispatches one node at a time in order → **identical to today** (verified by the 15-sample output-equality tests).

## 3. Per-node state + resume (the hard part)

Generalize the single cursor to **per-node status**: `pending → ready → running → { done | failed | skipped(when) | pruned(branch) | awaiting_approval }`.

- **Persist:** keep `step_results.jsonl` (already keyed by `step_id`) as the source of `done`/`failed`/`skipped` on resume; a node not in it is not-yet-run. Branch **pruning** must also be persisted (which nodes the taken arms excluded) so resume doesn't re-run a pruned node — today's branch resume re-derives the skip-set from the branch's recorded `output` (`runner.rs:1005-1015`); generalize that.
- **Resume:** rebuild done/failed/pruned from disk, recompute the ready-set, re-run only not-done reachable nodes. Multiple nodes may have been in-flight at pause — none of those are `done`, so they simply re-run (idempotent per the current model). `completed_units` (per-fan-out-unit checkpoint) is unchanged.

## 4. Concurrency — driven by the graph, not an artificial cap (operator-locked)

**Concurrency is mandatory and structural:** a `split` with N targets **automatically spins up N concurrent jobs**, and in general every node whose dependencies are met runs concurrently with every other ready node. The scheduler spins as many concurrent jobs as the graph's width demands — it does not serialize a fork. An **optional** top-level `max_concurrency` may cap the total in-flight count (for resource protection), but it defaults to unbounded (run everything the graph makes ready); it must never cap *below* a split's fan-out in a way that serializes the fork. Per-container `max_parallel` still governs a `parallel`/`for_each` node's *internal* fan-out.

## 5. Join execution — one rule (operator-locked)

**A join waits for a number of inbound paths and passes ALL the results it waited for to the next step.** Merging is universal — there is no separate "merge" vs "wait" mode; the join's output is always the collected results of the paths it waited for (a list / keyed-by-source structure downstream steps read via `steps.<join>.results`).

- **`all`** (default) — ready when every inbound path is `done`; output = all N results merged.
- **`count: k`** — ready when k inbound paths are `done`; output = those k results merged; the **remaining in-flight paths are cancelled**.
- **`first`** (wire value `any`) — ready when the first inbound path is `done`; output = that 1 result; the **losing in-flight paths are cancelled**.

Cancellation of the not-waited-for paths (in `first`/`count`) uses the same per-node cancellation-token mechanism as §8's cancel: their running jobs are **stopped** the moment the join's threshold is met.

## 6. Branch pruning

When a `branch` completes, compute the reachable set from its *taken* arm and mark every node reachable *only* through the untaken arm as `pruned`. A node reachable through both a taken and an untaken path is NOT pruned (it still runs). This replaces the linear skip-set with real subgraph pruning, and it's what makes "two completely different paths for an if" work.

## 7. Approval in a DAG (a real model change — DP3)

Today a run has ONE `awaiting_step_id`. In a DAG, **several gates can be parked at once** (two concurrent paths each hitting a gate). So:
- `RunRecord.awaiting_step_id: Option<String>` → **`awaiting: Vec<AwaitingGate>`** (each with step id, prompt, expiry). Run status is `awaiting_approval` while any gate is parked.
- **Approve/reject targets a specific gate id** — unblocks that gate's path; other gates stay parked; the scheduler continues the approved path.
- The CP approve/reject API + the `cp serve` gate sweep (timeout routing) move from "the run's single awaiting step" to "a named gate." This is the largest ripple outside the runner.

## 8. Cancel + restart-from-checkpoint (operator-locked)

Cancellation must be **reachable and restart-aware**: a cancel **stops every currently-running job** (a cancellation token per dispatched node, wired through `dispatch_one` so an in-flight agent run is interrupted, not just left detached), then the run can be **restarted, resuming each job from wherever its state allows** — a node that checkpointed (e.g. a fan-out unit list, `completed_units`) picks up from its checkpoint; a node with no checkpoint restarts clean; a `done` node is not re-run. This same mechanism powers `first`/`count` join loser-cancellation (§5) and pause. **Pause** is the soft form (stop launching new ready nodes, let in-flight reach a safe boundary, persist), leaving the run resumable. The orphan-reaper (`reap_if_orphaned`) stays as the backstop for a dead runner.

## 9. Remove the Phase 1 honesty gate

Delete `is_nonlinear`'s use as a runner gate (`NonlinearNotYetSupported`) — the scheduler executes non-linear workflows now. Keep `is_nonlinear`/`workflow_edges` as pure helpers (the editor + tooling still use them). The topological-order safety the gate provided is now moot (the scheduler runs by dependency, not list order).

## 10. Compatibility & testing (non-negotiable)

- **Output equality:** every `.rupu/workflows/*.yaml` (all linear) produces byte-for-byte identical `step_results`/`events`/`run.json` under the scheduler as under the old loop. This is the primary safety test (golden/snapshot comparison against a baseline run).
- DAG execution: a fork/join runs both tracks and joins; `any` fires on first + handles losers per DP2; a branch prunes the untaken subgraph; a diamond runs the reconverge once.
- Resume: pause a mid-DAG run (some nodes done, some in-flight, one gate parked) → resume re-runs only not-done reachable nodes, honors the parked gate.
- Approval: two concurrent gates park; approving one continues its path while the other waits; reject routes per `on_reject`; timeout sweep targets the right gate.
- `#![deny(clippy::all)]`; no schema break for legacy run records (the `awaiting` field is additive with a migration from `awaiting_step_id`).

## 11. Decisions (operator-locked, 2026-07-24)

- **Concurrency is mandatory and structural** — no sequential stepping-stone. A `split` spins up N concurrent jobs automatically; the scheduler runs every ready node concurrently (§4). Phase 2 is one arc: the concurrent scheduler.
- **`join` config** — `wait: all | first | { count: k }`; **always merges/passes the results of the paths it waited for** to the next step (§5). `first`/`count` **cancel** the not-waited-for in-flight paths.
- **Cancel is restart-aware** — stops all running jobs; restart resumes each from its checkpoint where possible (§8). Same mechanism powers join loser-cancellation and pause.
- **Approval-set** — `RunRecord.awaiting_step_id` → an awaiting **set**; approve/reject by gate id (§7). Required now that concurrent paths can each park a gate; ripples into the CP approve/reject API + the gate sweep.
- **`max_concurrency`** — optional top-level cap, **default unbounded** (run everything the graph makes ready); must never serialize a split's fan-out.

### Internal build order (not user-facing phases — one PR, sequenced tasks)
1. Scheduler skeleton over a linear chain (output-equality vs the old loop) — proves the executor/dispatch boundary is preserved.
2. Concurrent dispatch + split (N live jobs) + implicit all-join + `max_concurrency`.
3. Branch pruning + explicit `join` (`all`/`first`/`count`) with merged results + loser-cancellation.
4. Per-node state + resume (cancel/restart-from-checkpoint).
5. Approval-set (`awaiting` list, approve/reject-by-gate-id, sweep) + remove the Phase 1 honesty gate.
