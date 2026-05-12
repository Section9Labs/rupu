# Slice D Plan 3 — Live executor wiring + status pulse (design)

> **Status:** design ready · brainstorm complete · awaiting plan write-up

## Goal

Make the `rupu.app` Graph view (D-2) come alive. Workflow steps light up in real time as they execute, the drill-down pane streams the focused step's transcript, and the user can approve/reject `ask`-mode steps from inside the app (inline on the awaiting node *and* in the drill-down pane). Same code path serves runs started from the app (in-process) and runs started elsewhere — CLI, cron, MCP — by tailing the new on-disk event stream.

## Scope

**In scope:**

- New traits in `rupu-orchestrator`: `WorkflowExecutor`, `EventSink`, `Event` enum (step-level only).
- Concrete impls: `InProcessExecutor`, `InMemorySink` (broadcast), `JsonlSink` (on-disk `events.jsonl`), `FileTailRunSource` (notify-backed consumer for disk runs).
- `rupu-app` integration: `AppExecutor`, `RunModel`, live Graph view, drill-down pane with transcript stream, inline + drill-down approval UI, sidebar status dots, menubar badge wired to pending approvals.
- CLI refactor: `rupu run` and `rupu watch` route through the new executor traits with no user-visible behavior change.

**Out of scope (deferred):**

- Status pulse *animation* beyond the basic glyph color flip (breathing/glow on the active node lands in a polish pass).
- ForEach / Parallel fan-out rendering in the Graph view (D-2 collapsed these to a single linear row; D-3 keeps that — full fan-out arrives with Canvas view D-6).
- Run history list / "Recent runs" sidebar section (arrives with Transcript view D-8).
- Multi-run-per-workflow concurrency UI — `start()` is blocked if a run is already active for the same workflow; queueing / parallel runs is a D-4+ concern.
- Cross-workspace "Approvals inbox" view — menubar badge counts and jumps to the most recent, but no global queue UI.
- Cancel / pause buttons in the app (the trait method exists; CLI keeps being the sole consumer in D-3).
- `StepWorking` beacons from tools other than `rupu-agent` — only the agent's `on_tool_call` callback feeds them.
- Event-stream compression / GC — `events.jsonl` is uncapped per run.

## Background

- Slice D spec: `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md` §10 lists D-3 as the sub-slice that lands `WorkflowExecutor` + `EventSink` traits, the `InProcessExecutor` + `InMemorySink` + `JsonlSink` impls, "App subscribes to runs; Graph view comes alive. Drill-down pane works. Approve / reject from desktop." This design is that sub-slice fully specified.
- D-2 (just merged, PRs #196 and #197) introduced the pure-Rust `rupu-app-canvas` crate with the `NodeStatus` enum and the git-graph `Vec<GraphRow>` emitter, plus the GPUI `view::graph` renderer in `rupu-app`. All nodes currently render in `NodeStatus::Waiting`. D-3 makes them transition.
- Prior art: `rupu-tui`'s `JsonlTailSource` (`crates/rupu-tui/src/source/jsonl_tail.rs`) watches `run.json` + per-step transcript JSONLs via `notify` and pushes `SourceEvent`s into `App::apply()`. D-3 borrows the shape of this approach for the disk-tail variant, but pivots from "tail many files" to "tail one strictly-typed event log".

## Architecture

The system is four layers, top to bottom:

1. **`rupu-orchestrator::executor`** — new module holding the trait surface, the in-process executor, and the sinks. Pure Rust, async-tokio, no UI dep.
2. **`rupu-app::executor`** — thin app wrapper. Owns an `Arc<InProcessExecutor>`, decides whether to attach in-process or disk-tail for a given `run_id`, and exposes a uniform `EventStream` to the rest of the app.
3. **`rupu-app::run_model`** — `RunModel` struct holding per-run mutable state (node statuses, focused step, transcript). Pure function `(RunModel, Event) -> RunModel` mutator. Snapshot-testable.
4. **`rupu-app::view`** — GPUI views consuming `RunModel`: `view::graph` (extended to read statuses from `RunModel`), `view::drilldown` (new).

The CLI keeps its existing `rupu run` / `rupu watch` UX but uses the same `WorkflowExecutor` trait under the hood. The TUI is unchanged in D-3 — its `JsonlTail` source continues to work because the existing `run.json` + `transcripts/` files are untouched; the new `events.jsonl` is additive.

## Trait surface

```rust
// crates/rupu-orchestrator/src/executor/mod.rs

pub trait WorkflowExecutor: Send + Sync {
    async fn start(&self, opts: WorkflowRunOpts) -> Result<RunHandle, ExecutorError>;
    fn list_runs(&self, filter: RunFilter) -> Vec<RunRecord>;
    fn tail(&self, run_id: &str) -> Result<EventStream, ExecutorError>;
    async fn approve(&self, run_id: &str, approver: &str) -> Result<(), ExecutorError>;
    async fn reject(&self, run_id: &str, reason: &str) -> Result<(), ExecutorError>;
    async fn cancel(&self, run_id: &str) -> Result<(), ExecutorError>;
}

pub struct RunHandle {
    pub run_id: String,
    pub workflow_path: PathBuf,
}

pub struct WorkflowRunOpts {
    pub workflow_path: PathBuf,
    pub vars: HashMap<String, String>,
    // Mirrors today's OrchestratorRunOpts, minus run_store (the executor owns the store).
}

pub enum RunFilter {
    All,
    ByWorkflowPath(PathBuf),
    ByStatus(RunStatus),
    Active,
}

pub type EventStream = Pin<Box<dyn Stream<Item = Event> + Send>>;

pub trait EventSink: Send + Sync {
    fn emit(&self, run_id: &str, ev: &Event);
}
```

### Event enum

Step-level only. Transcript lines stay in their per-step JSONL files; `rupu-agent` is not refactored.

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    RunStarted {
        event_version: u32,            // 1
        run_id: String,
        workflow_path: PathBuf,
        started_at: chrono::DateTime<chrono::Utc>,
    },
    StepStarted {
        run_id: String,
        step_id: String,
        kind: StepKind,
        agent: Option<String>,
    },
    StepWorking {
        run_id: String,
        step_id: String,
        note: Option<String>,           // e.g. "calling tool: gh_pr_list"
    },
    StepAwaitingApproval {
        run_id: String,
        step_id: String,
        reason: String,
    },
    StepCompleted {
        run_id: String,
        step_id: String,
        success: bool,
        duration_ms: u64,
    },
    StepFailed {
        run_id: String,
        step_id: String,
        error: String,
    },
    StepSkipped {
        run_id: String,
        step_id: String,
        reason: String,
    },
    RunCompleted {
        run_id: String,
        status: RunStatus,
        finished_at: chrono::DateTime<chrono::Utc>,
    },
    RunFailed {
        run_id: String,
        error: String,
        finished_at: chrono::DateTime<chrono::Utc>,
    },
}
```

Notes:

- `Event` is `Serialize + Deserialize` so the same enum round-trips through `JsonlSink` (writes JSON Lines) and through the in-process broadcast channel.
- `event_version` lives on `RunStarted` only — it's the manifest version for the whole `events.jsonl` file. Readers reject unknown major versions; minor bumps are additive thanks to `#[serde(other)]` on the enum.
- `StepWorking` is a coarse "still alive" beacon. The runner translates `rupu-agent`'s `on_tool_call` callback into `StepWorking { note: Some(tool_name) }`. It drives the `Active → Working` glyph in the Graph view. Fine-grained tool detail stays in the transcript JSONL.

## Implementations

### Crate layout

```
crates/rupu-orchestrator/src/executor/
  mod.rs              # WorkflowExecutor + EventSink traits, Event enum, errors
  in_process.rs       # InProcessExecutor — spawns one tokio task per active run; runs are Arc<RunState>
  in_memory_sink.rs   # InMemorySink — tokio::sync::broadcast::Sender<Event>
  jsonl_sink.rs       # JsonlSink — appends events to <run_dir>/events.jsonl
  fan_out_sink.rs     # FanOutSink — Vec<Arc<dyn EventSink>>; runner uses this internally
  file_tail.rs        # FileTailRunSource — consumes events.jsonl for runs the app didn't start
  errors.rs           # ExecutorError (thiserror)
```

### `InProcessExecutor`

- Holds `runs: Arc<Mutex<HashMap<String, Arc<RunState>>>>` keyed by `run_id`.
- Each `RunState` carries `Arc<InMemorySink>` (broadcast sender, owned by the executor), `Arc<JsonlSink>` (one writer per run), a tokio `JoinHandle`, the current `RunStatus`, and a `tokio_util::sync::CancellationToken`.
- `start(opts)` spawns a tokio task that calls the existing `run_workflow()` under a wrapper that intercepts each step transition and calls `sink.emit()`. The wrapper is implemented via a new `RunnerCallbacks` struct passed into `run_workflow` — see "Runner wiring" below.
- `tail(run_id)` first replays the last N events from a small per-run in-memory ring buffer (so a late subscriber sees the run from the beginning), then attaches a `BroadcastStream` for live events. The ring is bounded by the worst-case event count for a typical workflow size (1024 events default; configurable).
- `approve(run_id, approver)` / `reject(run_id, reason)` mutate `run.json` via the existing `RunStore::approve()` / `reject()` paths, then push the appropriate `Event` onto the sink so subscribers unstick immediately.
- `cancel(run_id)` triggers the `CancellationToken`; the runner's step loop checks it between steps and emits `RunFailed { error: "cancelled" }`.

### `InMemorySink`

- Wraps `tokio::sync::broadcast::Sender<Event>` with a 1024-slot buffer.
- `emit()` is non-blocking — uses `Sender::send` and drops on lag (it returns `Err(SendError)` only when there are no subscribers, which we ignore).
- Slow subscribers see `RecvError::Lagged(n)`; they reconcile by switching to a `FileTailRunSource` against `events.jsonl` and replaying. The app's `AppExecutor::attach` handles this transparently.

### `JsonlSink`

- One file per run: `<run_dir>/events.jsonl`. One serialized `Event` per line.
- Append-only writer, `fsync` on drop, never rotated. Events are bounded by run size; runaway workflows are an orchestrator-level concern, not a sink-level one.
- Lives alongside the existing `run.json` + `step_results.jsonl` + `transcripts/<step_id>.jsonl` files. `events.jsonl` does **not** replace `step_results.jsonl` — both exist, serving different purposes:
  - `step_results.jsonl` is the durable step-result archive (consumed by `rupu transcript` and similar).
  - `events.jsonl` is the live event log for UI / disk-tail consumers.

### `FileTailRunSource`

- Counterpart to `rupu-tui`'s `JsonlTail`, but specifically for `events.jsonl`.
- Uses `notify` to watch the file for modification; on each event, reads new lines from `last_offset` and yields parsed `Event`s through a `Stream`.
- Same `Stream<Item = Event>` shape as `BroadcastStream`, so `EventStream` is interchangeable regardless of source.
- Handles file-not-yet-created (waits for create event), file truncation (logs warning, resyncs from offset 0), and concurrent appends (the writer's `fsync` semantics + line-oriented format make atomic reads trivial).
- For runs that lack `events.jsonl` (pre-D-3 runs), falls back to reading `step_results.jsonl` and synthesizing `Event::StepCompleted` per record. Synthesized events lack `StepStarted`/`StepWorking` granularity, so the Graph shows the run as a sequence of completed steps with no live pulse. Good enough for archived runs.

### Runner wiring

The only change to existing orchestrator code:

- `OrchestratorRunOpts` gains an `Option<Arc<dyn EventSink>>` field. When `Some`, the runner calls `sink.emit(...)` at each transition (`RunStarted` before the first step, `StepStarted` / `StepCompleted` / etc. for each step, `RunCompleted` after the last). When `None`, behavior is unchanged — direct callers (none exist in production, but tests do) keep working.
- A new `RunnerCallbacks { on_tool_call: Box<dyn Fn(&str, &str)> }` is passed into `rupu-agent::run_agent`. The agent invokes `on_tool_call(step_id, tool_name)` whenever it dispatches a tool. The runner translates that into `Event::StepWorking { note: Some(tool_name) }` and emits.
- No other change to the runner. Step iteration, panel handling, approval gating, and exit codes all stay as they are.

## On-disk run layout

```
<rupu-state>/runs/<run_id>/
  run.json                       # existing — RunRecord (status, started_at, etc.)
  step_results.jsonl             # existing — one StepResultRecord per completed step
  events.jsonl                   # NEW — append-only Event stream (JsonlSink writes this)
  transcripts/
    <step_id>.jsonl              # existing per-step transcript (rupu-agent owns these)
```

- No existing files change format.
- `<rupu-state>` resolves via the existing `RunStore` path resolver: `$XDG_STATE_HOME/rupu/runs/` on Linux, `~/Library/Application Support/rupu/runs/` on macOS.
- New runs always get `events.jsonl`; old runs (no file) use the `step_results.jsonl` fallback in `FileTailRunSource`.

## App integration

### Crate layout (additions to `rupu-app`)

```
crates/rupu-app/src/
  executor/
    mod.rs            # AppExecutor — wraps Arc<InProcessExecutor>, app-side entry point
    attach.rs         # decision: in-process tail OR FileTailRunSource for a given run_id
  run_model.rs        # RunModel — mutable state per attached run; pure apply(Event)
  view/
    graph.rs          # (extended) — takes &RunModel; nodes carry NodeStatus from RunModel
    drilldown.rs      # NEW — focused-step pane: transcript stream + approval buttons
  window/
    mod.rs            # (extended) — Graph view + drill-down in a horizontal split
    sidebar.rs        # (extended) — workflow rows show a status dot for active runs
```

### `AppExecutor`

- Singleton per app instance. Wraps `Arc<InProcessExecutor>`.
- `start_workflow(path) -> RunHandle` — delegates to the executor's `start()`.
- `attach(run_id) -> Result<EventStream, AttachError>` — checks `executor.list_runs(Active)` first; if the executor knows about this `run_id` it uses `executor.tail()` (in-process broadcast); otherwise it constructs a `FileTailRunSource` against the run's `events.jsonl` on disk. Returns the unified `EventStream`.
- `approve(run_id, approver)` / `reject(run_id, reason)` — for in-process runs delegates to the executor; for disk-tail runs mutates `run.json` via `RunStore` directly (same as TUI does today). Either way the next event arrives through whichever stream the caller is subscribed to.

### `RunModel`

- Owns `nodes: BTreeMap<step_id, NodeStatus>`, `active_step: Option<String>`, `focused_step: Option<String>`, plus the run metadata (`run_id`, `workflow_path`, `status`).
- `apply(Event) -> RunModel` is the only mutator — pure function, easy to snapshot-test.
- Lives in a GPUI `Model<RunModel>`; the window subscribes and the Graph view + drill-down re-render on update.

### Sidebar status dots

- Each workflow row in the sidebar queries `app_executor.list_runs(ByWorkflowPath(path), Active)` on render.
- Visual states:
  - **Active** → small blue dot, pulsing (subtle, 1Hz).
  - **AwaitingApproval** → small yellow dot, steady.
  - **Failed** (within last 10s) → small red dot, then auto-clears.
  - **Idle** → no dot.
- Clicking a row with an active run opens that run's `RunModel` in the main pane. Clicking a row with no active run shows the static `&Workflow` Graph (D-2 behavior) with a **Run** button in the toolbar.

### Window layout

- Horizontal split: Graph view on the left, drill-down pane on the right.
- Drill-down is collapsed by default (Graph takes full width) and slides open when `focused_step` becomes `Some`.
- Drill-down close button (`✕`) and `Escape` collapse it back; focus returns to the Graph.
- Resize handle between the two; drag to resize, persisted per-workspace in `workspace.toml` as `drilldown_width: u32`.
- The existing Graph view toolbar gets a **Run** button on the right. Disabled (greyed) while a run is active for this workflow; hover tooltip when disabled: "A run is already active for this workflow". Re-running after a finished run starts a new `run_id`.

### Drill-down pane

Top to bottom:

1. **Header row** — step name, agent name (e.g. `tour-perf-reviewer`), status pill (color-matched to `NodeStatus`), duration, close button.
2. **Approval bar** (only when status is `AwaitingApproval`) — green **Approve** button + red **Reject** button + reason input. Same approve/reject path as the inline buttons on the node.
3. **Transcript stream** — scrolling list of lines from `<run_dir>/transcripts/<step_id>.jsonl`. Auto-scrolls to bottom while the step is active; pinning (user scrolls up manually) pauses auto-scroll until they scroll back down or click a "Jump to end" affordance.

**Transcript rendering:**

- Reuse the line-stream vocabulary: bullet glyph + status color + monospace text, one line per transcript record. Subtle indentation visually pairs tool calls with tool results.
- D-3 renders the raw `text` / `tool_name` / `args_summary` from the existing transcript schema. Polish (token streaming, syntax highlighting) is deferred.

**Transcript source:**

- Per-focused-step watcher against the existing `transcripts/<step_id>.jsonl` file. Structurally identical to `FileTailRunSource` (same `notify`-driven file tail + offset bookkeeping) but with a different per-line parser (transcript record, not `Event`). Lives in `rupu-app::view::drilldown` and is local to the pane.
- One watcher active at a time. When `focused_step` changes, drop the old watcher and start a new one.
- For completed steps, the file is read in full on attach (no live tail needed). For active steps, `notify` drives appends.

### Approval UI

Approve / reject lives in two surfaces simultaneously:

- **Inline on the node** — when a node enters `AwaitingApproval`, render two small pill buttons (`✓` / `✗`) to the right of the node's label in the Graph row. One-click decision; inline reject uses a default reason `"rejected from graph"`.
- **Drill-down pane** — full Approve / Reject buttons + reason text field. Reject without text falls back to `"rejected from drill-down"`.

Both surfaces call the same `app_executor.approve / reject` path.

**Auto-focus on approval:** when any active run emits `StepAwaitingApproval`, the window's `focused_step` flips to that step automatically and the drill-down slides open — **unless** the user has manually focused a different step within the last 10s (avoids ripping focus while they're reading).

**Menubar badge:** the D-1 stub becomes real. Counts pending approvals across all open workspaces. Clicking the badge opens the most-recent workspace (by `last_active_at`) and focuses the awaiting node.

**Keyboard shortcuts:**

| Key | Action |
|---|---|
| `a` | Approve the focused step (when awaiting) |
| `r` | Reject the focused step; opens reason input in drill-down |
| `Escape` | Close the drill-down pane |
| `Tab` / `Shift-Tab` | Cycle focus through Graph nodes |

## CLI refactor

`rupu run`:

- Today: `rupu-cli/src/cmd/workflow.rs::Action::Run` calls `rupu_orchestrator::run_workflow()` directly with an optional `RunStore`.
- After D-3: same command builds a local `InProcessExecutor` (CLI-flavored — no broadcast channel needed, just attaches `JsonlSink`) and calls `executor.start(opts).await`. It then attaches a tail to print step progress to stdout — preserving today's CLI output verbatim.
- `--watch` (already present for `rupu watch`) reuses `FileTailRunSource` against `events.jsonl`. Today's implementation tails per-step transcript JSONLs; after D-3 it has a strictly-typed event stream to consume.

`rupu watch <run_id>`:

- Becomes `app_executor.attach(run_id)` semantically — same code path the app uses.

**Behavior preservation:** every existing CLI test (`crates/rupu-cli/tests/cli_usage.rs`, `rupu-tui/tests/*`) keeps passing. The refactor adds an indirection but no user-visible change.

## Backwards compat

- TUI's `JsonlTail` keeps tailing `run.json` + `transcripts/*.jsonl` as today. We add an optional `events.jsonl` consumer to TUI in a follow-up — TUI works either way during D-3.
- Old runs (no `events.jsonl`) are still openable in the app: `FileTailRunSource` falls back to `step_results.jsonl` synthesis (described above).
- The `RunRecord` schema in `run.json` is unchanged. `step_results.jsonl` format is unchanged. Per-step transcript JSONL format is unchanged.

## Error handling

- `ExecutorError` (`thiserror`) covers: `WorkflowParse(rupu_orchestrator::WorkflowParseError)`, `RunNotFound(String)`, `RunAlreadyActive(String)`, `Io(std::io::Error)`, `Cancelled`, `Internal(String)`.
- `AttachError` (in `rupu-app`) covers: `RunNotFound`, `Io(std::io::Error)`, `InvalidEventStream(serde_json::Error)`.
- All errors surface to the app as a non-modal toast at the bottom of the window (status pill + dismiss button). Errors are logged via `tracing::error` regardless.
- `JsonlSink` write failures are logged via `tracing::warn` but do **not** propagate — losing the on-disk event log is recoverable (subscribers still see the in-process broadcast); failing the run for a disk error is the wrong call.

## Testing strategy

| Layer | What we test | How |
|---|---|---|
| `Event` enum | round-trip JSON serialization, version field handling, unknown variant tolerance | `serde_json` unit tests + insta snapshots |
| `InProcessExecutor` | start → events emitted in order; approve unsticks; cancel stops emission | integration test in `rupu-orchestrator/tests/` with `MockProvider` + `BypassDecider` |
| `InMemorySink` | broadcast fan-out to multiple subscribers; lag handling | `tokio::test` + multiple subscribers |
| `JsonlSink` | line-by-line append; fsync on drop; schema-stable | filesystem integration test with `tempdir` |
| `FileTailRunSource` | yields events as file grows; handles file-not-yet-created; recovers from truncation; synthesizes from `step_results.jsonl` when `events.jsonl` is absent | `notify`-backed integration test |
| `RunModel::apply` | every `Event` variant → expected `NodeStatus` transition | pure-function unit tests + insta snapshots |
| `AppExecutor::attach` | routes correctly between in-process tail and disk-tail | integration test |
| CLI back-compat | `rupu run` / `rupu watch` / `rupu transcript list` behave identically | existing `rupu-cli/tests/` continue to pass |
| App smoke | start a workflow from the app; Graph nodes flip Waiting → Active → Complete; drill-down streams transcript; Approve unsticks an `ask`-mode step | `make app-smoke` extended with a scripted run |

## Acceptance criteria

1. From the app, clicking **Run** on a workflow containing an `ask`-mode step shows the nodes transition Waiting → Active → AwaitingApproval; clicking Approve in the drill-down (or the inline button) unsticks the run and the next node lights up.
2. Running the same workflow from `rupu run` while the app is open shows the same Graph come alive in the app via disk-tail.
3. `make app-smoke` and the full workspace gates (`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`) all pass.
4. Existing TUI behavior is unchanged — TUI tests pass without modification.

## Implementation phases

The plan author should decompose into roughly these task clusters, in order:

1. `Event` enum + `EventSink` + `JsonlSink` + tests (foundation, no behavior change).
2. `WorkflowExecutor` trait + `InProcessExecutor` + `InMemorySink` + tests.
3. Runner wiring: `OrchestratorRunOpts::event_sink` + `RunnerCallbacks::on_tool_call` + tests.
4. `FileTailRunSource` + fallback to `step_results.jsonl` + tests.
5. CLI refactor (`rupu run` and `rupu watch` route through the new traits) + existing test pass.
6. `rupu-app::executor` + `RunModel::apply` + snapshot tests.
7. `view::graph` extension — consumes `RunModel` instead of `&Workflow`.
8. `view::drilldown` + transcript-file watcher.
9. Sidebar status dots + Run button toolbar.
10. Inline approval buttons on nodes + drill-down approval bar.
11. Menubar badge wired to pending approvals.
12. `make app-smoke` extension + workspace gates.
13. `CLAUDE.md` updates + Slice D progress note in the slice spec.
