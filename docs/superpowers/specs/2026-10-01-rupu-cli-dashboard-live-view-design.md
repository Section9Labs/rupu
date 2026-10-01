# rupu CLI Dashboard Live View — Design

**Date:** 2026-10-01
**Status:** Approved (direction); design under review
**Author:** matt + Claude
**Supersedes:** the single-column graph/feed layout of `docs/superpowers/specs/2026-09-30-rupu-workflow-live-view-redesign-design.md` (that spec's `RunView` model, `NavState` drill axis, firehose, and correctness fixes are KEPT; only the on-screen composition is replaced).

## Problem

The first redesign (Plans 1–3) built the right plumbing — a correct `RunView`, a `NavState` drill model, a bounded firehose `TranscriptMux`, a diff `LiveRenderer`, in-view gates — but composed it as a **single scrolling column** (dashboard + flat step list + feed + footer). A tty smoke on a real run exposed two fatal readability failures:

1. **It doesn't read as a workflow.** The graph zone is a flat vertical list with no rails, edges, or fork/merge. You cannot see that a run has structure — splits, joins, parallel lanes, loops — let alone follow it. (`"I cannot even tell this is a workflow."`)
2. **The firehose is invisible until it isn't.** An empty feed renders zero rows with no frame, so the signature "watch the run breathe" surface is absent exactly when you first look.

A single column cannot legibly hold a DAG (loops, splits, 86-unit fan-outs, panels) **and** the live activity **and** the detail of what you're watching. The three compete for the same vertical space.

## Goal

A **dashboard-panes TUI** for `rupu workflow run` / `resume`: three live regions on screen at once — a rail-rendered **structure** pane (the DAG), the selected node's **stream** pane, and the global **firehose** pane — with a status header and a context footer. It reads as a workflow, the run always visibly breathes, and it scales to large/deep runs by collapse + drill rather than by cramming.

### Core principles

- **Three regions, always live.** Structure (where the run is), Stream (what the thing you're watching is doing), Firehose (what the whole run is doing). You never lose any of the three.
- **The structure pane IS a DAG.** Rails show flow; forks show splits/parallel; merges show joins; indentation shows containment (loop/panel members, fan-out units). One glyph per node kind so topology reads at a glance.
- **Scale by collapse + drill, never by cramming.** Settled upstream regions collapse to summary rows; the active frontier auto-expands; any node drills to its children; the stream follows the selection. A 512-unit, 18-node run stays legible.
- **Reuse the plumbing.** `RunView`, `NavState` (extended with pane focus), `TranscriptMux`, `LiveRenderer` are unchanged in purpose; `rupu-app-canvas`'s `Workflow`→`GraphRow` walk (a CLI-side crate) drives the structure pane's topology.
- **Correct first, always.** The correctness fixes from the first redesign (true cross-unit usage, no-mock meters, control-char-scrubbed feed, pinned-stream completeness) carry over unchanged.

## Non-goals

- No mouse. Keyboard only.
- No new `attach`/`watch` surface; this is the in-place view during `run`/`resume`.
- No change to `--plain` / non-tty / line-printer paths (they keep the Plan-1 completion summary).
- Not a general TUI framework; a purpose-built composition over the existing primitives.

## Layout

A frame is `header · body · footer`, where the body is a left **structure** column and a right column split into **stream** (top) and **firehose** (bottom):

```
 <workflow> · <crew> · ◐ <elapsed> · ⇡in ⇣out $cost · ⚑findings · cov P%     ← header (1 row)
 ────────────────────────────────┬───────────────────────────────────────────
  STRUCTURE                       │ <selected node> ············· ◐ active Ns   ← stream title
  ● preflight ········· ✓ 12s     │ 11:42:31 ▸ read_file  …                      │ stream
  ◈ split · recon      ◐          │ 11:42:33 ◇ thinking   "…"                    │ (selection-following)
  ┣ ⊞ sweep · 58/86 ◐ ▸           │ 11:42:39 ⚑ HIGH       …                      │
  ┃   ✓52 ◐6 ✗2 ○26  ⚑12          │ 11:42:44 ▪ assistant  "…"                    │
  ┣ ⇉ probe · 1/2 ◐               ├─ live · N active · H hosts ─────────────────
  ┗ ● crawl ◐                     │ otter#41 ⚑ HIGH …            @host-a         │ firehose
  ◈ join · recon 1/3 ◄ all        │ egret#1  ◇ think …           @host-b         │ (always live)
  ↻ loop:assess · iter 2/5        │ lynx#2   ✓ done  …           @host-b         │
  ⏸ approve · gate     ⏸          │ otter#07 ✗ fail  …           @host-c         │
  ○ report ·········· ○ @kuki     │ wren#1   ▸ grep  …           @host-a         │
 ────────────────────────────────┴───────────────────────────────────────────
 tab pane · ↑↓ nav · enter drill · ← back · / filter · a approve · Esc pause · q quit   ← footer
```

### Dimensions

- **Header**: 1 row, always. Adaptive meters (cost only when priced; `⚑` only when findings exist; coverage only when a ledger exists — no-mock).
- **Structure column**: a fixed fraction of width (default ~38 cols or 40%, clamped `[28, 48]`). Vertical divider `│` at a fixed column; the firehose sub-divider `├─ live … ─` starts in the right column.
- **Stream pane**: right column, top ~60% of body height.
- **Firehose pane**: right column, bottom ~40%, min 3 rows (always visible — empty → a dim `waiting for activity…` line, never zero rows).
- **Footer**: 1 row, context legend (pane- and gate-aware; `Esc pause` always labeled).
- **Body height** = `h - 2` (header + footer). Each pane is independently bounded and scrollable.

### Narrow-terminal fallback

Below a width threshold (e.g. `w < 76`) or height floor, collapse to a **single stacked pane**: header + structure (or the focused pane) + footer, cycling which pane is shown with `tab`. The three-pane split needs width; the fallback degrades gracefully rather than rendering an unreadable sliver. (This reuses the first redesign's adaptive-collapse discipline.)

## The structure pane

The structure pane renders the workflow DAG as rails, driven by `rupu-app-canvas`'s `render_rows(&Workflow, status_lookup)` (which already walks the step graph and emits `GraphRow { cells: [Pipe|Branch|Bullet|Space|Label|Meta] }` for linear, for_each, panel/panelist, gate, and branch structure). The CLI maps each `GraphCell` → a `Line`/`Segment` with `Style`, and overlays live state from `RunView` per node.

### Reuse boundary

- **Reused from `rupu-app-canvas`** (extend in-crate as needed — CLI-only consumer): the `Workflow`→rows walk, the branch/merge glyph vocabulary (`BranchGlyph::{Top,Mid,Bot,Merge}`), `NodeStatus` + its `glyph()`/`rgb()`, and the per-node anchor (`anchor_step_id`/`anchor_status`) used to attach live overlays and the selection marker.
- **Added by the CLI**: the live overlay (codename+role hue, agent, provider/model, duration, host `@chip`), the for_each **density line + movers** (Plan-2 `fanout_block`, re-homed under the node's rail), the **collapse/expand** at scale, the **selection marker**, and the per-kind leading glyph where app-canvas emits a generic bullet.
- **Extended in `rupu-app-canvas`** if its current output doesn't express it: explicit **`◈ split` / `◈ join`** nodes with lane nesting (`┣`/`┗`), **`↻ loop:<name>`** super-node framing its members with a loop-back affordance, **`⇉ parallel`** static-lane nesting, and **`◇ branch`** then/else arms (taken vs `⊘ skipped`). These map to `StepKind::{Split,Join,Loop,Parallel,Branch}` which the orchestrator already distinguishes. Any extension keeps app-canvas's existing snapshot tests green.

### Node vocabulary (one glyph per kind)

| Kind | Glyph | Notes |
|------|-------|-------|
| agent step | `●`/status | the status bullet is the glyph |
| run (command) | `▪` + `· run` | deterministic `run:` node |
| action (connector) | `◆` + `· action · <tool>` | |
| approval gate | `⏸` + `· gate` | amber; shows prompt + expiry when parked |
| for_each | `⊞` + `· N units` | then the density line + movers nested |
| parallel | `⇉` | static sub-steps nested as `┣`/`┗` lanes |
| split | `◈ split` | forks to named targets as lanes |
| join | `◈ join ◄─` | `wait:all/any/n` + `k/n` barrier count |
| loop | `↻ loop:<name> · iter n/max` | members nested; `↺` loop-back footer line |
| panel | `⟲ panel · round r/max` | panelists + fixer nested |
| branch | `◇ branch (when: …)` | `▶ then` / `⊘ else` arms |
| sub-agent / unit / lane | `┣`/`┗` child row | role codename + hue |

### Overlays (live state per node)

Status glyph + color (`NodeStatus`), the node's **codename** (role hue) · agent · provider/model, a dim dotted leader, then the right-hand state (`✓ <dur>` when done, else the status word / `working` / `◐`). Fan-out nodes carry the density line (`▓▓░░ 52/86  ✓52 ◐6 ✗2 ○26  ⇡tokens $cost`) + up to N live movers + `… +K more`. Loop/panel carry `iter n/max` / `round r/max`. Placed nodes carry an `@host` chip. Width-clip protects the right-hand state (Plan-2 `clip_row`).

### Scale: collapse + drill (the structure pane never overflows)

- **Collapse settled regions.** Contiguous `Complete`/`Skipped` steps that are not selected and not on the frontier fold to a summary row (`✓ preflight … inventory (+N done)`), oldest first, only as far as needed to fit the pane.
- **Phases.** A sub-DAG between a split and its join collapses to a single `⟦ phase ⟧` node with aggregate status when off the frontier; the **frontier phase auto-expands**. Drilling (`enter`) a collapsed phase expands its internal DAG (the subway "semantic zoom," applied in the structure pane).
- **Frontier is sacred.** Every `Running`/`AwaitingApproval` step (and its fan-out block) is never collapsed. The selected node is never collapsed.
- **Fan-out at scale** uses the Plan-2 density bar + movers; `enter` on it expands the full, filterable unit list (which drives the stream pane per selected unit).

## The stream pane

Selection-following: it shows the transcript of whatever the structure pane has selected — a step's own stream, or (after drilling a fan-out) a unit's / sub-agent's stream. Content is `TranscriptMux`'s **pinned** buffer for the selected transcript (complete regardless of sibling chatter), projected via `project_event` (tool calls, `◇ thinking`, `▪ assistant`, `⚑` findings, `✓` coverage, `✕` blocked, `!` notice) and control-char scrubbed. A title row names the selection (`<codename> · <unit_key> · <provider/model>  ◐ active Ns`), with a heartbeat. When nothing is selected (following the run), it shows the active frontier node's stream.

## The firehose pane

Always live, always framed. The cross-unit merged feed (`TranscriptMux::firehose_lines`), newest last, each row tagged with its codename (role hue) + host chip. Header `live · N active · H hosts`. Empty → one dim `waiting for activity…` row (never zero height). Bounded to the pane; scroll when focused.

## Interaction

- **Pane focus**: `tab` / `shift-tab` cycle focus Structure → Stream → Firehose. The focused pane takes `↑↓`/PgUp/PgDn for scroll; a subtle focus indicator (bright border/title) shows which pane is active. Structure-pane focus additionally drives nav (below).
- **Nav (structure focus)**: `↑↓`/`jk` move the selection within the current level; `enter`/`→`/`l` drill (phase → node → fan-out list → unit → sub-agent); `←`/`backspace`/`h` pop out; `a` (auto-follow) releases manual selection and tracks the frontier; `/` filters a fan-out list by status. The stream pane follows the selection; `p` is unnecessary (drilling a unit is the pin). Breadcrumb of the current depth is shown in the stream title or a thin line under the header.
- **Gate (modal)**: when a gate is focused (`focused_gate` — auto-focused when the run is `AwaitingApproval` and following; or the selected step when navigated), the footer shows and `a` approve / `r` reject / `v` view-findings act. One predicate drives footer + keys (the Plan-3 I1 contract).
- **Lifecycle**: `q` / Ctrl-C quit without pausing (alt screen restored); `Esc` pauses at the next safe boundary (labeled). Resize re-lays out + `renderer.invalidate()`. The resume-generation guard (Plan 3) governs exit.

## Reuse / change map

| Component | Status |
|-----------|--------|
| `output/run_model.rs` (`RunView`, `apply`, `from_run_dir`) | **unchanged** |
| `output/live_view/nav.rs` (`NavState`) | **extended**: add `Pane` focus + phase-level collapse awareness |
| `output/live_view/mux.rs` (`TranscriptMux`, `project_event`) | **unchanged** (pinned stream → stream pane; firehose → firehose pane) |
| `output/live_view/render.rs` (`LiveRenderer`, `style_ansi`, `AltScreen`) | **unchanged** (diffs any `Vec<Line>`; panes are just composed lines) |
| `output/live_view/row.rs` (`Line`/`Style`/`truncate_to`) | **unchanged** |
| `output/live_view/layout.rs` (`live_layout`, flat `graph`, `fanout_block`) | **replaced** by `panes.rs`: `dashboard_frame(view, nav, mux-lines, now, w, h) -> Vec<Line>` composing header + structure + stream + firehose + footer, with the narrow fallback. `fanout_block` is **re-homed** under the structure pane's rail; the flat `graph` is removed. |
| `output/live_view/structure.rs` (new) | drives `rupu-app-canvas` rows → `Line`s + live overlays + collapse/phase/drill + per-kind glyphs |
| `rupu-app-canvas` | **extended** for split/join/loop/parallel/branch lane rendering as needed (CLI-only consumer; keep its snapshot tests green) |
| `output/live_run.rs` (`run_live_view` loop) | **rewired**: build the pane frame (`dashboard_frame`) instead of `live_layout`; feed the stream pane from the pinned buffer, firehose pane from `firehose_lines`; `tab` pane focus; selection → mux pin |
| `cmd/workflow.rs` wiring, completion summary | **unchanged** |

## Error handling / edges

- Terminal too narrow/short → single-pane fallback; `w==0||h==0` → empty (no panic).
- A workflow whose structure `rupu-app-canvas` can't fully model (unknown kind) → render the node generically (app-canvas already falls back); never panic.
- Empty stream / empty firehose → `waiting for activity…` placeholder; never zero-height.
- Resize mid-frame → `invalidate()` + re-lay out.
- All exits restore the terminal (AltScreen RAII), including panic and Ctrl-C.

## Testing strategy

- **`structure.rs`**: snapshot the structure-pane rows for each construct (linear, run, action, gate, for_each+density, parallel, split→lanes→join, loop+members, panel+panelists, branch then/else) and for the collapse/phase cases, via `render_plain` (structure-only). Reuse `rupu-app-canvas`'s own tests for the underlying walk.
- **`panes.rs`**: snapshot the whole composed frame at representative sizes — wide (3-pane), narrow (fallback), gate-parked, 86-unit fan-out, a large collapsed/phased run. Assert the three panes and the firehose floor are present when wide; the structure pane never overflows its column; total height ≤ h.
- **`nav.rs`**: pane-focus cycling; phase drill/pop; selection → which transcript the stream pane requests; follow-frontier.
- **No flaky timing / real-terminal tests in CI.** `LiveRenderer` diff + `AltScreen` are driven headlessly (buffer). A human **tty smoke** is the required pre-merge gate (CLAUDE.md rule 7) — the first redesign's smoke is what caught the flat-list failure.
- **1.95 clippy** is CI-arbitrated (worktree is 1.97); write lint-clean proactively (match guards, no sole-if-in-arm, no collapsible/match-on-bool/needless-return).

## Open questions for review

1. **Structure/stream/firehose ratios** — default structure width (38 cols vs 40%) and stream/firehose split (60/40). Tunable; confirm defaults.
2. **`rupu-app-canvas` extension vs a fresh CLI rail renderer** — reuse + extend app-canvas (shared vocabulary, less code, but couples to its row model) vs a purpose-built `structure.rs` renderer (full control, more code). The spec assumes reuse-and-extend; flag if you'd rather a clean CLI renderer.
3. **Narrow-terminal fallback** — single-pane-cycling (above) vs a compressed 2-pane (structure + firehose, stream on drill). 
4. **Does this replace or stack on PR #695?** The plumbing in #695 is reused wholesale; this reworks the layout on the same branch, so #695's commits stay and the pane rework lands on top before a single merge (per the "hold #695, fix first" decision).
