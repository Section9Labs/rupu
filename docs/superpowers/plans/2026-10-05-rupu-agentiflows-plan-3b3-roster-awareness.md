# Agentiflows Plan 3b-3 — the lead's awareness + steering surface — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the lead **situational awareness + steering** without a round boundary: a roster of the agents/workflows it may use (pull tools + an ambient collector), live `goal.status`/`coverage.status` re-evaluation, and a `board.directive` write tool to steer the fleet. Plus the stale scope-doc fix. End state: the lead can enumerate/search the catalog, re-check whether its findings met a goal mid-round, and post a standing directive the fleet's `DirectiveCollector` (3b-1) already reads — all as always-on Tier-1 tools (plus the one Tier-2 directive write).

**Architecture:** All read-only roster/status tools and the one directive-write tool are pre-built `rupu_tools::Tool` impls injected via the lead's `extra_tools` (the 3b-1 mechanism), bound to run-scoped handles (the global/project paths, the pooled `CoveragePaths` + `ActiveSet`, the run's `Board`). The roster needs a library-level workflow lister, which does not exist yet (discovery is private to `rupu-cli`/`rupu-cp`) — a new `rupu-orchestrator` function provides it. The status tools re-run the Plan-2 evaluators (`GoalEvaluator::evaluate`, `CoverageEvaluator::evaluate`, both pure over the pooled scope) so the tally reflects evidence the lead just banked. A `RosterCollector` (the §8.5 collector not yet built) folds the pool's compact index into each turn.

**Tech Stack:** Rust 2021; `rupu-orchestrator` (`Workflow::parse` + a new `list_workflow_summaries`); `rupu-agent` (`load_agents` → `AgentSpec`; `TurnCollector`); `rupu-agentiflow` (the tools + collector + wiring; reuses `GoalEvaluator`/`CoverageEvaluator`/`CoveragePaths`/`ActiveSet` + `fleet_tools`/`lead_collectors` + `Board`); `rupu-fleet` (`Board::put_directive`/`Directive`); `serde_json`, `chrono`, `tracing`.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` — §8.5 (`RosterCollector`), §9.1 (board `directive`), §13 (Tier-1 `goal/budget/coverage.status`, `agents/workflows.list/get`, `catalog.search`; Tier-2 `board.directive`), §12 (the roster/index). Builds on 3a (envelope `Digest`/evaluators), 3b-1 (`fleet_tools`, `lead_collectors`, `DirectiveCollector`, `AgentRunOpts.extra_tools`, the pooled scope), 3b-2 (the lead already dispatches agent units).

## Global Constraints

- **Always-on Tier-1 via `extra_tools`** (spec §13): the roster + status tools are injected regardless of the agent's `tools:` list; the `PermissionDecider` still gates (lead runs `BypassDecider`). `board.directive` is Tier-2 (lead-held; here it's simply in the lead's `extra_tools`). No `capabilities:` frontmatter gate (dropped; confirmed across 3a/3b-1/3b-2).
- **Read tools are read-only + fail-soft.** A roster/status tool that can't read (a bad agent `.md`, a missing ledger) returns a `ToolOutput` with a partial result or an `error:` string — never a panic, never `Err` that aborts the turn. `load_agents` fails the whole call on one bad file (known) — the `agents.list` tool must tolerate that (catch + report, or the lister skips bad files).
- **Profiles are agnostic to scope (matt, 2026-10-05).** A profile is the *mode of work*; the scope lives in `AgentiflowDef.scope` and types its roots with the profile's real root kinds (`network:host`/`web:site`/`code:repo`). Task 1 corrects the stale `ScopeRoot.kind` doc comment accordingly. Do NOT invent `network:scope`/`web:target` kinds.
- **Roster scoping:** the `RosterCollector` injects the agentiflow's **pool** (what the lead may actually dispatch, from `def.pool`), not the entire global catalog. The `agents.list`/`workflows.list`/`catalog.search` *tools* read the full catalog (global + project) for discovery.
- Workspace-pinned deps only; `[lints] workspace = true`; no `unsafe`; `cargo clippy -p <crate> --all-targets -- -D warnings` clean for every touched crate; `thiserror` for library errors.
- **Don't break anything;** reuse, don't duplicate: `GoalEvaluator`/`CoverageEvaluator` (do not re-implement evaluation), `fleet_tools`/`lead_collectors`, `Board::put_directive`.

## Scope boundary

3b-3 includes ONLY: the scope-doc fix; the library workflow lister; the roster read tools (`agents.list/get`, `workflows.list/get`, `catalog.search`); the `RosterCollector`; the `goal.status`/`coverage.status` pull-tools; the `board.directive` write tool; and wiring them into the lead's run. It does NOT include: `budget.status` (needs envelope round-state coupling, and budget is already in the round digest — deferred; see Deferrals); `run_workflow` / **workflow units** / the workflow engagement-profile carrier (3b-4); `generate_workflow` (3b-4); the verify path (3c); the `rupu agentiflow` CLI / CP surfaces + the orphan reaper (Plan 4).

## Deferrals (stated, not silent)
- **`budget.status`**: the budget is rendered into every round prompt already (3a `render_round_prompt`); a mid-round pull-tool would need a handle to the envelope's live round counter + `UsageSource`, coupling a tool to envelope internals for little gain. Deferred to a later plan; note it in the `*.status` tool docs.

## File Structure

- `crates/rupu-agentiflow/src/def.rs` — modify: correct the `ScopeRoot.kind` doc comment (`network:scope`/`web:target` → `network:host`/`web:site`).
- `crates/rupu-orchestrator/src/` (e.g. `workflow.rs` or a new `catalog.rs`) — add `WorkflowSummary` + `list_workflow_summaries(global, project)`.
- `crates/rupu-agentiflow/src/roster.rs` — **new**: `RosterCtx` (run-scoped: global/project paths + the pool) + `agents.list/get`, `workflows.list/get`, `catalog.search` tools + `roster_tools(ctx)`; `RosterCollector` (`TurnCollector`).
- `crates/rupu-agentiflow/src/status_tools.rs` — **new**: `goal.status` + `coverage.status` tools (+ `board.directive`) over run-scoped handles; `status_tools(...)`.
- `crates/rupu-agentiflow/src/run.rs` — modify: build the roster/status/directive tools + the `RosterCollector`, extend the lead's `extra_tools` + `collectors`.
- `crates/rupu-agentiflow/src/lib.rs` — modify: new `mod`s + re-exports.

---

## Task 1: Correct the stale `ScopeRoot.kind` doc comment

**Files:** Modify `crates/rupu-agentiflow/src/def.rs`.

**Why:** the `ScopeRoot.kind` doc (def.rs:86) says `e.g. "network:scope" / "web:target"`, but `validate_scope` requires a REAL root asset kind of an active profile (the tests use `network:host`; `network:service` is rejected as non-root). Profiles are the mode of work and carry no scope/target kind. The comment misleads.

- [ ] **Step 1: Fix the comment.** Change def.rs:86 from
  `/// Profile-namespaced root kind, e.g. "network:scope" / "web:target".`
  to
  `/// Profile-namespaced ROOT asset kind of an active profile, e.g. "network:host" / "web:site" / "code:repo" (parent == None). The scope's concrete coordinates are the agentiflow's own, agnostic to the profile.`
  Scan def.rs for any other `network:scope`/`web:target` occurrence in comments/doctests and fix likewise. Do NOT change any code or validation (the validation is already correct).

- [ ] **Step 2: Verify + commit.** `cargo test -p rupu-agentiflow` (unchanged, green) + `cargo clippy -p rupu-agentiflow --all-targets -- -D warnings`. `git commit -m "docs(agentiflow): scope root kind is a profile root asset kind (host/site), not a scope kind"` (The spec §18 example is illustrative and frozen; the binding behavior is the validation — leave the spec file alone.)

---

## Task 2: Library workflow lister (`rupu-orchestrator`)

**Files:** Add to `crates/rupu-orchestrator/src/` (place next to `Workflow` — `workflow.rs`, or a small `catalog.rs` module; pick per the crate's conventions). Modify `lib.rs` to re-export.

**Interfaces:**
- Produces:
  ```rust
  pub struct WorkflowSummary {
      pub name: String,
      pub description: Option<String>,
      pub scope: String,        // "global" | "project"
      pub input_keys: Vec<String>,
      pub step_count: usize,
      pub parse_error: Option<String>,  // Some => the file didn't parse; still listed
  }
  /// Scan `<global>/workflows/*.yaml` then `<project>/.rupu/workflows/*.yaml`;
  /// project entries shadow global by name. A file that fails `Workflow::parse`
  /// is listed with `parse_error: Some(..)` (never aborts the whole scan).
  pub fn list_workflow_summaries(global: &Path, project: Option<&Path>) -> Vec<WorkflowSummary>;
  ```
- Consumes: `Workflow::parse` (`rupu_orchestrator::Workflow`) + its `name`/`description`/`inputs`/`steps`.

Dossier facts: there is NO library workflow lister today — discovery is duplicated privately in `rupu-cli` (`push_yaml_names`/`list` in `cmd/workflow.rs`) and `rupu-cp` (`scan_workflow_names`). This is the shared API they could later adopt; for 3b-3 it only needs to serve the roster. `.yaml` only (match `rupu-cli`'s `list`). The dirs: global = `<global>/workflows`, project = `<project>/.rupu/workflows` (the caller passes the parents; mirror `load_agents(global, project)`'s shape — `global` is `<global>`, `project` is the dir containing `.rupu`).

- [ ] **Step 1: Failing test.** In the new module's tests: write two `.yaml` files (a valid 2-step workflow + a deliberately broken one) under a temp `global/workflows/`, call `list_workflow_summaries(global, None)`, assert the valid one has `name`/`step_count==2`/`parse_error==None` and the broken one has `parse_error: Some(..)` (and the scan returned BOTH, didn't abort). A second test: a project file shadows a global one of the same name.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement.** Read dir entries ending `.yaml`, `Workflow::parse_file` each; on `Ok`, fill the summary from the parsed `Workflow`; on `Err`, emit a summary with `name` = the file stem, `parse_error: Some(e.to_string())`. Global first, then project (project shadows by name — a `BTreeMap<String, WorkflowSummary>` keyed by name, project overwrites). A missing dir ⇒ no entries (not an error).
- [ ] **Step 4: Run → PASS**; full crate test + clippy (`cargo test -p rupu-orchestrator` is large — scope to the new test module: `cargo test -p rupu-orchestrator <module>::`). `cargo clippy -p rupu-orchestrator --all-targets -- -D warnings`.
- [ ] **Step 5: Commit.** `git commit -m "feat(orchestrator): list_workflow_summaries — a tolerant library workflow lister"`

---

## Task 3: Roster read tools + `catalog.search` (`rupu-agentiflow::roster`)

**Files:** Create `crates/rupu-agentiflow/src/roster.rs`; modify `lib.rs`. (Add a `rupu-orchestrator` dep to `rupu-agentiflow` if absent — the dossier noted the edge is acyclic; confirm.)

**Interfaces:**
- Consumes: `rupu_agent::load_agents(global, project) -> Result<Vec<AgentSpec>, _>` (`AgentSpec.{name, description, tools, dispatchable_agents, ...}`); `rupu_orchestrator::{list_workflow_summaries, WorkflowSummary}` (Task 2); `rupu_tools::{Tool, ToolOutput, ToolError, ToolContext}`.
- Produces: `RosterCtx { global: PathBuf, project: Option<PathBuf> }` + the tools `agents.list` (name+description+declared tools, each agent), `agents.get {name}` (one agent's detail), `workflows.list`, `workflows.get {name}`, `catalog.search {query}` (case-insensitive substring over agent+workflow name+description, returns matches tagged agent/workflow); `pub fn roster_tools(ctx: Arc<RosterCtx>) -> Vec<Arc<dyn Tool>>`.

Fail-soft: `load_agents` aborts on one bad `.md` (dossier) — wrap it so `agents.list` returns the agents it could load with an `error:` note about the bad file, or (simpler) catch the `Err` and return `ToolOutput{ error: Some(..) }` naming the parse failure. `workflows.list` already tolerates bad files (Task 2 lists them with `parse_error`). Tools return compact JSON (not the raw `.md`/`.yaml`) so the lead's context stays small.

- [ ] **Step 1: Failing tests.** Temp `global/agents/*.md` (2 agents, one with a description) + `global/workflows/*.yaml` (1 workflow): `agents.list` returns both with names+descriptions; `agents.get {"name":"recon"}` returns that agent; `workflows.list` returns the workflow; `catalog.search {"query":"recon"}` returns the recon agent (and not unrelated ones). Reuse `rupu-agent`/`rupu-coverage` test fixtures or write minimal valid `.md`/`.yaml` — NO invented security/assessment content (benign placeholders only).
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** the five tools (`#[async_trait] impl Tool`), each holding `Arc<RosterCtx>`, with minimal input schemas. `roster_tools(ctx)`.
- [ ] **Step 4: Run → PASS**; crate test + clippy.
- [ ] **Step 5: lib.rs** `mod roster;` + `pub use roster::{roster_tools, RosterCtx};` (+ the collector in Task 4). Commit: `git commit -m "feat(agentiflow): roster read tools (agents/workflows list/get + catalog.search)"`

---

## Task 4: `RosterCollector` + `goal.status`/`coverage.status` + `board.directive`

**Files:** add `RosterCollector` to `roster.rs`; create `crates/rupu-agentiflow/src/status_tools.rs`; modify `lib.rs`.

**Interfaces:**
- `pub struct RosterCollector { pool_agents: Vec<String>, pool_workflows: Vec<String>, ctx: Arc<RosterCtx> }` — `TurnCollector`; `collect` emits ONE `Injection { kind: Status, cadence: EveryTurn, priority: 180, source: "roster" }` whose `content` is a compact one-line-per-entry index of the POOL's agents + workflows (name — description), resolved against the catalog (an unknown pool name is listed as "(not found)"). `pub fn roster_collector(pool_agents, pool_workflows, ctx) -> Arc<dyn TurnCollector>`.
- `status_tools.rs`: `goal.status` (re-run `GoalEvaluator::evaluate(goal, &paths, &active)` for each goal → JSON: per-goal id/objective/satisfied/tally) and `coverage.status` (`CoverageEvaluator::evaluate(target, &paths, &active)` → reach/depth/satisfied) over the pooled scope; `board.directive {body, addressed_to?}` (write a `rupu_fleet::Directive { author: "lead", ts, body, addressed_to }` via `Board::put_directive`). `pub fn status_tools(goals: Vec<Goal>, coverage: Option<CoverageTarget>, paths: CoveragePaths, active: Arc<ActiveSet>, board: Arc<Board>) -> Vec<Arc<dyn Tool>>`.

Notes: the evaluators are pure over the pooled scope (§3a), so `goal.status` reflects findings the lead JUST banked (its value over the round-start digest). `board.directive` is the lead→fleet steering write that the 3b-1 `DirectiveCollector` already delivers to units. `budget.status` is deferred (see Deferrals) — add a one-line note in the `goal.status` doc that budget is shown in the round digest.

- [ ] **Step 1: Failing tests.** `RosterCollector::collect` over a pool of `["recon"]` + a temp catalog returns one `Status`/`EveryTurn` injection whose content names `recon` + its description (and marks an unknown pool entry "(not found)"). `goal.status` over a seeded pooled scope (reuse 3b-1/3b-2's finding-seed helper — benign fixture) returns the goal satisfied once the finding is present. `board.directive {"body":"focus on auth"}` writes a directive readable via a fresh `Board::read_directives`.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** the collector + three tools. `goal.status`/`coverage.status` degrade an eval error to an `error:`-carrying `ToolOutput` (never panic).
- [ ] **Step 4: Run → PASS**; crate test + clippy.
- [ ] **Step 5: lib.rs** re-exports (`roster_collector`, `status_tools`). Commit: `git commit -m "feat(agentiflow): RosterCollector + goal/coverage.status + board.directive"`

---

## Task 5: Wire the awareness + steering surface into the lead's run + e2e

**Files:** Modify `crates/rupu-agentiflow/src/run.rs`.

**Interfaces:**
- In `run_agentiflow`, build `RosterCtx { global: global.clone(), project: Some(workspace.clone()) }` (the agentiflow workspace holds `.rupu`); extend the lead's `extra_tools` with `roster_tools(ctx.clone())` ++ `status_tools(def.goals.clone(), def.coverage-as-CoverageTarget, paths.clone(), Arc::new(active.clone()), board.clone())` (reuse the `Board` already built in 3b-1's wiring); extend the lead's `collectors` with `roster_collector(def.pool.agents..., def.pool.workflows..., ctx)`. Keep the 3b-1 `fleet_tools`/`lead_collectors` and 3b-2 `fleet_dispatch_tools` wiring intact (append, don't replace).

- [ ] **Step 1: e2e test** (`run.rs` tests, `MockUnitLauncher` + `CapturingMockProvider` as in 3b-2): script the lead to call `agents.list`, then `board.directive {"body":"prioritize the gateway"}`, then (after a mock unit's seeded finding) `goal.status`, then stop. Assert via captured requests + store state: the roster-tool output + the RosterCollector index reached the model, the directive is readable via `Board::read_directives`, and `goal.status` reports the goal satisfied once the finding is pooled.
- [ ] **Step 2: Run → FAIL (tools not wired).**
- [ ] **Step 3: Wire** as above; ensure `paths`/`active`/`board` are cloned before any move into the envelope.
- [ ] **Step 4: Run → PASS**; full crate test + clippy.
- [ ] **Step 5: Commit.** `git commit -m "feat(agentiflow): wire roster/status/directive tools + RosterCollector into the lead"`

---

## Self-Review

- **Spec coverage:** §13 `agents/workflows.list/get` + `catalog.search` (Task 3) ✓; `goal/coverage.status` (Task 4) ✓ (`budget.status` deferred, stated); §8.5 `RosterCollector` (Task 4) ✓; §9.1 `board.directive` write (Task 4) ✓; §13 Tier-1 always-on via `extra_tools` (Task 5) ✓. Deferred (Scope boundary): `budget.status`, `run_workflow`/workflow units + engagement carrier, `generate_workflow` (3b-4); verify (3c); CLI/CP + orphan reaper (Plan 4).
- **Reuse (no re-implementation):** evaluation via `GoalEvaluator`/`CoverageEvaluator` (Task 4), not a parallel evaluator; `Board::put_directive` (not a new directive store); `fleet_tools`/`lead_collectors`/`fleet_dispatch_tools` left intact and appended to (Task 5); the `DirectiveCollector` that reads the written directive already exists (3b-1).
- **Type consistency:** `WorkflowSummary` identical in Task 2 (def) and Task 3 (consumer); `RosterCtx` shared by the tools (Task 3) and the collector (Task 4); the status tools' `(goals, coverage, paths, active, board)` match what `run_agentiflow` already holds.
- **Fail-soft/placeholder scan:** `load_agents`'s one-bad-file-aborts behavior is explicitly handled (Task 3); evaluator errors → `error:` output, not panic (Task 4); no invented assessment content in fixtures. The only "read the real struct" is `def.coverage` → `CoverageTarget` conversion (Task 5 — read how 3a builds the envelope's `CoverageTarget` from `def.coverage` and reuse it).

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-10-05-rupu-agentiflows-plan-3b3-roster-awareness.md`. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, task review between, broad review at the end.
2. **Inline Execution** — execute in this session with checkpoints.

3b-4 (workflow units + `run_workflow` + the workflow engagement-profile carrier + `generate_workflow` taught the full format + profile stamping + pool validation) is the next plan; it reuses this roster (the lister + `catalog.search` feed `generate_workflow`'s `available_agents` and the lead's choice of what to run) and 3b-2's supervisor/launcher for workflow units.
