# Agentiflows Plan 3b-4 — workflow units (`run_workflow`, non-blocking, process-isolated) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the lead run a **pool workflow as a non-blocking, process-isolated unit** (`rupu workflow run <name>` as a detached subprocess, reusing 3b-2's supervisor), carrying the agentiflow's engagement profile and pooling the workflow's findings into the agentiflow scope the goal evaluator reads — fail-closed to the pool. End state: `run_workflow {workflow, inputs}` validates (workflow ∈ pool; its dispatched agents ⊆ pool; inputs resolve; not gated/placed), spawns the workflow unit, and the lead `join`s it like an agent unit.

**Architecture:** Extends 3b-2's dispatch path to a second unit kind. `UnitSpec` gains `kind: UnitKind{Agent, Workflow}` + `inputs`; `SubprocessUnitLauncher` emits a `workflow run …` argv for a workflow unit and polls it workflow-aware (its liveness is `run.json` + `step_results.jsonl`, not a transcript — each step gets its own transcript). `rupu workflow run` gains an `--engagement-profile` flag and hidden `--fleet-run-dir`/`--fleet-participant`, threading the engagement + a coverage-scope override through the step factory to every step's agent run (findings pool into `target_id(workspace, agentiflow_id)`); the threading is additive and a no-op when the flags are absent, so existing workflow runs are unchanged. The lead's `run_workflow` tool (in `extra_tools`) validates against the pool + refuses gated/placed workflows in v1 (which sidesteps the resume-rebuild scope leak) and drives the supervisor.

**Tech Stack:** Rust 2021; `rupu-orchestrator` (`Workflow::dispatched_agents`, `catalog::load_workflow`, `RunStore::load`/`read_step_results`, `runner::resolve_inputs`, `DefaultStepFactory`, `WorkflowRunner`); `rupu-cli` (`rupu workflow run` flags + threading); `rupu-agentiflow` (`UnitSpec` kind/inputs, workflow poll, `run_workflow` tool); `rupu-coverage` (`FindingWriteOptions.engagement`, `target_id`); `serde_json`, `tracing`.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` — §10.1 (non-blocking `run_workflow`), §10.2 (process isolation + each unit carries the active engagement profile; a unit may narrow, never widen), §13 (Tier-2 `run_workflow`), §23 (pool ⊇ the workflow's dispatched agents). Builds on 3b-2 (the `UnitLauncher`/`FleetSupervisor`/`SubprocessUnitLauncher` + the `rupu run` attach hook + `dispatch`/`join` tools) and 3b-3 (the roster: `list_workflow_summaries`, `pool_workflow_ids`, `catalog`).

## Global Constraints

- **Existing workflow runs are byte-for-byte unchanged.** Every new `rupu workflow run` flag / factory field defaults to absent/`None`; the engagement stays `None` and the scope stays `Some(workflow.name)` exactly as today when the agentiflow flags aren't passed. A reviewer must confirm the non-fleet workflow path is untouched.
- **Fail-closed (spec §23).** `run_workflow` refuses: a workflow not in `def.pool.workflows`; a workflow whose statically-dispatched agents are NOT all in `def.pool.agents`; inputs that don't `resolve_inputs`; and — v1 — a workflow containing an approval gate or a `host:`/`distribute:` step (those would leak the agentiflow scope on resume/placement; deferred). Each refusal is a `ToolOutput.error`, nothing spawned.
- **Findings-only for workflow steps.** A workflow unit pools its steps' FINDINGS into the agentiflow scope (engagement + scope override). It does NOT give each step board tools/collectors (per-step participant identity is a deferred design question; the spec requires profile propagation, not board access for steps).
- **Legacy dispatch untouched** (spec §10.1); the agent-unit path from 3b-2 is unchanged (the `UnitKind` default is `Agent`).
- **Workspace-pinned/path deps per crate convention;** no `unsafe`; `cargo clippy -p <crate> --all-targets -- -D warnings` clean for every touched crate; integration tests under `crates/<c>/tests/it/` or `#[cfg(test)]` (never a new top-level `tests/*.rs`), env/cwd-mutating CLI tests in `tests/serial/` holding `ENV_LOCK`.

## Scope boundary

3b-4 includes ONLY: `Workflow::dispatched_agents` + `catalog::load_workflow`; the `rupu workflow run` engagement + fleet-scope flags and their threading; `UnitSpec` kind/inputs; the workflow-aware poll/terminate in `SubprocessUnitLauncher`; the `run_workflow` tool + wiring; e2e. It does NOT include: `generate_workflow` (3b-5 — full-format prompt + profile stamping + pool-validation-on-generate + running an unsaved workflow); board access for workflow steps; per-step engagement narrowing; a `WorkflowDefaults.engagement_profiles` field; resume parity for gated workflow units (v1 refuses them); `budget.status` / CLI / CP / orphan reaper (Plan 4); verify (3c).

## Deferrals (stated, not silent)
- **Gated + `host:`/`distribute:` workflows as units** — refused in v1 (scope would leak on the resume-rebuild / placement path, which don't carry the overlay). Lifting this needs persisting the engagement+scope overlay across resume (dossier §2 item 5).
- **Board tools/collectors for workflow steps** — findings-only in v1 (participant-identity design).

## File Structure

- `crates/rupu-orchestrator/src/workflow.rs` — add `Workflow::dispatched_agents(&self) -> BTreeSet<String>` (step.agent incl. `for_each`, `parallel[].agent`, `panel.panelists`, `panel.gate.fix_with`, `approval.on_reject[].agent`); helpers to detect a gate / `host:`/`distribute:` step.
- `crates/rupu-orchestrator/src/catalog.rs` — add `load_workflow(global, project, id) -> Option<Workflow>` (locate by stem project-then-global, parse).
- `crates/rupu-orchestrator/src/step_factory.rs` — add `scope_name_override: Option<String>` to `DefaultStepFactory` (used at the `scope_name` site, `.or(Some(workflow.name))`); the 9 constructors gain the field.
- `crates/rupu-cli/src/cmd/workflow.rs` — `Run` gains `--engagement-profile` (+ hidden `--fleet-run-dir`/`--fleet-participant`); thread `resolve_engagement` into the run's `findings_base` (the 3 `base_options` sites) + the `FindingsContext.scope_name` + `DefaultStepFactory.scope_name_override`.
- `crates/rupu-agentiflow/src/unit.rs` — `UnitKind{Agent, Workflow}`; `UnitSpec` gains `kind` + `inputs: Vec<(String,String)>`.
- `crates/rupu-agentiflow/src/subprocess.rs` — workflow argv branch + workflow-aware `poll`/`terminate`.
- `crates/rupu-agentiflow/src/dispatch_tools.rs` — the `run_workflow` tool.
- `crates/rupu-agentiflow/src/run.rs` — wire `run_workflow` into the lead's `extra_tools`.

---

## Task 1: `Workflow::dispatched_agents` + gate/placement detection + `catalog::load_workflow`

**Files:** `crates/rupu-orchestrator/src/workflow.rs`, `src/catalog.rs`.

**Interfaces:**
- Produces: `pub fn Workflow::dispatched_agents(&self) -> std::collections::BTreeSet<String>` (every statically-named agent: `step.agent` (covers `for_each`), `step.parallel[].agent`, `step.panel.panelists[]`, `step.panel.gate.fix_with`, `step.approval.on_reject[].agent`); `pub fn Workflow::has_approval_gate(&self) -> bool` and `pub fn Workflow::has_placed_step(&self) -> bool` (any step with `host:` set or a `for_each` with `distribute:`); `pub fn catalog::load_workflow(global: &Path, project: Option<&Path>, id: &str) -> Option<Workflow>` (locate `<project>/workflows/<id>.yaml` then `<global>/workflows/<id>.yaml` — same `(global, project=.rupu dir)` convention as `list_workflow_summaries` — and `Workflow::parse_file`; `None` if absent or unparseable).

The existing private `collect_workflow_agents` (`cmd/workflow.rs:1891`) covers all but `approval.on_reject[].agent` — port it to the orchestrator as the public method INCLUDING `on_reject`, then (optional) have the CLI call the new method.

- [ ] **Step 1: Failing test.** In `workflow.rs` tests: parse a workflow with a linear `agent: a`, a `parallel` with `agent: b`, a `panel` with `panelists: [c]` + `gate.fix_with: d`, and a gate step with `approval.on_reject: [{agent: e}]`; assert `dispatched_agents()` == `{a,b,c,d,e}`. Assert `has_approval_gate()` true for the gated one and false for a plain linear workflow; `has_placed_step()` true for a workflow with a `host:` step. In `catalog.rs` tests: `load_workflow` finds a global `foo.yaml` by stem and returns `None` for a missing/garbage one.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement.** Walk `self.steps`; a shared helper collects agents from a `Step` (and its `parallel`/`panel`/`approval.on_reject` sub-shapes). `has_approval_gate` = any step with `approval` set (standalone gate OR inline). `has_placed_step` = any `step.host.is_some()` or `for_each` with `distribute`. `catalog::load_workflow` mirrors `list_workflow_summaries`'s dir resolution for one id.
- [ ] **Step 4: Run → PASS**; scope the test run (`cargo test -p rupu-orchestrator workflow::` / `catalog::`) + `cargo clippy -p rupu-orchestrator --all-targets -- -D warnings`.
- [ ] **Step 5: Commit.** `git commit -m "feat(orchestrator): Workflow::dispatched_agents + gate/placement detection + catalog::load_workflow"`

---

## Task 2: `rupu workflow run` — engagement + fleet-scope flags, threaded to every step (no-op when absent)

**Files:** `crates/rupu-cli/src/cmd/workflow.rs`, `crates/rupu-orchestrator/src/step_factory.rs`.

**Interfaces:**
- Produces: `rupu workflow run` gains `#[arg(long = "engagement-profile", visible_alias = "engagement-profiles", value_delimiter = ',')] engagement_profiles: Vec<String>` and hidden `#[arg(long, hide=true)] fleet_run_dir: Option<PathBuf>`, `#[arg(long, hide=true)] fleet_participant: Option<String>`. `DefaultStepFactory` gains `scope_name_override: Option<String>`.
- Behavior: when `engagement_profiles` is non-empty, the run's `findings_base.engagement = resolve_engagement(&global, &workspace, &engagement_profiles)?` (so every step's `FindingWriteOptions` carries it — the factory already clones `findings_base`). When `fleet_run_dir` is set, `scope_name_override = Some(<run-dir basename>)` and `FindingsContext.scope_name = <basename>`; the factory uses `self.scope_name_override.clone().or(Some(self.workflow.name.clone()))` at the step `scope_name` site. **When the flags are absent: engagement stays `None`, scope stays `Some(workflow.name)` — identical to today.**

Dossier anchors: the 3 `base_options(&global, &cfg.findings)` sites in the run path (`workflow.rs:5434` dispatcher, `:5472` FindingsContext, `:5500` factory); the hard-wired `scope_name: Some(self.workflow.name.clone())` at `step_factory.rs:565`; the `FindingsContext.scope_name` at `workflow.rs:5466-5481`; `resolve_engagement` (`findings_opts.rs:9`); `DefaultStepFactory`'s 9 construction sites in `step_factory.rs` (prod `:124` region + `:1312/:1985/:2047/:2133/:2336/:2410` tests). Thread the two new values either on `ExplicitWorkflowRunContext` (`workflow.rs:4439`, built at `:4808/:4883` + autoflow `:10963/:11784` + 3 test sites) OR as parameters down `run_with_outcome`→`execute_workflow_invocation` — pick the lower-churn one and keep it to the RUN path (do NOT touch `autoflow` behavior; pass through its existing-default values).

- [ ] **Step 1: Failing tests.** A CLI/parse test: `rupu workflow run w --engagement-profile network,web --fleet-run-dir /x --fleet-participant p` parses the three values. A factory test (`step_factory.rs`): a `DefaultStepFactory` with `scope_name_override: Some("af_123")` builds a step `AgentRunOpts` with `scope_name == Some("af_123")`; with `None` it's `Some(<workflow name>)` (unchanged). (If a full run test is heavy, assert the factory + the `findings_base.engagement` plumb-through at the construction site.)
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement.** Add the flags; compute `engagement` + `scope_override` once in the run path; set them on `findings_base`, the `FindingsContext`, and the factory. Add `scope_name_override` to `DefaultStepFactory` (default `None` at every existing site — the compiler lists the 9). Keep `autoflow` passing `None`/empty (unchanged).
- [ ] **Step 4: Run → PASS**; `cargo test -p rupu-orchestrator step_factory::` + a `rupu-cli` parse test (per repo test rules); `cargo build -p rupu-cli`; clippy both crates. Also run an existing workflow-run integration test to confirm the non-fleet path is unchanged.
- [ ] **Step 5: Commit.** `git commit -m "feat(cli): rupu workflow run --engagement-profile + fleet scope threaded to every step (no-op when absent)"`

---

## Task 3: `UnitSpec` kind + inputs; workflow argv in `SubprocessUnitLauncher`

**Files:** `crates/rupu-agentiflow/src/unit.rs`, `src/subprocess.rs` (argv only; poll in Task 4), + the `UnitSpec` literal sites.

**Interfaces:**
- Produces: `pub enum UnitKind { Agent, Workflow }` (default `Agent`); `UnitSpec` gains `pub kind: UnitKind` and `pub inputs: Vec<(String, String)>` (empty for agents). The `SubprocessUnitLauncher` argv builder, for `UnitKind::Workflow`, emits `workflow run <name> --run-id <id> --mode bypass --plain [--input k=v]… [--engagement-profile a,b] --fleet-run-dir <dir> --fleet-participant <p>` (name = `spec.agent`, reused as the workflow id for a workflow unit — or add a clearer field; keep `agent` as "the thing to run" to minimize churn, documented). For `UnitKind::Agent` the argv is unchanged (3b-2).

- [ ] **Step 1: Failing test.** The argv builder for a `UnitSpec{ kind: Workflow, agent: "web-assess", inputs: vec![("target".into(),"x".into())], engagement: vec!["web".into()], participant: "web-assess#1".into(), .. }` emits `workflow run web-assess --run-id … --mode bypass --plain --input target=x --engagement-profile web --fleet-run-dir … --fleet-participant web-assess#1`; the Agent-kind argv is unchanged (regression-guard the 3b-2 shape).
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement.** Add `UnitKind` + the two fields; update the `UnitSpec` literals (`dispatch_tools.rs`, the test literals in `subprocess.rs`/`supervisor.rs`/`unit.rs` — the compiler lists them; default `kind: UnitKind::Agent`, `inputs: vec![]`). Branch the argv builder on `kind`.
- [ ] **Step 4: Run → PASS**; `cargo test -p rupu-agentiflow` + clippy.
- [ ] **Step 5: Commit.** `git commit -m "feat(agentiflow): UnitSpec kind (Agent|Workflow) + inputs; workflow run argv"`

---

## Task 4: Workflow-aware `poll` + `terminate` in `SubprocessUnitLauncher`

**Files:** `crates/rupu-agentiflow/src/subprocess.rs`.

**Why (dossier §1/§3):** a workflow run writes `<global>/runs/<run_id>/run.json` (status, `runner_pid: Some(pid)` at start) + `step_results.jsonl`, but its `run_id` is NOT a transcript (each step gets a fresh `run_<ULID>.jsonl`), and `RunRecord.final_output` is `None`. So the 3b-2 transcript-based poll reports empty output / false liveness for a workflow unit. A workflow unit needs a `run.json`-based poll.

**Interfaces:**
- For a `UnitKind::Workflow` unit, `poll` keeps `Child::try_wait()` as the PRIMARY terminal signal (unchanged zombie-safety), but derives the status/outcome from `rupu_orchestrator::RunStore::new(global.join("runs")).load(run_id)`:
  - not exited (`try_wait()==Ok(None)`): `Running` if `run.json` exists (status `Running`/`AwaitingApproval`/etc.), else `Pending`.
  - exited: read `run.json` status → `Completed` ⇒ `Done{success:true, output: <last non-skipped top-level step's output from step_results.jsonl>}`; `Failed`/`Rejected`/`Cancelled` ⇒ `Failed(error_message)`; `AwaitingApproval`/`Paused` ⇒ `Failed("workflow parked at a gate")` (v1 refuses gated workflows upstream, so this is a backstop); missing `run.json` ⇒ `Failed("workflow exited before writing run.json")`.
- `terminate` for a workflow unit: `RunStore::cancel(run_id, "agentiflow", "fleet terminate", now)` AND the existing `terminate_group(pid)` (so bash grandchildren of steps die too and `run.json` doesn't stay `Running`). Agent-unit terminate unchanged.
- `RunStore::load`/`read_step_results` are public in `rupu-orchestrator` (already a dep).

- [ ] **Step 1: Failing test (`#[cfg(unix)]`).** Drive the launcher with a fake `rupu` exe (a shell script the test writes) that, for `workflow run … --run-id <id>`, creates `<global>/runs/<id>/run.json` with `status: "completed"` + a `step_results.jsonl` with one step output, then exits 0. Assert `poll` → `Running` while alive (or after the child exits, terminal), then `Done{success:true, output: "<the step output>"}`. A second fake that writes `status: "failed"` → `Failed`. (Keep the fake deterministic; the launcher's `ArgvBuilder`/exe seam from 3b-2 makes this testable.)
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** the `kind`-branched poll + terminate. Factor the agent-unit poll (3b-2) and the workflow-unit poll into two arms; share the `try_wait` + started/zombie handling. The launcher already holds `global`; construct the `RunStore` from it.
- [ ] **Step 4: Run → PASS**; full crate test + clippy; confirm the agent-unit lifecycle tests (3b-2) still pass (the Agent arm is unchanged).
- [ ] **Step 5: Commit.** `git commit -m "feat(agentiflow): workflow-aware unit poll (run.json + step_results) + cancel"`

---

## Task 5: the `run_workflow` tool + wiring

**Files:** `crates/rupu-agentiflow/src/dispatch_tools.rs`, `src/run.rs`.

**Interfaces:**
- `run_workflow {workflow: String, inputs?: {k: v}}` (Tier-2, in the lead's `extra_tools`): fail-closed validation in order, each a `ToolOutput.error` (nothing spawned):
  1. `workflow ∈ pool_workflows` (the resolved pool-workflow id set — reuse `pool_workflow_ids` from 3b-3).
  2. `let wf = catalog::load_workflow(global, project, workflow)` — else "unknown or unparseable workflow".
  3. `wf.has_approval_gate()` or `wf.has_placed_step()` → refuse ("v1 cannot run a gated or host/distribute workflow as a unit").
  4. `wf.dispatched_agents()` ⊆ `def.pool.agents` → else "workflow dispatches agents outside the pool: <names>".
  5. `rupu_orchestrator::runner::resolve_inputs(&wf, &inputs)` → else the input error (pre-validation, because the unit's stderr is nulled).
  Then mint a unique participant (`<workflow id>#<n>`, the 3b-2 atomic-counter pattern) and `sup.dispatch(UnitSpec{ kind: Workflow, agent: workflow, prompt: String::new(), inputs, engagement: engagement.clone(), participant })` → return `{handle, participant}`.
- `fleet_dispatch_tools` (3b-2) extends to also return the `run_workflow` tool (it already has `sup`/`pool`/`engagement`; add `global`/`project` for the catalog lookups), OR a sibling `fleet_workflow_tools(...)` appended in `run.rs`. Keep `dispatch`/`join` unchanged. `join` already works for any handle (the supervisor polls by id, now workflow-aware).

- [ ] **Step 1: Failing e2e** (`run.rs` tests, `MockUnitLauncher` + `CapturingMockProvider`): seed a pool workflow on disk (a benign 1-step `.yaml` whose agent is in the test def's pool); the mock launcher's `on_spawn` writes a finding into the pooled scope (as the 3b-2 e2e does) + the run.json a workflow poll reads. Script the lead: `run_workflow {"workflow":"<id>"}` → get a handle → `join` → stop. Assert: handle returned, the finding is in `target_id(workspace, id)`, run reaches `GoalsMet`, `units/<id>/unit.json` kind=workflow. Plus fail-closed tests: a non-pool workflow id errors; a workflow whose agent is outside the pool errors (nothing spawned); a gated workflow errors.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** the tool + the 5 checks + wiring in `run_agentiflow` (append to `extra_tools`; pass `global`/`project`/`pool`/`engagement`/`sup`). `resolve_inputs` wants a `BTreeMap`; convert the tool's `inputs` object.
- [ ] **Step 4: Run → PASS**; full crate test + clippy.
- [ ] **Step 5: Commit.** `git commit -m "feat(agentiflow): run_workflow tool — pool-gated, process-isolated workflow units"`

---

## Self-Review

- **Spec coverage:** §10.1 non-blocking `run_workflow` (Task 5) ✓; §10.2 process-isolated unit carrying the engagement profile (Tasks 2/3) + a unit may narrow-never-widen (the unit inherits the agentiflow set; per-step narrowing deferred) ✓; §13 Tier-2 `run_workflow` ✓; §23 pool ⊇ dispatched agents (Tasks 1/5) ✓. Deferred (Scope boundary): gated/placed workflow units, board-for-steps, generate_workflow (3b-5), per-step narrowing.
- **No-op-when-absent (the safety property):** Task 2's threading leaves engagement `None` + scope `Some(workflow.name)` when the flags aren't passed → existing workflow runs unchanged (Task 2 Step 4 re-runs an existing workflow test).
- **Type consistency:** `UnitKind`/`UnitSpec.{kind,inputs}` identical in unit.rs (def), subprocess.rs (argv + poll), dispatch_tools.rs (run_workflow builds it); `Workflow::dispatched_agents`/`catalog::load_workflow` (Task 1) consumed by `run_workflow` (Task 5); the pooled target `target_id(workspace, agentiflow_id)` identical to 3a/3b-2/3b-3.
- **Fail-closed order** (Task 5): pool → load → gate/placed → agents⊆pool → inputs, all before spawn; `resolve_inputs` pre-validation compensates for the nulled stderr.
- **Placeholder scan:** the one "read the real thing" is Task 2's exact `ExplicitWorkflowRunContext`-vs-parameter threading choice (dossier gives both site lists; implementer picks the lower-churn one). All signatures are named.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-10-06-rupu-agentiflows-plan-3b4-workflow-units.md`. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, task review between, broad review at the end. Task 2 (the orchestrator/CLI engagement+scope threading) is the highest-risk — review it hardest for the no-op-when-absent property and the 9 factory sites + autoflow non-interference.
2. **Inline Execution** — in this session with checkpoints.

3b-5 (`generate_workflow`: full-format prompt + profile stamping + pool-validation-on-generate + running an unsaved workflow) is the next plan and reuses this plan's pool-validation + the 3b-3 roster.
