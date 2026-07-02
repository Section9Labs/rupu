# Pause / interrupt-and-resume Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A non-terminal **Pause** (interrupt-and-resume) for agent runs and workflows — like session Esc but resumable — across in-process, remote-host, and distributed fan-out runs.

**Architecture:** A new `RunStatus::Paused` + a two-mode interrupt signal (cancel vs pause) that the `rupu-agent` loop and `rupu-orchestrator` runner honor **cooperatively** at safe boundaries (stop the stream, let a running tool finish), persisting a resumable checkpoint. Resume re-enters the loop with the preserved transcript (a fresh provider request). Remote reach via new `HostConnector::pause_run`/`resume_run`; fan-out reuses the "resume only incomplete units" model. `/api/runs/:id/pause|resume` + CLI + CP buttons.

**Tech Stack:** Rust 2021 (MSRV 1.88), tokio + `tokio_util::sync::CancellationToken`, thiserror (libs) / `ApiError` (cp); React + vitest (web).

## Global Constraints

- Pause is **non-terminal + genuinely resumable** (`Running → Paused → Running`); Cancel stays terminal and unchanged.
- **No silent no-op:** pause must set `Paused` and emit `RunPaused` at a real boundary; resume must issue a real new provider request and continue; an unsupported transport returns a clear error — never a fake pause.
- Resume requires the full `cp serve` runtime (launcher-gated, record-in-CP / resume-in-cp-serve); pausing an already-terminal run is rejected; a `workspace: sync` workflow pause is **refused** (mirrors the `ResumeWithWorkspaceSync` resume refusal) with a `// TODO(pause-workspace-sync)` note.
- **Backward compatible:** additive `RunStatus` variant + serde-additive events + defaulted `HostConnector` methods ⇒ existing run/cancel/approve behavior unchanged; a run that never pauses behaves exactly as today.
- `#![deny(clippy::all)]`; no `unsafe`; libraries use `thiserror`; cp/cli use `anyhow`/`ApiError`; workspace deps only (no new dep needed — `tokio_util` is already used).
- Hexagonal: `rupu-agent` + `rupu-orchestrator` know only the interrupt signal + the `UnitDispatcher`/executor ports — never `rupu-cp`.
- **Per-file rustfmt only**: never run bare `rustfmt` on `lib.rs` or any crate-root / `mod`-declaring file (rustfmt follows `mod` and reformats the whole crate tree — the recurring ~16-file drift). Format specific non-root files with `rustfmt --edition 2021 <file>`; use `--skip-children` if you must format a `mod` file. Never `cargo fmt`/`cargo fmt --all`. Check `git status --short` before each commit and `git restore` stray drift by name.
- Clippy `--no-deps`, scoped to changed crates. Pre-existing 1.95-only lints in untouched files (`rupu-orchestrator/src/runner.rs` `items_after_test_module`, `rupu-config` config.rs:148, `node_tunnel.rs`) are unrelated. Web: `cd crates/rupu-cp/web && npm test && npx tsc --noEmit && npm run build`.
- GUI: matt validates the CP web Pause/Resume UI before merge.

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `crates/rupu-orchestrator/src/runs.rs` | `RunStatus::Paused` + non-terminal semantics | 1 |
| `crates/rupu-orchestrator/src/executor/in_process.rs` | two-mode interrupt signal; `pause`/`resume` executor methods | 1 |
| `crates/rupu-orchestrator/src/executor/event.rs` (+ sinks) | `RunPaused`/`RunResumed`/`StepPaused`/`StepResumed` events | 1 |
| `crates/rupu-agent/src/runner.rs` | cooperative pause in `run_agent`; `Paused` run outcome; resume entry | 2 |
| `crates/rupu-orchestrator/src/runner.rs` | wire pause into run/workflow; checkpoint; unified resume; workspace-sync refusal | 3 |
| `crates/rupu-cp/src/host/connector.rs` + `host/local.rs` | `pause_run`/`resume_run` (default Unsupported; Local impl) | 4 |
| `crates/rupu-cp/src/api/runs.rs` (+ `server.rs` already merges it) | `/pause` + `/resume` endpoints | 4 |
| `crates/rupu-cp/src/host/{ssh,http}.rs` + `api/` | remote pause/resume; `rupu-cli` fleet routing | 5 |
| `crates/rupu-orchestrator/src/runner.rs` (fan-out) | pause/resume distributed units | 6 |
| `crates/rupu-cli/src/cmd/{run,workflow}.rs` + `output/live_run.rs` | CLI pause/resume + Esc | 7 |
| `crates/rupu-cp/web/src/pages/RunDetail.tsx`, graph, `lib/api.ts` | Pause/Resume UI + Paused node | 8 |
| `crates/rupu-orchestrator/tests/pause_resume_e2e.rs` (new) | e2e run/workflow/fan-out | 9 |

---

## Task 1: `Paused` status + two-mode interrupt signal + events (foundation)

**Files:**
- Modify: `crates/rupu-orchestrator/src/runs.rs` (`RunStatus` ~53), `crates/rupu-orchestrator/src/executor/in_process.rs` (per-run state, `cancel`, trait), `crates/rupu-orchestrator/src/executor/event.rs` (+ sinks that construct events)
- Test: same files

**Interfaces:**
- Produces: `RunStatus::Paused` (`as_str()` → `"paused"`); `WorkflowExecutor::pause(&self, run_id) -> Result<(), ExecutorError>` and `resume(&self, run_id) -> Result<(), ExecutorError>`; per-run `Interrupt { cancel: CancellationToken, pause: CancellationToken }`; `Event::RunPaused { run_id }` / `RunResumed { run_id }` / `StepPaused { run_id, step_id }` / `StepResumed { run_id, step_id }`.

- [ ] **Step 1: Write the failing tests**

In `runs.rs` tests:
```rust
#[test]
fn paused_status_serializes_and_is_non_terminal() {
    assert_eq!(RunStatus::Paused.as_str(), "paused");
    // round-trip through the record's serde
    let j = serde_json::to_string(&RunStatus::Paused).unwrap();
    let back: RunStatus = serde_json::from_str(&j).unwrap();
    assert_eq!(back, RunStatus::Paused);
    // if a `is_terminal`/terminal helper exists on RunStatus, Paused must be false:
    // assert!(!RunStatus::Paused.is_terminal());
}
```
In `in_process.rs` tests (mirror the existing cancel test):
```rust
#[tokio::test]
async fn pause_sets_the_pause_signal_not_cancel() {
    // start a long run via the test harness used by the cancel test, call
    // executor.pause(run_id), assert the run's pause token is triggered and the
    // cancel token is NOT (pause != cancel).
}
```
In `event.rs` tests:
```rust
#[test]
fn run_paused_resumed_round_trip() {
    let ev = Event::RunPaused { run_id: "r1".into() };
    let j = serde_json::to_string(&ev).unwrap();
    assert!(j.contains("run_paused") || j.contains("RunPaused"));
    let back: Event = serde_json::from_str(&j).unwrap();
    assert!(matches!(back, Event::RunPaused { .. }));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-orchestrator -- paused_status pause_sets run_paused`
Expected: FAIL — variants/methods/events missing.

- [ ] **Step 3: Add `RunStatus::Paused`**

In `runs.rs`, add `Paused` to the enum (after `Cancelled`) and `Self::Paused => "paused"` to `as_str()`. If a `from_str`/`TryFrom<&str>`/`is_terminal` exists, add `"paused" => Paused` and ensure `Paused` is **not** terminal. Grep for exhaustive `match RunStatus` sites (CP status mapping, CLI) and add the `Paused` arm everywhere (compile will point them out).

- [ ] **Step 4: Two-mode interrupt signal**

In `in_process.rs`, the per-run state currently holds `cancel: CancellationToken`. Add `pause: CancellationToken` beside it (keep `cancel` semantics). Construct both when a run starts. `cancel(run_id)` unchanged (`state.cancel.cancel()`). Add to the `WorkflowExecutor` trait + impl:
```rust
async fn pause(&self, run_id: &str) -> Result<(), ExecutorError> {
    let runs = self.runs.lock().unwrap();
    let state = runs.get(run_id).ok_or_else(|| ExecutorError::RunNotFound(run_id.to_string()))?.clone();
    drop(runs);
    state.pause.cancel(); // "cancel" the pause token = request pause
    Ok(())
}
async fn resume(&self, run_id: &str) -> Result<(), ExecutorError> {
    // Re-launch the run from its persisted checkpoint via the existing
    // resume_from path. Implemented against run_store + the launch path the
    // executor uses; if resume can't run in this executor context it returns a
    // clear ExecutorError (the CP resume endpoint is launcher-gated anyway).
    // Fill against how `cancel`/launch are wired in this file.
}
```
The pause token is threaded to the run's agent/runner in Task 2/3 (the run task holds a clone). Give the trait methods a default (`Err(ExecutorError::...Unsupported...)` or the like) only if other impls exist; otherwise implement on `InProcessExecutor`. Update any other `WorkflowExecutor` impls/mocks to add the two methods.

- [ ] **Step 5: Add the events**

In `event.rs`, add the four variants (serde-additive; match the existing variant tagging — check whether events use `#[serde(tag="type", rename_all="snake_case")]`):
```rust
    RunPaused { run_id: String },
    RunResumed { run_id: String },
    StepPaused { run_id: String, step_id: String },
    StepResumed { run_id: String, step_id: String },
```
Add them to `Event::run_id()`'s match and any exhaustive matches in the sinks (`in_memory_sink.rs`/`jsonl_sink.rs`). No emit sites yet (Task 3 emits).

- [ ] **Step 6: Run tests, format, lint, commit**

```bash
cargo test -p rupu-orchestrator
rustfmt --edition 2021 crates/rupu-orchestrator/src/runs.rs crates/rupu-orchestrator/src/executor/in_process.rs crates/rupu-orchestrator/src/executor/event.rs
cargo clippy -p rupu-orchestrator --all-targets --no-deps
git add crates/rupu-orchestrator/src
git commit -m "feat(pause): RunStatus::Paused + two-mode interrupt + pause events (T1)"
```
Expected: new tests pass; full suite green (additive); clippy clean. (If you touched a `mod`-declaring file like `executor/mod.rs`, format it with `--skip-children`.)

---

## Task 2: Cooperative pause in the `rupu-agent` loop

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs` (`AgentRunOpts` ~535–596, `run_agent` ~632, the turn loop ~793, stream ~850, tool dispatch ~1048)
- Test: `crates/rupu-agent/src/runner.rs`

**Interfaces:**
- Consumes: nothing from T1 directly (the pause signal is a plain `tokio_util::sync::CancellationToken` passed in).
- Produces: `AgentRunOpts.pause: Option<tokio_util::sync::CancellationToken>`; `run_agent` returns, on pause, a `RunResult` whose status is a new `Paused` outcome (extend the loop's `result_status` type — add a `Paused` variant to whatever enum `result_status` is, or return `RunResult { paused: true, .. }`). A resume is just calling `run_agent` again with the persisted transcript as the seed messages (the caller in T3 does this).

- [ ] **Step 1: Write the failing tests**

Add to `runner.rs` tests, using the crate's existing fake provider + a fake tool (look at existing `run_agent` tests / `MockProvider` for the harness):
```rust
#[tokio::test]
async fn pause_during_stream_stops_and_drops_partial_text() {
    // Fake provider whose stream emits a few chunks then would continue.
    // Set opts.pause = Some(token); trigger token.cancel() after the first chunk.
    // Assert run_agent returns a Paused outcome and the persisted transcript's
    // last message is NOT the partial assistant text (ends at the prior complete
    // message; no dangling partial assistant entry).
}

#[tokio::test]
async fn pause_during_tool_lets_it_finish_and_records_result() {
    // Fake provider returns one tool call; a fake tool that sleeps briefly.
    // Trigger pause while the tool is running. Assert the tool result IS recorded
    // in the transcript, THEN the run pauses (Paused outcome), and there is no
    // dangling tool_call without a result.
}

#[tokio::test]
async fn resume_continues_from_transcript() {
    // After a pause, call run_agent again seeded with the persisted transcript;
    // fake provider now returns a final answer. Assert it completes (a fresh
    // provider request was issued — the fake records call count == +1).
}

#[tokio::test]
async fn no_pause_token_behaves_exactly_as_today() {
    // opts.pause = None → an existing run_agent test's behavior is unchanged.
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-agent -- pause_during resume_continues no_pause_token`
Expected: FAIL — `pause` field / `Paused` outcome missing.

- [ ] **Step 3: Add the pause field + outcome**

In `AgentRunOpts` add:
```rust
    /// When set and triggered, the loop stops at the next safe boundary
    /// (after the stream / after a running tool completes) and returns a
    /// `Paused` outcome instead of erroring. `None` = today's behavior.
    pub pause: Option<tokio_util::sync::CancellationToken>,
```
Update all `AgentRunOpts { .. }` construction sites (grep `AgentRunOpts {`) to add `pause: None` (or `pause: opts.pause` where threaded). Extend the loop's `result_status` enum with a `Paused` variant (or add `paused: bool` to `RunResult`), and make `run_agent` return it on pause.

- [ ] **Step 4: Cooperative checks at safe boundaries**

In `run_agent`:
- **During the stream** (~850): the stream is consumed via `provider.stream(&req, &mut on_event)`. To stop it immediately on pause, race the stream future against the pause token: `tokio::select! { r = provider.stream(...) => r, _ = pause_cancelled(&opts.pause) => { /* paused mid-stream */ } }` where `pause_cancelled` returns a future that resolves when the token is set (or immediately-pending when `None`). On the pause branch: do NOT append the partially-streamed assistant text to the transcript/messages (drop it), and break to the pause exit. (The transcript is written incrementally via `on_stream_event`; ensure the partial assistant message is only committed on a *complete* stream — if the current code commits incrementally, gate the final assistant-message commit on stream completion so a paused partial is not persisted as a message.)
- **Before/after tool dispatch** (~1048–1120): after the stream yields tool calls, if pause is set, still EXECUTE the already-decided tool calls (let them finish, record results — a tool mid-flight must not be abandoned), then after recording results check pause and exit to the pause branch instead of looping to the next turn. If pause is set BEFORE any tool executes for this turn (i.e. paused mid-stream), skip issuing new tool calls.
- **Pause exit:** return the `Paused` outcome with the transcript persisted through the last complete message/tool result.

Add a small helper:
```rust
async fn wait_pause(pause: &Option<tokio_util::sync::CancellationToken>) {
    match pause {
        Some(t) => t.cancelled().await,
        None => std::future::pending::<()>().await, // never resolves
    }
}
```

- [ ] **Step 5: Run tests, format, lint, commit**

```bash
cargo test -p rupu-agent
rustfmt --edition 2021 crates/rupu-agent/src/runner.rs
cargo clippy -p rupu-agent --all-targets --no-deps
git add crates/rupu-agent/src/runner.rs
git commit -m "feat(pause): cooperative pause in the agent loop (T2)"
```
Expected: the 4 new tests pass; existing `run_agent` tests green (no-pause path unchanged); clippy clean. (Grep for other `AgentRunOpts {` construction sites across the workspace — rupu-orchestrator/rupu-cli — and add `pause: None`; the crate builds may fail until they're updated. Update them in this task if they're pure `pause: None` additions, or note them for T3.)

---

## Task 3: Wire pause/resume into the orchestrator (run + workflow)

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`run_workflow`, `run_linear_step`, the resume path, `OrchestratorRunOpts`)
- Test: `crates/rupu-orchestrator/src/runner.rs`

**Interfaces:**
- Consumes: `RunStatus::Paused` + events + executor `pause` signal (T1); `AgentRunOpts.pause` + the `Paused` agent outcome (T2); existing `resume_from`/`ResumeState`/`AwaitingInfo`.
- Produces: `OrchestratorRunOpts.pause: Option<tokio_util::sync::CancellationToken>` threaded to each agent dispatch; a workflow that pauses → `Paused` + a `ResumeState` checkpoint; a unified `resume` entry parameterized by reason; a `workspace: sync` pause refusal.

- [ ] **Step 1: Write the failing tests** (fake provider/factory harness already used by runner tests)
```rust
#[tokio::test]
async fn agent_run_pauses_and_resumes() {
    // a linear step; trigger opts.pause during its agent run → the run result is
    // Paused (RunPaused emitted). Then resume (opts.resume_from set) → completes.
}
#[tokio::test]
async fn workflow_pauses_at_step_boundary_and_resumes_remaining() {
    // 2-step workflow; pause after step 1 completes → Paused + checkpoint with
    // step1 result persisted. Resume → runs step 2 only, completes.
}
#[tokio::test]
async fn workspace_sync_workflow_pause_is_refused() {
    // a workspace: sync workflow + pause requested → clear error (mirrors
    // ResumeWithWorkspaceSync).
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-orchestrator -- agent_run_pauses workflow_pauses workspace_sync_workflow_pause`
Expected: FAIL.

- [ ] **Step 3: Thread the pause signal + handle the Paused outcome**

- Add `OrchestratorRunOpts.pause: Option<CancellationToken>`; pass a clone into every `AgentRunOpts { .. pause: opts.pause.clone() }` (linear, fan-out unit inline path, panel/parallel — set on the agent dispatch). Update all `AgentRunOpts {` sites to include `pause` (finishes T2's note).
- In `run_linear_step`: if the agent returns the `Paused` outcome, build a `StepResult`/run outcome that signals paused (not success/failure), set the run record `Paused`, emit `Event::RunPaused` (+ `StepPaused`), and unwind the workflow loop into a **paused checkpoint** (persist `ResumeState` via the same mechanism `awaiting_approval` uses).
- In the step loop (`run_steps_inner`): between steps, check `opts.pause` (if triggered) → stop before the next step, persist the checkpoint, set `Paused`, emit `RunPaused`. (Step-boundary pause.)
- **Resume:** the existing resume path (driven by `opts.resume_from`) already re-runs from persisted step results. Parameterize its "why did it pause" so both `AwaitingApproval` and `Paused` flow through it (a `PauseReason { Approval, Manual }` on the checkpoint/AwaitingInfo, or reuse the status). A paused-incomplete step re-runs from its transcript on resume.
- **workspace: sync refusal:** at the point pause would checkpoint a `workspace: sync` workflow, return a clear error (reuse/mirror `RunWorkflowError::ResumeWithWorkspaceSync` — add a `PauseWithWorkspaceSync` variant or reuse) with an inline `// TODO(pause-workspace-sync): support delta-persisting resume so workspace:sync workflows can pause/resume`.

- [ ] **Step 4: Run tests, format, lint, commit**

```bash
cargo test -p rupu-orchestrator
rustfmt --edition 2021 crates/rupu-orchestrator/src/runner.rs
cargo clippy -p rupu-orchestrator --all-targets --no-deps
git add crates/rupu-orchestrator/src/runner.rs
git commit -m "feat(pause): wire pause/resume into run + workflow orchestration (T3)"
```
Expected: the 3 new tests pass; full suite green (no-pause path byte-for-byte); clippy clean.

---

## Task 4: `rupu-cp` — `pause_run`/`resume_run` + `/api/runs/:id/pause|resume` (local)

**Files:**
- Modify: `crates/rupu-cp/src/host/connector.rs` (`HostConnector` trait), `crates/rupu-cp/src/host/local.rs` (impl), `crates/rupu-cp/src/api/runs.rs` (endpoints; `server.rs` already merges `runs::routes()`)
- Test: `crates/rupu-cp/src/api/runs.rs`, `crates/rupu-cp/src/host/local.rs`

**Interfaces:**
- Produces: `HostConnector::pause_run(&self, run_id) -> Result<(), HostConnectorError>` + `resume_run(...)` (default `Err(HostConnectorError::Unsupported("pause".into()))`); `LocalHostConnector` impls (delegate to the in-process executor's `pause`/`resume`); `POST /api/runs/:id/pause` + `POST /api/runs/:id/resume`.

- [ ] **Step 1: Write the failing tests** (mirror api/runs.rs's cancel test harness + AppState builder)
```rust
#[tokio::test]
async fn pause_running_local_run_sets_paused() { /* start run, POST /pause, assert Paused */ }
#[tokio::test]
async fn pause_terminal_run_is_rejected() { /* completed run → 4xx */ }
#[tokio::test]
async fn resume_requires_launcher() { /* launcher None → 501 */ }
#[tokio::test]
async fn resume_non_paused_run_is_rejected() { /* Running/Completed → 4xx */ }
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p rupu-cp --lib api::runs -- pause resume`

- [ ] **Step 3: Implement**

- `connector.rs`: add the two trait methods with default `Unsupported`.
- `local.rs`: implement `pause_run`/`resume_run` by calling the in-process executor's `pause`/`resume` (LocalHostConnector already holds the run_store / executor handle it uses for cancel — reuse that path).
- `api/runs.rs`: add routes `.route("/api/runs/:id/pause", post(pause_run)).route("/api/runs/:id/resume", post(resume_run))`. `pause_run` handler: look up the run; if terminal → `ApiError::conflict("run is not running")`; else route to the owning host's `pause_run` (or the in-process executor), map errors. `resume_run`: **launcher-gated** (`require_writable`/`not_available` → 501); if the run isn't `Paused` → `ApiError::conflict`; else resume via the executor/host; map errors.

- [ ] **Step 4: Run tests, format, lint, commit**

```bash
cargo test -p rupu-cp --lib
rustfmt --edition 2021 crates/rupu-cp/src/host/connector.rs crates/rupu-cp/src/host/local.rs crates/rupu-cp/src/api/runs.rs
cargo clippy -p rupu-cp --no-deps
git add crates/rupu-cp/src
git commit -m "feat(pause): HostConnector pause/resume + /api/runs pause|resume (T4)"
```

---

## Task 5: Remote transports (SSH/HttpCp) + fleet routing

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs`, `crates/rupu-cp/src/host/http.rs` (+ `api/` for HttpCp endpoints if the remote is a CP), `crates/rupu-cli/src/fleet_unit_dispatcher.rs` (fakes + routing)
- Test: those files

**Interfaces:** SSH/HttpCp `pause_run`/`resume_run` reaching the remote `rupu`'s in-process executor (the remote runs this same feature); Bucket/Tunnel inherit `Unsupported`.

- [ ] **Step 1: Write the failing tests** — with a fake `RemoteExec`/connector: `ssh_pause_run_invokes_remote`, `http_pause_resume_round_trip`, `bucket_pause_unsupported`.
- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement.** SSH: reach the remote via the same mechanism `cancel_run` uses (a remote `rupu` command / CP call); HttpCp: POST to the remote CP's new `/api/runs/:id/pause|resume` (which exist after T4, since the remote is a CP). Bucket/Tunnel keep the `Unsupported` default. Update the fake `cancel_run` impls in fleet_unit_dispatcher.rs to also implement `pause_run`/`resume_run` (default Unsupported or recorded).
- [ ] **Step 4: tests/format/lint/commit** — `cargo test -p rupu-cp --lib host`, `cargo test -p rupu-cli --lib fleet_unit_dispatcher`; per-file rustfmt; commit `feat(pause): remote SSH/HttpCp pause/resume + fleet routing (T5)`.

---

## Task 6: Distributed fan-out pause/resume

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`run_fanout_step`)
- Test: same file

- [ ] **Step 1: Write the failing test** (fake UnitDispatcher, mirror `resume_reruns_only_failed_fanout_units`): pause a `distribute:` fan-out mid-flight → in-flight units pause at their boundary, completed units keep results, not-yet-dispatched units aren't dispatched; resume re-dispatches only the incomplete/paused units and the step completes.
- [ ] **Step 2: Run to verify it fails.**
- [ ] **Step 3: Implement.** Thread `opts.pause` into the per-unit dispatch (each unit's agent honors it via T2/T3). On pause: stop dispatching new units, let in-flight units pause at their boundary, record completed unit results in the checkpoint, mark the step `Paused`. Resume: the existing fan-out resume path re-dispatches units whose recorded state is incomplete/paused (extend the incomplete-filter to include `Paused`). Per-unit paused state persisted in the checkpoint (`unit_checkpoints.jsonl` / the existing fan-out resume state).
- [ ] **Step 4: tests/format/lint/commit** — `cargo test -p rupu-orchestrator`; rustfmt runner.rs; commit `feat(pause): distributed fan-out pause/resume (T6)`.

---

## Task 7: CLI — `pause`/`resume` subcommands + Esc in the live-run view

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs`, `crates/rupu-cli/src/cmd/workflow.rs`, `crates/rupu-cli/src/output/live_run.rs`, `crates/rupu-cli/src/lib.rs` (subcommand wiring)
- Test: those files

- [ ] **Step 1: Write the failing tests** — arg-parse tests for `rupu run pause|resume <id>` / `rupu workflow pause|resume <id>`; a live_run event-handling test that a `RunPaused`/`RunResumed` event updates the view state (mirror existing live_run tests).
- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement.** Add `pause`/`resume` actions to the run/workflow subcommands (call the CP `/api/runs/:id/pause|resume`, or the local executor for `rupu workflow run`). In `live_run.rs`, on Esc trigger pause (call pause), render a "paused — press <key> to resume" affordance; handle `RunPaused`/`RunResumed` in the event `apply`. Mirror how session Esc (`cancel_active_turn`) is wired.
- [ ] **Step 4: tests/format/lint/commit** — `cargo test -p rupu-cli --lib` (scope to changed modules; ignore pre-existing session-test failures); per-file rustfmt (NOT lib.rs bare — use `--skip-children` if needed); commit `feat(pause): CLI pause/resume + Esc in live-run (T7)`.

---

## Task 8: Web — CP Pause/Resume UI

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/RunDetail.tsx`, the graph view (NodeStatus), `crates/rupu-cp/web/src/lib/api.ts`
- Test: `crates/rupu-cp/web/src/pages/RunDetail.test.tsx`

- [ ] **Step 1: Write the failing vitest** — mock a Running run: a **Pause** button calls `POST /api/runs/:id/pause`; mock a Paused run: a **Resume** button calls `/resume`; a Paused node renders a Paused status style; a 501 on resume renders a read-only message. Mirror the existing Approve/Reject button tests.
- [ ] **Step 2: Run to verify it fails.**
- [ ] **Step 3: Implement.** api.ts: `pauseRun(id)` / `resumeRun(id)`. RunDetail: Pause button when Running, Resume when Paused (mirror Approve/Reject placement/handlers). Graph: add a `Paused` NodeStatus (color/label) for paused nodes. Render `RunPaused`/`RunResumed` in the run model/event stream; a status dot for Paused. No secret concerns.
- [ ] **Step 4: web checks + commit** — `cd crates/rupu-cp/web && npm test -- RunDetail && npx tsc --noEmit && npm run build`; commit `feat(pause): CP RunDetail pause/resume UI + paused node (T8)`.

---

## Task 9: e2e — pause/resume round-trips

**Files:**
- Create: `crates/rupu-orchestrator/tests/pause_resume_e2e.rs`
- Test: that file

- [ ] **Step 1: Write the e2e tests** (mirror the existing orchestrator e2e harnesses — fake provider/factory + a pause token):
```rust
// pseudocode shape — fill against the real run_workflow/opts + fake dispatcher
#[tokio::test]
async fn run_pause_then_resume_completes() {
    // start an agent run with a pause token; trigger pause mid-run →
    // assert RunStatus::Paused + a RunPaused event + no partial/half-done
    // transcript (no dangling tool call / partial assistant msg). Then resume
    // (resume_from) → assert it completes.
}
#[tokio::test]
async fn workflow_pause_resume_runs_remaining_steps() {
    // 2-step workflow; pause at the boundary → Paused + checkpoint; resume →
    // step 2 runs, run Completed.
}
#[tokio::test]
async fn fanout_pause_resumes_only_incomplete_units() {
    // distribute step; pause mid-fanout → completed units kept, incomplete paused;
    // resume → only incomplete re-dispatched; step Completed.
}
```
- [ ] **Step 2: Run + format + lint + commit** — `cargo test -p rupu-orchestrator --test pause_resume_e2e`; `cargo test -p rupu-orchestrator`; rustfmt the new file; commit `test(pause): e2e run/workflow/fan-out pause+resume (T9)`.

---

## Self-Review

**Spec coverage:**
- `Paused` status + two-mode signal + events → T1. ✅
- Boundary (stop stream/drop partial, let tool finish) + agent resume → T2. ✅
- Run + workflow wiring, checkpoint, unified resume, workspace-sync refusal + TODO → T3. ✅
- API + local host pause/resume, gating, terminal/non-paused rejection → T4. ✅
- Remote (SSH/HttpCp) + Bucket/Tunnel Unsupported + fleet routing → T5. ✅
- Fan-out pause (in-flight pause, keep completed, resume incomplete) → T6. ✅
- CLI pause/resume + Esc → T7. ✅
- CP UI (buttons, Paused node, events) → T8. ✅
- e2e (run/workflow/fan-out) → T9. ✅
- No-silent-noop / non-terminal / launcher-gated resume / backward-compat → enforced in T1/T3/T4 + asserted across tests. ✅

**Placeholder scan:** T1/T2 carry complete code for the foundational status/signal/events + the agent-loop `select!`/`wait_pause` boundary logic; T3–T9 give exact interfaces, grounded test cases, and concrete algorithms pointing at named existing patterns (the cancel path, `resume_from`, `resume_reruns_only_failed_fanout_units`, the Approve/Reject buttons, the api/runs cancel harness) — deliberate for tasks that must read substantial existing code, not vague "handle it". No "TBD"/vacuous-test placeholders. The `resume` executor body (T1 Step 4) and the SSH/HttpCp remote reach (T5) are the two spots the implementer fills against the exact existing cancel/launch wiring — flagged as such.

**Type consistency:** `RunStatus::Paused`/`as_str "paused"` (T1) used everywhere; `Event::{RunPaused,RunResumed,StepPaused,StepResumed}` (T1) emitted in T3, consumed in T7/T8; `AgentRunOpts.pause` + the `Paused` agent outcome (T2) consumed by T3/T6; `OrchestratorRunOpts.pause` (T3) threaded in T6; `HostConnector::{pause_run,resume_run}` (T4) impl'd in T5, called by the fleet + api; `/api/runs/:id/{pause,resume}` (T4) consumed by T7 CLI + T8 web. Names align across tasks.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-01-rupu-pause-resume-plan.md`. Build via subagent-driven-development: fresh implementer per task, task review (spec + quality) after each, a broad whole-branch review at the end, then a single PR to `main` (no self-merge — matt reviews and validates the CP Pause/Resume UI before merge).
