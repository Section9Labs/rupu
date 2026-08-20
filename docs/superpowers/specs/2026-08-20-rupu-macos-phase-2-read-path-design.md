# rupu.app macOS — Phase 2: Read Path (Activity + Run detail) design

**Date:** 2026-08-20
**Status:** Approved in design session (matt, 2026-08-20); spec review pending
**Parent:** `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md` (umbrella, §8 Phase 2 row)
**Visual contract:** `docs/macOS_design/HANDOFF.md` — screens 2 (Activity) and 8 (Run detail)

Phase 2 replaces the Activity and Run-detail placeholders with the real read path:
the execution table, run detail (step graph, transcript feed, netflow + findings
rails), and read-only session detail. It also delivers the spec §6 discipline the
umbrella deferred to its first consumer: **REST snapshot + SSE delta + re-snapshot
on reconnect**, as testable store primitives.

## 1. Scope decisions (settled with matt)

1. **Strict read-only.** No mutations render. A gated run shows its awaiting state
   (pulsing gate node, awaiting banner) with no Approve/Reject buttons — those are
   Phase 3. No-silent-noop: absent, not disabled.
2. **Sessions: list + read-only detail.** Sessions appear in Activity; clicking
   opens a read-only session view (metadata, transcript, child runs). The send box
   is Phase 3.
3. **Saved views deferred to Phase 4**, designed once together with the saved-view
   dashboard widget. Phase 2 ships kind control + additive status chips +
   live-tail switch, no filter-set persistence.
4. **Display-only host awareness.** The Host column renders what `/api/runs`
   reports. Live event tail and transcript streaming are local-runs-only; a remote
   run's detail page is fully REST-backed and shows an honest
   "remote streaming lands with Fleet (Phase 5)" note where a tail would be.
   (Remote streams need `?host=&run=` and hit the known CP-side remote-transcript
   resolution gap — deferred by design, tracked in the umbrella's parity ledger.)

## 2. Data layer — RupuStore primitives

Three reusable, headless-testable primitives; every Phase 2+ store composes them.

- **`PagedSnapshot<Row>`** — offset/limit paging over a REST list endpoint.
  Per-page `content | loading | failed(error, retry)`; `loadMore()`, `refresh()`.
  Refresh replaces page 0 and truncates (no splicing heuristics).
- **`LiveReducer`** — pure functions applying a `CPEvent` to typed row/detail
  state: run started → prepend; step/unit/run lifecycle events → update status,
  duration, counters; unknown variants → ignored. No I/O; fixture-driven tests.
- **`StreamLifecycle`** — owns one `EventStreamClient` consumer task per store:
  connect → fresh; disconnect → stale (UI may show staleness); **reconnect →
  await the store's `resnapshot()` BEFORE resuming delta application** — the
  spec-§6 no-gap-guessing rule, implemented once, tested with scripted
  connect/drop/reconnect sequences. Torn down when the owning store deactivates
  (route change) — closes the Phase 1 ledger item about stream teardown.

Stores:

| Store | Snapshot | Delta | Notes |
|---|---|---|---|
| `ActivityStore` | `GET /api/runs` (paged; kind via existing endpoint/params) | global firehose `/api/events/stream` | Status chips filter client-side on loaded pages. Live-tail switch ON → deltas apply directly; OFF → deltas accumulate behind a "N new runs" refresh pill so a scrolled table never jumps. |
| `RunDetailStore` | `GET /api/runs/:id` + `/graph` + `/netflow` + per-run findings — four independent blocks | run-scoped `/api/events/stream?run=<id>` | Per-block state: one slow rail never blanks the page (spec §9). Terminal run event stops the stream. Transcript: `GET /api/transcript` snapshot + `/api/transcript/stream` tail for the focused step (local runs). |
| `SessionDetailStore` | `GET /api/sessions/:id` + `/:id/runs` | transcript tail (same component) | Child-run rows route to Run detail. |

## 3. Activity screen — module `RupuActivity`

- Kind segmented control bound to the SAME `Route.activity(kind)` state as the
  sidebar (contract proven by Phase 1's routing test — extend it, don't fork it).
- Additive status chips; live-tail switch; columns
  Status·Kind·Subject·Project·Host·Trigger·Dur·Cost·Started (Subject flexible;
  numerals `Font.numeral`; unknown cells `—`; gated rows 4% await tint).
- Row activation routes: run rows → `Route.runDetail(id)`, session rows →
  `Route.sessionDetail(id)` (new Route cases; back returns to the filtered list).
- Per-block empty/loading/failed states; the failed state carries retry.

## 4. Run detail screen — module `RupuRunDetail`

- Header: breadcrumb (Activity ▸ subject), status pill, facts row.
- **Step graph**: custom SwiftUI component over `GET /api/runs/:id/graph` node
  kinds, with live event overlay — done ✓, running pulse, gate pulsing in await
  color, pending dashed; branch/fan-out grouping as the graph data describes.
  Layout logic (node placement, lane assignment) is a pure, unit-tested function;
  the view only renders its output. All pulses guarded by Reduce Motion.
- **Transcript feed**: agent prose as text blocks; tool calls as collapsed JSON
  blocks (`Font.identifier`); gate rows with accent left edge. Snapshot + live
  tail per §2. The feed component is session-reusable.
- **Rails**: run facts; per-run netflow (unexpected hosts loud in fail color);
  per-run findings (2px severity left edge, worst-severity figure coloring).
- Gated runs: awaiting banner, no buttons. Remote runs: REST blocks render,
  stream slots show the Phase-5 note.

## 5. Session detail (read-only, inside `RupuRunDetail` module)

Metadata header (agent, model, started, usage), child-run list (→ Run detail),
transcript feed reuse. No input affordances.

## 6. Fixtures & Phase 1 carry-overs

New golden fixtures emitted by rupu-cp tests, same drift-check pattern:
`run_list_row.json`, `run_detail.json`, `run_graph.json`, `run_netflow.json`,
`findings.json`, `session.json`, `transcript_lines.json`, `event_rows.json`.

Carry-overs picked up this phase (from the Phase 1 ledger):
- `CPEventRow` golden fixture; `ts`/`pos` become **non-optional** (the endpoint
  guarantees them on every row).
- Stream teardown on route change (via `StreamLifecycle` ownership).
- SSE backoff reset keys off successful connect (not only first decoded frame).
- matt's button-polish notes from the Phase 1 validation run get a pass while the
  shell is open.

## 7. Testing & validation

- Store tests: `LiveReducer` against fixture-derived event sequences;
  `StreamLifecycle` connect/drop/reconnect choreography asserting the
  resnapshot-before-resume ordering; `PagedSnapshot` paging/refresh semantics.
- Decode tests for every new fixture; graph-layout pure-function tests.
- Rendering: computer-use screenshots where possible; matt runs the app before
  merge (standing GUI rule). Integration smoke: live `rupu run` against a scratch
  project with the app tailing it — table row appears live, run detail streams,
  graph nodes progress, terminal state settles.

## 8. Out of scope

Mutations of any kind (Phase 3), saved views and dashboard widgets (Phase 4),
remote streaming and per-host freshness UI (Phase 5), coverage/security screens
(Phase 5), transcript search, cost aggregation beyond what `RunListRow` carries.
