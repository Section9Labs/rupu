# rupu.app macOS — Performance & Interaction Parity

**Date:** 2026-08-25
**Status:** Executing under matt's standing autonomy directive (2026-08-25: "continue autonomously… parity or even better… a native app should run faster than the web UI"). Docs travel with the execution PR rather than a separate docs PR — deliberate cadence choice, recorded here.
**Parent:** `docs/superpowers/specs/2026-08-24-rupu-macos-design-alignment-design.md` (visual arc, complete)
**Evidence base:** 2026-08-25 code audits — app hot-path audit (file:line findings embedded in the plans) + web interaction inventory (`usePagedList.ts`/`useInfiniteScroll.ts`, per-kind table columns, `RunGraph.tsx` interaction config).

matt ran the post-Plan-4 build and rejected the experience: slow everywhere, sidebar groups effectively only open via selection, one generic combined table where the web has four per-kind tables, history capped at the first fetched page (no infinite scroll — `ActivityStore.loadMore()` shipped in Phase 2 with zero callers), graphs not zoomable/pannable with live effects not visibly rendering. Governing rule (now also a memory): **perceived speed and direct-manipulation affordances are merge gates, same rank as tests; never YAGNI-cut a web interaction affordance.**

## 1. Plan 5 — Responsiveness & data depth

### Perceived speed (the audit's top offenders, fixed at the source)
- **Local-first everywhere.** Security/Projects/Library/Usage/Fleet stores block first paint on a serial fleet fan-out (client docs: 2.5–4.0s with one offline host vs 60–70ms local). Add `host:` params to the client methods that lack them; every list/detail store adopts the ActivityStore shape: local fetch paints first, remote hosts merge progressively in background tasks.
- **Cold launch:** cache the discovered binary path (UserDefaults) and use it optimistically; fast probe (~300ms) before the slow path; collapse the observation→Task→onChange→onChange activation chain so the default screen fetches as soon as a client exists. DashboardStore stops serializing `hosts()` before the local dashboard fetch.
- **Allocation storms:** lucide `Icon` parses SVG path strings per render — cache parsed paths once (`static` per-icon table). `Color.status/severity/severityBg` + pill helpers mint dynamic NSColors per call — precomputed static tables. `Fmt.cost/count` allocate NumberFormatters per row — static formatters (or integer arithmetic, like `Fmt.duration`). `ActivityRow.parseISO` builds 2 ISO8601 formatters per row (×4 duplicated sites) — static/`FormatStyle`. SSE frame decode reuses one decoder.
- **Per-event O(n) rebuilds move out of view bodies into stores/memoized state:** ActivityTable's sort (store-side `sortedRows`), TranscriptFeed's `buildFeedRows` (computed TWICE per body) + `sawRunComplete` full scans, `layoutGraph` (hoisted to RunDetailStore, event-coalesced; fanout counts single-pass; canvas goes `LazyHStack`), `MarkdownView` parse-in-init, `StructuredView` prettyPrinted-in-body, Sidebar/Overview per-access JSON decodes (→ `@State` + `.onChange`). CodeHighlighter: hash-keyed cache, O(1) LRU, `setTheme` hoisted out of the per-call path.
- **Connection hygiene:** Overview stops constructing a second ActivityStore (shares the screen store or a needs-you projection), cutting resting SSE connections (currently 4–6, each replaying history from byte 0).
- **Measurement, not vibes:** a lightweight `RenderMeter` (debug-only os_signpost/counter seam) instruments the fixed screens; the plan's checkpoint records before/after counts for a scripted interaction (sidebar hover sweep, activity live tail, run-detail live tail).

### Sidebar
- The reported "only opens via selection" bug is hit-testing: the chevron button's tappable area is the glyph's ~1pt stroke (no `contentShape`), and the row background sits outside both buttons. Fix: `contentShape(Rectangle())` + background-inside-label (the `railRow` pattern), chevron hit target ≥24×30pt. Disclosure state logic is already correct (explicit toggle wins; verified).

### Per-kind tables (web parity)
- The web has NO combined activity table — four pages on one `SortableTable` primitive with per-kind column sets (inventoried exactly, incl. Sessions' Model column, AgentRuns' two-line agent subject + source badge, WorkflowRuns' trigger chip, AutoflowRuns' chevron-expandable `cycle_failed` detail rows and blank-not-dash non-run events). The app keeps its combined table ONLY for the Activity parent ("All runs" — a deliberate native extra), and each sidebar kind child renders a dedicated per-kind table with the web's column set, alignment (`fit`/right-aligned tabular numerals), one truncating subject, default sort `{started, desc}` (Sessions: server order), and per-kind actions.

### Data depth (pagination + infinite scroll)
- Port the web's list machinery: page size 20 (server clamps at 200), scroll-sentinel append (List `.onAppear` on a trailing sentinel row, threshold-equivalent to the web's 240px), footer states ("loading more…" / "scroll for more" / "— end of N —" / "{n} matches of {m} loaded" when finding), reset-on-filter-change, and head-freshness via the existing SSE live tail (the app's superior equivalent of the web's 5s page-0 poll-splice — keep splice semantics: never reset scroll). `loadMore()` finally gets its caller; per-kind tables page their own kind endpoints (`/api/runs/agents|workflows|autoflows/events|/api/sessions` with offset/limit/lifecycle/scope/host).
- Honesty: the top-bar range control does not apply to lists (web has no list ranges — depth is scroll); it stays scoped to Overview/Usage and is hidden/disabled on Activity routes so it can't imply a filter it doesn't perform. Client-side status/scope narrowing gets the web's "{n} matches of {m} loaded" honesty footer.

## 2. Plan 6 — Graph interactivity & live-effect correctness

- **Zoom/pan, native:** the graph canvas becomes a magnification-capable surface (NSScrollView via NSViewRepresentable with `allowsMagnification`, wheel/pinch zoom bounds ≈0.5–2.0 matching React Flow defaults, drag-pan, double-click zoom, fit-on-open ≤1.0 like the web's `fitView maxZoom:1`), with zoom-in/out/fit controls in the corner (web `Controls` parity, `showInteractive=false` equivalent). Minimap explicitly deferred (not in matt's complaint; tracked, not dropped).
- **Live effects that actually show:** effects stay state-derived (web parity — never connection-derived). Root-cause and fix the animation visibility: per-event `layoutGraph` array rebuilds can re-identify nodes/edges and remount views mid-animation (killing ants/pulses); stabilize identity (keyed by step id, Equatable cards) on top of Plan 5's store-side coalesced VM so animations survive live updates. Verify the replay path (run log replays events.jsonl from byte 0) populates `liveStates` when opening an already-running run — pulse/ants must appear on open, not just for events after mount.
- Graph area keeps height 420 but non-content states (loading/empty/failed) stop reserving it (parked Plan 3 minor, promoted here).

## 3. Verification & sequencing

- Plan 5 → Plan 6, subagent-driven, per-task reviews + whole-branch finals, merge on green CI. RenderMeter before/after counts recorded at each plan's close-out. matt's GUI pass is the perceived-speed gate — explicitly: sidebar toggling without navigation, cold launch to first Overview paint, Activity scroll past 200 rows into month-old history, per-kind tables, graph zoom/pan on a live run with pulse/ants visible.
- Store/contract tests stay green throughout; every perf fix that changes observable behavior (sorting location, pagination) carries tests.

## 4. Out of scope

Netflow aggregate surface (Security ▸ Network child — separate arc); minimap; sub-run navigation; web-side fixes (subrun field bug tracked separately); saved views; row virtualization changes to `List` (already NSTableView-backed).
