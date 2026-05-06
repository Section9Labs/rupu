# Slice C — CLI TUI design

**Date:** 2026-05-05
**Status:** Design (pre-plan)
**Companion docs:** [Slice A design](./2026-05-01-rupu-slice-a-design.md), [Slice B-1 design](./2026-05-02-rupu-slice-b1-multi-provider-design.md), [Slice B-2 design](./2026-05-03-rupu-slice-b2-scm-design.md), [Slice B-3 design](./2026-05-04-rupu-slice-b3-init-design.md)

## 1. Slice scope clarification

What was historically called "Slice C" turned out to be three independent products. This spec covers only the first.

| Slice | Surface |
|---|---|
| **C** | **CLI TUI** — friendlier terminal UX inside the existing `rupu` binary (this spec) |
| **D** | rupu.app — local GUI on a codex-style appserver harness, possibly Rust |
| **E** | rupu.cloud — SaaS Go web app + React UI; clients connect via API or browser |

D and E get their own brainstorm/spec/plan cycles when prioritized. The earlier Slice A spec definition of Slice C ("Control plane (Go web app + React UI), auth, sandboxed remote runs, …") is superseded — that scope now lives in Slice E.

## 2. Goals

1. Replace `rupu run`'s line-spam output with a coherent, status-aware terminal interface.
2. Render multi-step workflow runs as a live DAG canvas in the terminal (Okesu-canvas idiom in text), with the status glyphs already sketched in memory.
3. Make `rupu watch <run_id>` a first-class re-attach surface — any run, any time, from any shell.
4. Eliminate context-switching for human approval gates: approve/reject inline with `a`/`r`.
5. One renderer for all run shapes (single-agent, linear workflow, fan-out workflow, panel workflow). No flag escape hatches in v0.

## 3. Non-goals

- Web UI / GUI app (Slice D and E).
- Mouse interaction (deferred to v0.1 polish).
- Editing workflows from the TUI (read-only viewer).
- Multi-run dashboard listing (`rupu workflow runs` stays as today's text table).
- Cross-emulator pixel-perfect parity (designed for modern truecolor terminals; degrades gracefully on minimal terminals via `NO_COLOR=1`).

## 4. Surfaces

| CLI invocation | Behavior |
|---|---|
| `rupu run <agent> [prompt]` | Spawns the run, attaches the TUI as a degenerate one-node canvas. |
| `rupu workflow run <wf>` | Spawns the workflow run, attaches the TUI as a multi-node canvas. |
| `rupu watch <run_id>` | Re-attaches the TUI to an existing in-flight or finished run by id. |
| `rupu watch <run_id> --replay [--pace=<events/sec>]` | Replays a finished run at a controlled pace; `space` pauses, `n` advances one event. |

`rupu watch` is a **new top-level subcommand** added by this slice. The CLAUDE.md "Ten subcommands" list (`init` / `run` / `agent` / `workflow` / `transcript` / `config` / `auth` / `models` / `repos` / `mcp`) becomes eleven.

`q` always detaches without affecting the run itself. Quitting the viewer never kills the runner.

## 5. Architecture

```
rupu-cli ──▶ rupu-tui ──▶ rupu-transcript (Event types)
                  │
                  └──▶ rupu-orchestrator (RunRecord, approve_run, reject_run)
```

- New crate `rupu-tui`. Hexagonal: depends only on `rupu-transcript` and `rupu-orchestrator` (the latter for `RunRecord`/`RunStatus` types and approval library functions).
- `rupu-cli` stays thin — clap parse + delegation to `rupu_tui::run_attached`/`run_watch`/`run_replay`.
- Runner code in `rupu-orchestrator` is unmodified. The TUI is a pure consumer.

**Framework.** `ratatui` + `crossterm` backend (workspace-pinned). De-facto Rust TUI library; mature ecosystem (`gh dash`, `bandwhich`, `gitui`, `bottom`). `ratatui::backend::TestBackend` enables headless snapshot testing.

**FS watching.** `notify` crate (workspace-pinned) on the run's `transcript_dir` and `run.json`. Falls back to 250ms `mtime` polling if `notify` registration fails (NFS, sandboxes); logged once at startup.

**Architecture rule preserved:** all TUI logic lives in `rupu-tui`; nothing leaks into `rupu-cli` business logic.

## 6. Components

```
crates/rupu-tui/src/
  lib.rs              — re-exports + pub fn entry points
  app.rs              — top-level App struct, event loop, key dispatcher
  source/
    mod.rs            — EventSource trait
    jsonl_tail.rs     — disk-tail impl (live + reattach)
    replay.rs         — bounded-pace replay of a finished run
  state/
    mod.rs            — RunModel: in-memory projection of all events
    node.rs           — NodeState (status, agent, tokens, last_action, …)
    edges.rs          — derived from WorkflowSpec (parent → child)
  view/
    mod.rs            — View enum { Canvas, Tree }, focus model, scroll
    canvas.rs         — surface A: horizontal LTR layout + box-draw + edges
    tree.rs           — surface B: vertical TTB tree
    panel.rs          — focused-node detail pane
    palette.rs        — status colors + glyphs (single source of truth)
    layout.rs         — auto-layout: column assignment, fan-out positioning,
                        viewport pan/zoom math
  control/
    mod.rs            — KeyBinding enum + dispatcher
    approval.rs       — D-in: approve/reject calls into rupu-orchestrator
  err.rs              — TuiError (thiserror)
```

### 6.1 `EventSource` trait

```rust
pub trait EventSource: Send {
    /// Drain any events available now; returns immediately.
    fn poll(&mut self) -> Vec<SourceEvent>;
    /// Block up to `dur` for the next event (used for tick budgeting).
    fn wait(&mut self, dur: Duration) -> Option<SourceEvent>;
}

pub enum SourceEvent {
    StepEvent { step_id: String, event: rupu_transcript::Event },
    RunUpdate(RunRecord),
    Tick,
}
```

Two impls in v0:
- `JsonlTailSource` — live + reattach; `notify` watcher → mpsc → consumer.
- `ReplaySource` — pace-controlled iteration of a finished run.

Slice D / E can later add `WebSocketSource` (or similar) without touching anything else.

### 6.2 Public surface from `rupu-cli`

```rust
rupu_tui::run_attached(run_id, transcript_dir, View::default()) -> Result<()>
rupu_tui::run_watch   (run_id, View::default())                 -> Result<()>
rupu_tui::run_replay  (run_id, View::default(), pace_ms)        -> Result<()>
```

## 7. Data flow

```
rupu-orchestrator (runner, unchanged)
  │ writes
  ▼
~/.rupu/runs/<run_id>/
  ├── run.json                  (RunRecord — status, awaiting_step_id)
  ├── step_results.jsonl        (StepResultRecord — one per finished step)
  └── transcripts/
      └── step_<sid>_<run>.jsonl (Event stream — per agent run)
                       │
                       │ filesystem watch + tail
                       ▼
JsonlTailSource ──── mpsc::Receiver<SourceEvent> ───▶ App::on_event
                                                            │
                                                            ▼
                                              RunModel (mutated in place)
                                                            │
                                                            │ on State change
                                                            │ OR every 33ms
                                                            ▼
                                              View::render(model, focus, viewport)
                                              ratatui draw — canvas or tree
```

### 7.1 Event projection

| Event | RunModel mutation |
|---|---|
| `RunStart` | seed nodes from spec; this node's status = `Active` |
| `TurnStart` | node.status = `Working`; node.turn_idx++ |
| `AssistantMessage` | append to node.transcript_tail (ring buffer of 5) |
| `ToolCall` | node.last_action = (tool, summary); node.tools_used[tool]++ |
| `ToolResult` | node.last_action.duration_ms = … |
| `FileEdit` | node.tools_used["edit"]++; node.last_action |
| `CommandRun` | node.tools_used["bash"]++; node.last_action |
| `ActionEmitted` | node.actions_emitted++; if `!allowed`, append to node.denied_actions |
| `TurnEnd` | node.tokens.input += tokens_in?; node.tokens.output += tokens_out? |
| `GateRequested` | node.status = `Awaiting`; mirror prompt into node.gate_prompt |
| `Usage` | node.tokens.input += …; node.tokens.output += …; node.tokens.cached += … |
| `RunComplete` | node.status = match status { Ok→Complete, Error→Failed, Aborted→Failed } |
| `RunUpdate(rec)` | merge top-level run state (status, awaiting_step_id, expires_at) |

### 7.2 Invariants

- **`RunModel` is the single source of truth.** Every event mutates it; render is a pure function of it.
- **No event is dropped.** Tail keeps last byte offset per file; partial trailing line is held until next FS-watch event.
- **Render is bounded.** 30 fps cap via tick coalescing; events arriving faster are batched into one render.
- **Spec drives topology, events drive status.** `WorkflowSpec` (loaded from `<workspace>/.rupu/workflows/<name>.yaml` via `RunRecord.workflow_name`) defines which nodes exist and which feed which. Events only update node *state*. Pre-start runs render meaningful skeletons (`○ waiting` everywhere).
- **Single-agent `rupu run` degenerates cleanly.** Synthetic one-node DAG; identical render path.

## 8. Layout & view system

### 8.1 View toggle

- `v` hot-swaps Canvas ↔ Tree.
- Default = **Canvas (A)** if terminal width ≥ desired-canvas-width-for-spec, else **Tree (B)**.
- User preference persists per-session via `RUPU_TUI_DEFAULT_VIEW=tree|canvas`.

### 8.2 Canvas (A) auto-layout

- Topological sort assigns each node a column = longest-path depth from root.
- Vertical position within a column packs siblings tightly (greedy from top).
- Edges drawn as `─ ─▶` for same-row hops; fan-out drops vertically with `┬ │ └─▶` connectors.
- Node card: width 16 cols (status glyph + 12-char trimmed step_id + padding), height 3 rows, inter-column gap 4 cols.
- Viewport supports horizontal pan (`h`/`l` or arrows) and vertical pan (`j`/`k` or arrows) when DAG exceeds terminal box.

### 8.3 Tree (B) layout

- Pre-order DFS from root.
- Linear children → vertical chain (`│`). Fan-out → branching (`├──▶ child` / `└──▶ child`). Indent matches depth.
- Always single-column; never needs horizontal pan.

### 8.4 Focus model

- Always exactly one focused node. Highlighted with `> ` prefix in tree, bright border in canvas.
- Right side / bottom (depending on terminal aspect ratio) shows the **focused-node panel**: status, agent, model, last 5 transcript lines, tool counters, tokens (in/out/cached), last action.
- `tab` / `shift-tab` cycle focus along the topological order.
- Arrow keys also work for spatial focus in canvas mode.

### 8.5 Status palette (single source of truth in `palette.rs`)

| Glyph | State | Color |
|---|---|---|
| `●` | Active (turn boundary just opened) | `bright_blue` |
| `◐` | Working / streaming | `blue` |
| `✓` | Complete | `green` |
| `✗` | Failed | `red` |
| `!` | Error but `continue_on_error` | `yellow` |
| `○` | Waiting (not yet started) | `dim_grey` |
| `↺` | Retrying | `magenta` |
| `⏸` | Awaiting human approval | `bright_yellow`, pulse-bold every 1s |
| `⊘` | Skipped (`when:` evaluated false) | `dim_grey` |

Edges are colored by upstream-node status: green if upstream completed, blue if running, grey if waiting, red if failed.

### 8.6 Key bindings

| Key | Action |
|---|---|
| `tab` / `shift-tab` | Focus next / prev node (topological) |
| `↑↓←→` or `hjkl` | Focus / pan (context-dependent) |
| `enter` | Expand focused node — full transcript pager |
| `v` | Toggle Canvas ↔ Tree view |
| `a` | Approve focused gate (only when `⏸`) |
| `r` | Reject focused gate (only when `⏸`) |
| `f` | Filter: hide completed nodes |
| `/` | Search node by id |
| `q` | Quit (detaches; run keeps running) |
| `?` | Help overlay (full key map) |

### 8.7 Width handling

- Canvas mode below minimum width: render warning bar `(canvas truncated — press v for tree view)` and pan as needed.
- Hard floor at 40 cols → render single line `terminal too narrow (40 cols min); resize or pipe to file` and exit.
- `crossterm::Event::Resize` invalidates layout cache; full re-layout on next render. No flicker due to ratatui's double buffer.

### 8.8 No mouse in v0

Wheel-scroll on the panel is the obvious v0.1 follow-on. Single-click focus is a v0.1 nice-to-have.

## 9. Approval flow (D-in)

### 9.1 Detection

Runner pauses; writes `RunRecord` with `awaiting_step_id`, `approval_prompt`, `expires_at`. JSONL emits `Event::GateRequested` on the step's transcript. `JsonlTailSource` picks both up; `RunModel` flips that node to `⏸ Awaiting`.

### 9.2 Visual

The `⏸` glyph pulses bold every 1s on the awaiting node. A bottom toast appears:

```
⏸ deploy-gate: "Deploy v2.31 to prod?"  [a] approve  [r] reject  [enter] expand
```

The toast persists until the gate resolves. If `expires_at` is set, a countdown is shown next to the prompt (`expires in 4m 12s`).

### 9.3 Auto-focus

When a node enters `⏸`, focus jumps to it automatically — *unless* the user has manually focused something else in the last 5 seconds (debounce so we don't yank focus mid-keystroke).

### 9.4 Approve

Pressing `a` on a focused `⏸` node:

1. Calls `rupu_orchestrator::approve_run(run_id, step_id, approver = whoami)` — same library function `rupu workflow approve <id>` already calls.
2. On success, optimistically flips local `RunModel` node to `◐ Working` and shows a 2-second confirmation toast `✓ approved`. The next `RunUpdate` from the source confirms / corrects.
3. On error (e.g., expired, already decided), shows the error in the toast and refetches `RunRecord`.

### 9.5 Reject

Pressing `r`:

1. Pops a one-line input prompt at the bottom: `reject reason: ` (max 200 chars, ESC cancels).
2. On enter, calls `rupu_orchestrator::reject_run(run_id, step_id, approver, reason)`.
3. On success, flips node to `✗`; the run transitions to `Rejected` (terminal); TUI stays attached to show the final state until user hits `q`.

### 9.6 Expiry

When `RunRecord.expires_at` passes:

- `RunUpdate` will eventually flip the run to `Failed` with `error_message: "approval expired"`. TUI mirrors that and shows the toast in red.
- Pressing `a`/`r` after expiry returns the orchestrator's existing error; we surface it.

### 9.7 Multiple gates

If two parallel branches both hit gates simultaneously, both nodes pulse. The toast shows the focused one. `tab` cycles between them; the toast updates with focus.

### 9.8 Identity

`approver` field = `whoami` output (Unix login name). Sufficient for Slice C; Slice D / E will add real identity.

### 9.9 No mode switching

Approval is a single key tap on the focused node. The `a`/`r` keys are silently ignored on non-`⏸` nodes — except on the first attempt, which shows a brief `not awaiting approval` toast for discoverability.

## 10. Error handling

| Condition | Behavior |
|---|---|
| Run not found (`rupu watch <bad_id>`) | Exit 2 with `error: run "<id>" not found in ~/.rupu/runs/`. Suggest `rupu workflow runs`. No TUI drawn. |
| Run dir disappears mid-attach | Render last-known state with red banner `⚠ run state lost`; quit on next keypress. |
| Malformed JSONL line | Skip; `tracing::warn`; increment `parse_errors` counter shown in help overlay. Never crash. |
| Truncated final JSONL line | Tail keeps byte offset before partial line; retries on next FS-watch event. |
| `run.json` write race | Read with `flock` shared lock + retry-3-times on `EINTR/EAGAIN`. (Orchestrator already writes with exclusive lock per PR #41.) |
| Workflow spec missing | Degraded mode: render nodes purely from observed events (no edges, no `○` skeleton). Top banner: `⚠ workflow spec not found; rendering events-only`. |
| Terminal too narrow | Warning bar + auto-suggest tree view (§8.7). |
| Hard floor (<40 cols) | Single-line message, exit. |
| Terminal resize | `crossterm::Event::Resize` → invalidate layout cache; re-layout on next render. |
| Lost terminal control on panic | `App::Drop` restores raw mode + cursor + alt screen. `std::panic::set_hook` calls the same teardown before panic message. |
| SIGTERM / Ctrl-C | Same teardown path. Detaches; prints `detached from run_<id>`. No prompt confirmation — quitting never affects the run. |
| Approve/reject error | Surfaced verbatim in toast (red, 5s). State refetched. TUI never out of sync. |
| `notify` registration fails | Fall back to 250ms mtime polling; `tracing::info` once. |
| `NO_COLOR=1` or non-TTY | Glyphs only, no ANSI color. Layout unchanged. |
| Replay EOF | Footer banner `replay complete`; `space`/`n` no-op. |

## 11. Testing strategy

Three layers, smallest blast radius first.

### 11.1 Pure unit tests

- `state/`: feed a vector of `SourceEvent`s into `RunModel`, assert resulting `NodeState`. Property-based tests for "any disk-monotonic event order produces the same final state."
- `view/layout.rs`: snapshot tests of column assignments and fan-out positioning. Input = small `WorkflowSpec` fixture; output = `Vec<(node_id, x, y)>`.
- `view/palette.rs`: glyph + color matrix tests.
- `control/`: key-bind dispatch is a pure `(KeyEvent, FocusState) -> Action` function — table-driven tests.

### 11.2 Renderer snapshot tests (`ratatui::backend::TestBackend`)

- Drive a known `RunModel` + `View` + `(width, height)` through `view::render()`; capture the `Buffer`; pretty-print to a stable text representation; snapshot via `insta`.
- Coverage matrix: `{linear DAG, fan-out, panel run with 2 findings, awaiting-approval}` × `{Canvas, Tree}` × `{80×24, 120×40, 200×60}` × `{focused, unfocused}`. ≈30 snapshots locking the rendering pixel-for-pixel.
- Snapshots checked in under `crates/rupu-tui/tests/snapshots/`.

### 11.3 Integration tests (live FS, real orchestrator types)

- End-to-end: spawn a `MockOrchestratorRun` that writes scripted events to a tempdir; mount `JsonlTailSource` against it; drive the App; assert the final rendered buffer matches a snapshot.
- One scenario per surface: single-agent run, linear workflow, fan-out workflow, approval gate (approve), approval gate (reject), expired gate, malformed JSONL line.
- Use `tokio::time::pause()` so the 30 fps tick + the approval pulse-bold + the toast timeout are deterministic.

### 11.4 Out of CI scope

- Cross-emulator behavior (iTerm vs Alacritty vs Windows Terminal) — manual smoke matrix on contributor's box, documented in `crates/rupu-tui/README.md`.
- Color output — `TestBackend` is glyph-only. Style attributes snapshotted separately for sanity.

### 11.5 Smoke target

`make tui-smoke` runs the binary against a bundled fixture run with the headless TestBackend for 5s; exits 0 if no panics. Catches link / startup regressions even when integration tests don't run.

### 11.6 No flaky timing tests

The "render is a pure function of `RunModel`" invariant means every visual bug is reproducible from a fixture event sequence — no real-time-dependent assertions.

## 12. Out-of-scope explicitly noted

- Web UI / GUI (Slice D and E)
- Mouse interaction (v0.1)
- Workflow editing from TUI (read-only)
- Multi-run dashboard listing (`rupu workflow runs` text table stays as today)
- Pixel-perfect cross-emulator parity
- Authenticated approval identity (Slice D / E)
- TUI for `rupu workflow approve` / `reject` from outside an attached session (CLI commands stay)

## 13. Open questions

None blocking. Two follow-ons logged for v0.1:

- Mouse: wheel-scroll panel, click-to-focus.
- Color theme: ship a `light` palette alongside the default dark palette; auto-detect via `COLORFGBG` env.
