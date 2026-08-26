# rupu.app Perf Arc — Plan 5: Responsiveness & Data Depth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the app *feel* faster than the web (kill the audited allocation/rebuild/fan-out hot paths, measured), fix the sidebar hit-testing bug, replace the one-size-fits-all activity table with web-parity per-kind tables, and wire real infinite scroll so history past the first page is reachable.

**Architecture:** Mechanical hot-path fixes land first (static caches, store-side derived state, local-first fetches) since everything else renders through them; then per-kind tables on a shared native SortableTable-equivalent; then pagination through the existing `PagedSnapshot`/`loadMore` machinery finally wired to a scroll sentinel. A debug-only `RenderMeter` seam records render/alloc counts so the checkpoint reports numbers, not vibes.

**Tech Stack:** SwiftUI (macOS 14+), Swift 6, Swift Testing. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-25-rupu-macos-perf-interaction-design.md` §1 · Audit findings cited inline are from the 2026-08-25 hot-path audit; verify each against source before editing (line numbers may drift — symbols are authoritative).

## Global Constraints

- Perceived speed + affordances are merge gates (spec governing rule). Every fix that changes observable behavior carries tests; pure-cache fixes carry at least an equivalence test (same output as before).
- Local-first mandate: NO list/detail store may block first paint on a fleet fan-out. `host: "local"` first, remotes merge progressively (the `ActivityStore` shape).
- Web pagination contract to port: page size 20; append on scroll-sentinel; footer "loading more…"/"scroll for more"/"— end of {n} —"/"{n} matches of {m} loaded"; reset on filter change; head freshness never resets scroll (SSE splice keeps this property).
- Test hygiene (standing repo rules): Swift Testing; View-member tests `@Test @MainActor` (statics on View types too); no timed-sleep stubs (semaphore gating); condition-poll terminal states. `make macos-test` + `make macos-build` green after every task; `cargo test -p rupu-cp` when serde-adjacent files change. NEVER bare `git stash pop`. Null discipline `—` never 0.
- Don't regress the two-channel graph rule or any Plan 1–4 contract.

## File structure

```
RupuKit/Sources/RupuDesign/Icons/Icon.swift              # T1: parsed-path cache
RupuKit/Sources/RupuDesign/Tokens.swift                  # T1: static status/severity tables
RupuKit/Sources/RupuDesign/Formatters.swift              # T1: static formatters
RupuKit/Sources/RupuAPI/{ListRows,SSE,CPClient}.swift    # T1 (ISO/static decode), T2 (host params)
RupuKit/Sources/RupuShell/Sidebar.swift                  # T0: hit areas + @State groups decode
RupuKit/Sources/RupuOverview/OverviewScreen.swift        # T0 widgets decode; T2 shared store
RupuKit/Sources/RupuBackend/{BackendController,EmbeddedServer}.swift  # T2: fast start
RupuKit/Sources/RupuStore/{SecurityStore,ProjectsStore,LibraryStore,UsageStore,FleetStore,DashboardStore}.swift  # T2: local-first
RupuKit/Sources/RupuStore/{ActivityStore,RunDetailStore}.swift        # T3: derived state; T5: paging
RupuKit/Sources/RupuRunDetail/{TranscriptFeed,RunDetailScreen,StepGraphView,Rendering/*}.swift  # T3
RupuKit/Sources/RupuDesign/RenderMeter.swift             # T1: debug seam
RupuKit/Sources/RupuActivity/KindTables/*.swift          # T4: per-kind tables
RupuKit/Sources/RupuActivity/{ActivityScreen,ActivityTable,FilterBar}.swift  # T4/T5
```

---

### Task 0: Sidebar hit-testing + per-access JSON decodes (batched quick wins)

**Files:**
- Modify: `RupuShell/Sidebar.swift`, `RupuOverview/OverviewScreen.swift`
- Test: extend `RupuShellTests/SidebarGroupsTests.swift` only if logic moves; hit-area fixes are view-only (matt's GUI pass verifies)

**The bug (audit #6):** in `groupParentRow`, the chevron `Button`'s label is a stroked 10×10 `Icon` inside an empty (non-hit-testable) `.frame(width:16,height:30)` — its clickable region is the ~1pt stroke. The row `.background(...)` sits on the outer HStack, OUTSIDE both buttons, so the label button's hit area is only its rendered content. Result: toggling without navigating is nearly impossible — matt's exact complaint. `railRow`/`childRow` already do it right (background inside the button label).

- [ ] **Step 1:** `groupParentRow`: move the background/clip inside each button's label (railRow pattern); add `.contentShape(Rectangle())` to BOTH button labels; widen the chevron button's label to a filled `.frame(width: 24, height: 30)` containing the glyph, `.contentShape(Rectangle())`. Verify: chevron toggles a non-active group without navigating; clicking anywhere on the label area navigates.
- [ ] **Step 2:** `Sidebar.groups` computed property decodes `groupsData` JSON per access (4×/body pass; body re-evaluates per hover change) → `@State private var groups: SidebarGroups`, initialized once and updated in `.onChange(of: groupsData)`; writes go through the existing encode path. Same pattern for `OverviewScreen.widgets` (≈9 decodes per body pass) → `@State` + `.onChange`.
- [ ] **Step 3:** `make macos-test` + `make macos-build` green. **Commit** — `fix(macos-perf): sidebar chevron/label hit areas + cached sidebar/overview pref decodes`

### Task 1: Allocation storms — static caches (batched same-shape fixes) + RenderMeter

**Files:**
- Modify: `RupuDesign/Icons/Icon.swift` (+`SVGPathParser` consumers), `RupuDesign/Tokens.swift`, `RupuDesign/Formatters.swift`, `RupuAPI/ListRows.swift` (`ActivityRow.parseISO`) + the duplicated ISO sites (`UsageKit`'s `UsageAggregation.swift`, `RupuStore/UsageStore.swift`, `RupuSituation/StreamCards.swift`), `RupuAPI/SSE.swift` (frame decoder reuse)
- Create: `RupuDesign/RenderMeter.swift`
- Test: `RupuDesignTests` — equivalence tests (cached output == freshly-computed output) for icon paths, color tables (RGB-resolved both appearances), formatter outputs incl. locale pinning; ISO parse round-trip

**Interfaces:**
- `Icon`: `private static let parsedPaths: [LucideIcon: [SVGPath]]` built once (lazy, thread-safe via `static let`); `IconShape.path(in:)` reads the cache — zero parsing per render. (Do NOT change stroke rendering or sizes.)
- `Tokens`: `Color.status(_:)`, `.severity(_:)`, `.severityBg(_:)`, `statusPillBackground/Ring/Ink` become dictionary/switch lookups over `static let` precomputed `Color`s (9 status + 5 severity + 5 severityBg + 3×9 pill variants). Public signatures unchanged.
- `Fmt.cost/count`: `static let` formatters (`nonisolated(unsafe)` with a doc comment on thread-confinement, or integer arithmetic mirroring `Fmt.duration`) — output byte-identical (tests assert exact strings incl. the en_US_POSIX pinning).
- `ActivityRow.parseISO` + duplicates: shared `nonisolated` static parsers (`ISO8601FormatStyle` is Sendable — prefer it); one helper reused by all four sites.
- `RenderMeter` (DEBUG-only seam): `RenderMeter.tick(_ label: StaticString)` — a `#if DEBUG` os_signpost + atomic counter, `RenderMeter.count(for:)`/`reset()` internal for tests. Insert ticks in: Sidebar body, ActivityTable body, TranscriptFeed body, StepGraphView body. Release builds compile it to nothing (assert via a release-flag test if cheap, else document).
- [ ] **Step 1:** TDD equivalence tests (RED where they pin current behavior against the new path) → implement all → GREEN.
- [ ] **Step 2:** `make macos-test` + `make macos-build`. **Commit** — `perf(macos): static icon-path/color/formatter caches + RenderMeter seam`

### Task 2: Local-first everywhere + fast cold start + connection hygiene

**Files:**
- Modify: `RupuAPI/CPClient.swift` (add `host: String? = nil` to `findings`, `coverage`, `projects`, `agentDefinitions`, `workflowDefinitions`, `autoflowDefinitions`, `usage`, `usageRuns`, `usageOutliers`, `workers` — verify each endpoint accepts `?host=` in `crates/rupu-cp` handlers; any that genuinely can't gets a doc comment and is excluded, honestly)
- Modify: `RupuStore/{SecurityStore,ProjectsStore,LibraryStore,UsageStore,FleetStore}.swift` — adopt the ActivityStore local-first shape: local fetch paints, per-online-host background tasks merge progressively (generation-guarded, same idioms; reuse the existing progressive-merge helpers where they exist)
- Modify: `RupuStore/DashboardStore.swift` — stop awaiting `hosts()` before the local dashboard fetch (fire local immediately; hosts + remotes follow)
- Modify: `RupuBackend/{BackendController,EmbeddedServer}.swift` — cache discovered binary path in UserDefaults (`backend.binaryPath.cached`), use optimistically with fallback to full discovery on spawn failure; initial probe timeout ~300ms before the slow path; collapse the health→activate chain: expose a `clientGeneration` (or make `client()` identity observable) so screens activate directly on client availability instead of the onChange relay
- Modify: `RupuOverview/OverviewScreen.swift` — stop constructing a second `ActivityStore`; consume the shared one (inject via environment from RootView, constructed once) for needs-you derivation
- Test: `RupuStoreTests` — per-store: local rows visible while a gated (semaphore) remote host is still pending; generation guard on late remote merge; DashboardStore local-before-hosts ordering; BackendController cached-path fast start (stubbed) + fallback-on-spawn-failure

- [ ] **Step 1:** Rust check: confirm which handlers accept `?host=` (grep the axum routes); report any gaps rather than faking.
- [ ] **Step 2:** TDD the store conversions (semaphore-gated remote stubs; NEVER timed sleeps) → implement → GREEN.
- [ ] **Step 3:** Backend fast-start + Overview shared store; full suite + build. **Commit** — `perf(macos): local-first paints on all screens, fast cold start, single shared activity store`

### Task 3: Per-event O(n) work out of view bodies

**Files:**
- Modify: `RupuActivity/ActivityTable.swift` + `RupuStore/ActivityStore.swift` (store-side `sortedRows`: store already sorts on recompute — expose it; view's `sortActivityRows` computed property deleted; sort key changes go through the store; hoist the per-row `Date()` to one per pass)
- Modify: `RupuRunDetail/TranscriptFeed.swift` (single memoized `rows` + `sawRunComplete` in `@State` updated `.onChange(of: events.count)` — computed exactly once per event; `ForEach` keyed on a stable `FeedRow` identity (make `FeedRow: Identifiable` — turn id / gate id / synthetic run-complete id); `MarkdownView` parse moves out of init into a cached-by-source memo (static NSCache keyed on source hash) or is computed alongside `rows`)
- Modify: `RupuRunDetail/RunDetailScreen.swift` + `RupuStore/RunDetailStore.swift` (`layoutGraph` hoisted into the store as a derived `graphVM: [GraphNodeVM]` recomputed on a coalesced trigger — batch events within a ~100ms window via a pending-flag Task, not per event; fanout counts single-pass)
- Modify: `RupuRunDetail/StepGraphView.swift` (`HStack` → `LazyHStack`; verify node identity stays step-id-stable so Plan 6's animation work has a base)
- Modify: `RupuRunDetail/Rendering/ToolCards.swift` (`StructuredView` prettyPrinted memoized per value — compute once in init, not per body)
- Modify: `RupuRunDetail/Rendering/CodeBlock.swift` (`CacheKey` hashes a digest of `code` (e.g. `Hasher`-based or SHA prefix) + keeps full string only for equality confirmation; LRU via generation-stamped dictionary (O(1) touch: store `lastUsed` tick, evict min on overflow) replacing the array scan; `setTheme` called only when theme actually changes, not per compute)
- Test: `RupuStoreTests`/`RupuRunDetailTests` — sortedRows updates on event patch without view-side sort; rows/sawRunComplete recomputed exactly once per event (counter seam); graphVM coalescing (N rapid events → 1 recompute after the window; semaphore/clock-injected, no sleeps); CodeBlock LRU O(1) behavior + digest-collision equality guard; RenderMeter counts drop vs Task-1 baseline for a scripted event burst (store-level, deterministic)

- [ ] **Step 1:** TDD store-side derivations → implement view rewires → GREEN.
- [ ] **Step 2:** Full suite + build. **Commit** — `perf(macos): store-side derived state — sorted rows, memoized feed, coalesced graph VM, O(1) highlight cache`

### Task 4: Activity restructure + per-kind tables (amended 2026-08-26 per matt's build feedback)

**Restructure (new, overrides earlier text where they conflict):** the Activity PARENT page shows NO combined table and NO kind-picker buttons — navigation between kinds is the SIDEBAR's job (matt: "no need to have the above buttons"). The parent becomes a lightweight stats surface: KPI cards (counts by kind; running/awaiting/failed today; oldest awaiting age) derived from the store's loaded rows + a compact "needs attention" list reusing the existing awaiting machinery — honest client-side numbers over loaded data, labeled as such. `FilterBar`'s kind segmented picker is DELETED; status chips + live-tail toggle move to the kind pages (they apply per-table). Sidebar kind children render ONLY their kind's dedicated table.

**Files:**
- Create: `RupuActivity/KindTables/{AgentRunsTable,WorkflowRunsTable,AutoflowRunsTable,SessionsTable}.swift` + a shared `KindTableColumn` helper if it stays clean (the existing generic `ActivityTable` remains for the "All runs" parent view ONLY)
- Modify: `RupuActivity/ActivityScreen.swift` (route kind → dedicated table; `.all` → existing combined table)
- Test: `RupuActivityTests` — column order/content per kind (pure row→cell-model mapping tests), status normalization (session `ok|error|aborted`→`completed|failed|cancelled`), autoflow non-run events render BLANK (not "—") in run-shaped columns, default sorts

**Column sets (ported verbatim from the web inventory — web files cited for the implementer to verify):**
- AgentRuns (`pages/runs/AgentRuns.tsx` AGENT_RUN_COLUMNS): Status · Agent(subject; two-line: name + "via {trigger} · session {shortId}") · Run · Source(badge: session=info tone else neutral) · Host · In · Out · Cached · Cost · Turns · Duration · Started · actions. Right-aligned tabular numerals for In/Out/Cached/Cost/Turns/Duration/Started. Default sort started desc.
- WorkflowRuns: Status · Workflow(subject) · Run · Trigger(chip: manual/cron/event tones) · Host · In · Out · Cached · Cost · Turns · Duration · Started · actions. Default started desc.
- AutoflowRuns (events): chevron(expand) · Status · Workflow(subject, falls back to kind label) · Run · Event(KindBadge; cycle_failed borrows failed pill tone) · Issue Ref · Worker · Host · In…Started · actions; expandable ONLY for events carrying failure `detail` (mono err text); non-run events render BLANK in run-shaped columns.
- Sessions: Status(session vocabulary pill) · Agent(subject) · Session · Host · Model(mono) · In · Out · Cached · Cost · Turns(total_turns) · Duration(created→updated) · Started · actions. NO initial client sort (server updated_at order).
- Row actions per kind mirror the app's existing verbs (approve/reject stay on awaiting rows; archive et al. as the current table has them) — do NOT invent verbs the app lacks; note web-only actions (e.g. session delete) as deferred if the client verb doesn't exist.
- All tables reuse the existing sortable-header + hover/selection machinery; exactly ONE truncating subject column each.

- [ ] **Step 1:** TDD the pure row→cells mappers per kind → implement tables + routing → GREEN.
- [ ] **Step 2:** Full suite + build. **Commit** — `feat(macos-activity): per-kind tables — agents/workflows/autoflows/sessions columns per web parity`

### Task 5: Infinite scroll + real filters (amended 2026-08-26 per matt's build feedback)

**Filters (new scope — replaces the 7d/30d/all idea entirely on lists):** each kind page gets a filter bar: text search (client-side over loaded rows, web-parity, with the "{n} matches of {m} loaded" honesty footer), the existing status chips, and a CUSTOM DATE RANGE (from/to date pickers). Deep time filtering must be server-side to be honest: add optional `since`/`until` (RFC-3339) query params to the rupu-cp list endpoints (`/api/runs/agents`, `/api/runs/workflows`, `/api/runs/autoflows/events`, `/api/sessions`) filtering on the rows' started/created timestamps BEFORE offset/limit paging (Rust: extend the handlers' query structs + store queries; follow `pagination.rs` conventions — bad params degrade to defaults, never 500; unit-test the Rust filtering; `make macos-fixtures` + `cargo test -p rupu-cp` drift gate). Client: `CPClient` list methods gain `since:`/`until:`; the store threads them; setting a date range resets paging (generation-guarded). The top-bar 7d/30d/all is REMOVED from Activity routes (stays for Overview/Usage).

**Files:**
- Modify: `RupuStore/ActivityStore.swift` (verify/adapt `loadMore()` for per-kind paging: page size 20 for kind views — the server clamps ≤200; `.all` may keep its current merged snapshot size; expose `hasMore`/`isLoadingMore`/`loadedCount`)
- Modify: `RupuActivity/{ActivityTable,KindTables/*,ActivityScreen,FilterBar}.swift` — trailing sentinel row (`.onAppear` → `store.loadMore()`, re-entrancy-guarded in the store), footer states ported ("loading more…", "scroll for more", "— end of {n} —", "{n} matches of {m} loaded" when client-side narrowing/find is active); filter changes reset paging (generation-guarded); SSE new-run pill/splice must NOT reset scroll position (existing behavior — add a regression test at the store level: appended pages survive a head splice)
- Modify: `RupuShell` top bar — range control hidden/disabled on Activity routes (it never applied to lists; web has no list ranges; showing it implies a filter that doesn't exist — spec honesty rule)
- Test: `RupuStoreTests` — loadMore appends without reordering existing rows; hasMore false on short page; filter change resets to page 0 (late old-generation page dropped); head splice preserves appended pages; per-kind endpoints called with growing offset

- [ ] **Step 1:** TDD store paging → wire sentinel + footers → GREEN.
- [ ] **Step 2:** Full suite + build. **Commit** — `feat(macos-activity): infinite scroll — loadMore finally wired, honesty footers, range scoped to Overview/Usage`

### Task 7: Sidebar IA & alignment (added 2026-08-26 per matt's build feedback; runs before the Task 6 close-out)

**Files:** `RupuShell/Sidebar.swift` (+ `RupuStore` route/order if reorder needs it); test: sidebar-mapping tests adapt.

- **Alignment:** every row (with or without children) reserves the same leading chevron gutter (24pt) so icons and labels align in one column; rows without children (Overview, Projects, Usage, Settings) render an empty gutter, not a shifted label — matt: "Projects… by not having a > looks out of order".
- **IA reorder (matt vetoes at checkpoint):** order by operation — OPERATE: Overview · Activity (▸ kinds); SUBJECTS: Projects · Library (▸ Agents/Workflows/Autoflows); OBSERVE: Security (▸ Findings/Coverage) · Usage; INFRA: Fleet (▸ Hosts/Workers); Settings pinned bottom. Thin uppercase `Eyebrow` section captions (web v1 group-header idiom, v2 chrome). Routes unchanged — presentation-only reorder + captions.
- [ ] **Step 1:** implement; `make macos-test` + `make macos-build` green. **Commit** — `feat(macos-shell): sidebar IA — aligned chevron gutter, operation-ordered sections`

### Task 6: Gates + measured checkpoint (runs LAST, after Task 7)

**Files:** `CLAUDE.md` (module-map perf notes: local-first everywhere, per-kind tables, infinite scroll), full gates.

- [ ] **Step 1:** `make macos-test && make macos-build && cargo test -p rupu-cp`. Grep: no view-body `sortActivityRows`/`buildFeedRows` computed properties; no `JSONDecoder()` in view computed vars in the touched files; `NumberFormatter()` only in static initializers.
- [ ] **Step 2:** RenderMeter evidence: a deterministic store-level scripted burst (N events through ActivityStore + RunDetailStore with meters read before/after) recorded in the report as the before/after table. matt's live GUI pass is the final perceived-speed gate (cold launch, sidebar toggle, deep scroll, live run).
- [ ] **Step 3: Commit** — `perf(macos): Plan 5 close-out — gates, greps, measured render counts`

---

## Self-review notes

- Spec §1 coverage: local-first→T2; cold start→T2; allocation storms→T1; per-event rebuilds→T3; sidebar→T0; per-kind tables→T4; pagination+honesty→T5; RenderMeter→T1/T6.
- Order matters: T1 before T3 (RenderMeter baselines), T3 before T6 (measured deltas), T4 before T5 (kind tables page their own endpoints).
- Known risk: `?host=` support may be missing on some Rust handlers (T2 Step 1 reports honestly; local-first still lands for endpoints that support it).
- Deliberate native divergences: "All runs" combined view kept (native extra); SSE live tail replaces the web's 5s head poll; page size 20 on kind views.
