# rupu Live Workflow Run View — Design

**Date:** 2026-06-17
**Status:** Approved (design); implementation pending
**Author:** matt + Claude

## Problem

`rupu workflow run` has no real live UI: it prints flat text lines per step, with
no graph, no animation, no per-step progress, and no token/cost visibility. On a
long agentic run the watcher can't tell *where* the run is, *whether it's alive
or stalled*, or *what it's costing* — the three questions that matter most for a
multi-minute, multi-agent assessment.

## Goal

A live, in-place terminal view for `rupu workflow run` (and `resume`) with three
zones: a **dashboard** (orientation + cost), a **node/edge graph** (structure +
per-step status), and a **focus feed** (the active agent's live activity + a
stall-detecting heartbeat). In-place redraw, no alt-screen.

## Layout (three zones)

```
  oracle-assessor-workflow                                  running · 4m 12s
  ───────────────────────────────────────────────────────────────────────────
  step 2/4   ████████████░░░░░░░░░░░░   assess
  ⇡ 1.2M   ⇣ 45K   $3.40          findings 12          coverage ███████░ 78%

  ● understand · oracle-recon ········· ✓ 18s ⇡102K
  ┃
  ● assess · for_each ················· ◐ 3/5 ⇡890K
  ┣━● conf-manager ···· ✓  ⇡210K
  ┣━● tlb-agent ······· ✓  ⇡180K
  ┣━◐ app-gw ·········· working ⇡120K
  ┣━○ rtc ············· queued
  ┗━○ auth ············ queued
  ┃
  ● sweep · panel ⟲ ··· ◐ iter 2/10 · 2 found
  ┃
  ○ report ············ ◌ pending

  ╭ app-gw · oracle-assessor                      ⇡120K   ◐ active 2s ago ╮
  │  17:42:31  ▸ read_file    services/app-gw/handler.go                  │
  │  17:42:33  ⚑ finding      HIGH  path traversal via X-Waf-Requestid    │
  │  17:42:35  ▸ grep         "exec.Command" under services/app-gw        │
  │  17:42:38  ✓ coverage     cwe-78-os-command-injection · finding       │
  ╰─────────────────────────────────────────────────────────────────────────╯
```

### Zone 1 — Dashboard
- Title (brand), run status, elapsed (top-right).
- A horizontal rule.
- Overall progress bar `step N/M` + active step name. Bar fills `completed_steps / total_steps`.
- Meters row, **adaptive** (each shown only when the run has produced it):
  always `⇡ input  ⇣ output  $cost` (compact K/M units + 2-dp cost, reusing the
  status-bar formatters); `findings N` when any `report_finding` / panel finding
  has fired; `coverage P%` (+ mini-bar) when a coverage ledger exists for the
  run's workspace.

### Zone 2 — Git-graph spine
Built on `rupu_app_canvas::render_rows(workflow, |node| live_status(node))`,
painted by the CLI with **heavy** edges:
- Nodes: `✓` complete · `◐` working (animated) · `✗` failed · `○` queued ·
  `◌` pending/never-reached. Colored by status (existing `node_status_color`).
- Edges: `┃` spine, `┣━`/`┗━` fan-out branches.
- A `panel` step is annotated `⟲` and shows `iter K/max · N found` while looping.
- A `for_each` step expands its **units** as branch nodes (each an agent run),
  with per-unit status + tokens. Per design choice: only the **active** step's
  units expand; completed/pending fan-outs collapse to a one-line
  `◐ 3/5` summary.
- Dotted leaders (`····`) right-align each node's status + per-node tokens/time.

`live_status(node)` is derived from the run's event state (see Data flow).

### Zone 3 — Focus feed (fills remaining terminal height)
- A bordered panel for the **active** agent run: header `unit · agent  ⇡tokens
  ◐ active Ns ago`.
- Body: a rolling feed of the active agent's transcript events — `▸ tool_call`
  (name + key arg), `⚑ finding` (severity + title), `✓ coverage` marks, text
  deltas summarized. Timestamped. This reuses the **same transcript streaming**
  the `session attach` TUI already does.
- **Heartbeat:** `active Ns ago` = seconds since the last event from the active
  agent. It climbs visibly when the model stalls — the at-a-glance "is it alive?"
  signal (and the new stream-timeout will turn a true stall into a `✗` + resume
  hint).
- Height: grows to fill the terminal below Zone 2 (rolling window sized to the
  available rows); shrinks gracefully on small terminals.

### Failure state
The failed step/unit renders `✗`; Zone 3's last line becomes the recovery hook:
`↳ rupu workflow resume <run-id>`.

## Data flow

The live run already exposes step events via `EventSink` / the
`live_event_hook` (`crate::output::workflow_printer`). A new `LiveRunState`
accumulates, from those events + the per-agent transcripts:
- per-step status + per-step tokens/elapsed,
- per-unit (fan-out) status + tokens,
- panel iteration count + findings,
- the active agent + its rolling activity feed + last-event timestamp,
- run totals (tokens, cost via the pricing table, findings, coverage).

A render tick (~10/s, `tokio::time::interval`) repaints the whole fixed-height
block in place (cursor-up + clear-to-end + reprint), advancing the spinner frame
and the heartbeat. Step/unit events also trigger an immediate repaint.

## Touch points
- `crates/rupu-cli/src/cmd/workflow.rs` — the live run path: a `LiveRunState`, the
  render loop, the three-zone renderer (Zones 1/2/3), wired to the existing
  `live_event_hook`/`EventSink`. Reuses `render_graph_rows`, `node_status_color`,
  the status-bar token/cost formatters, and the transcript-tail logic.
- `crates/rupu-app-canvas` — only if the spine needs a heavy-edge variant or
  per-unit fan-out expansion not currently emitted; otherwise unchanged.
- No orchestrator changes — it already emits the events.

## Testing
- Pure renderer tests: feed a synthetic `LiveRunState` (various states — fan-out
  3/5, panel iterating, failed unit) into the Zone-2 + dashboard renderers and
  assert the produced rows (ANSI-stripped) — same pattern as the existing
  `render_workflow_show_graph` / session-header tests.
- Heartbeat formatting + progress-bar fill unit tests.
- Live rendering itself (cursor control, resize, animation) is validated by
  **running it** — terminal rendering can't be asserted in CI (per the rupu-app
  rule); matt runs the binary before merge.

## Out of scope (v1)
- Full alt-screen interactive TUI (scrollback, key handling) — this is the
  in-place redrawing variant. Promotable later.
- Mid-panel-iteration drill-down beyond the iteration counter.
