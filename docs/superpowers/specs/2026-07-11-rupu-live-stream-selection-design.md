# rupu — Live-view stream selection + sub-agent dispatch nodes

Status: Draft (design) — pending review
Date: 2026-07-11

## Context

The live three-zone workflow view (`rupu run` / `rupu workflow run`, alt-screen TUI in `crates/rupu-cli/src/output/live_run.rs`) shows a dashboard, a git-graph spine of steps, and a focus/stream zone. When a `for_each`/`parallel` step fans out, each iteration appears as a **live node** in the spine (driven by orchestrator `UnitStarted`/`UnitCompleted` events, keyed `(step_id, index)`, each carrying its own `transcript_path`). Two gaps motivate this work (both verified in-code):

1. **No stream selection.** The focus zone (zone 3) is **auto-follow / last-writer-wins**: it streams whichever unit fired `UnitStarted` most recently. With several concurrent units you see all their statuses in the spine, but zone 3 only shows the latest, and there is **no keyboard way to switch** between them (the only bound key is Esc → pause). So a user cannot choose which concurrent stream to watch.
2. **Sub-agent dispatch is invisible to this view.** `dispatch_agent` / `dispatch_agents_parallel` (shipped, PRs #121/#132) run child agents whose transcripts live at `<parent_run>/sub/<sub_run_id>/`, but dispatch is a plain tool call inside the parent's transcript — it emits **no orchestrator event**, so `live_run.rs` never sees it. Children only render post-hoc in the non-interactive line printer.

## Goal

Make **any concurrent live node user-selectable** in the three-zone view — the user navigates the spine with the keyboard and the focus zone streams the **selected** node — and **surface dispatched sub-agent children as live spine nodes** so they participate in the same selection. One consistent model for all concurrency: `for_each`, `parallel`, panel panelists, and sub-agent dispatch.

## Non-goals

- The non-interactive / `--plain` / non-TTY path: unchanged (keeps post-hoc replay). Selection is a TTY-only feature.
- The GPUI desktop app (`rupu-app`) and `rupu-app-canvas` git-graph: out of scope (separate UI stack; the CLI live view's `render_graph` is hand-rolled and does not use `GraphRow`).
- True side-by-side multi-stream rendering (multiple focus zones): out of scope — one focus zone, user selects which node feeds it.
- Deep (>1) dispatch nesting rendering polish: children of a top-level agent are covered; a child that itself dispatches renders its grandchildren best-effort (see Open questions).

## Architecture

### Part A — node-selection cursor (rupu-cli, `live_run.rs`)

- Add a **selection cursor** to `LiveRunState`: `selected: Option<NodeRef>` where `NodeRef` identifies a spine node (a linear step, or a concurrent child keyed `(step_id, index)` — the same key `units` already uses). Default `None` = today's auto-follow behavior (fully backward compatible).
- **Key handling** (extend `handle_live_run_keypress`, `live_run.rs:1201`): a node cursor over the currently-navigable nodes (active step + its concurrent children). Bindings mirror the existing selection UIs in `cmd/autoflow.rs`/`cmd/session.rs` (`switch_focus`/`selected_index`/`sync_selection`): `↑`/`k` + `↓`/`j` to move, `Tab`/`Shift-Tab` to cycle, `Esc` still pauses (keep), and a key (e.g. `a` or `Enter`) to release back to auto-follow. Document the keymap in a footer hint line in the dashboard zone.
- **Focus resolution** (the loop's `desired_transcript`, `live_run.rs:1284-1317`): if `selected` is `Some`, stream that node's `transcript_path`; else keep the current auto-follow (`active.active_unit_transcript` → linear step). A manual selection **pins** until the user moves it or releases; when a pinned node completes, keep showing its final feed (do not auto-jump) until the user moves.
- **Spine rendering** (`render_graph`, `live_run.rs:709-836`): highlight the selected node (a cursor marker / reverse-video). Show all concurrent children as selectable rows (already rendered for fan-out; extend to dispatch children from Part B).
- This part alone closes gap #1 for `for_each`/`parallel`/panel.

### Part B — dispatch children as live nodes (rupu-orchestrator + rupu-cli)

- **New orchestrator events** (`crates/rupu-orchestrator/src/executor/event.rs`, additive/serde-optional): `DispatchStarted { step_id, index, sub_run_id, agent, transcript_path }` and `DispatchCompleted { step_id, index, sub_run_id, success, tokens_in, tokens_out }` — deliberately analogous to `UnitStarted`/`UnitCompleted` so `live_run.rs` can treat dispatch children with the **same** node machinery.
- **Emission:** `CliAgentDispatcher::dispatch()` (`crates/rupu-cli/src/cmd/dispatch.rs`) already allocates the sub-run (`create_sub_run`, path known immediately). Give the dispatcher a handle to the run's event sink (the same sink the orchestrator writes `UnitStarted` through — thread it in at construction, `resume.rs`/`step_factory.rs`) and emit `DispatchStarted` right after `create_sub_run` (before awaiting the child) and `DispatchCompleted` when the child returns. The child's `sub/<id>/` transcript is written live during the run, so tailing it is real live streaming (not replay). Correlation key: `call_id` for single dispatch; the caller `id` for parallel (already in scope at the `dispatch_agents_parallel` call site).
- **View integration** (`LiveRunState::apply`, `live_run.rs:239-393`): handle the two new events like `UnitStarted`/`UnitCompleted` — add/refresh a child node under the step the dispatch call is inside, carrying the child transcript path; the selection cursor (Part A) then covers them for free. Auto-follow default applies to children too.
- **Non-interactive path unchanged:** the line printer keeps its post-hoc `render_dispatch_child` replay (the new events are additive; that path can ignore them).

### What does NOT change

- `dispatch_agent`/`dispatch_agents_parallel` tool semantics, allowlist, depth limits (shipped). The dispatcher still runs children the same way; it just also emits observability events.
- Workflow YAML, provider plumbing, transcript event vocabulary for tools (`tool_call`/`tool_result` still carry the payloads for the line-printer path).
- Auto-follow remains the default when no manual selection is active — existing behavior + tests preserved.

## Errors & safety

- Selection is bounded to existing nodes; moving past the ends clamps or wraps (pick one; document). Releasing selection returns to auto-follow.
- Dispatcher event emission is best-effort: a failed emit must never fail the child run (log + continue). Missing `DispatchCompleted` (crash) → the node stays "running"; the parent's `tool_result` / run completion reconciles it.
- Additive events: older readers ignore unknown variants (serde). A run produced by a newer binary renders on an older viewer minus the dispatch nodes.
- `#![deny(clippy::all)]`; no `unsafe`; `thiserror`; workspace deps only. Per-file rustfmt.

## Testing

- **Part A:** `live_run.rs` unit tests — cursor moves across concurrent units; `desired_transcript` follows the selection not auto-follow when pinned; release returns to auto-follow; a pinned completed node stays shown; render highlights the selected node; new keybindings tested alongside the existing Esc tests (do not replace them). Keep the whole existing `live_run.rs` suite green (fan-out expansion, UnitStarted/Completed state machine, three-zone composition).
- **Part B:** orchestrator — `DispatchStarted/Completed` events emitted by the dispatcher (assert on the event stream via a test sink); `live_run.rs::apply` adds a child node from `DispatchStarted` + completes it on `DispatchCompleted`; selection cursor covers dispatch children. Keep the shipped dispatch e2e tests (`dispatch_agent.rs`, `dispatch_agents_parallel.rs`) green (dispatcher signature change → update `FakeDispatcher`).

## Decomposition (plan)

- **Plan 1 — Part A (selection cursor)**: self-contained in `rupu-cli`; delivers stream selection for existing `for_each`/`parallel`/panel concurrency. Landable + validatable on its own.
- **Plan 2 — Part B (dispatch nodes)**: orchestrator events + dispatcher emission + `live_run.rs::apply`; dispatched children become selectable nodes via Part A's cursor.
Each is an independent PR; matt validates the TUI (alt-screen rendering + keys) at runtime before each merge — subagents cannot validate TUI rendering.

## Open questions (resolve in plan)

- **Q1. Cursor scope:** does the cursor navigate only the *active* step's concurrent children, or *all* live nodes across the spine (including completed)? Recommendation: active step's children + the active linear step (the live-relevant set); completed nodes not navigable (their feed is in scrollback / the line-printer replay).
- **Q2. Wrap vs clamp** at the ends of the node list. Recommendation: wrap (Tab-cycle feel).
- **Q3. Dispatch node nesting:** a dispatched child that itself dispatches — render grandchildren as nested nodes now, or flat/best-effort? Recommendation: model depth-1 children fully; deeper nesting shows the child node but not a full grandchild sub-tree in v1 (its transcript still streams when selected).
- **Q4. Selection persistence across step boundaries:** when the selected node's step completes and a new step starts, release to auto-follow or hold? Recommendation: release to auto-follow on step change (the selected concurrency context is gone).
