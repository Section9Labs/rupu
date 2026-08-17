# Finding: autoflow's agent-execution path

## Call chain

Two autoflow entry points exist, and both converge on the same execution
mechanism — there is no separate "entity engine" runtime distinct from the
tick loop; "issue autoflows" and "PR/event autoflows" are just two dispatch
branches inside one tick:

- `rupu autoflow tick` / `rupu autoflow serve` (CLI) call
  `autoflow_runtime::tick_with_resolver` /
  `autoflow_runtime::serve_with_resolver_and_hooks`
  (`crates/rupu-cli/src/cmd/autoflow.rs:1828`, `:1904`).
- `rupu cp serve`'s background reconcile loop calls the **same**
  `autoflow_runtime::tick_with_resolver`
  (`crates/rupu-cli/src/cmd/cp.rs:153`), per the comment at
  `crates/rupu-cli/src/cmd/cp.rs:134-141`: "periodically calls the SAME
  entrypoint `rupu autoflow tick` uses ... covering both issue and PR entity
  autoflows".

Inside one tick (`crates/rupu-cli/src/cmd/autoflow_runtime.rs`), the loop
resolves each claim to one of two dispatch calls, both into the `legacy`
module (`crate::cmd::autoflow` re-exported as `legacy` at
`autoflow_runtime.rs:1`):

- Issue-driven claims → `legacy::execute_autoflow_cycle(...)`
  (`autoflow_runtime.rs:401`, `:456`, `:509`).
- Pending wake/event dispatch (webhook/polled-event driven) →
  `legacy::execute_pending_dispatch_workflow(...)` (`autoflow_runtime.rs:417`).

Both bottom out in the same call:

1. `execute_autoflow_cycle` (`crates/rupu-cli/src/cmd/autoflow.rs:10633`)
   calls `run_with_explicit_context(&resolved.name, ExplicitWorkflowRunContext { .. })`
   at `autoflow.rs:10744`, `.await`-ed directly (no subprocess spawn).
2. `execute_pending_dispatch_workflow` (`autoflow.rs:11520`) does the same:
   `run_with_explicit_context(workflow_name, ExplicitWorkflowRunContext { .. })`
   at `autoflow.rs:11565`.
3. `run_with_explicit_context` (`crates/rupu-cli/src/cmd/workflow.rs:4215`)
   parses the workflow file and calls
   `execute_workflow_invocation(name, workflow, body, path, global, ctx).await`
   at `workflow.rs:4224`.
4. `execute_workflow_invocation` (`workflow.rs:4557`) is the **same function
   `rupu workflow run` / `rupu run` use** — it is also called from
   `workflow.rs:4110` and `workflow.rs:4181` with `invocation_source` set to
   `RunTriggerSource::WorkflowCli` / `EventDispatch` / `CronEvent`, versus
   `RunTriggerSource::Autoflow` from the autoflow call sites. It builds a
   `DefaultStepFactory` (`rupu-orchestrator/src/step_factory.rs:43`) in the
   current process (`workflow.rs:4667-4682`) and runs the workflow with either
   `tokio::spawn(run_workflow(opts))` (`workflow.rs:4762`) or
   `run_workflow_with_live_view(opts, ...)` (`workflow.rs:4752`) — both are
   in-process tokio tasks, not OS processes.
5. `run_workflow` (`crates/rupu-orchestrator/src/runner.rs:686`) executes
   workflow steps via the `DefaultStepFactory`, which for each agent step
   calls `provider_factory::build_for_provider_with_config(...)` directly
   (`crates/rupu-orchestrator/src/step_factory.rs:235`), per the comment at
   `step_factory.rs:200-204`: "the same path `rupu run` uses".

No `Command::new`, `tokio::process`, or `current_exe`-based re-invocation of
the `rupu` binary appears anywhere in this chain. The only `Command::new`
hits inside `autoflow.rs` are unrelated: `/bin/kill` for PID cleanup
(`autoflow.rs:2167`, `:2176`) and `git` plumbing for worktree diff stats
(`autoflow.rs:7144` etc.). `runner.rs` and `step_factory.rs` have zero
subprocess-spawn hits at all (checked via
`grep -n "Command::new\|tokio::process\|current_exe"`).

Separately, `cp serve`'s pause-delivery mechanism does spawn detached `rupu
workflow run <id>` subprocesses (referenced in a comment at
`crates/rupu-cli/src/cmd/workflow.rs:4705-4711`), but that is for *resuming a
paused run started elsewhere*, not for autoflow's initial dispatch — autoflow
always calls `execute_workflow_invocation` in the same process that is
running the tick.

## Verdict: in-process

Autoflow builds providers in-process, in the same OS process running the
tick (`rupu autoflow tick`/`serve`, or `rupu cp serve`'s reconcile loop). It
reaches `build_for_provider_with_config` through the identical
`execute_workflow_invocation` → `DefaultStepFactory` path that `rupu
workflow run` uses — same function, different `RunTriggerSource` tag
(`Autoflow` vs `WorkflowCli`/`EventDispatch`/`CronEvent`).

## Task 7 consequence

**Autoflow does not need its own sink-wiring call site.** Because autoflow
dispatch funnels through `execute_workflow_invocation`
(`crates/rupu-cli/src/cmd/workflow.rs:4557`) — the same function backing
`rupu workflow run` / `rupu run` — wiring the per-run netflow sink into that
one function (specifically into the `DefaultStepFactory` construction at
`workflow.rs:4667` and the `build_for_provider_with_config` call at
`rupu-orchestrator/src/step_factory.rs:235`) covers autoflow automatically.
There is no separate autoflow-only provider-construction path to wire.

## Ambiguity notes

- The codebase does *not* currently contain a distinct "entity engine"
  runtime as a separate execution mechanism — `rg`/`grep` for
  `entity_engine`/`EntityEngine`/"entity engine" across `crates/` returned no
  hits. The "two autoflow subsystems" referred to in prior session memory
  appears to describe the two *dispatch branches inside one tick*
  (issue-claim reconciliation via `execute_autoflow_cycle` vs.
  webhook/polled-event pending-dispatch via
  `execute_pending_dispatch_workflow`), not two separate runtimes — both are
  driven by the same `tick_with_resolver` call and both bottom out in the
  same `run_with_explicit_context` → `execute_workflow_invocation` chain
  traced above. If a genuinely separate execution path exists elsewhere
  (e.g. a not-yet-landed CP-native dispatcher), it was not found by this
  search; the CLI/`cp serve` paths above are the only ones this repo's
  current `crates/rupu-cli` and `crates/rupu-orchestrator` source contains.
