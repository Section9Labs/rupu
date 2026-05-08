# rupu Autoflow Plan 1 — Foundation, Runtime, and CLI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `autoflow` as a first-class, persistent execution mode for existing workflow YAML files. The feature must keep one workflow language, add durable issue ownership across multiple runs, reuse the existing workflow engine, and ship with a top-level `rupu autoflow ...` CLI surface.

**Spec:** [docs/superpowers/specs/2026-05-08-rupu-autoflow-design.md](../specs/2026-05-08-rupu-autoflow-design.md)

**Locked decisions from design review:**
- One YAML language: workflow files gain optional `autoflow:` and `contracts:` blocks.
- Autoflow execution is exposed as `rupu autoflow ...`, not `rupu workflow auto ...`.
- Child dispatch requested by an autoflow outcome is persisted and picked up on the **next** tick, not executed inline.
- Repo-to-local-path tracking lives under `rupu repos ...`.
- Autoflow structured outputs must validate against declared JSON Schema contracts.
- V1 supports both controller-style and directly-autonomous phase workflows, but only **one active claim per issue** may exist.
- `claim.ttl` stays in v1 and governs the logical lease duration for issue ownership.
- `autoflow.priority` resolves multiple matches; higher wins, then workflow name.

**Architecture:**
- Extend `Workflow` parsing with `autoflow:` and `contracts:` metadata.
- Extend layered config with `[autoflow]` operational defaults.
- Add contract loading and JSON Schema validation from `.rupu/contracts/`.
- Add a machine-local repo registry under `~/.rupu/repos/` and manage it through `rupu repos ...`.
- Add an autoflow claim store under `~/.rupu/autoflows/claims/` with explicit lease metadata and active-cycle lock files.
- Add a dedicated internal autoflow runtime entrypoint that feeds explicit `project_root`, `workspace_path`, `issue`, `event`, and `run_id` into the existing workflow runner.
- Add a new top-level CLI family: `rupu autoflow list/show/run/tick/status/claims/release`.
- Reuse the existing event-ingestion infrastructure (`[triggers].poll_sources`, `EventConnector`, `rupu webhook serve`) as autoflow wake sources.

**Tech stack:** Rust 2021, existing workspace crates (`rupu-config`, `rupu-orchestrator`, `rupu-cli`, `rupu-scm`, `rupu-workspace`), existing JSON Schema dependencies in the workspace (`jsonschema`, `schemars`), existing run store and issue/SCM registry.

**Files touched:**
```
crates/rupu-config/src/config.rs
crates/rupu-config/src/lib.rs
crates/rupu-orchestrator/src/workflow.rs
crates/rupu-orchestrator/src/runner.rs
crates/rupu-orchestrator/src/templates.rs
crates/rupu-orchestrator/src/runs.rs
crates/rupu-cli/src/lib.rs
crates/rupu-cli/src/cmd/mod.rs
crates/rupu-cli/src/cmd/workflow.rs
crates/rupu-cli/src/cmd/repos.rs
crates/rupu-cli/src/cmd/issues.rs
crates/rupu-cli/src/cmd/autoflow.rs                 # NEW
crates/rupu-cli/src/paths.rs
crates/rupu-scm/src/types.rs
crates/rupu-workspace/src/store.rs
crates/rupu-workspace/src/record.rs
crates/rupu-cli/tests/*
crates/rupu-orchestrator/tests/*
docs/workflow-format.md
docs/workflow-authoring.md
docs/using-rupu.md
docs/development-flows.md
docs/superpowers/specs/2026-05-08-rupu-autoflow-design.md
docs/superpowers/plans/2026-05-08-rupu-autoflow-plan-1-foundation-runtime-and-cli.md
examples/workflows/*
examples/agents/*
```

---

## Task 1 — Extend workflow/config schema

- [ ] Add optional `autoflow:` and `contracts:` blocks to `crates/rupu-orchestrator/src/workflow.rs`.
- [ ] Add supporting structs for:
  - `Autoflow`
  - `AutoflowSelector`
  - `AutoflowClaim`
  - `AutoflowWorkspace`
  - `AutoflowOutcomeRef`
  - `Contracts`
  - `WorkflowOutputContract`
  - step-level `Contract`
- [ ] Add a `priority` field on `Autoflow` with default `0`.
- [ ] Preserve `#[serde(deny_unknown_fields)]` on all new structs.
- [ ] Add `[autoflow]` to `crates/rupu-config/src/config.rs` as a new optional config table with only machine-local / operational defaults.
- [ ] Keep logical cadence / ownership policy in workflow YAML, not config.
- [ ] Add parser tests for:
  - valid autoflow workflow
  - unknown field rejection
  - missing contract references
  - invalid duration grammar
  - invalid `entity`
  - priority ordering and tie-break behavior

**Verify:** `cargo test -p rupu-orchestrator -p rupu-config`

---

## Task 2 — Contract loading and validation

- [ ] Add contract resolution for:
  - `<repo>/.rupu/contracts/<name>.json`
  - `~/.rupu/contracts/<name>.json`
- [ ] Implement project-first, global-fallback resolution.
- [ ] Validate workflow-declared outputs against JSON Schema using the workspace JSON Schema stack.
- [ ] Make workflow-level `contracts.outputs.*` authoritative.
- [ ] Treat step-level `contract:` metadata as documentation + prompt guardrails only.
- [ ] Add a normalized runtime representation for validated autoflow outcomes.
- [ ] Fail autoflow cycles loudly on invalid structured output.

**Verify:** unit tests for valid schema, invalid schema, missing schema, invalid emitted JSON, mismatched step/output declaration.

---

## Task 3 — Repo registry under `rupu repos`

- [ ] Add a machine-local repo registry store at `~/.rupu/repos/<platform>--<owner>--<repo>.toml`.
- [ ] Record:
  - `repo_ref`
  - `preferred_path`
  - `known_paths`
  - `origin_urls`
  - `default_branch`
  - `last_seen_at`
- [ ] Auto-upsert registry entries whenever `rupu` runs inside a checkout and resolves its remote.
- [ ] Extend `rupu repos` with:
  - `attach <repo-ref> [path]`
  - `prefer <repo-ref> <path>`
  - `tracked`
  - `forget <repo-ref> [--path <path>]`
- [ ] Require an explicit preferred path when multiple local checkouts exist.

**Verify:** unit tests for attach/prefer/forget; CLI tests for tracked output; repo auto-upsert smoke test.

---

## Task 4 — Claim store, lease model, and active-cycle locking

- [ ] Add claim files under `~/.rupu/autoflows/claims/<issue-key>/claim.toml`.
- [ ] Add active-cycle lock files under `~/.rupu/autoflows/claims/<issue-key>/.lock`.
- [ ] Claim record must include at minimum:
  - `issue_ref`
  - `repo_ref`
  - `workflow`
  - `status`
  - `worktree_path`
  - `branch`
  - `last_run_id`
  - `last_error`
  - `next_retry_at`
  - `claim_owner`
  - `lease_expires_at`
  - optional persisted child dispatch
- [ ] Implement the lease model:
  - active cycle acquires lock + renews lease
  - paused/waiting states keep the claim without a long-held active-cycle lock
  - takeover allowed only when lease expired **and** no active-cycle lock is held
- [ ] Add explicit lifecycle transitions for `eligible`, `claimed`, `running`, `await_human`, `await_external`, `retry_backoff`, `blocked`, `complete`, `released`.
- [ ] Keep claim lifecycle distinct from `RunStatus`.

**Verify:** tests for concurrent acquisition, stale claim recovery, approval-paused state retention, and duplicate-run prevention.

---

## Task 5 — Persistent worktrees and workspace resolution

- [ ] Add autoflow worktree allocation under `~/.rupu/autoflows/worktrees/<repo-key>/issue-<n>/`.
- [ ] Default to `workspace.strategy = worktree`.
- [ ] Reuse existing repo checkout information from the repo registry to create or refresh worktrees.
- [ ] Keep the operator's main checkout untouched.
- [ ] Support resuming existing worktrees on later ticks.
- [ ] Add cleanup behavior gated by config and claim terminal state.

**Verify:** tests for create/reuse/cleanup; smoke test with a real local checkout.

---

## Task 6 — New internal autoflow runtime entrypoint

- [ ] Extract a non-interactive internal runtime entrypoint from the current `crates/rupu-cli/src/cmd/workflow.rs` wrapper.
- [ ] Require explicit:
  - `project_root`
  - `workspace_path`
  - `workspace_id`
  - `event`
  - `issue`
  - `issue_ref`
  - optional deterministic `run_id`
- [ ] Keep this entrypoint a thin caller over `run_workflow` in `crates/rupu-orchestrator/src/runner.rs`.
- [ ] Add strict-template mode for autoflow runs; missing template variables become hard errors.
- [ ] Do not allow autoflows to silently fall back to `cwd` when target resolution fails.

**Verify:** unit tests and CLI integration tests for manual workflow mode vs autoflow mode behavior.

---

## Task 7 — `rupu autoflow` CLI surface

- [ ] Add a new top-level command module `crates/rupu-cli/src/cmd/autoflow.rs`.
- [ ] Implement:
  - `rupu autoflow list`
  - `rupu autoflow show <name>`
  - `rupu autoflow run <name> <issue-ref>`
  - `rupu autoflow tick`
  - `rupu autoflow status`
  - `rupu autoflow claims`
  - `rupu autoflow release <issue-ref>`
- [ ] Keep `rupu workflow run` working for autoflow-enabled files, but ignore claim/worktree/tick semantics there.
- [ ] Make `rupu autoflow run` hard-error on non-issue targets in v1.

**Verify:** CLI tests covering parsing, help text, target validation, and read-only inspection commands.

---

## Task 8 — Discovery and tick engine

- [ ] Discover autoflows from:
  - `~/.rupu/workflows/`
  - each preferred repo checkout in the repo registry
- [ ] Reuse `[triggers].poll_sources`, `EventConnector`, event cursors, and `rupu webhook serve` for autoflow wake hints.
- [ ] For each enabled autoflow:
  1. resolve config + workflow metadata
  2. resolve repo binding from registry
  3. list candidate issues via existing `IssueConnector`
  4. filter via the v1 selector surface (`state`, AND labels, `limit`)
  5. when multiple autoflows match one issue, choose the winner by `priority`, then workflow name
  6. merge with existing claim state
  7. determine whether each claim is due based on new issue / wake event / reconcile interval / retry backoff
  8. run one autoflow cycle
  9. validate outcome
  10. persist updated claim state
- [ ] Persist child dispatch requests onto claim state and execute them on the **next** tick.
- [ ] Enforce one active claim per issue across all autoflows.

**Verify:** integration tests for first-run discovery, wake-event reconciliation, retry timing, paused approval state, and next-tick dispatch.

---

## Task 9 — Outcome schemas and child dispatch semantics

- [ ] Ship `autoflow_outcome_v1` as the first canonical contract.
- [ ] Recommended fields:
  - `status`
  - `summary`
  - optional `dispatch`
  - optional `retry_after`
  - optional `pr_url`
  - optional `reason`
  - optional `artifacts`
- [ ] Normalize `dispatch` to:
  - `workflow`
  - `target`
  - `inputs`
- [ ] Remove ad-hoc phase routing from top-level outcome fields; phase data should live inside `dispatch.inputs`.
- [ ] Add `workflow_dispatch_v1`, `phase_plan_v1`, and `review_packet_v1` examples to docs/examples.

**Verify:** schema fixtures + outcome parsing tests + child-dispatch persistence tests.

---

## Task 10 — Docs, examples, and verification

- [ ] Update:
  - `docs/workflow-format.md`
  - `docs/workflow-authoring.md`
  - `docs/using-rupu.md`
  - `docs/development-flows.md`
- [ ] Add autoflow authoring guidance and examples under `examples/workflows/` and `examples/agents/`.
- [ ] Include both:
  - a controller autoflow example
  - a directly-autonomous phase workflow example
- [ ] Include explicit precedence examples showing how `autoflow.priority` affects ownership when multiple autoflows match the same issue.
- [ ] Add end-to-end documentation for:
  - macOS `launchd`
  - Linux `systemd --user` / cron
  - Windows Task Scheduler
- [ ] Run focused tests first, then broader workspace tests.
- [ ] Do not introduce `autoflow serve` in this plan.

**Verify:**
- `cargo test -p rupu-config -p rupu-orchestrator -p rupu-cli -p rupu-scm -p rupu-workspace`
- targeted CLI smoke tests for `repos`, `workflow`, `autoflow`, and issue flows

---

## Risks / design dependencies

- **Multiple matching autoflows for one issue.** The rule is now fixed (`priority`, then workflow name), but the operator UX must surface the losing candidates clearly so ownership decisions are explainable.
- **Cross-platform locking behavior.** Active-cycle locks and claim leases must behave consistently on macOS, Linux, and Windows. The implementation must not rely on a Unix-only lock primitive.
- **Project discovery outside cwd.** Autoflow tick cannot depend on `paths::project_root_for(current_dir)`; it must resolve project roots from the repo registry.
- **Strict template rendering split.** Manual workflows remain permissive; autoflow mode becomes strict. The runtime boundary must be explicit.
- **Controller vs direct phase autoflows.** One issue may match more than one autoflow style. Ownership must remain single-claim.

---

## Acceptance criteria

- The same workflow file can run via `rupu workflow run` and `rupu autoflow run`, with autoflow semantics only active in the latter path.
- `rupu autoflow tick` can reconcile issues for repos not currently open in the operator's shell.
- One issue can persist across multiple workflow runs using the same claim and worktree.
- Invalid autoflow structured output is rejected by contract validation and does not silently advance claim state.
- Repo tracking and preferred-path selection work through `rupu repos ...`.
- Child workflow dispatch is durable and picked up on a later tick without double-dispatch.
- When multiple autoflows match the same issue, the winning owner is deterministic and visible to the operator.
- The implementation works on macOS, Linux, and Windows without requiring a long-running daemon.
