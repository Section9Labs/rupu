import Testing
import Foundation
import RupuAPI
import RupuStore
import RupuUsageKit
@testable import RupuUsage

// Pure-function tests for the Usage screen's (Phase 5B, Task 6) small
// testable seams — `SpendChart`/`OutlierPanel`/`UsageScreen`'s own View
// bodies aren't unit-tested (this codebase's established convention: see
// `RupuSecurityTests/FindingNavigationRouteTests.swift`, which tests only
// `findingNavigationRoute`, not `FindingsTabView` itself).

private func row(
    runID: String = "run_1",
    startedAt: String = "2026-07-01T10:00:00Z",
    workflowName: String = "wf",
    agent: String = "agent_1",
    provider: String = "anthropic",
    model: String = "claude",
    workspaceID: String = "ws_1",
    hostID: String = "local",
    inputTokens: UInt64 = 100,
    outputTokens: UInt64 = 50,
    cachedTokens: UInt64 = 0,
    totalTokens: UInt64 = 150,
    costUSD: Double? = 1,
    priced: Bool = true
) -> APIUsageRunRow {
    APIUsageRunRow(
        runID: runID, startedAt: startedAt, workflowName: workflowName, agent: agent, provider: provider,
        model: model, workspaceID: workspaceID, hostID: hostID, inputTokens: inputTokens,
        outputTokens: outputTokens, cachedTokens: cachedTokens, totalTokens: totalTokens,
        costUSD: costUSD, priced: priced
    )
}

// MARK: - spendChartRows

@Test func spendChartRowsOneEntryPerBucketPerPivotKey() {
    let buckets = [
        SpendBucket(day: "2026-07-01", rows: [
            row(runID: "a", model: "claude", costUSD: 1),
            row(runID: "b", model: "gpt", costUSD: 2),
        ]),
        SpendBucket(day: "2026-07-02", rows: [
            row(runID: "c", model: "claude", costUSD: 3),
        ]),
    ]
    let out = spendChartRows(buckets: buckets, pivot: .model)
    #expect(out.count == 3)
    #expect(Set(out.map(\.series)) == ["claude", "gpt"])
    #expect(out.first { $0.day == "2026-07-01" && $0.series == "claude" }?.value == 1)
    #expect(out.first { $0.day == "2026-07-01" && $0.series == "gpt" }?.value == 2)
    #expect(out.first { $0.day == "2026-07-02" && $0.series == "claude" }?.value == 3)
}

@Test func spendChartRowsUnpricedContributionIsZeroNotDropped() {
    let buckets = [
        SpendBucket(day: "2026-07-01", rows: [
            row(runID: "a", model: "untracked-model", costUSD: nil, priced: false),
        ]),
    ]
    let out = spendChartRows(buckets: buckets, pivot: .model)
    #expect(out.count == 1)
    #expect(out[0].series == "untracked-model")
    #expect(out[0].value == 0)
}

@Test func spendChartRowsEmptyBucketContributesNoEntries() {
    let buckets = [SpendBucket(day: "2026-07-01", rows: [])]
    #expect(spendChartRows(buckets: buckets, pivot: .model).isEmpty)
}

@Test func spendChartRowsUsesPivotLabelFallbackForEmptyKey() {
    // A `PivotRow` from the `.workflow` pivot on a row with no workflow name
    // reads "—" (`pivotLabel`'s documented fallback), not a blank series.
    let buckets = [SpendBucket(day: "2026-07-01", rows: [row(workflowName: "", costUSD: 5)])]
    let out = spendChartRows(buckets: buckets, pivot: .workflow)
    #expect(out.count == 1)
    #expect(out[0].series == "—")
}

// MARK: - parseSpendDayKey

@Test func parseSpendDayKeyParsesUTCMidnight() {
    let date = parseSpendDayKey("2026-07-04")
    #expect(date != nil)
    var cal = Calendar(identifier: .gregorian)
    cal.timeZone = TimeZone(identifier: "UTC")!
    let comps = cal.dateComponents([.year, .month, .day, .hour], from: date!)
    #expect(comps.year == 2026)
    #expect(comps.month == 7)
    #expect(comps.day == 4)
    #expect(comps.hour == 0)
}

@Test func parseSpendDayKeyRejectsMalformedInput() {
    #expect(parseSpendDayKey("not-a-date") == nil)
    #expect(parseSpendDayKey("") == nil)
}

// MARK: - assignSeriesColors

@Test func assignSeriesColorsIsStableAndDeterministicAcrossInputOrder() {
    let mapA = assignSeriesColors(["gpt", "claude", "gemini"])
    let mapB = assignSeriesColors(["claude", "gemini", "gpt"])
    #expect(mapA == mapB)
    #expect(mapA.keys.count == 3)
}

@Test func assignSeriesColorsCyclesPastTenDistinctKeys() {
    let labels = (0..<13).map { "series-\($0)" }
    let map = assignSeriesColors(labels)
    #expect(map.count == 13)
    // The 11th (index 10, sorted) key wraps back to the palette's first
    // color, same as the 1st (index 0) sorted key.
    let sorted = labels.sorted()
    #expect(map[sorted[0]] == map[sorted[10]])
}

// MARK: - pivotTitle

@Test func pivotTitleCoversEveryPivotWithADistinctLabel() {
    let titles = UsagePivot.allCases.map(pivotTitle)
    #expect(Set(titles).count == UsagePivot.allCases.count)
    #expect(pivotTitle(.model) == "Model")
    #expect(pivotTitle(.project) == "Project")
}

// MARK: - outlierNavigationRoute

@Test func outlierNavigationRouteIsALocalRunDetailRoute() {
    let route = outlierNavigationRoute(runID: "run_xyz")
    #expect(route == .runDetail(id: "run_xyz", host: nil))
}

// MARK: - usageHostSlice

@Test func usageHostSliceMapsOkWithCapturedAt() {
    let freshness = APIHostFreshness(
        hostID: "local", name: "Local", transportKind: "local", state: "ok",
        capturedAt: "2026-07-04T00:00:00Z", reason: nil
    )
    let slice = usageHostSlice(freshness)
    #expect(slice.id == "local")
    #expect(slice.name == "Local")
    #expect(slice.state == .ok(capturedAt: "2026-07-04T00:00:00Z"))
}

@Test func usageHostSliceMapsOffline() {
    let freshness = APIHostFreshness(
        hostID: "h1", name: "Host 1", transportKind: "ssh", state: "offline",
        capturedAt: nil, reason: nil
    )
    #expect(usageHostSlice(freshness).state == .offline)
}

@Test func usageHostSliceMapsUnavailableWithReason() {
    let freshness = APIHostFreshness(
        hostID: "h2", name: "Host 2", transportKind: "ssh", state: "unavailable",
        capturedAt: nil, reason: "needs rupu >= 0.49"
    )
    #expect(usageHostSlice(freshness).state == .unavailable(reason: "needs rupu >= 0.49"))
}

@Test func usageHostSliceNeverProducesLoadingRegardlessOfWireState() {
    // `.loading` is a client-side-fetch-in-flight state `APIHostFreshness`
    // (already server-resolved) has no wire value for — every branch of
    // `usageHostSlice` must land on `.ok`/`.offline`/`.unavailable`.
    for wireState in ["ok", "offline", "unavailable", "some-future-state"] {
        let freshness = APIHostFreshness(
            hostID: "h", name: "H", transportKind: "local", state: wireState,
            capturedAt: nil, reason: nil
        )
        if case .loading = usageHostSlice(freshness).state {
            Issue.record("usageHostSlice produced .loading for wire state \(wireState)")
        }
    }
}

@Test func usageHostSliceMapsUnrecognizedStateToUnavailableWithRawStringAsReason() {
    let freshness = APIHostFreshness(
        hostID: "h3", name: "Host 3", transportKind: "ssh", state: "degraded",
        capturedAt: nil, reason: nil
    )
    #expect(usageHostSlice(freshness).state == .unavailable(reason: "degraded"))
}
