# rupu.app macOS — Phase 4: Overview Dashboard

**Date:** 2026-08-24
**Status:** Executed under matt's standing "continue with all of the phases" authorization (2026-08-24); design decisions recorded here for review at the phase checkpoint
**Parent:** `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md` (umbrella §8, Phase 4)
**Visual contract:** `docs/macOS_design/V2-CONTRACT.md`; layout intent per the v2 arc (`docs/superpowers/plans/2026-08-04-rupu-cp-shell-v2-arc.md` Plan 3 row) and `docs/redesign/README.md` §1a–1c

Replaces the `.overview` placeholder with the real Overview screen, born in the v2
language: needs-you queue + instrument strip + trend charts + fleet band, backed by
`GET /api/dashboard` and the app's existing federated run machinery.

## 1. Composition (top to bottom)

1. **Host freshness strip** — one pill per registered host (`ok`/`offline`/
   `unavailable` + relative `captured_at`), painted from the dashboard response's
   `hosts` array; range comes from the global toolbar (7d/30d/all — existing
   app-level state, no local selector).
2. **Needs-you queue** (headline block) — fleet-wide, **ignores the project scope
   selector** (v2 arc locked decision #4: a parked gate must never be hidden by a
   filter). Row types this phase: approval gates (awaiting tone, oldest first) and
   failed runs (failed tone, newest first, within the active range). Cap 6 rows +
   "+N more" footer that navigates to Activity pre-filtered (awaiting). Inline
   actions: gates get Approve / Reject (the same gate-scoped
   `PendingActions` machinery as Activity/RunDetail — pending state visible in
   place, confirm-from-observed-status, never optimistic-remove); failed runs get
   Open (run detail). Empty state: a single quiet row "nothing needs you"
   (`ink-mute`) — never an empty card. Critical-finding rows are **Phase 5**
   (global findings endpoint) — the row model is built to accept a third type
   then.
3. **Instrument strip** — one bordered panel, six equal cells: Awaiting you ·
   Active now · Paused · Failed (range) with inline sparkline · Success rate ·
   Open findings. Values from the merged dashboard summary; null discipline: a
   poisoned/partial field renders `—` (plus `+` marker when `*_partial`), never a
   fabricated 0.
4. **Charts row** — two side-by-side stacked-area charts via SwiftUI Charts
   (first-party, allowed): **Outcomes** over `terminal_buckets`
   (completed/failed/rejected/cancelled, status-tone colors) and **Throughput by
   trigger** over `throughput_buckets` (manual/cron/event, trigger palette ported
   from the web's TriggerChip — three new tokens). No animation (web rule:
   liveness is per-transport, charts don't pretend). Buckets arrive zero-filled
   from the server — no client-side gap fill.
5. **Cycle summary line** — "N cycles · M clean · K with failures" (mono data,
   `+` when `cycles_partial`).
6. **Fleet strip** — dim inventory band (repos · providers (+unhealthy loud) ·
   autoflows on/off · workers · claims · issues, `issues_capped` renders `N+`);
   weight only on fault.

## 2. Data: `DashboardStore`

New `CPClient.dashboard(range:host:) -> APIDashboardResponse` (`GET
api/dashboard?range=<r>&host=<id>`). `APIDashboardResponse` decodes the flattened
response: `hosts: [APIHostFreshness]`, partial flags, `active`, `activeLongest?`,
`terminalBuckets`, `throughputBuckets`, `cycles`, `findingsOpen?`, `fleet`,
`capturedAt`.

`DashboardStore` mirrors the web's `useDashboardData` (the Phase-2 fan-out
lesson, encoded):

- Seed host slices from `client.hosts()` (cheap), paint the freshness strip
  immediately.
- Fetch `?host=<id>` **independently per host** — never one all-hosts request,
  never `Promise.all` semantics; local paints first, each remote merges as it
  answers, a hung host never blocks the page.
- Client-side merge with the server's own semantics (ported + unit-tested): sum
  only present values; a missing value from any reporting host poisons the field
  and sets the matching partial flag — never fabricate 0; `captured_at` = oldest;
  `active_longest` = max age.
- Refresh: firehose events are an invalidation signal only → 250ms-coalesced
  refetch of local; a 60s reconcile pass refetches every host; range change
  refetches all. Generation-guarded (the `remoteGeneration` idiom) so stale
  fetches never overwrite.
- Per-block rendering: blocks paint from whatever has resolved (`BlockState`
  discipline); page-level failure only when nothing has resolved.

## 3. Needs-you data (disposition)

No attention endpoint exists server-side, and building `GET /api/attention` is
real new API surface gated on known `RunListRow` gaps (no kind/project fields
fleet-wide). **Disposition: deferred (tracked)** — it belongs with the web's own
v2 Plan 3 and the Phase 5 parity pass. This phase computes needs-you
**client-side** from the app's already-federated run sources: Overview owns an
`ActivityStore` instance (kind `.all`, live tail on) and derives the queue with a
pure, tested function — awaiting rows oldest-first, then failed rows within
range newest-first, cap 6. Approve/Reject go through that store's existing
gate-scoped mutations; the shared `PendingActions` ledger keeps Overview,
Activity, and RunDetail in agreement about in-flight state. Honest consequence,
stated in-app: the queue covers runs the list APIs return (fleet coverage
matches Activity's), not a server-defined attention set.

## 4. Customize & persistence (umbrella "WidgetConfig" scoped honestly)

A toolbar "Customize" menu on Overview toggles block visibility (needs-you,
instruments, charts, cycles, fleet) and persists as JSON under the
`overview.widgets` UserDefaults key (`[String: Bool]`, absent = visible).
Reordering, size classes, drag/jiggle, and the widget gallery from HANDOFF are
**deferred to the post-phases UI-redesign pass** (matt, 2026-08-24) — recorded
here so the umbrella's "WidgetConfig persistence" line has an explicit, honest
scope: visibility persistence now, layout editing later.

## 5. Verification

- Fixture: `dashboard.json` emitted by `macos_fixtures.rs` from the real
  `DashboardResponse` serde type; Swift decode test; drift-gated by
  `cargo test -p rupu-cp`.
- Merge semantics unit-tested against poisoning/partial/oldest-capture cases
  (mirroring `merge_dashboard_summaries`' own test table).
- Needs-you derivation pure-function tests (ordering, cap, range window, empty).
- Store lifecycle condition-polled (no fixed sleeps); all existing suites stay
  green; zero warnings; Plan-1 exit greps stay zero.
- Checkpoint: screenshots dark+light; matt runs the app before merge.

## 6. Out of scope

`GET /api/attention` (deferred, tracked); critical-finding needs-you rows
(Phase 5); hosts rail with per-host load bars (needs richer host data — Phase 5
Fleet screen); live event stream block (Phase 6 Situation Room family); widget
drag/reorder (UI-redesign pass); web-side changes.
