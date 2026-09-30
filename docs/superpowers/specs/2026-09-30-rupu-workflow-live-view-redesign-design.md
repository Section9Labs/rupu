# rupu Workflow Live View — Redesign (Approach 2)

**Date:** 2026-09-30
**Status:** Approved (design); implementation pending
**Author:** matt + Claude
**Supersedes the implemented parts of:** `docs/superpowers/specs/2026-06-17-rupu-live-workflow-run-view.md`

## Problem

`rupu workflow run`'s live view (`crates/rupu-cli/src/output/live_run.rs`, ~3.6k
lines) cannot hold a large or long run. On a real 5-hour, 15-step run whose
`for_each` fanned out to 86 units across the fleet, the view is not just cramped —
several things are **broken**:

1. **Token/cost totals are fiction.** The view tails exactly one transcript (the
   focused unit). Unfocused units contribute 0; re-focusing re-reads from offset 0
   and double-counts; sub-agent `DispatchCompleted` double-counts again
   (`live_run.rs:1776`, `:724`; `UnitCompleted` carries 0 tokens,
   `executor/event.rs:113`).
2. **The active frontier scrolls off with no way back.** Overflow keeps the *top*
   rows plus `… (+N more)` and drops the focus panel entirely (`live_run.rs:1556`).
   In a 40-step / 86-unit run the live work is below the fold and unreachable.
3. **Single-active-step assumption.** DAG workflows run steps concurrently; they
   fight over one "active step" slot, and every `StepStarted`/`UnitStarted` clears
   the feed and resets selection.
4. **Fan-out is one row per unit** — 86 rows, no aggregation, no status filter.
5. **The feed looks stalled.** It shows only tool calls / findings / coverage — no
   assistant text, no thinking (`live_run.rs:1875`) — so a long model turn reads as
   dead air. The spinner never animates (`let _ = tick;`, `:1828`).
6. **Dead meters:** findings count, coverage bar, panel `iter n/max`, "N found" are
   never populated outside tests; `PanelRound` events are ignored (`:636`).
7. **Sub-agent slot corruption.** `DispatchStarted` appends at `units.len()`
   (`:688`); a later `UnitStarted` with that index overwrites the slot and the
   `DispatchCompleted` marks the wrong unit complete.
8. **Long-run formatting breaks:** elapsed has no hours (`187m 12s`, `:929`);
   heartbeat prints raw `3605s ago`.
9. **Codenames half-landed:** crew tint shows in the title, but per-unit role badges
   render uncolored.
10. **Gates exit silently.** At an approval gate the view's loop never breaks; the
    process tears down the alt screen with no approve hint (`workflow.rs:4925`).

The control plane (CP web) already does most of this well. This redesign keeps the
work **in the CLI's inline surface** — the in-place view during `workflow run` /
`resume` — rather than adding a new attach command or deferring to the CP.

## Goal

A **global run view you can go deep from, without leaving it.** A bird's-eye
dashboard + collapsed DAG + cross-unit firehose that stays legible at any scale
untouched, and that you can drill into (step → unit → sub-agent) and pop back out
of with a breadcrumb that tells you exactly where you are. Correctness first: the
numbers must be true.

### Core principles

- **Adaptive by default, navigable on demand.** Zero keypresses gives a correct,
  legible overview that keeps the live frontier and the activity feed on screen no
  matter how tall the run is. Keys let you go deeper when you choose.
- **Drill in / pop out.** One coherent depth axis: run → step → unit → sub-agent.
  A breadcrumb shows your position; you are never lost and never leave the overview
  to see detail.
- **The run breathes.** The default feed is a run-scoped firehose across all active
  units; drilling into a unit replaces it with that unit's live stream.
- **The numbers are true.** Token, cost, findings, and coverage totals aggregate
  across every unit and sub-agent, counted exactly once.

## Non-goals

- No new `attach`/`watch`-style command, and no change to `rupu watch`. (Reusing
  this renderer for post-hoc attach is noted as future work, not built here.)
- No shared `rupu-run-view` crate across CLI / `app-canvas` / CP (Approach 3).
  Kept CLI-local.
- No mouse support.
- No change to `--output json` / structured-format contracts. `workflow run`
  still has no JSON mode; that is unchanged.
- The non-live paths (`--plain` retained view B, the non-TTY line printer C in
  `workflow_printer.rs`/`printer.rs`) are out of scope except where they share the
  new run-model (see §"Shared model").

## Architecture (Approach 2)

Replace the `live_run.rs` monolith with four testable units under
`crates/rupu-cli/src/output/`, plus a small navigation-state type. Each has one
job, a defined interface, and can be tested in isolation.

```
events.jsonl ─┐
run.json ─────┤
transcripts ──┘
     │  (tail)
     ▼
┌───────────────┐   RunView    ┌────────────────┐   Vec<Row>   ┌───────────────┐
│  run_model    │ ───────────▶ │  live_layout   │ ───────────▶ │  live_render  │
│ (projection)  │              │ (adaptive rows)│              │ (diff redraw) │
└───────────────┘              └────────────────┘              └───────────────┘
        ▲                              ▲
        │                              │
┌───────────────┐              ┌────────────────┐
│ transcript_mux│──firehose──▶ │   NavState     │
│ (bounded tail)│              │ (depth+select) │
└───────────────┘              └────────────────┘
```

### 1. `run_model.rs` — the projection (`RunView`)

Consumes `events.jsonl`, `run.json`, and usage from transcripts; produces a
`RunView`: the canonical, correct state of the run. Pure state transitions over an
input event stream — no I/O, no rendering — so it is unit-testable by feeding event
sequences.

`RunView` holds:

- **Steps** as a DAG-aware structure. `StepStarted.kind` (currently ignored) is the
  source of truth for node kind — `run` / `for_each` / `parallel` / `panel` /
  `loop` / `gate` / `action` / `branch` / `split` / `join`. Multiple steps may be
  active simultaneously (**frontier**, a set — not one slot).
- **Fan-out aggregates** per `for_each`/`parallel` step: counts by status
  (`queued / running / done / failed`), %-complete, aggregated usage, and the full
  unit list (indexed by `unit_key`, not by a reused array slot — fixes the
  sub-agent overwrite bug). Sub-agents attach to their **parent unit** via
  `DispatchStarted`'s parentage, not to `units.len()`.
- **Panel** rounds from `PanelRound{round, max_iterations, max_severity_remaining}`
  and **loop** iterations from `loop_progress` / `loop_iteration`.
- **Per-unit usage** folded correctly: each transcript's `usage` events are counted
  once, keyed by transcript path with a byte-offset cursor so a re-read never
  double-counts; sub-agent usage is attributed to the child and rolled up, never
  added on top of an already-tailed stream.
- **Totals:** `tokens_in`, `tokens_out`, `cost`, `findings` (with severity
  breakdown), `coverage %` — all derived, all true.
- **Codenames:** crew (run-level tint) + per-agent role/instance/attempt + `›`
  sub-agent chain, from the `codename` fields already on the events and `run.json`.
- **Gates:** `awaiting[]` from `run.json` (`AwaitingGate{step_id, prompt, since,
  expires_at}`), so the view can park in-place.
- **Timing:** run elapsed, per-step elapsed, per-unit last-activity (for the
  heartbeat), with hours-aware formatting.

### 2. `transcript_mux.rs` — the firehose source

Tails a **bounded** set of transcripts and merges them into one time-ordered event
stream (the firehose), each event tagged with its unit/sub-agent codename.

- **fd budget.** A wide fan-out has 86+ transcripts; tailing all of them would blow
  the descriptor limit. The mux tails only the *N most-recently-active* transcripts
  (N derived from the runtime open-file budget, PR #672 / `--max-open-files` /
  `[runtime].max_open_files`), evicting the least-recently-active. Units not
  currently tailed still count toward totals via `run_model` (their `usage`/
  completion is read on completion, not by holding a live handle).
- When the user **drills into** a specific unit or sub-agent, the mux guarantees
  that one transcript is tailed (pinned, exempt from eviction) so its stream is
  complete regardless of activity.
- Emits the rich transcript v2 kinds the feed needs: `tool_call`, `thinking` /
  `thinking_delta`, `assistant_delta` / `assistant_message`, `file_edit`,
  `command_run`, `action_emitted`, finding rows, coverage, `tool_audit` blocks,
  `notice` (retry/trim). The firehose shows a compact one-line projection; a
  drilled-in unit shows the fuller stream.

### 3. `NavState` — depth + selection

The navigation model matt asked for. A small value type, fully testable:

- **Depth axis:** `Run → Step → Unit → SubAgent`. `NavState` holds the current
  focus path (the breadcrumb, e.g. `mint-tundra › hunt › otter#41 › wren#1`) and
  the selection index at the current level.
- **Drill in** (`enter` / `→` / `l`): descend one level on the selected node — a
  `for_each` step expands to its filterable unit list; a unit with a sub-agent
  descends into the child. At a leaf, `enter` does nothing.
- **Pop out** (`←` / `backspace` / `h`): ascend one level toward the global
  overview. At the top, a no-op (does **not** quit).
- **Move** (`↑↓` / `j` `k` / `Tab`): change selection within the current level.
- **Auto-follow** (`a`): release any manual selection; the view tracks the newest
  activity again (the zero-touch default). (This binding is suspended while a gate
  panel is focused — see §"Gate handling", where `a`/`r` are modal approve/reject.)
- **Filter** (`/`): at an expanded fan-out, cycle/enter a status filter
  (`running` / `failed` / `done` / all).
- **Quit** (`q`): leave the viewer from any depth **without pausing the run**.
- **Pause** (`Esc`): cooperative pause at the next safe boundary — kept, but now
  **labeled** in the footer so it is never an accidental exit (today it is the only
  way out and it silently pauses the run).

The feed follows depth: at `Run`/`Step` level it shows the firehose; at
`Unit`/`SubAgent` level it shows that stream (the "pinned" frame). Drilling into a
unit **is** the pin gesture from the mock — there is no separate `p` key; `enter`
into a unit pins its stream, `←`/`backspace` unpins by popping back to the
firehose.

#### Full key reference

The depth axis: `↑↓` move within a level; `enter`/`→` drills in; `←`/`backspace`
pops out.

```
  ↑↓ move          RUN overview            mint-tundra
  within     ┌────────────────────────────────────────────────┐
  a level    │  dashboard · collapsed graph · firehose feed    │
             └───────────────┬────────────────────────────────┘
       enter / → / l  ⇣ drill in        ⇡ ← / backspace / h  pop out
             ┌───────────────▼────────────────────────────────┐
             │  STEP     mint-tundra › hunt                    │
             │  full fan-out unit list · filter with /         │
             └───────────────┬────────────────────────────────┘
       enter / → / l  ⇣                 ⇡ ← / backspace / h
             ┌───────────────▼────────────────────────────────┐
             │  UNIT     mint-tundra › hunt › otter#41         │
             │  this unit's live stream (thinking · tools)     │
             └───────────────┬────────────────────────────────┘
       enter / → / l  ⇣                 ⇡ ← / backspace / h
             ┌───────────────▼────────────────────────────────┐
             │  SUB-AGENT   … › otter#41 › wren#1              │
             │  the dispatched child's live stream             │
             └────────────────────────────────────────────────┘
```

| Key | Context | Action |
|-----|---------|--------|
| `↑`/`k` | anywhere | move selection up within the current level |
| `↓`/`j` | anywhere | move selection down within the current level |
| `Tab`/`Shift+Tab` | anywhere | cycle selection (alias of ↓/↑) |
| `enter`/`→`/`l` | on a step/unit | drill in one level (expand fan-out → pin unit → enter sub-agent) |
| `←`/`backspace`/`h` | drilled in | pop out one level toward the overview; no-op at the top |
| `a` | navigating | auto-follow — drop manual selection, track newest activity |
| `/` | expanded fan-out | filter units by status: running → failed → done → all |
| `a` | at a gate (modal) | approve the gate |
| `r` | at a gate (modal) | reject the gate |
| `v` | at a gate (modal) | view findings behind the gate |
| `enter` | at a gate (modal) | gate details |
| `q` | always | quit the viewer — the run keeps running |
| `Esc` | always | pause the run cooperatively at the next safe boundary |
| `Ctrl-C` | always | quit cleanly (restores terminal); the run keeps running |

Two keys are **modal** and the footer legend always shows the live set: `a` is
auto-follow while navigating but approve at a focused gate; `enter` is drill-in on a
step/unit but gate details at a focused gate. Safety invariant: only `Esc` pauses
the run — `q` and `Ctrl-C` leave it running.

### 4. `live_layout.rs` — adaptive rows

Pure function: `(RunView, NavState, terminal W×H) → Vec<Row>`. No I/O. This is the
unit that makes a huge run legible, and it is snapshot-tested the way
`rupu-app-canvas` tests its `GraphRow` output.

Layout contract (top to bottom), with **guaranteed-visible** zones:

1. **Dashboard** (4–5 rows, always): title + crew codename + status + hours-aware
   elapsed; overall `step N/M` progress bar; adaptive meter row
   (`⇡in ⇣out $cost · ⚑findings · coverage P%`), each meter shown only once the run
   has produced it.
2. **Breadcrumb** (1 row, when drilled below Run level).
3. **Graph** (flexible), with **adaptive collapse** so the frontier never scrolls
   off:
   - Settled, contiguous completed steps collapse to a single summary row
     (`✓ preflight  ✓ inventory  …`), expandable.
   - `for_each`/`parallel` steps render as a **density bar + live movers**:
     the status-count bar (`58/86  ✓52 ◐6 ✗2 ○26`) + aggregated usage, plus only
     the few most-recently-moved units as indented rows; `enter` expands the full
     filterable list.
   - The **active frontier** (all running steps/units) is always kept on screen; if
     space is tight, settled regions collapse *before* the frontier is touched.
   - Rows are width-clipped last, and status/usage on the right are protected from
     truncation ahead of the left-hand label.
4. **Feed** (flexible, always retains a minimum height): firehose or drilled-in
   stream per `NavState`.
5. **Footer** (1 row, always): context-sensitive key legend.

### 5. `live_render.rs` — diff redraw

`Vec<Row>` → terminal. Replaces the every-tick full-screen
`MoveTo(0,0)+Clear(All)` (the flicker source) with a **diff** against the last
frame: only changed rows are repainted. Owns alt-screen enter/exit, raw-mode, and
resize. A dirty-flag from `run_model`/`transcript_mux` skips redraw entirely when
nothing changed.

### Wiring

`execute_workflow_invocation` (`workflow.rs:4929`) keeps spawning the runner and the
view as sibling tokio tasks. The selection logic (live view A vs retained B vs line
printer C vs no-UI D) is unchanged. The view task's internals become: tick → feed
new bytes into `run_model` + `transcript_mux` → recompute layout → diff-render.

### Shared model (retained/line paths)

`RunView` is the correct projection of a run. The retained view B and line printer C
currently re-read all of `step_results.jsonl` every 250 ms and hide fan-out until a
step finishes. They are **out of scope to rebuild**, but where cheap they should
read `RunView` instead of re-parsing — specifically the completion summary (below)
is produced from `RunView` and shared by A/B/C so all three finally print cost +
findings + links. Anything deeper is future work.

## Completion summary

All live paths currently print only `completed · run_…` (`run` discards the outcome,
`workflow.rs:4028`). Replace with a `RunView`-derived summary (frame 4 of the mock):
status + hours-aware duration; per-step outcome line; unit ok/failed counts;
`⇡/⇣/$` totals; findings by severity; coverage %; failed units by codename with a
`resume` hint; and `show-run` + CP URL pointers. The CP URL is emitted only when a
CP base is known (config); otherwise just the `show-run` line.

## Gate handling (park in-view)

When `run.json` shows `awaiting[]`, the live view does **not** exit. It renders the
gate panel in-place (frame 3): reason, findings summary, `since`/`expires_at`
countdown, and `[a] approve / [r] reject / [v] view findings` inline. Approve/reject
route through the existing `workflow approve`/`reject` code paths (marker file;
`cp serve`/resume worker does the actual resume — unchanged). Multiple concurrent
gates render as a selectable set. This closes the "silent teardown at a gate" bug
for the live view; the underlying "runner emits no run-level awaiting event" gap is
handled by reading `run.json`'s `awaiting[]` on tick (already the source of truth).

## Data flow

1. View task ticks (~100 ms interval retained; cheap because of diff-render).
2. Each tick: append-read new bytes of `events.jsonl` → `run_model`; poll `run.json`
   (codename, awaiting, active steps); `transcript_mux` advances its bounded tail
   set and emits firehose events → `run_model` (usage) + feed buffer.
3. `run_model` updates `RunView`; sets a dirty flag if anything changed.
4. If dirty (or the terminal resized): `live_layout` recomputes rows;
   `live_render` diffs and repaints.
5. Keys drain non-blocking into `NavState`, which redirects the feed and the layout
   selection.

### Correctness fixes (folded in, each verifiable)

| # | Bug | Fix |
|---|-----|-----|
| 1 | Only focused transcript counted; double counts | `run_model` folds every transcript's `usage` once via a per-path byte cursor; `transcript_mux` tail set is for *display*, totals are independent of what's tailed |
| 2 | Frontier scrolls off | `live_layout` collapses settled regions before the frontier; frontier + feed guaranteed-visible |
| 3 | One active step | `RunView` frontier is a set; feed/selection not reset per event |
| 4 | 86 flat rows | density bar + movers; `enter` → filterable list |
| 5 | Feed looks stalled | firehose + drilled feed include `thinking`/`assistant`; real heartbeat |
| 6 | Dead meters | findings/coverage/panel/loop counts derived in `RunView` (or the meter is removed if genuinely unavailable — no silent-zero) |
| 7 | Sub-agent slot overwrite | units keyed by `unit_key`; sub-agents attach to parent unit |
| 8 | No hours; raw seconds | hours-aware duration + humanized "active Ns/Nm ago" |
| 9 | Uncolored role badges | role badge rendered with its palette hue; crew tint at run level |
| 10 | Silent gate exit | park in-view with approve/reject |

Per the project rule "no mock features": a meter that cannot be truthfully computed
is **removed**, never shown as a silent zero.

## Error handling

- **Missing/short files.** `events.jsonl` may lag `run.json`; partial last lines are
  buffered until newline-terminated (existing `jsonl_reader` behavior, retained).
- **Resume replay.** `events.jsonl` is append-only across resumes; on attach the
  model replays from the start but must not exit on a *previous* run's
  `RunFailed`/`RunPaused` before the new `RunStarted` (today's bug n). The model
  treats terminal events as terminal only for the *current* run generation
  (keyed by the latest `RunStarted`).
- **fd exhaustion.** `transcript_mux` never exceeds its budget; eviction is
  last-recently-active; a drilled-in unit is pinned. If a tail fails to open, the
  unit still contributes to totals (read-on-complete) and shows a `?`-tinted state
  rather than vanishing.
- **Remote-host units.** The `host` field on events/records is surfaced (host chip
  on a drilled unit; host count in the firehose header). Transcripts are read from
  the locally mirrored path (fleet dispatcher already mirrors them). A not-yet-
  mirrored remote transcript shows "streaming from <host>…" rather than blank.
- **Terminal too small.** Below a minimum height, collapse to dashboard + feed only,
  with a one-line "graph hidden — widen/enlarge" note.
- **Quit vs pause.** `q` restores the terminal and leaves the run running; `Esc`
  pauses; Ctrl-C is trapped today (raw mode) — install a handler so Ctrl-C quits
  cleanly (restore terminal) without pausing.

## Testing strategy

- **`run_model` unit tests.** Feed synthetic event + usage sequences; assert
  `RunView` totals, frontier set, fan-out aggregates, sub-agent attachment, gate
  parking, and resume-generation handling. This is where the correctness fixes are
  pinned.
- **`live_layout` snapshot tests** (`insta`), like `rupu-app-canvas`. Fixtures at
  representative scales: small linear; 86-unit fan-out at 24-row and 60-row
  terminals (frontier stays visible); panel rounds; loop iterations; nested
  sub-agents; gate parked; multiple concurrent gates; completion. Assert the
  frontier and a minimum feed height are always present.
- **`NavState` unit tests.** Drill-in/pop-out across all four depth levels;
  breadcrumb correctness; auto-follow release; filter cycling; that `q` ≠ `Esc`.
- **`transcript_mux` tests.** Bounded tail eviction under a small budget; a pinned
  drilled unit is never evicted; merged ordering; totals independent of tail set.
- **`live_render` diff test.** Given two frames, only changed rows are emitted;
  resize forces a full repaint.
- **No flaky timing tests.** All of the above drive the model directly; no real
  clocks or sleeps in CI. A `make`-only smoke target may exercise the real terminal.

## Out of scope (explicit)

- Reusing this renderer for `rupu watch` / post-hoc attach (future; the `RunView`
  model makes it cheap later).
- A CP-API terminal client (`HttpHostConnector` reuse) for runs started elsewhere
  or on HTTP-only hosts.
- Per-host lanes in the fan-out block (density bar is status-first; host regroup is
  a noted follow-up, not built here).
- Rebuilding retained view B / line printer C beyond sharing `RunView` for the
  completion summary.

## Open question for review

The navigation key scheme (§NavState) is my recommendation; the one thing worth
confirming before planning is the drill-in/pop-out bindings — `enter`/`→` in,
`←`/`backspace` out, `q` quit, `Esc` pause. If you prefer a different out-key
(e.g. `Esc` to pop out and an explicit `[P]ause`), that inverts the safest-default
choice and is easier to change now than later.
