# rupu.app Flows & Composition (Plan 2 of design alignment) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recompose the app's navigation and page shapes to the CP v2 IA — flat rail with host footer, v2 top bar with a native ⌘K palette, web-shaped run detail (single stack + selection-following tabbed panel), SortableTable density on Activity, and dialog-chrome polish on launcher/session screens.

**Architecture:** All view recomposition; stores gain only named additive APIs (`selectedStepID`/`events` on RunDetailStore, `scopeFilter` on ActivityStore, new `HostsFooterStore` + `PaletteStore`). Route/SidebarItem collapse the Runs group into one Activity item — kind state stays route-bound exactly as today. One Rust touch: a `projects` golden fixture for the new scope picker.

**Tech Stack:** SwiftUI (macOS 14+), Swift Testing, RupuDesign v2 kit (Plan 1), rupu-cp fixture rig.

**Spec:** `docs/superpowers/specs/2026-08-24-rupu-macos-design-alignment-design.md` §3 (visual authority: `docs/macOS_design/V2-CONTRACT.md`; web anchors: `crates/rupu-cp/web/src/components/v2/Shell.tsx`, `components/lists/SortableTable.tsx`, `components/CommandPalette.tsx`, `pages/RunDetail.tsx`).

## Global Constraints

- Branch `feat/macos-flows-composition`, stacked on `feat/macos-design-language` (PR #598). Merge order: #598 first.
- **No store behavior regressions.** Existing store APIs unchanged except the additions named per task. Existing tests adapt (rename/re-route), never weaken; store/contract semantics tests must not need changes beyond the Route/SidebarItem collapse.
- V2-CONTRACT geometry verbatim: rail 204pt, nav rows 30pt, active row = `surface` fill + inset 2px left `brand-500` (radius 5); top bar 48pt; table rows 8pt vertical padding; panel radius 7; dialogs = centered card over 40% black scrim; **no shadows**.
- New icons ONLY via the committed extractor (`apps/rupu-macos/scripts/extract-lucide.mjs`): add `chevron-up`, `chevron-down`, `more-horizontal` to its table, re-run, commit SVGs + regenerated `LucideIconData.swift`. No hand-authored path data.
- Plan-1 exit greps stay zero (`systemName`, `MicroLabel`, `RunTone`, `.shadow(`, `cornerRadius(8)`, ad-hoc `.font(.system(size:` outside RupuDesign).
- Swift Testing only; timing-sensitive tests condition-poll (`pollUntil`), never fixed sleeps.
- Gates per task: `make macos-test` + `make macos-build`; Task 2 additionally `cargo test -p rupu-cp` (fixture drift). Zero compiler warnings.
- Honesty rule: no fake server-side filtering — the scope picker narrows client-side (like status chips) and says nothing else; remote runs show the existing "streaming lands with Fleet" note in Events, never an empty state pretending to have looked.

---

### Task 1: Route collapse + v2 rail with host footer

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuStore/Route.swift` (SidebarItem), `RupuStore/AppModel.swift:84-110` (selectedSidebarItem), `RupuShell/Sidebar.swift` (full rewrite)
- Create: `RupuKit/Sources/RupuStore/HostsFooterStore.swift`
- Modify tests: `Tests/RupuStoreTests/RoutingTests.swift`, `Tests/RupuStoreTests/RouteTests.swift`
- Create test: `Tests/RupuStoreTests/HostsFooterStoreTests.swift`

**Interfaces:**
- Consumes: `CPClient.hosts() -> [APIHostRow]` (exists), `AppModel.route`/`lastActivityRoute`, RupuDesign tokens/Icon.
- Produces:
  - `SidebarItem` becomes `case overview, activity, projects, security, library, fleet, usage` (`.runs`/`.runsLeaf` deleted).
  - `AppModel.selectedSidebarItem` getter: `.activity(_)`, `.runDetail`, `.sessionDetail`, `.agentRunDetail` → `.activity`; others 1:1. Setter for `.activity`: `route = lastActivityRoute` (preserves the user's kind tab; `lastActivityRoute` defaults `.activity(.all)`).
  - `@MainActor @Observable public final class HostsFooterStore { public private(set) var summary: (total: Int, down: Int)?; public func apply(_ rows: [APIHostRow]); public func activate(client: CPClient); public func deactivate() }` — `apply` is the pure seam: `down = rows.filter { $0.status != "online" }.count`; `activate` polls every 60s (web parity, `Shell.tsx` HostFooter), each failure leaves the last summary in place.

- [ ] **Step 1: failing tests** — adapt `RoutingTests.sidebarLeavesAndKindFilterAreSameState` → new name `allActivityKindsMapToTheSingleActivitySidebarItem` (every `RunKindFilter` case: `.activity(k)` → `.activity`); adapt highlight tests (`.runs` → `.activity`); new `selectingActivityRestoresLastActivityKind` (navigate to `.activity(.workflows)`, then `.projects`, set `selectedSidebarItem = .activity` → route is `.activity(.workflows)`). New `HostsFooterStoreTests`: `applyComputesDownCountFromNonOnlineStatuses` (3 rows: online/online/offline → (3,1)), `applyWithEmptyRowsYieldsZeroZero`. Run: RED.
- [ ] **Step 2: implement** Route/AppModel/HostsFooterStore. Run store tests: GREEN.
- [ ] **Step 3: rail rewrite.** Replace the `List(.sidebar)` with a custom flat rail (this deletes the system-blue selection):

```swift
// Sidebar.swift — v2 rail per Shell.tsx: w-204, rows 30, active = surface + inset 2px brand-500
var body: some View {
    VStack(alignment: .leading, spacing: 0) {
        nav
        Spacer(minLength: 0)
        SettingsLink { railLabel("Settings", icon: .settings, active: false) }
            .buttonStyle(.plain).padding(.horizontal, 8)
        footer  // backend health line + hosts line
    }
    .frame(width: 204)
    .background(Color.rupuPanel)
    .overlay(alignment: .trailing) { Color.rupuBorder.frame(width: 1) }
}
private var nav: some View {
    VStack(spacing: 2) {
        ForEach(railItems, id: \.self) { item in railRow(item) }
    }.padding(.horizontal, 8).padding(.vertical, 8)
}
private func railRow(_ item: SidebarItem) -> some View {
    let active = model.selectedSidebarItem == item
    return Button { model.selectedSidebarItem = item } label: {
        HStack(spacing: 8) {
            Icon(icon(for: item), size: 15)
            Text(title(for: item)).font(.leadText)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .frame(height: 30)
        .foregroundStyle(active ? Color.rupuInk : Color.rupuDim)
        .background(active ? Color.rupuSurface : hovered == item ? Color.rupuSurfaceHover : .clear)
        .overlay(alignment: .leading) { if active { Color.rupuBrand500.frame(width: 2) } }
        .clipShape(RoundedRectangle(cornerRadius: 5))
    }
    .buttonStyle(.plain)
    .onHover { hovered = $0 ? item : (hovered == item ? nil : hovered) }
}
```
  `railItems` order: overview, activity, projects, security, library, fleet, usage (flat — no sections). Icons per V2-CONTRACT nav table (Activity → `.activity`). Keep a small brand header row (app name, height 48, bottom border) matching `Shell.tsx:95`'s rail header. `footer`: existing health dot/label line + new mono `metaText` line `"\(total) hosts" + (down > 0 ? " · \(down) down" : "")` with dot `.status(down == 0 ? .done : .failed)` (pending-tone dot + `"— hosts"` while `summary == nil`), top border, driven by a `HostsFooterStore` owned by `RootView` (activate on appear with `backend.client`, deactivate on disappear).
- [ ] **Step 4: gates** — `make macos-test && make macos-build`; verify no `NSVisualEffectView`/`List` remnants in Sidebar.swift. **Commit** — `feat(macos-flows): v2 rail — flat IA, single Activity item, pinned Settings, host footer`

### Task 2: Top bar — scope select, persisted range, search affordance

**Files:**
- Modify: `crates/rupu-cp/tests/` fixture emitter (the test that writes `apps/rupu-macos/Fixtures/*.json` — add `projects.json` emission from the same serde type `GET /api/projects` serializes)
- Create: `apps/rupu-macos/Fixtures/projects.json` (via `make macos-fixtures`), `RupuKit/Sources/RupuAPI/` APIProjectRow (in `Models.swift`)
- Modify: `RupuAPI/CPClient.swift` (+`projects()`), `RupuStore/AppModel.swift` (scope + range persistence), `RupuStore/ActivityStore.swift` (scope narrowing), `RupuShell/ShellToolbar.swift`
- Tests: `Tests/RupuAPITests/FixtureDecodingTests.swift` (+projects), `Tests/RupuStoreTests/ActivityStoreTests.swift` (+scope), new toolbar-state asserts in `Tests/RupuStoreTests/RoutingTests.swift`

**Interfaces:**
- Produces:
  - `public struct APIProjectRow: Decodable, Equatable, Sendable { public let wsID: String; public let name: String; public let runCount: Int?; public let lastRunAt: String? }` (CodingKeys ws_id/name/run_count/last_run_at; extra JSON keys ignored).
  - `CPClient.projects() async throws -> [APIProjectRow]` — `GET api/projects`.
  - `AppModel.scopeWsID: String?` (nil = all projects) and `AppModel.range` both persisted to `UserDefaults` (keys `"scope.wsID"`, `"range"`) using the same manual pattern as `"onboarding.complete"`.
  - `ActivityStore.scopeFilter: String?` — client-side narrowing identical in mechanism to `statusFilter`: a row passes when `scopeFilter == nil || row matches the workspace identifier`. The implementer verifies which `ActivityRow` field carries the ws identifier (the table's PROJECT column source) and matches on that exact field; if some kinds' rows have no workspace field, they pass only when `scopeFilter == nil` (honest narrowing — a scoped view shows only rows that provably belong).

- [ ] **Step 1 (Rust): fixture** — extend the fixture emitter with `projects.json` (one row, realistic values, from the actual serde type used by the `/api/projects` handler); `make macos-fixtures`; `cargo test -p rupu-cp` GREEN (drift gate now covers it). Commit separately — `test(cp): emit projects.json golden fixture for the macOS scope picker`.
- [ ] **Step 2: failing Swift tests** — fixture decode test (`projects.json` → non-empty, first row fields); `ActivityStoreTests.scopeFilterNarrowsMergedRowsAndNilRestores`; `rangeAndScopePersistAcrossAppModelInstances` (two AppModels over the same injected UserDefaults suite). RED → implement → GREEN.
- [ ] **Step 3: toolbar recomposition** — item order per `Shell.tsx` TopBar: title (navigation) · scope `Picker` (`.menu`; "All projects" + one entry per `APIProjectRow.name`, loaded once on appear via `client.projects()`, failure → picker shows "All projects" only) · range segmented (existing) · then primaryAction group: search-field-styled `Button` (magnifier `Icon` + `Text("Search…")` `.uiText` `.rupuMute` + `⌘K` badge, 1px `rupuBorderStrong` border, radius 6, opens the palette — Task 3 wires the action; this task lands it disabled-hidden behind `paletteAvailable = false` so the bar composes now) · `+ New run` · live pill · theme picker. Delete the "deliberately absent" doc comment.
- [ ] **Step 4: gates + commit** — `feat(macos-flows): v2 top bar — project scope select, persisted range, search affordance`

### Task 3: Command palette (⌘K)

**Files:**
- Create: `RupuKit/Sources/RupuStore/PaletteStore.swift`, `RupuKit/Sources/RupuShell/CommandPaletteView.swift`
- Modify: `RupuShell/RootView.swift` (overlay + hidden ⌘K button + toolbar search wiring), `RupuShell/ShellToolbar.swift` (enable the search button)
- Create test: `Tests/RupuStoreTests/PaletteStoreTests.swift`

**Interfaces:**
- Consumes: `CPClient.runs/agentDefinitions/workflowDefinitions`, `CPClient.runDetail` (gate resolution), `PendingActions` (shared ledger from `BackendController`), `AppModel.navigate(to:)`.
- Produces:
  - `public struct PaletteItem: Identifiable, Equatable { public enum Kind: String { case page, run, agent, workflow, approve }; public let id: String; public let kind: Kind; public let title: String; public let subtitle: String?; public let action: PaletteAction }`
  - `public enum PaletteAction: Equatable { case navigate(Route); case approveGate(runID: String, host: String?) }`
  - `public func paletteRank(query: String, items: [PaletteItem]) -> [PaletteItem]` — pure, case-insensitive: empty query → pages first then insertion order, cap 30; else score = 3 for title prefix, 2 for word-boundary prefix, 1 for subsequence, 0 drops; stable within score.
  - `@MainActor @Observable public final class PaletteStore { public var isOpen: Bool; public var query: String; public var activeIndex: Int; public private(set) var items: [PaletteItem]; public var results: [PaletteItem] { paletteRank(query: query, items: items) }; public func open() async; public func close(); public func execute(_ item: PaletteItem) async }` — `init(client:pendingActions:onNavigate: @escaping (Route) -> Void)`.

- [ ] **Step 1: failing ranker tests** — table over `paletteRank`: prefix beats word-boundary beats subsequence; non-match drops; empty query returns pages first; cap 30; stability. RED → implement → GREEN.
- [ ] **Step 2: store** — `open()` fetches the three sources concurrently (`async let`, each `catch → []`, local host only for runs), maps: 7 static page items (`navigate(.overview)` etc.); runs → `navigate(.runDetail(...))` / session rows → `.sessionDetail` (reuse the same row→route mapping ActivityRow navigation uses); `.awaiting` runs additionally yield an `approve` item titled `"Approve: \(subject)"`. `execute`: navigate items call `onNavigate` + `close()`; approve items resolve the sole awaiting gate via `client.runDetail` (same contract as `ActivityTable.resolveSoleAwaitingGate`), POST approve keyed into the shared `PendingActions` gate-scoped key, then navigate to the run so the pending state is visible. Tests: page items always present after `open()` with a failing client (sources fail → pages still there — the fail-open contract); approve item appears only for awaiting rows (mock client fixture). GREEN.
- [ ] **Step 3: view + wiring** — `CommandPaletteView(store:)`: ZStack scrim `Color.black.opacity(0.4)` (tap → close), card top-centered (`.frame(maxWidth: 640)`, `.padding(.top, 96)`): TextField (`.uiText`, focused on open) + results list (rows: kind `Eyebrow` + title `.uiText` + subtitle `.rupuMute`, active row `rupuSurface` + brand inset like the rail), `.panelStyle(.panel)`. Keys via `.onKeyPress`: ↑/↓ move `activeIndex`, Return executes, Esc closes. RootView: `.overlay { if palette.isOpen { CommandPaletteView(store: palette) } }` + hidden `Button` with `.keyboardShortcut("k", modifiers: .command)` (same pattern as ⌘N at `RootView.swift:59-63`); toolbar search button calls `palette.openFromUI()` → `Task { await store.open() }`; remove the Task-2 `paletteAvailable` gate.
- [ ] **Step 4: gates + commit** — `feat(macos-flows): native command palette — navigation, run/definition search, gate approve`

### Task 4: Run detail — single stack + selection-following tab panel

**Files:**
- Modify: `RupuStore/RunDetailStore.swift` (additive), `RupuRunDetail/RunDetailScreen.swift` (recompose), `RupuRunDetail/StepGraphView.swift` (tap selection), `RupuRunDetail/TranscriptFeed.swift` (only if the feed needs a height contract)
- Delete: `RupuRunDetail/RailViews.swift` (FactsCard folds into header meta; Netflow/Findings cards move into tab content files)
- Create: `RupuRunDetail/RunDetailTabs.swift` (tab enum + panel + Events feed + moved Findings/Netflow content)
- Tests: extend `Tests/RupuStoreTests/RunDetailStoreTests.swift`; move `RailViewsSeverityTests.swift` → `FindingsSeverityTests.swift` (same 2 tests, new home of `severity(for:)`)

**Interfaces:**
- Produces:
  - `RunDetailStore.selectedStepID: String?`; `public func select(step: String) async` — sets `selectedStepID` then awaits existing `focusStep(step)` (generation token already guards overlap). Initial selection = existing `initialFocusStepID()` result.
  - `RunDetailStore.events: [CPEvent]` — appended in the existing `startRunStream()` handler, capped at 500 (drop oldest); cleared on `activate()`. `public func eventsForSelection() -> [CPEvent]` — when `selectedStepID != nil`, only events whose step id matches (run-level events without a step id drop out — web parity, `RunDetail.tsx:726-731`); else all.
  - `public enum RunDetailTab: String, CaseIterable { case transcript, events, findings, netflow }` (view-local `@State`, default `.transcript`).

- [ ] **Step 1: failing store tests** — `selectStepSetsSelectionAndFocuses` (select → `selectedStepID` set, transcript path resolved via existing machinery); `eventsAccumulateFromRunStreamAndCapAt500`; `eventsForSelectionFiltersByStepAndKeepsAllWhenUnselected`. RED → implement → GREEN.
- [ ] **Step 2: recompose the screen** — single `VStack`: header (existing back/breadcrumb/pill/actions/facts row, **plus** a second mono meta line absorbing FactsCard: `RUN <id>  ·  WS <wsID>  ·  <permission>` — Fmt-formatted, truncating middle) → `awaitingBanner` (unchanged) → step graph (height 140, unchanged layout; each node gets `.onTapGesture { store.select(step: node.id) }` + a selected affordance: `rupuBrand500` 1px ring on the selected node) → **tab panel**: `TabBar` of 4 text buttons (`.uiText`, active = ink + 2px bottom brand underline; findings button shows count badge when > 0) over a content area `.frame(minHeight: 420, idealHeight: geo.size.height * 0.65)` inside a `GeometryReader`. Content per tab: `.transcript` = existing `TranscriptFeed` + live/remote notes; `.events` = rows `ts (dataMono meta) · event type (uiText) · step (Badge)` from `eventsForSelection()`, remote runs show the existing "Remote streaming lands with Fleet (Phase 5)" note; `.findings` / `.netflow` = the moved card contents (list layouts, full width). Delete `RailColumn`/`RailViews.swift`; token/cost figures live in the existing facts row only.
- [ ] **Step 3: gates** — full suite (the 30 RunDetailStore tests must stay green unmodified except new ones), build, exit greps. **Commit** — `feat(macos-flows): run detail — single stack, header meta, selection-following tab panel`

### Task 5: Activity SortableTable pass + new glyphs

**Files:**
- Modify: `apps/rupu-macos/scripts/extract-lucide.mjs` (+`chevron-up`, `chevron-down`, `more-horizontal`), regenerate `Icons/LucideIconData.swift` + 3 SVGs
- Modify: `RupuActivity/ActivityTable.swift`, `RupuRunDetail/RunDetailScreen.swift` + `SessionDetailScreen.swift` (overflow trigger `.archive` → `.moreHorizontal` — closes the parked Plan-1 item)
- Create: `RupuKit/Sources/RupuActivity/ActivitySort.swift`
- Create test: `Tests/RupuStoreTests/ActivitySortTests.swift` (or RupuActivity test target if one exists — match the repo's layout)

**Interfaces:**
- Produces: `public struct ActivitySort: Equatable { public enum Key: String, CaseIterable { case status, kind, subject, project, host, trigger, duration, cost, started }; public var key: Key; public var ascending: Bool }` and `public func sortActivityRows(_ rows: [ActivityRow], by sort: ActivitySort) -> [ActivityRow]` — pure, stable, **nulls always last regardless of direction** (SortableTable.tsx contract); `started` compares the row's date value, `duration`/`cost` numeric, text keys case-insensitive. Default sort `ActivitySort(key: .started, ascending: false)` must reproduce today's order byte-for-byte on the merged fixture rows.

- [ ] **Step 1: extractor** — add the 3 kebab names to the table, verify files exist in `lucide-react@0.468`, run, commit regenerated assets (deterministic diff only adds).
- [ ] **Step 2: failing comparator tests** — default sort reproduces store order; nulls-last both directions (cost `—` rows); stable tie-break; text case-insensitivity. RED → implement → GREEN.
- [ ] **Step 3: table pass** — header `Eyebrow` labels become sort buttons: tap toggles asc/desc (first tap = ascending for text, descending for `started`/`duration`/`cost` — web `defaultDirFor`); active header shows `Icon(.chevronUp/.chevronDown, size: 9)`; sort is view-`@State`, applied over `store.rows` (live-tail updates re-sort naturally). Row vertical padding 6 → 8. Subject cell gets `.help(row.subject)` (truncation tooltip). Numeric/time columns confirm `.trailing` + `monospacedDigit` (Fmt already). Overflow menus: `.archive` → `.moreHorizontal` at both call sites.
- [ ] **Step 4: gates + commit** — `feat(macos-flows): Activity SortableTable — sortable headers, density, honest overflow glyph`

### Task 6: Launcher dialog chrome + session/agent-run recomposition

**Files:**
- Modify: `RupuLauncher/LauncherSheet.swift`, `RupuLauncher/DefinitionPicker.swift` (chrome only), `RupuRunDetail/SessionDetailScreen.swift`, `RupuRunDetail/AgentRunDetailScreen.swift`

**Interfaces:** consumes Tasks 1–5; no store changes at all in this task.

- [ ] **Step 1: launcher chrome** — sheet content adopts web dialog chrome (`LauncherSheet.tsx`/`AgentLauncherSheet.tsx`): card on `Color.rupuPanel`, 1px `rupuBorder`, radius 7 (native sheet supplies the scrim); width stays 560 (ruled: macOS input comfort over web's 448 — record in the sheet's doc comment); section headers become `Eyebrow`; footer buttons already `RupuButtonStyle` from Plan 1 — verify Launch=primary / Cancel=outline still hold.
- [ ] **Step 2: session detail** — recompose to the stacked idiom: header (back, breadcrumb, archived state, overflow menu) + mono meta line (`AGENT · MODEL · PROVIDER · TURNS · TOKENS · COST` — the existing facts row content); then the session-runs list as a full-width stacked panel section (`Eyebrow("Runs")`, rows unchanged, still navigate to `.agentRunDetail`); then transcript (`minHeight: 420`) with the send box: field on `rupuSurface`, 1px `rupuBorder`, radius 7, `.uiText`; Send = `RupuButtonStyle.primary`; the three-state (normal/stopped/archived) logic unchanged. Fix the residual: line ~208 Retry `.plain` → `RupuButtonStyle.outline`.
- [ ] **Step 3: agent-run detail** — same header idiom (back, breadcrumb, "Read-only" eyebrow) + transcript at full width; confirm no rails/leftover width constants.
- [ ] **Step 4: gates + commit** — `feat(macos-flows): launcher dialog chrome; session + agent-run stacked recomposition`

### Task 7: Docs + checkpoint package

**Files:**
- Modify: `CLAUDE.md` (module map: rail/palette notes; Read-first: this plan line), `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md` (phase table note: design-alignment Plans 1–2)
- Test: full gates

- [ ] **Step 1: docs edits; full gates** (`make macos-test && make macos-build && cargo test -p rupu-cp`).
- [ ] **Step 2: checkpoint evidence** — controller-run: app screenshots (dark + light) of the new shell (rail + top bar), Activity sorted view, run detail tabs, palette open, launcher; annotate anything deferred (Phase 4 dashboard, Library). **Commit** — `docs(macos-flows): pointers + Plan 2 checkpoint evidence`

---

## Self-review notes

- Spec §3 coverage: shell/rail+footer→T1, top bar+scope+range→T2, ⌘K palette→T3, run detail recomposition→T4, SortableTable→T5, launcher+session/agent detail→T6, checkpoint→T7. Sidebar-one-state tests adapt in T1 (named).
- Type consistency: `SidebarItem.activity` (T1) is what T2's toolbar and T3's palette navigate against; `RunDetailTab`/`select(step:)` (T4) consumed only within RunDetail; `ActivitySort` (T5) self-contained; `PaletteAction.approveGate` uses the gate-scoped `PendingActions` keys from Phase 3.
- Known risks, accepted: palette approve resolves the *sole* awaiting gate (same limitation as the table's inline actions — multi-gate runs route through run detail); Events tab is local-runs-only (stream-fed; remote note is honest); scope narrowing is client-side and says so.
- Not in scope (spec §5): Library screen, saved views, menu bar extra, Situation Room, web-side changes, new API surface (the projects fixture emits an existing endpoint's type).
