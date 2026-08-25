# rupu.app Phase 5B (Security · Usage) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Security and Usage placeholders — the two screens whose API surface is entirely net-new on the Swift side (coverage, global findings, usage/usage_runs/usage_outliers).

**Architecture:** One Rust fixtures task, one Swift API-surface task, then screen tasks composing Plan A's ListSort/SortableHeaderRow groundwork, Phase 4's chart idioms, and the established store patterns. New modules `RupuSecurity`, `RupuUsage`. The Usage chart is fed by a pure, tested port of the web's client-side aggregation over `/api/usage/runs` (web parity — the server's `/api/usage/timeline` stays dormant, as on web).

**Tech Stack:** SwiftUI + SwiftUI Charts, Swift Testing, rupu-cp fixture rig.

**Spec:** `docs/superpowers/specs/2026-08-24-rupu-macos-phase-5-breadth-design.md` §4 (+§5 dispositions). Web references: `crates/rupu-cp/web/src/pages/{Findings,Coverage,CoverageDetail,Usage}.tsx`, `components/findings/*`, `components/coverage/*`, `components/usage/*` (esp. the `buildTimeline`/`aggregateRuns` client aggregation and the ModelBreakdownTable header comment on why it does NOT use `data.breakdown`); server: `crates/rupu-cp/src/api/{findings,coverage,usage,usage_outliers}.rs`.

## Global Constraints

- Branch continues `feat/macos-phase5a-breadth` OR a fresh `feat/macos-phase5b-security-usage` off main after Plan A merges — decided at execution time by the controller; record in the ledger.
- All Plan A global constraints carry over verbatim (gates, exit greps, @MainActor rule, condition-polling, stub-isolation tokens, per-file rustfmt, fixtures for every consumed endpoint, v2 chrome, null discipline, no silent-noop, store ownership pattern).
- Depends on Plan A Task 1 (`ListSort`/`SortableHeaderRow`) — Plan B must not start before that lands.
- `GET /api/usage` fans out server-side (no per-host client fan-out possible) — fetch once, tolerate the latency, render the response's own host-freshness honestly; NEVER block the other blocks on it (usage_runs/outliers are independent local fetches).

---

### Task 1: Rust golden fixtures

**Files:**
- Modify: `crates/rupu-cp/tests/macos_fixtures.rs` (+`findings_global.json` (no-filter response with a non-empty summary + rows across ≥3 severities incl. one long-form "critical"/"medium" wire string), `coverage_summary.json` (≥2 projects), `coverage_detail.json` (assertions + findings + a catalog), `usage.json` (summary + breakdown rows + one unpriced model + 2-host freshness), `usage_runs.json` (≥6 flat rows across 2 models / 2 days / one nil-cost unpriced row), `usage_outliers.json` (≥2 outliers))

- [ ] **Step 1:** emitters from the REAL serde types per the established pattern; regen; `cargo test -p rupu-cp` GREEN. **Commit** — `test(cp): golden fixtures — global findings, coverage, usage family`

### Task 2: Swift API surface

**Files:**
- Create: `RupuAPI/CoverageModels.swift`, `RupuAPI/UsageModels.swift`
- Modify: `RupuAPI/CPClient.swift`, `Tests/RupuAPITests/FixtureDecodingTests.swift`

**Interfaces — Produces (field-for-field vs serde, snake_case CodingKeys, Optionals where Rust is Option):**
- `CPClient.findings(wsID: String? = nil) async throws -> APIFindings` — `GET api/findings[?ws_id=]` (reuses the EXISTING `APIFindings` envelope; verify the global rows' extra fields `ws_id`/`project`/`target_id`/`workflow_name?` decode — extend `APIFinding` additively if missing, existing tests untouched).
- `APICoverageSummary { wsID, project, targetID, assertionLines, hasCatalog, findings }`; `APICoverageDetail { targetID, assertionLines, hasCatalog, assertions: [APICoverageAssertion], findings, files? }` (assertion/catalog nested types per `coverage.rs`); `CPClient.coverage() -> [APICoverageSummary]`, `coverageDetail(target: String, wsID: String?) -> APICoverageDetail`, `coverageCatalog(target:wsID:)` if the catalog is a separate route (verify `coverage.rs` — `/：target/catalog`).
- `APIUsageResponse { summary: APIUsageSummary, breakdown: [APIUsageBreakdownRow], unpriced: APIUnpricedGap, hosts: [APIHostFreshness] }` (reuse Phase 4's `APIHostFreshness`); `APIUsageRunRow { runID, startedAt, workflowName?, agent?, provider?, model?, workspaceID?, hostID?, inputTokens, outputTokens, cachedTokens, totalTokens, costUSD: Double?, priced: Bool }` (verify exact fields vs `usage.rs`); `APIOutlierRun { runID, workflowName?, costUSD, baselineUSD, ratio, startedAt }`; `CPClient.usage(since:until:groupBy:) -> APIUsageResponse` (params mirroring web's window use — verify how web builds since/until), `usageRuns(since:until:wsID:) -> [APIUsageRunRow]`, `usageOutliers(since:until:) -> [APIOutlierRun]`.

- [ ] **Step 1:** failing decode tests (each fixture: counts + a poisoned/nil field + the unpriced row + long-form severity mapping through the existing `severity(for:)` seam) → implement → GREEN; gates. **Commit** — `feat(macos-breadth): API surface — coverage, global findings, usage family`

### Task 3: Security screen (findings + coverage tabs)

**Files:**
- Create: `RupuKit/Sources/RupuSecurity/SecurityScreen.swift` (+`FindingsTable.swift`, `CoverageList.swift`), `RupuStore/SecurityStore.swift`; Package.swift entries
- Modify: `RupuShell/RootView.swift`
- Test: store tests + a pure severity-summary test

**Produces:** `SecurityStore` (two BlockState blocks: global findings, coverage summaries; lazy per-tab fetch on first visit; client-identity rebuild pattern). Screen: segmented Findings/Coverage. Findings tab: summary strip (severity figures with the severity token colors — reuse the Phase 4-fixed `severity(for:)` mapping, lifted to a shared home if it still lives in RupuRunDetail: judge the cleanest seam and document) + sortable findings table (2px severity left edge per row, columns severity/title/project/target/workflow, ONE truncating title column); rows navigate to the owning run's detail when `run`-linked data exists — verify what `FindingOut` carries; if no run linkage, rows are non-navigating (no dead affordance). Coverage tab: grouped-by-project rows (target, assertion lines, catalog chip, findings count) → `.coverageDetail` route (Task 4).

- [ ] **Step 1:** failing store tests (lazy tabs, summary computation) → implement → GREEN.
- [ ] **Step 2:** screen + wiring + RoutingTests adaptation; gates. **Commit** — `feat(macos-breadth): Security — global findings + coverage summary`

### Task 4: Coverage detail

**Files:**
- Create: `RupuSecurity/CoverageDetailScreen.swift`; Modify: `RupuStore/Route.swift` (+`.coverageDetail(target:wsID:)` + mapping + tests), `RupuStore/SecurityStore.swift` or a small `CoverageDetailStore`, `RupuShell/RootView.swift`

**Produces:** header (target, project, assertion-lines figure, catalog chip) + tabs **Overview** (assertions list — mono lines; findings for the target) and **Catalog** (catalog entries). Deferred tabs (audit/gap/runs/diff) get NO tab stubs — the tab bar simply has two tabs (spec disposition; a doc comment names the deferral).

- [ ] **Step 1:** failing store/route tests → implement → screens; gates. **Commit** — `feat(macos-breadth): coverage detail — overview + catalog`

### Task 5: Usage aggregation port (pure) + UsageStore

**Files:**
- Create: `RupuKit/Sources/RupuUsage/UsageAggregation.swift` (pure), `RupuStore/UsageStore.swift`
- Test: `Tests/RupuUsageTests/UsageAggregationTests.swift`, store tests

**Produces:**
- PURE `buildSpendTimeline(rows: [APIUsageRunRow], window: TimeRange, now: Date) -> [SpendBucket]` and `aggregateRows(rows:, pivot: UsagePivot) -> [PivotRow]` — port the web's `buildTimeline`/`aggregateRuns` semantics EXACTLY (read the source; day bucketing, nil-cost/unpriced handling, pivot keys model/provider/agent/workflow/host/project with their null-key labels). `public enum UsagePivot: String, CaseIterable`. Test tables ported from the web functions' behavior (bucket day boundaries, unpriced exclusion/inclusion rules as the web does them, empty).
- `UsageStore`: three independent BlockState fetches (usage / usageRuns / outliers), window from `TimeRange` → since/until (verify the web's conversion and mirror it; `.all` = no since), pivot state, generation-guarded refetch on range change; NO reconcile loop (web refetches on window change only — parity; doc-comment it).

- [ ] **Step 1:** failing aggregation tables → implement → GREEN. **Step 2:** failing store tests (independence of the three blocks, range regen) → implement → GREEN; gates. **Commit** — `feat(macos-breadth): usage aggregation port + UsageStore`

### Task 6: Usage screen

**Files:**
- Create: `RupuUsage/UsageScreen.swift` (+`OutlierPanel.swift`, `BreakdownTable.swift`, `UnpricedBanner.swift`); Package.swift entries; Modify: `RupuShell/RootView.swift`

**Produces:** vertical stack — unpriced banner (TintBanner warn tone, named models — only when non-empty), freshness strip (REUSE Phase 4's `FreshnessStrip` over the usage response's hosts), spend-over-time chart (Phase 4 stacked-area idiom over `SpendBucket`s — single series or by-pivot stacking to match web; verify what web stacks and mirror), pivot picker (segmented or menu — web parity), breakdown sortable table (aggregated rows: pivot key / runs / tokens / cost, right-aligned numerals, unpriced rows flagged), outlier panel (run / cost vs baseline / ratio; rows navigate to run detail). Global toolbar range drives the window; `.onChange` → store (guarded per the Overview precedent).

- [ ] **Step 1:** screen composition; gates. **Commit** — `feat(macos-breadth): Usage — spend chart, pivot breakdown, outliers`

### Task 7: Docs + checkpoint + phase close

**Files:** `CLAUDE.md` (module map + read-first), umbrella §8 Phase 5 row → delivered wording + ALL §5 dispositions recorded (scope-aware launch fixed; usage-timeline dormant-matching-web; transcripts/run_resolve re-deferred; repos deferred; repo_scope n/a; per-screen deferrals).

- [ ] **Step 1:** docs edits; full gates incl. `cargo test -p rupu-cp`. **Commit** — `docs(macos-breadth): Phase 5 complete — pointers + dispositions`
- [ ] **Step 2 (controller):** checkpoint screenshots (Security + Usage, dark+light) + notes; phase-close summary to matt.

---

## Self-review notes

- Spec §4 Security→T3/T4, Usage→T5/T6; §5 dispositions→T7; fixtures→T1/T2 pairing mirrors every prior phase.
- Type consistency: `APICoverageSummary/Detail`, `APIUsageRunRow`, `UsagePivot`, `SpendBucket` defined in T2/T5 and consumed by name in T3/T4/T6; `APIFindings` extension additive only.
- Risks accepted: web's `buildTimeline` details must be read at implementation time (the plan names the semantics to port, not re-specified constants — the source is the contract); `GET /api/usage` fan-out latency is server-side and unavoidable (block independence protects the screen).
- T3→T4 sequential (shared SecurityStore/Route); T5→T6 sequential; T3/T4 vs T5/T6 still executed sequentially per SDD (no parallel implementers).
