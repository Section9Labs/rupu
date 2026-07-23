# Gate Nodes & Action Steps — Plan 4: Notify Hooks + cp-serve Gate Sweep

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Final PR ④ of the arc (`docs/superpowers/specs/2026-07-23-rupu-workflow-gate-and-action-nodes-design.md` §4.1 notify + timeout). Gate `notify:` hooks fire when a gate parks; a `rupu cp serve` background **gate sweep** fires `on_timeout` routing unattended (executing the web-orphaned reject cleanup) AND reaps runs whose runner process died — closing the "runs spin forever" class for good.

**Architecture:** notify hooks fire *inside* the gate block in `run_workflow` (the `action_dispatcher` is already threaded onto `OrchestratorRunOpts` from Plan 2, and `execute_action_step` is state-free) — synchronous, best-effort, before the run parks; works identically for CLI foreground and detached cp-serve subprocess runs. The gate sweep is a new `run_periodic_tick` task in `cp.rs` that reuses the existing `expire_if_overdue` contract + `build_reject_cleanup_opts`/`run_reject_cleanup` (same code the CLI approve/reject paths call) — no opts-builder duplication. Orphan reaping needs one new `RunStore` method that finalizes a dead-pid Running run as Failed with a store-appended terminal event.

**Tech Stack:** Rust (rupu-orchestrator runner + runs store, rupu-cli cp.rs tokio tasks, rupu-config). No web changes.

## Global Constraints

- Workspace deps only; thiserror (lib) / anyhow (CLI); `#![deny(clippy::all)]`; never package-wide cargo fmt.
- **No new executor `Event` variants; no new `StepKind` variants.** Every store-side terminal transition MUST append its terminal event via `append_terminal_event` (runs.rs:1448) — no runner is alive to emit it, and the Situation Room folds the newest event (this is the exact bug PR #501 fixed; don't reintroduce it).
- notify + sweep are **best-effort and fail-closed**: a notify failure never blocks the pause; a sweep error on one run never aborts the sweep or the other runs; but nothing silently no-ops — every skip/failure is logged.
- New `[cp]` config flags follow the existing `#[serde(default = ...)]` + `Default`-impl idiom exactly (the struct is `deny_unknown_fields` — a field missing its default breaks older configs).
- Baseline: 4 `linear_runner.rs` flakes + rupu-cli ANSI/session redness + rupu-mcp `schema_snapshot` drift + `host/ssh.rs` clippy (1.95 toolchain) are all pre-existing; don't chase.
- Line refs are v0.66.1 main (`c862e1d7`); re-locate by quoted code if drifted.

---

### Task 1: Gate `notify:` hooks fire when a gate parks

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (gate block ~line 1129-1181 — the `is_approval_gate` NODE block; insert notify before the `StepAwaitingApproval` emit ~1163)
- Test: `crates/rupu-orchestrator/tests/gate_node.rs` (extend)

**Interfaces:**
- Consumes: `Approval.notify: Vec<NotifyAction>` (`NotifyAction { action: String, with: serde_json::Value }`, workflow.rs:736), `opts.action_dispatcher: Option<Arc<ToolDispatcher>>` (runner.rs:286), `execute_action_step` (runner.rs:1656, state-free), `render_action_args`, the in-scope `ctx`/`render_mode`/`opts.transcript_dir`.
- Produces: a `fire_notify_hooks(opts, step_id, notify: &[NotifyAction], ctx, mode)` helper (or an inline loop) that, for each notify entry, synthesizes a throwaway `Step { id: format!("{step_id}.notify"), action: Some(n.action.clone()), with: Some(n.with.clone()), ..Default::default() }` and calls `execute_action_step(dispatcher, &synth, ctx, mode, /*continue_on_error*/ true, opts.transcript_dir...)` — matching execute_action_step's REAL current signature (read it: recon shows `(dispatcher, step, ctx, mode, continue_on_error) -> Result<StepResult, RunWorkflowError>` possibly with a transcript arg; match exactly). Errors are logged (`tracing::warn!`) and swallowed. Guarded: `if let Some(d) = opts.action_dispatcher.as_ref()` — else `tracing::warn!` "notify skipped: no action dispatcher" and continue. Fires ONLY on the actual-park path (after `auto_approve` resolves falsy / is absent), NOT when the gate auto-approves or is resume-suppressed.

- [x] **Step 1: Failing test** (extend `gate_node.rs`, reuse the fake-connector + dispatcher harness from `action_step.rs`)

```rust
// notify fires on park: a gate with notify: [{ action: issues.comment, with: {..} }]
// and auto_approve absent → when the run parks AwaitingApproval, the fake
// connector recorded ONE comment call with the rendered body; the run is
// still AwaitingApproval (notify didn't change the outcome).
//
// notify does NOT fire on auto-approve: same gate but auto_approve "true" →
// run completes, connector recorded ZERO calls.
//
// notify failure doesn't block the park: fake connector errors on the notify
// call → run still parks AwaitingApproval (best-effort).
```

- [x] **Step 2: RED** — `cargo test -p rupu-orchestrator --test gate_node notify 2>&1 | tail -5` (notify never fires today).
- [x] **Step 3: Implement** the guarded best-effort loop in the gate block's park path. Read the gate block fully first — the `auto_approve` check, the `gate_suppressed` (resume) check, and the `StepAwaitingApproval` emit are all there; notify goes after the auto-approve/suppress early-exits and before (or right at) the awaiting emit.
- [x] **Step 4: GREEN** — `cargo test -p rupu-orchestrator` (gate_node + lib green; 4 flakes excepted), `cargo build --workspace`.
- [x] **Step 5: Commit** — `feat(orchestrator): gate notify hooks fire best-effort when a gate parks`

---

### Task 2: `RunStore` — finalize an orphaned Running run as Failed

**Files:**
- Modify: `crates/rupu-orchestrator/src/runs.rs` (new method near `cancel` ~1462 / `expire_if_overdue` ~910)
- Test: `crates/rupu-orchestrator/src/runs.rs` in-file `mod tests` (near `expire_if_overdue_appends_terminal_event`)

**Interfaces:**
- Consumes: `pid_is_running(pid) -> bool` (runs.rs:1695), `RunStatus`, `append_terminal_event` (runs.rs:1448), the `TimeoutAction::Fail` arm's field mutations (runs.rs:934-954) as the template.
- Produces: `pub fn reap_if_orphaned(&self, record: &mut RunRecord, now: DateTime<Utc>) -> Result<bool, RunStoreError>` — returns `Ok(true)` and finalizes when `record.status` is `Running` (and/or `Pending`) AND `record.runner_pid` is `Some(pid)` AND `!pid_is_running(pid)`; else `Ok(false)`. Finalization: status → `Failed`, `finished_at = Some(now)`, `error_message = Some("runner process <pid> is no longer alive; run marked failed by the gate sweep")`, clear `runner_pid`/active-step fields (mirror cancel's field clears), `self.update(record)?`, then `append_terminal_event(&record.id, &Event::RunFailed { run_id, error, finished_at: now })`. A run with `runner_pid: None` in Running state (e.g. legitimately mid-handoff) is NOT reaped (returns false) — only a *dead recorded pid* is an orphan, to avoid racing a run between spawn and pid-write.

- [x] **Step 1: Failing tests**

```rust
// reap_if_orphaned: Running + runner_pid = a definitely-dead pid (e.g. use
//   a pid we know is free — spawn+reap a child, or pid 2^31-1) → returns true,
//   status Failed, RunFailed appended as the last events.jsonl line, error
//   mentions the pid.
// NOT reaped: Running + runner_pid = std::process::id() (alive, self) → false,
//   status unchanged.
// NOT reaped: Running + runner_pid = None → false (mid-handoff guard).
// NOT reaped: terminal run (Completed) → false.
```
(For the dead-pid case: the existing tests use a known-free pid; check how `cancel_running_*` tests fabricate pids and reuse that approach — do NOT actually kill anything.)

- [x] **Step 2: RED**, **Step 3: implement**, **Step 4: GREEN** — `cargo test -p rupu-orchestrator --lib reap 2>&1 | tail`.
- [x] **Step 5: Commit** — `feat(orchestrator): RunStore::reap_if_orphaned finalizes dead-pid runs as Failed`

---

### Task 3: cp-serve gate sweep — timeout routing, web-reject cleanup, orphan reaping

**Files:**
- Modify: `crates/rupu-config/src/policy_config.rs` (`CpConfig` — add `gate_sweep_enabled` + `gate_sweep_interval_secs` following the `cron_tick_*` pair at :36-41, and the `Default` impl at :72-84), `crates/rupu-cli/src/cmd/cp.rs` (spawn a `run_periodic_tick("gate-sweep", ...)` alongside cron at ~:124; await it at ~:198), `crates/rupu-orchestrator/src/runs.rs` (update the `request_resume_approval` `TimeoutAction::Reject` orphaned-cleanup comment at ~:1281 — the sweep now owns it, so reword from "orphaned until the sweep lands" to "executed by the cp-serve gate sweep")
- Test: a unit test of the sweep's per-run decision (extract a pure classifier), + a config parse test for the new flags
- Config test: `crates/rupu-config/tests/parse.rs`

**Interfaces:**
- Consumes: `RunStore::{list, expire_if_overdue, reap_if_orphaned}`, the private `gate_on_timeout` resolver (via a public entry — either add a thin `pub fn resolve_gate_timeout(&self, record) -> Option<TimeoutAction>` on RunStore that calls the existing private `gate_on_timeout`, OR replicate via public `read_workflow_snapshot` + `workflow::gate_timeout_action`), `crate::resume::build_reject_cleanup_opts` + `rupu_orchestrator::runner::run_reject_cleanup` (both reachable from cp.rs — same crate), `crate::resume::action_dispatcher_for` (for the approve-resume path if the sweep resumes in-process; simpler: the sweep spawns `rupu workflow approve <id>` like the resume worker does — mirror `run_resume_worker`'s detached-subprocess act at cp.rs:470 for the Approve case).
- Produces: `run_gate_sweep(store, hosts, now)` tick body. Per tick: `store.list()`, then for each run:
  - **AwaitingApproval** with a gate `on_timeout`: call `expire_if_overdue(&mut rec, now, on_timeout)`. On `Ok(Some(TimeoutAction::Reject))` → the run is now `Rejected`; immediately run cleanup: `if cheap_on_reject_chain_len(...) != Some(0) { build_reject_cleanup_opts(...).await → run_reject_cleanup(opts, step_id, reason, "timeout").await }` (the exact CLI pattern from workflow.rs:2223). On `Ok(Some(TimeoutAction::Approve))` → spawn a detached `rupu workflow approve <id>` (mirror resume worker). On `Ok(Some(Fail))`/`Ok(None)` → nothing (Fail already finalized inside expire).
  - **Running/Pending** with a dead recorded pid: skip if the run is owned by a remote host (mirror the resume worker's `remote_workers` guard, cp.rs:422-432 — a dead *local* pid check is meaningless for a run whose runner lives on another host), else `store.reap_if_orphaned(&mut rec, now)`.
  - Every branch: log what it did; per-run errors are logged and swallowed (`continue`), never abort the sweep.
- Extract the *decision* (not the IO) into a testable pure fn where practical, e.g. `fn sweep_decision(status: RunStatus, on_timeout: Option<TimeoutAction>, pid_alive: Option<bool>, is_remote: bool) -> SweepAction` where `SweepAction ∈ {Skip, ExpireThenCleanupReject, ExpireApprove, Reap}` — unit-test its truth table; the tick body maps `SweepAction` to the IO calls above.

- [x] **Step 1: Failing tests** — (a) `CpConfig` parse test: `gate_sweep_enabled` defaults true, `gate_sweep_interval_secs` defaults 60, both override from TOML (mirror the cron_tick test in `crates/rupu-config/tests/parse.rs`). (b) `sweep_decision` truth table: AwaitingApproval+Reject→ExpireThenCleanupReject; +Approve→ExpireApprove; +Fail/None→Skip; Running+dead-local-pid→Reap; Running+dead-pid-but-remote→Skip; Running+alive→Skip; terminal→Skip.
- [x] **Step 2: RED**, **Step 3: implement** (config flags + `sweep_decision` + the tick body wired via `run_periodic_tick` + the comment reword), **Step 4: GREEN** — `cargo test -p rupu-config`, `cargo test -p rupu-orchestrator`, `cargo build -p rupu-cli` (the sweep compiles + wires; the IO tick body is exercised manually in Task 4).
- [x] **Step 5: Commit** — `feat(cli): cp-serve gate sweep — timeout routing, web-reject cleanup, orphan reaping`

---

### Task 4: Sample + manual sweep verification + docs + memory

**Files:**
- Modify: `.rupu/workflows/gate-demo.yaml` (add an `on_timeout` + a `notify:` hook using a READ tool so dogfooding never writes unexpectedly — e.g. `notify: [{ action: scm.prs.list, with: { owner: Section9Labs, repo: rupu, state: open } }]`; verify keys), `CLAUDE.md` (note notify + gate sweep; the [cp] flags), `docs/.../plan-4` tick "Deferred" done
- Manual verification script (documented in the report, not committed): drive a real gate with a short `timeout_seconds` + `on_timeout: reject` + an `on_reject` step, start `rupu cp serve`, confirm the sweep expires + runs cleanup + the run ends Rejected with the cleanup step's result AND events.jsonl ends terminal.

- [x] **Step 1:** Update the sample; `cargo run -p rupu-cli -- workflow show gate-demo --view full` parses clean.
- [x] **Step 2: Manual sweep smoke** (document commands + observed outcome in the report): a gate with `timeout_seconds: 5, on_timeout: reject, on_reject: [<agent step>]`; `rupu workflow run` it to the park, then run `rupu cp serve` (or invoke the sweep tick directly if a test hook exists) and confirm after the interval: run.json status Rejected, the on_reject step_result persisted, `events.jsonl` last line is `run_completed`(rejected). Also fabricate an orphan (a Running run.json with a dead runner_pid) and confirm the sweep reaps it to Failed. If running the full daemon is impractical in the harness, invoke `run_gate_sweep`'s tick body once via a small integration test instead and say so.
- [x] **Step 3: Full verification** — `cargo test -p rupu-orchestrator -p rupu-config -p rupu-cli` (baseline flakes/redness excepted — compare, don't chase), `cargo build --workspace`, `cargo clippy -p rupu-orchestrator -p rupu-cli 2>&1 | grep -c "^error"` (report count; host/ssh.rs toolchain artifact excepted).
- [x] **Step 4: Update memory** — append to the arc memory (`project_gate_action_nodes_arc.md`) + the run-state-closure memory (`project_run_state_closure.md`): the orphan reaper is now BUILT (was the open follow-up), and the web-path timeout-reject cleanup is no longer orphaned.
- [x] **Step 5: Commit** — `feat(cli): gate-demo notify + on_timeout sample; sweep docs + memory`

---

## Arc complete after this plan
All four PRs merged = the full spec §4-5 shipped: gate nodes (schema/runtime/renderers/editor) + action steps (schema/execution/renderers/editor) + notify + unattended timeout routing + orphan reaping. Remaining genuinely-optional follow-ups (NOT blocking arc completion, file as issues): `[scm.default]` config wiring in `Registry::default_platform` (still the v0 first-registered fallback); Read/Write badge on ActionNode; gate `notify:` form editor in the web editor (round-trips verbatim today); unattended-cleanup permission-mode review (cleanup currently runs in the run's mode); full auto-synthesized phantom gate node for legacy inline approvals (Plan 3 shipped the dashed badge + convert button instead).
