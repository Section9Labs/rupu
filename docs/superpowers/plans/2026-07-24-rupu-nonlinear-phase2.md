# Non-linear Orchestration — Phase 2 (DAG scheduler) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Execute non-linear workflows — a genuinely concurrent ready-set scheduler (a `split` spins N concurrent jobs), configurable `join` (`all`/`first`/`count`, always merging the waited-for results), branch pruning, per-node resume with cancel/restart-from-checkpoint, and multi-gate approval — while every existing linear workflow produces **byte-for-byte identical** results.

**Architecture:** Replace `run_steps_inner`'s `for step in &workflow.steps` driver with a ready-set scheduler that reuses the existing per-node dispatch (panel/parallel/for_each/action/linear/branch, +split/join) via an extracted `run_node`, and reuses the `parallel`-step's `tokio::spawn`+`Semaphore`+join primitive promoted to workflow scope. The executor boundary (one task per run, `dispatch_one`/`StepFactory`, `EventSink`) is unchanged.

**Tech Stack:** Rust 2021, `tokio`, `thiserror`; `cargo test -p rupu-orchestrator`.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-24-rupu-nonlinear-phase2-scheduler-design.md` (all decisions locked in §11). Branch: `nonlinear-phase2`, stacked on `nonlinear-phase1`.
- **Byte-for-byte output equality for legacy workflows is the primary safety invariant.** Every `.rupu/workflows/*.yaml` (all linear) must produce identical `step_results.jsonl` / `events.jsonl` / `run.json` under the scheduler as under the old loop. A golden-comparison test guards every task.
- **The executor/dispatch boundary does not change** — `dispatch_one`/`StepFactory`/`EventSink`/the per-node `run_*_step` functions are reused, not rewritten.
- `#![deny(clippy::all)]`; `thiserror`. `cargo test -p rupu-orchestrator`. The 4 pre-existing `tests/linear_runner.rs` mock-provider flaky failures are baseline (verify against baseline, don't chase).
- Concurrency is structural (a split → N live jobs); `max_concurrency` optional, default unbounded, must never serialize a fork. `join` always passes the merged results of the paths it waited for. Cancel stops all jobs + restarts from checkpoint. Approval → an awaiting set.

## Build order (sequenced tasks — one PR)

### Task 1: Extract `run_node`; scheduler skeleton over a linear chain (output-equality)

**Files:** `crates/rupu-orchestrator/src/runner.rs`; tests in-crate.

**Interfaces:** Produces `async fn run_node(step, ctx-deps…) -> Result<NodeOutcome, RunWorkflowError>` where `NodeOutcome` is `{ Completed(StepResult), Paused{…} }` (the existing per-step dispatch match, lifted verbatim). And a `run_scheduler(...)` driver that, over a purely linear workflow, reproduces `run_steps_inner`'s behavior exactly.

- [ ] **Step 1: Golden-equality test first.** Write a test that runs each `.rupu/workflows/*.yaml` (all linear) through the CURRENT `run_steps_inner` and captures `step_results` (ids, outputs, success, skipped, kind, order). This is the golden baseline the scheduler must reproduce. (Use the crate's MockProvider harness — deterministic agent outputs.)
- [ ] **Step 2:** Confirm it passes against the current loop (baseline capture).
- [ ] **Step 3: Extract `run_node`.** Lift the per-step dispatch match (`runner.rs:1333-1463`, the `if step.panel.is_some() … else if … else {linear}` block that produces a `StepResult`/`InnerOutcome::Paused`) into `async fn run_node(...)` returning `NodeOutcome`. Do NOT change what any arm does — pure extraction. `run_steps_inner`'s loop now calls `run_node` for the current step. Run the golden test — still identical (this is a behavior-preserving refactor).
- [ ] **Step 4: Add `run_scheduler` for the linear case.** A driver that maintains a ready-set (initially graph entry nodes) and, for a linear chain, dispatches nodes one at a time in dependency order — calling `run_node`, persisting, marking done, readying successors — reproducing the loop. Gate it behind a `use_scheduler` path but assert (via the golden test) it produces the identical `step_results` as the old loop for every sample. Keep the old loop in place; the scheduler is proven equal before it replaces anything.
- [ ] **Step 5: Commit** `feat(orch): extract run_node + linear-equivalent scheduler skeleton`.

*(This task lands NO behavior change — it proves the extraction + the ready-set driver reproduce the linear runner byte-for-byte. Everything after builds on a proven-equal base.)*

### Task 2: Concurrent dispatch + `split` + implicit all-join + `max_concurrency`

**Interfaces:** the scheduler dispatches every ready node concurrently (`tokio::spawn` + a workflow-scope `Semaphore`, reusing the parallel-step pattern); a `split` node makes all its targets ready at once (N concurrent jobs); a regular node with multiple inbound edges runs once, after all inbound `done` (implicit all-join). `max_concurrency` (optional top-level, default unbounded) caps in-flight count without serializing a split.

- [ ] Tests: a `split → a,b → (regular reconverge d)` workflow runs a and b **concurrently** (assert overlap via a barrier/timestamp in the mock), d runs once after both. `max_concurrency: 1` serializes but still completes. Legacy golden equality still holds (a linear chain has width 1 → sequential). Split executes as a no-op orchestration node (no agent), just readying its targets.
- [ ] Implement, test, commit `feat(orch): concurrent scheduler + split fan-out + implicit all-join`.

### Task 3: Branch pruning + explicit `join` (all/first/count, merged, loser-cancel)

**Interfaces:** a `branch` completing prunes the subgraph reachable only through the untaken arm (persisted so resume honors it). A `join` node evaluates its `wait` policy over inbound edges; its output is the **merged results of the waited-for paths** (`steps.<join>.results` = the collected inbound outputs). `first`/`count` **cancel** the not-waited-for in-flight paths (cancellation token per dispatched node).

- [ ] Tests: `join: { wait: all }` merges both inbound outputs into its result; `first` proceeds on the first, cancels the other (assert the loser's job was cancelled, not completed); `count: 2` over 3 inbound proceeds on 2, merges those 2, cancels the 3rd; branch prunes the untaken arm (the pruned node is never dispatched); a node reachable via both a taken and untaken path still runs.
- [ ] Implement, test, commit `feat(orch): branch pruning + join (all/first/count) with merged results + loser cancellation`.

### Task 4: Per-node state + resume (cancel/restart-from-checkpoint)

**Interfaces:** per-node status persisted (done/failed/skipped/pruned via `step_results.jsonl` + the branch-prune record); resume rebuilds the ready-set and re-runs only not-done reachable nodes; cancel stops all in-flight jobs and a subsequent restart resumes each from its checkpoint where possible (reusing `completed_units` for fan-out nodes), clean otherwise.

- [ ] Tests: pause a mid-DAG run (some nodes done, some in-flight) → resume re-runs only not-done reachable nodes and completes; cancel stops all jobs (assert cancellation reached the in-flight agent run, not just detached); restart-after-cancel resumes a checkpointed fan-out from its `completed_units`; a pruned node stays pruned across resume.
- [ ] Implement, test, commit `feat(orch): per-node resume + cancel/restart-from-checkpoint`.

### Task 5: Approval-set + remove the Phase 1 honesty gate

**Interfaces:** `RunRecord.awaiting_step_id: Option<String>` → `awaiting: Vec<AwaitingGate>` (additive migration; a legacy record with the old single field maps to a one-element set). Approve/reject targets a gate id; the CP approve/reject API + the `cp serve` gate sweep move to per-gate. The `is_nonlinear`→`NonlinearNotYetSupported` runner gate is removed (the scheduler executes non-linear now); `is_nonlinear`/`workflow_edges` stay as helpers.

- [ ] Tests: two concurrent gates park → run is `awaiting_approval` with both in `awaiting`; approving gate A continues A's path while B stays parked; reject routes `on_reject`; the timeout sweep fires the right gate; a legacy run record with `awaiting_step_id` still resolves (migration); a `split`/`join` workflow now RUNS (no `NonlinearNotYetSupported`); every legacy sample still golden-equal.
- [ ] Implement, test, commit `feat(orch): multi-gate approval set + remove the Phase 1 non-linear gate`.

## Operator gate (before merge, with Phase 1)
matt runs a real fork/join workflow end to end: split → concurrent tracks → join (try `all`, `first`, `count`), confirm concurrency + merged results downstream; branch prunes the untaken path; cancel stops all jobs and restart resumes; two concurrent gates each approvable independently; **every existing workflow still runs identically** (the primary risk).

## Self-review notes
- Task 1 is a behavior-preserving extraction + a proven-equal scheduler — nothing downstream is trusted until legacy golden-equality holds, and every task re-runs it.
- Decisions map: concurrency/split → T2; join semantics + branch pruning → T3; resume/cancel → T4; approval-set + gate removal → T5. All per spec §11.
