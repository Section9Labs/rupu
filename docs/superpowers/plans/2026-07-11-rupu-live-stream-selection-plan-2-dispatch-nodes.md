# Live-view stream selection — Plan 2 (Part B: dispatch children as live nodes)

> **For agentic workers:** subagent-driven-development. Steps use `- [ ]`.

**Goal:** Surface sub-agent dispatch children as **live, selectable nodes** in the live workflow view, so a user watching a parent agent delegate can pick and follow any child's stream — reusing Part A's selection cursor (already merged). A dispatched child becomes a unit of the currently-active step, carrying its `sub/<id>/` transcript.

**Spec:** `docs/superpowers/specs/2026-07-11-rupu-live-stream-selection-design.md` (Part B; Q3 resolved: depth-1 children full, deeper best-effort).

## Design (refined by the emission-path map — key simplifications)
- **Same enum, no bridge:** `live_run.rs:18` aliases `rupu_orchestrator::executor::Event as WfEvent`. Adding `DispatchStarted`/`DispatchCompleted` to `executor::Event` makes them directly matchable in `live_run.rs::apply` — no CLI mirror, no From-mapping.
- **Key children by `sub_run_id`, NOT a threaded correlation id.** `CliAgentDispatcher::dispatch()` already has `sub_run_id` (from `create_sub_run`) in scope for both the start and completion emit points, so **the `AgentDispatcher` trait signature does NOT change** — the 5 `Fake`/`Stub` impls and the shipped dispatch e2e tests stay untouched. `DispatchCompleted` correlates to `DispatchStarted` by `sub_run_id`.
- **Attach to the active step:** the events carry no `step_id`; `live_run.rs::apply` attaches the child to `self.active.step_id`'s `units` vec (dispatch happens inside the running step's tool loop). This reuses `ensure_unit_slot` + the `units` machinery, so Part A's `navigable_nodes`/`NodeRef::Unit`/`focused_transcript`/`select_*` cover dispatch children with **zero Part-A changes**.
- **Sink seam:** thread `Option<Arc<dyn EventSink>>` into `CliAgentDispatcher` at its 3 construction sites (each already builds the run's `JsonlSink` a few lines later). A separate `JsonlSink` handle appending to the same `events.jsonl` (O_APPEND-safe).
- **Scope (Q3):** depth-1 children (a top-level agent's dispatches, emitted into the top-level run's `events.jsonl` that the live view tails). A child that itself dispatches (depth-2+) still runs + its transcript streams when selected, but its grandchildren aren't surfaced as nodes in v1.

## Global Constraints
- **Additive + backward compatible:** `executor::Event` is `#[serde(tag="type")]` and NOT `#[non_exhaustive]`; new variants are serde-additive (older readers with a wildcard arm ignore them). A run with no dispatch behaves exactly as today. `event_sink: None` on the dispatcher (tests) → no emission, no behavior change.
- **Exhaustive-match ripple (must compile):** add arms to `Event::run_id()` (event.rs ~142), `live_run.rs::apply` (no wildcard), and `rupu-app/src/run_model.rs` (~93-95, fold into the existing `UnitStarted | UnitCompleted | PanelRound => {}` no-op arm). `rupu-cp/src/api/graph.rs` (~246) has a `_ => {}` wildcard — compiles as-is; note as a known gap (CP web graph won't show dispatch children — out of scope).
- Emission is best-effort: a failed `emit` must never fail the child run (the sink `emit` is infallible per the trait, but guard the `Option`). `#![deny(clippy::all)]`; no `unsafe`; `thiserror`; workspace deps only. Per-file rustfmt (event.rs, dispatch.rs, live_run.rs, run_model.rs — none are mod-roots). rupu-cli/orchestrator cold-compile slowly.

## Grounded shapes (verified — from the emission-path map)
- `crates/rupu-orchestrator/src/executor/event.rs:15-140` — `Event` enum, `#[serde(tag="type", rename_all="snake_case")]`. `UnitStarted` (:75-89): `{run_id, step_id, index, unit_key, agent: Option<String>, transcript_path: PathBuf, host: Option<String>}`. `UnitCompleted` (:94-106): `{run_id, step_id, index, unit_key, success, tokens_in, tokens_out, host}`. `Event::run_id()` exhaustive (:142-163). Round-trip tests (:165-366).
- `EventSink` trait (`executor/sink.rs:8-10`): `fn emit(&self, run_id: &str, ev: &Event)`. `JsonlSink` (`executor/jsonl_sink.rs`). Re-exported via `executor/mod.rs` as `rupu_orchestrator::executor::{Event, EventSink, JsonlSink}`.
- `CliAgentDispatcher` (`crates/rupu-cli/src/cmd/dispatch.rs:25-39`), `new(...)` (:55-79); constructed at `resume.rs:109-118`, `workflow.rs:2323-2333`, `workflow.rs:3551-3561` (each builds the run's JsonlSink a few lines later). `dispatch()` (:91-213): `create_sub_run` (:105-108, gives `(sub_run_id, transcript_path)`), `run_agent().await` (:197-199), `duration_ms`/tokens available after (:200,210).
- `live_run.rs`: `use rupu_orchestrator::executor::Event as WfEvent;` (:18). `LiveRunState::apply` (:368-527), `UnitStarted` arm (:430-457), `UnitCompleted` arm (:458-489). `StepState.units: Vec<UnitState>` (:127); `UnitState` (:65-77): `{key, status, tokens, elapsed_secs, transcript_path: Option<PathBuf>}`. `ensure_unit_slot` (:642-652). `active: ActiveFocus` (:171, has `step_id`). Part A: `NodeRef` (:169), `navigable_nodes` (:271), `focused_transcript` (:346).

---

## Task 1: `DispatchStarted`/`DispatchCompleted` events + dispatcher emission (orchestrator + cli-tools)

**Files:** `crates/rupu-orchestrator/src/executor/event.rs` (variants + `run_id()` + round-trip tests); `crates/rupu-cli/src/cmd/dispatch.rs` (sink field + emit); `crates/rupu-cli/src/resume.rs` + `crates/rupu-cli/src/cmd/workflow.rs` (3 construction sites: pass the sink); `crates/rupu-app/src/run_model.rs` (no-op arm, to compile). Tests: event.rs + a dispatcher-emits test.

**Interfaces — Produces:** `Event::DispatchStarted { run_id, sub_run_id, agent, transcript_path }` + `Event::DispatchCompleted { run_id, sub_run_id, success, tokens_in, tokens_out }`; `CliAgentDispatcher` emits both.

- [ ] **Step 1: Failing tests.**
  - `event.rs`: `dispatch_started_round_trips` / `dispatch_completed_round_trips` (serde tag = `"dispatch_started"`/`"dispatch_completed"`; all fields survive), mirroring the UnitStarted round-trip test (:171-192). `Event::run_id()` returns the parent `run_id` for both.
  - dispatcher emit test (in dispatch.rs or a cli test): construct a `CliAgentDispatcher` with a capturing test `EventSink` (records emitted events), run a dispatch against a mock/stub agent path, assert a `DispatchStarted` (with the child's `sub_run_id` + `transcript_path`) is emitted before the child completes and a `DispatchCompleted` (same `sub_run_id`, success, tokens) after. (If a full run_agent is too heavy in a unit test, assert at minimum that `dispatch()` emits Started-then-Completed with matching `sub_run_id` via the smallest viable harness; keep it deterministic.)
- [ ] **Step 2:** `cargo test -p rupu-orchestrator --lib -- event` + the dispatcher test → FAIL.
- [ ] **Step 3: Implement.**
  - `event.rs`: add the two variants (fields above; `#[serde(default, skip_serializing_if=...)]` where an Option is optional, matching UnitStarted's `host` style). Add their arms to `Event::run_id()` returning `run_id`.
  - `dispatch.rs`: add `event_sink: Option<Arc<dyn EventSink>>` field to `CliAgentDispatcher` + constructor param; in `dispatch()`, right after `create_sub_run` (:108) `if let Some(s) = &self.event_sink { s.emit(parent_run_id, &Event::DispatchStarted{ run_id: parent_run_id.into(), sub_run_id: sub_run_id.clone(), agent: Some(agent_name.into()), transcript_path: transcript_path.clone() }) }`; after `run_agent` returns, emit `DispatchCompleted{ run_id: parent_run_id.into(), sub_run_id, success, tokens_in, tokens_out }`. Best-effort (guard the Option; never fail the run).
  - The 3 construction sites: build/obtain the run's `Arc<dyn EventSink>` (hoist the existing `JsonlSink::create` above the dispatcher build, or create a second handle to the same `events.jsonl`) and pass it. For paths without a sink (if any), pass `None`.
  - `run_model.rs`: add `DispatchStarted | DispatchCompleted` to the existing no-op arm (~93-95) so rupu-app compiles.
- [ ] **Step 4:** tests pass; `cargo test -p rupu-orchestrator --lib` + `cargo build -p rupu-cli -p rupu-app` green. Confirm the shipped dispatch e2e tests still pass unchanged (no trait signature change): `cargo test -p rupu-orchestrator --test dispatch_agent --test dispatch_agents_parallel`.
- [ ] **Step 5:** rustfmt changed files; `cargo clippy -p rupu-orchestrator -p rupu-cli --no-deps`; commit `feat(orchestrator,cli): DispatchStarted/Completed events emitted by the dispatcher`.

## Task 2: Live-view dispatch child nodes (rupu-cli `live_run.rs`)

**Files:** `crates/rupu-cli/src/output/live_run.rs`. Test: same file.

**Interfaces — Consumes:** Task 1's `WfEvent::DispatchStarted`/`DispatchCompleted`. Reuses Part A's `units`/`NodeRef`/`navigable_nodes`/`focused_transcript`.

- [ ] **Step 1: Failing tests.**
  - `apply_dispatch_started_adds_child_node_to_active_step` — with an active step, applying `DispatchStarted{sub_run_id, agent, transcript_path}` appends a `UnitState` to the active step's `units` (status Working, `transcript_path` set, display key = agent, correlation `sub_run_id` stored).
  - `apply_dispatch_completed_marks_child_done_by_sub_run_id` — `DispatchCompleted{sub_run_id, success, tokens}` finds the unit with that `sub_run_id` and marks Complete/Failed + tokens.
  - `dispatch_child_is_navigable_and_selectable` — after a `DispatchStarted`, `navigable_nodes()` includes the new child and `focused_transcript` for its `NodeRef::Unit{index}` resolves to its `transcript_path` (Part A integration).
  - `two_parallel_dispatch_children_get_distinct_slots` — two `DispatchStarted` (distinct sub_run_ids) → two units; each `DispatchCompleted` completes the right one by sub_run_id.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: Implement.**
  - Add `sub_run_id: Option<String>` to `UnitState` (None for for_each units; Some for dispatch children) — the correlation key. (for_each units keep index-keying; dispatch children are matched by sub_run_id.)
  - `apply`: add `WfEvent::DispatchStarted` arm — resolve the active step (`self.active.step_id`); append a new `UnitState` to its `units` (find-or-append by `sub_run_id` to be idempotent), status Working, `transcript_path = Some(path)`, `key = agent name`, `sub_run_id = Some(id)`. Follow the `UnitStarted` arm's shape (auto-follow default still applies via existing focus logic). Add `WfEvent::DispatchCompleted` arm — find the active step's unit whose `sub_run_id` matches; set Complete/Failed + tokens (mirror `UnitCompleted`).
  - No changes needed to `navigable_nodes`/`NodeRef`/`select_*`/`focused_transcript` (they operate on `units` by index) — verify a dispatch child is covered.
- [ ] **Step 4:** `cargo test -p rupu-cli --lib -- live_run` green (new + all Part A + existing). `cargo build -p rupu-cli` clean.
- [ ] **Step 5:** rustfmt live_run.rs; clippy `-p rupu-cli --no-deps`; commit `feat(cli): render dispatched children as selectable live nodes`.

---

## Self-Review
Coverage: events + emission (T1); live-view node wiring reusing Part A (T2). No trait-signature change (sub_run_id keying) → dispatch e2e tests untouched (verified in T1 Step 4). Same-enum (no bridge). Additive/backward-compatible; run_model.rs no-op arm keeps rupu-app compiling; rupu-cp graph.rs wildcard = known out-of-scope gap. Type flow: `Event::Dispatch*` (T1) → `WfEvent::Dispatch*` alias → `apply` → `units` → Part A selection (T2). Depth-1 scope per Q3.

## Execution
Subagent-driven: T1 → review → T2 → review → final whole-branch review → PR to main (no self-merge). **matt validates the TUI**: run a `dispatch_agents_parallel` workflow, confirm children appear as nodes under the active step and the cursor (↑↓/Tab) selects between their live streams.
