# rupu.app Phase 4 (Overview Dashboard) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Overview placeholder with the real v2 dashboard — needs-you queue, instrument strip, trend charts, cycle line, fleet strip — backed by `GET /api/dashboard` with per-host progressive fetch.

**Architecture:** New `DashboardStore` (per-host independent fetch + ported merge semantics + SSE-invalidation/reconcile refresh) in RupuStore; a new `RupuOverview` feature module for views; needs-you derives client-side from an Overview-owned `ActivityStore` (spec §3 disposition). One Rust touch: the `dashboard.json` golden fixture.

**Tech Stack:** SwiftUI + SwiftUI Charts (first-party), Swift Testing, rupu-cp fixture rig.

**Spec:** `docs/superpowers/specs/2026-08-24-rupu-macos-phase-4-dashboard-design.md` (authority; web references: `crates/rupu-cp/web/src/pages/Dashboard.tsx`, `lib/dashboard/useDashboardData.ts` + `mergeSummaries.ts`, `components/dashboard/{KeyPointTiles,TerminalTrend,ThroughputChart,HostFreshnessStrip,FleetStrip,CycleSummaryLine}.tsx`; server: `crates/rupu-cp/src/api/dashboard.rs`, `src/host/dashboard_summary.rs`)

## Global Constraints

- Branch `feat/macos-phase4-dashboard` off origin/main (after PR #599 lands). All prior suites stay green; zero warnings; Plan-1 exit greps stay zero (`systemName`, `MicroLabel`, `RunTone`, `.shadow(`, `cornerRadius(8)`, ad-hoc `.font(.system(size:` outside RupuDesign).
- V2 chrome throughout: PanelStyle radius 7, no shadows, sans UI / mono data, Eyebrow for true captions, StatusTone palette; charts unanimated.
- Null discipline: poisoned/partial fields render `—` (+ `+` marker when the matching `*_partial` flag is set); never fabricate 0. `issues_capped` → `N+`.
- Fetch discipline (Phase-2 lesson): per-host independent requests, local first, never block on a hung host; generation-guarded merges (`remoteGeneration` idiom).
- Timing tests condition-poll (`pollUntil`), never fixed sleeps. Swift Testing only.
- Any test that touches a member of a SwiftUI `View`/`ViewModifier` type (static helpers included) MUST be `@Test @MainActor` — the CI runner's toolchain infers View isolation strictly and rejects nonisolated access even where the local toolchain compiles it (bit PR #599's CI).
- Gates per task: `make macos-test` + `make macos-build`; Rust-touching task adds `cargo test -p rupu-cp`. Never bare `git stash pop`. Never package-wide `cargo fmt` (per-file rustfmt only).
- New module `RupuOverview` registers in `Package.swift` + `project.yml` following the existing feature-module pattern (depends on RupuAPI/RupuStore/RupuDesign only); `make macos-gen` after project.yml changes.

---

### Task 1: Dashboard fixture + API models + client method

**Files:**
- Modify: `crates/rupu-cp/tests/macos_fixtures.rs` (+`dashboard_fixture_is_current` → `apps/rupu-macos/Fixtures/dashboard.json`, from the real `DashboardResponse`/`DashboardSummary` types with representative values: 2 hosts (one ok, one offline), one `None`-poisoned field with its partial flag set, non-empty buckets, `active_longest` present, `issues_capped: true`)
- Create (Swift, in `RupuKit/Sources/RupuAPI/Models.swift` or a new `DashboardModels.swift`): the decode types
- Modify: `RupuAPI/CPClient.swift`
- Test: `Tests/RupuAPITests/FixtureDecodingTests.swift`

**Interfaces — Produces:**
- `public struct APIDashboardResponse: Decodable, Equatable, Sendable { hosts: [APIHostFreshness]; findingsPartial/cyclesPartial/fleetPartial: Bool; active: APIActiveCounts; activeLongest: APIActiveLongest?; terminalBuckets: [APITerminalBucket]; throughputBuckets: [APIThroughputBucket]; cycles: APICycleCounts; findingsOpen: Int?; fleet: APIFleetCounts; capturedAt: String? }` with nested types mirroring `dashboard_summary.rs` field-for-field (snake_case CodingKeys; Optionals where the Rust side is `Option`): `APIHostFreshness { hostID, name, transportKind, state, capturedAt?, reason? }`, `APIActiveCounts { running, awaitingApproval, paused, pending: Int }`, `APIActiveLongest { runID, workflowName, ageMs }`, `APITerminalBucket { ts, completed, failed, rejected, cancelled }`, `APIThroughputBucket { ts, manual, cron, event }`, `APICycleCounts { total: Int, clean: Int?, withFailures: Int? }`, `APIFleetCounts { repos/providersConfigured/providersUnhealthy/autoflowsEnabled/autoflowsDisabled/workers/claimsActive/issuesPending/issuesOpen: Int?, issuesCapped: Bool, inventoryCapturedAt: String? }`.
- `CPClient.dashboard(range: String, host: String?) async throws -> APIDashboardResponse` — `GET api/dashboard?range=<r>[&host=<id>]`, following the existing generic-get pattern.

- [ ] **Step 1 (Rust):** fixture emitter per the existing `check_fixture` pattern; `REGEN_FIXTURES=1 cargo test -p rupu-cp --test macos_fixtures`; `cargo test -p rupu-cp` GREEN. **Commit** — `test(cp): emit dashboard.json golden fixture for the macOS Overview`
- [ ] **Step 2 (Swift):** failing decode test (`decodesDashboardFixture`: hosts count, one poisoned field nil + flag true, bucket values, capped flag) → models + client method → GREEN. `make macos-test && make macos-build`. **Commit** — `feat(macos-overview): dashboard API models + client method`

### Task 2: DashboardStore

**Files:**
- Create: `RupuKit/Sources/RupuStore/DashboardStore.swift`
- Test: `Tests/RupuStoreTests/DashboardStoreTests.swift`

**Interfaces:**
- Consumes: `CPClient.dashboard(range:host:)`, `CPClient.hosts()`, `BackendController.makeFirehoseStream` seam (same signal source ActivityStore consumes — reuse its factory-injection pattern for testability), `TimeRange` (`model.range` passed in).
- Produces: `@MainActor @Observable public final class DashboardStore`:
  - `public private(set) var hostStates: [HostSlice]` where `public struct HostSlice: Equatable { let id, name, transportKind: String; var state: SliceState }`, `enum SliceState: Equatable { case loading, ok(capturedAt: String?), offline, unavailable(reason: String?) }` — seeded from `hosts()` immediately.
  - `public private(set) var merged: MergedDashboard?` where `public struct MergedDashboard: Equatable { active, activeLongest?, terminalBuckets, throughputBuckets, cycles, findingsOpen: Int?, fleet, findingsPartial/cyclesPartial/fleetPartial: Bool, capturedAt: String? }` (same shapes as the API types).
  - `public static func merge(_ summaries: [APIDashboardResponse]) -> MergedDashboard` — PURE, ports `merge_dashboard_summaries` semantics: sum only non-nil; any reporting host's nil poisons the field AND sets the matching partial flag; `activeLongest` = max ageMs; `capturedAt` = oldest; `issuesCapped` ORs; partial flags also OR in the per-host flags from single-host responses.
  - `public func activate(range: String) async` — seed slices, fetch local first then remaining hosts concurrently-independently, merge progressively (re-merge on each arrival, generation-guarded); `public func setRange(_ r: String) async` refetches all; `public func deactivate()`; firehose signal → 250ms-coalesced local refetch; 60s reconcile task refetches all (weak-self-per-iteration loop, HealthMonitor idiom).
  - Failure honesty: a host fetch error → that slice `unavailable(reason)`; merged built from ok slices only; zero ok slices + all done → `merged` stays nil and `public var pageError: String?` set.

- [ ] **Step 1:** failing merge tests — table ported from the Rust unit tests' cases: sum-of-Some, None-poisons+flag, oldest capturedAt, max activeLongest, OR issuesCapped, empty-input. RED → implement `merge` → GREEN.
- [ ] **Step 2:** failing lifecycle tests (mock client, pollUntil): local paints before slow remote resolves; slow remote merges in when it lands; failed remote → unavailable slice, merged unchanged; setRange regeneration drops stale in-flight results; deactivate cancels reconcile. RED → implement → GREEN. `make macos-test && make macos-build`. **Commit** — `feat(macos-overview): DashboardStore — per-host progressive fetch + ported merge semantics`

### Task 3: Trigger tokens + chart components

**Files:**
- Modify: `RupuKit/Sources/RupuDesign/Tokens.swift` (+trigger palette)
- Create: `RupuKit/Sources/RupuOverview/Charts.swift` (module scaffold: `Package.swift`, `project.yml` entries + `make macos-gen` belong to this task)
- Test: `Tests/RupuOverviewTests/ChartAdaptersTests.swift`

**Interfaces:**
- Produces: `Color.trigger(_ kind: TriggerKind)` with `public enum TriggerKind: String { case manual, cron, event }` — RGB values read from the web's TriggerChip/styles.css at implementation time (both themes; if the web defines only one, use it for both and note it).
- `OutcomesChart(buckets: [APITerminalBucket])` and `ThroughputChart(buckets: [APIThroughputBucket])` — SwiftUI Charts stacked `AreaMark`s, `fillOpacity` ~0.25, no animation, 164pt plot height, integer y-axis, day-formatted x labels; series colors: outcomes → `Color.status(.done/.failed/.rejected/.cancelled)`, throughput → trigger palette. `FailedSparkline(buckets:)` — 32pt single-series area.
- Pure adapters (the testable seam): `chartRows(from: [APITerminalBucket]) -> [(ts: Date, series: String, value: Int)]` (and throughput equivalent) — ISO ts parsing, series order stable (stack order matches web: completed bottom → cancelled top; manual → event).

- [ ] **Step 1:** module scaffold + failing adapter tests (parse, ordering, zero rows pass through — no client fill) → implement → GREEN.
- [ ] **Step 2:** chart views (no render tests — adapters are the tested logic); `make macos-test && make macos-build`. **Commit** — `feat(macos-overview): trigger palette + stacked-area chart components`

### Task 4: Instrument strip, freshness strip, cycles line, fleet strip

**Files:**
- Create: `RupuKit/Sources/RupuOverview/InstrumentStrip.swift`, `FreshnessStrip.swift`, `FleetStrip.swift` (cycles line folds into FleetStrip file or its own small view)
- Test: `Tests/RupuOverviewTests/InstrumentValuesTests.swift`

**Interfaces:**
- Produces: `InstrumentStrip(merged: MergedDashboard?)` — six equal cells (Awaiting you / Active now / Paused / Failed+`FailedSparkline` / Success rate / Open findings), KPI numerals `dataMono`, captions `Eyebrow`; `FreshnessStrip(slices: [DashboardStore.HostSlice])` — per-host pill (state tone: ok→done, offline→failed, unavailable→awaiting, loading→pending) + relative capture age; `CycleSummaryLine(cycles:partial:)`; `FleetStrip(fleet:partial:)` — dim band, `providersUnhealthy > 0` loud (err), `issuesCapped` → `N+`.
- The pure seam: `public struct InstrumentValues: Equatable { awaiting, activeNow, paused: String; failedTotal: String; successRate: String; openFindings: String }` + `static func compute(from: MergedDashboard?, findingsPartial…) -> InstrumentValues` — success rate = completed/(completed+failed+rejected+cancelled) over the buckets, "—" when denominator 0 or merged nil; every partial-flagged figure suffixed `+`; nil → "—". Port the arithmetic from web `KeyPointTiles.tsx` (read it at implementation time; its logic is the reference).

- [ ] **Step 1:** failing `InstrumentValues.compute` table (happy, zero-denominator, partial suffix, nil merged) → implement → GREEN.
- [ ] **Step 2:** views; gates. **Commit** — `feat(macos-overview): instrument/freshness/fleet strips + cycle line`

### Task 5: Needs-you queue

**Files:**
- Create: `RupuKit/Sources/RupuOverview/NeedsYou.swift` (derivation + views)
- Test: `Tests/RupuOverviewTests/NeedsYouDerivationTests.swift`

**Interfaces:**
- Consumes: `ActivityStore` (rows, `approve(runID:gate:host:)`/`reject`, `PendingActions`), `ActivityRow` (status/startedAt/subject/navigation), `TimeRange`.
- Produces: `public struct NeedsYouItem: Equatable, Identifiable { enum Kind { case gate, failedRun }; id, kind, row: ActivityRow }` and PURE `public func deriveNeedsYou(rows: [ActivityRow], range: TimeRange, now: Date) -> (items: [NeedsYouItem], overflow: Int)` — awaiting rows oldest-first, then failed rows whose startedAt falls inside `range` (relative to `now`; `.all` = no window) newest-first; cap 6; overflow = total-6 floor 0. `NeedsYouCard(...)` — v2 row anatomy: 2px leading tone edge, kind tag (Eyebrow), subject + breadcrumb line, right-aligned age (`dataMono`, awaiting-tinted when oldest gate), actions: gates → compact Approve (primaryOk)/Reject (dangerOutline) wired through the store's gate-scoped mutations with pending/stale states visible (reuse the ActivityTable inline-action pattern incl. busy guard + sole-gate resolution); failed → Open (outline, navigates). Footer "+N more" → `.activity(.all)` (Activity's awaiting chip is one click; don't fake a status-preset route that doesn't exist — unless Route already supports a status filter, verify at implementation time). Empty state: one 36pt "nothing needs you" row. Fleet-wide: derivation reads UNSCOPED rows (spec: ignores scope selector — since scope narrowing lives in ActivityStore.scopeFilter, Overview's own store instance simply never sets it; note this in a comment).

- [ ] **Step 1:** failing derivation table (ordering across kinds, range window incl. `.all`, cap+overflow, empty) → implement → GREEN.
- [ ] **Step 2:** views + wiring to Overview's ActivityStore instance; gates. **Commit** — `feat(macos-overview): needs-you queue — client-side aggregate with inline gate actions`

### Task 6: OverviewScreen composition + customize + wiring

**Files:**
- Create: `RupuKit/Sources/RupuOverview/OverviewScreen.swift`
- Modify: `RupuShell/RootView.swift` (route `.overview` → OverviewScreen; remove placeholder use for it), `RupuShell/ShellToolbar.swift` (Customize menu, Overview-only), `App/` wiring if the module needs registering
- Test: `Tests/RupuOverviewTests/WidgetConfigTests.swift`

**Interfaces:**
- Produces: `OverviewScreen(model:backend:)` — owns a `DashboardStore` + an `ActivityStore` (kind .all, liveTail on, scopeFilter never set), activates both on appear / deactivates on disappear (rebuild on client change per the RunDetailScreen ownership pattern); vertical ScrollView stack per spec §1 order; every block renders per-block from available data (`BlockState` discipline; page error only when nothing resolved). `OverviewWidgets` persistence: `public struct OverviewWidgets: Codable, Equatable { needsYou, instruments, charts, cycles, fleet: Bool }` + load/save on UserDefaults key `"overview.widgets"` (absent → all true); Customize = toolbar `Menu` with five checkmark toggles, visible only on `.overview`.
- Range reactivity: `model.range` change → `dashboardStore.setRange` + needs-you re-derivation (derivation takes range as input — recompute is free).

- [ ] **Step 1:** failing `OverviewWidgets` round-trip test (encode/decode, absent-key default) → implement → GREEN.
- [ ] **Step 2:** screen composition + route/toolbar wiring; full gates (`make macos-test && make macos-build`). **Commit** — `feat(macos-overview): Overview screen — composition, customize menu, route wiring`

### Task 7: Docs + checkpoint

**Files:**
- Modify: `CLAUDE.md` (module map + read-first line), umbrella spec §8 (Phase 4 row → delivered wording + disposition pointer to the Phase 4 spec)
- Test: full gates incl. `cargo test -p rupu-cp`

- [ ] **Step 1:** docs edits; full gates. **Commit** — `docs(macos-overview): Phase 4 pointers + dispositions`
- [ ] **Step 2 (controller):** checkpoint evidence — Overview screenshots dark+light (with data), annotate deferred items (attention endpoint, finding rows, drag-reorder).

---

## Self-review notes

- Spec coverage: §1 composition→T3/T4/T5/T6, §2 store→T1/T2, §3 needs-you disposition→T5, §4 customize→T6, §5 verification→per-task gates + T7. Fleet-wide needs-you honored by never setting scopeFilter (T5/T6).
- Type consistency: `MergedDashboard`/`APIDashboardResponse` shapes defined once (T1/T2) and consumed by name in T3–T6; `TriggerKind` (T3) used only by charts; `NeedsYouItem` self-contained.
- Known risk, accepted: SwiftUI Charts first use in the codebase — adapters carry the tested logic, chart bodies stay thin; matt's checkpoint judges the rendering.
- Two stores on one screen (DashboardStore + ActivityStore) is deliberate — reuse over a new federated fetcher; double-fetch cost only while Overview is open.
