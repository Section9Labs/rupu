import Testing
import Foundation
@testable import RupuStore
import RupuAPI
import RupuUsage

// MARK: - Test doubles

/// Thread-safe call counter/log — same rationale as
/// `ProjectDetailStoreTests.Counter`/`LimitLog`: a plain captured `var`
/// can't cross into a `@Sendable` fetch closure under Swift 6 strict
/// concurrency.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
    var value: Int { lock.withLock { v } }
}

private final class Log<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var v: [T] = []
    func record(_ item: T) { lock.withLock { v.append(item) } }
    var snapshot: [T] { lock.withLock { v } }
}

private struct StubError: Error, CustomStringConvertible {
    let description: String
}

// MARK: - Fixture builders

private func usageSummary(costUSD: Double? = 3.5) -> APIUsageSummary {
    APIUsageSummary(inputTokens: 1000, outputTokens: 500, cachedTokens: 0, totalTokens: 1500, costUSD: costUSD, priced: costUSD != nil, runs: 2)
}

private func breakdownRow(model: String = "claude") -> APIUsageBreakdownRow {
    APIUsageBreakdownRow(
        provider: "anthropic", model: model, agent: "", workflow: "", hostID: "", workspaceID: "",
        inputTokens: 1000, outputTokens: 500, cachedTokens: 0, totalTokens: 1500, costUSD: 3.5, priced: true, runs: 2
    )
}

private func hostFreshness(id: String = "local", state: String = "ok") -> APIHostFreshness {
    APIHostFreshness(hostID: id, name: id, transportKind: "local", state: state, capturedAt: "2026-08-20T12:00:00Z", reason: nil)
}

private func usageResponse() -> APIUsageResponse {
    APIUsageResponse(
        summary: usageSummary(),
        breakdown: [breakdownRow()],
        unpriced: APIUnpricedGap(models: [], rows: 0),
        hosts: [hostFreshness()]
    )
}

private func runRow(id: String = "run-1", model: String = "claude") -> APIUsageRunRow {
    APIUsageRunRow(
        runID: id, startedAt: "2026-08-20T12:00:00Z", workflowName: "nightly-health", agent: "rupuso",
        provider: "anthropic", model: model, workspaceID: "ws-1", hostID: "local",
        inputTokens: 100, outputTokens: 50, cachedTokens: 0, totalTokens: 150, costUSD: 1, priced: true
    )
}

private func outlierRow(id: String = "run-9") -> APIOutlierRun {
    APIOutlierRun(runID: id, workflowName: "nightly-health", costUSD: 12, baselineUSD: 1, ratio: 12, startedAt: "2026-08-20T12:00:00Z")
}

// MARK: - makeStore

@MainActor
private func makeStore(
    usageResult: @escaping @Sendable (String?, String?, String?) async throws -> APIUsageResponse = { _, _, _ in usageResponse() },
    usageRunsResult: @escaping @Sendable (String?, String?) async throws -> [APIUsageRunRow] = { _, _ in [] },
    outliersResult: @escaping @Sendable (String?, String?) async throws -> [APIOutlierRun] = { _, _ in [] }
) -> UsageStore {
    UsageStore(fetchUsage: usageResult, fetchUsageRuns: usageRunsResult, fetchOutliers: outliersResult)
}

// MARK: - windowBounds (port of the web's `presetWindow`/`usageRangeSince`)

@Test func windowBoundsD7IsSevenDaysBackFromNow() {
    let now = Date(timeIntervalSince1970: 1_000_000)
    let bounds = UsageStore.windowBounds(for: .d7, now: now)
    #expect(bounds.until == now)
    #expect(bounds.since == now.addingTimeInterval(-7 * 86_400))
}

@Test func windowBoundsD30IsThirtyDaysBackFromNow() {
    let now = Date(timeIntervalSince1970: 1_000_000)
    let bounds = UsageStore.windowBounds(for: .d30, now: now)
    #expect(bounds.since == now.addingTimeInterval(-30 * 86_400))
}

/// Resolved ambiguity (see task report): `.all` is `[epoch, now]`, NOT an
/// omitted `since` — the web's `usageRangeSince` doc comment explains why
/// (an absent `since` defaults server-side to "last 30 days", which would
/// silently NARROW `.all` rather than broadening it). Pinned down here so a
/// future change can't accidentally revert to "omit since for .all".
@Test func windowBoundsAllIsEpochToNowNeverOmitted() {
    let now = Date(timeIntervalSince1970: 5_000_000)
    let bounds = UsageStore.windowBounds(for: .all, now: now)
    #expect(bounds.since == Date(timeIntervalSince1970: 0))
    #expect(bounds.until == now)
}

// MARK: - activate() dispatches all three, independently

@MainActor @Test func activateLoadsAllThreeBlocks() async {
    let store = makeStore(
        usageResult: { _, _, _ in usageResponse() },
        usageRunsResult: { _, _ in [runRow()] },
        outliersResult: { _, _ in [outlierRow()] }
    )
    await store.activate(range: .d30)

    #expect(store.usage.value?.summary.totalTokens == 1500)
    #expect(store.usageRuns.value?.count == 1)
    #expect(store.outliers.value?.count == 1)
}

@MainActor @Test func activateSendsRFC3339SinceUntilToEveryEndpoint() async {
    let usageArgs = Log<(String?, String?)>()
    let runsArgs = Log<(String?, String?)>()
    let outliersArgs = Log<(String?, String?)>()
    let store = makeStore(
        usageResult: { since, until, _ in usageArgs.record((since, until)); return usageResponse() },
        usageRunsResult: { since, until in runsArgs.record((since, until)); return [] },
        outliersResult: { since, until in outliersArgs.record((since, until)); return [] }
    )
    await store.activate(range: .d7)

    #expect(usageArgs.snapshot.count == 1)
    #expect(usageArgs.snapshot[0].0 != nil, "since must be a concrete RFC-3339 string, never omitted")
    #expect(usageArgs.snapshot[0].1 != nil)
    #expect(runsArgs.snapshot.count == 1)
    #expect(outliersArgs.snapshot.count == 1)
}

@MainActor @Test func activateSendsGroupByMatchingTheDefaultPivot() async {
    let groupBys = Log<String?>()
    let store = makeStore(usageResult: { _, _, groupBy in groupBys.record(groupBy); return usageResponse() })
    await store.activate(range: .d30)
    #expect(groupBys.snapshot == ["model"])
}

// MARK: - Independence: one block's failure never blanks the others

@MainActor @Test func usageFailureLeavesUsageRunsAndOutliersUnaffected() async {
    let store = makeStore(
        usageResult: { _, _, _ in throw StubError(description: "usage boom") },
        usageRunsResult: { _, _ in [runRow()] },
        outliersResult: { _, _ in [outlierRow()] }
    )
    await store.activate(range: .d30)

    guard case .failed(let message) = store.usage else { Issue.record("expected usage .failed"); return }
    #expect(message.contains("usage boom"))
    #expect(store.usageRuns.value?.count == 1)
    #expect(store.outliers.value?.count == 1)
}

@MainActor @Test func usageRunsFailureLeavesUsageAndOutliersUnaffected() async {
    let store = makeStore(
        usageResult: { _, _, _ in usageResponse() },
        usageRunsResult: { _, _ in throw StubError(description: "runs boom") },
        outliersResult: { _, _ in [outlierRow()] }
    )
    await store.activate(range: .d30)

    #expect(store.usage.value != nil)
    guard case .failed(let message) = store.usageRuns else { Issue.record("expected usageRuns .failed"); return }
    #expect(message.contains("runs boom"))
    #expect(store.outliers.value?.count == 1)
}

@MainActor @Test func outliersFailureLeavesUsageAndUsageRunsUnaffected() async {
    let store = makeStore(
        usageResult: { _, _, _ in usageResponse() },
        usageRunsResult: { _, _ in [runRow()] },
        outliersResult: { _, _ in throw StubError(description: "outliers boom") }
    )
    await store.activate(range: .d30)

    #expect(store.usage.value != nil)
    #expect(store.usageRuns.value?.count == 1)
    guard case .failed(let message) = store.outliers else { Issue.record("expected outliers .failed"); return }
    #expect(message.contains("outliers boom"))
}

@MainActor @Test func emptyResultsSetEmptyNotContent() async {
    let store = makeStore(usageRunsResult: { _, _ in [] }, outliersResult: { _, _ in [] })
    await store.activate(range: .d30)
    guard case .empty = store.usageRuns else { Issue.record("expected .empty"); return }
    guard case .empty = store.outliers else { Issue.record("expected .empty"); return }
}

@MainActor @Test func cancellationLeavesBlockAtLoadingUntouched() async {
    let store = makeStore(usageResult: { _, _, _ in throw CancellationError() })
    await store.activate(range: .d30)
    guard case .loading = store.usage else { Issue.record("expected .loading (untouched by cancellation)"); return }
}

// MARK: - setPivot(_:) refetches usage ALONE

@MainActor @Test func setPivotRefetchesOnlyUsageLeavingUsageRunsAndOutliersUntouched() async {
    let usageCount = Counter()
    let runsCount = Counter()
    let outliersCount = Counter()
    let store = makeStore(
        usageResult: { _, _, _ in usageCount.increment(); return usageResponse() },
        usageRunsResult: { _, _ in runsCount.increment(); return [runRow()] },
        outliersResult: { _, _ in outliersCount.increment(); return [outlierRow()] }
    )
    await store.activate(range: .d30)
    #expect(usageCount.value == 1)
    #expect(runsCount.value == 1)
    #expect(outliersCount.value == 1)

    await store.setPivot(.workflow)
    #expect(usageCount.value == 2, "setPivot must refetch usage")
    #expect(runsCount.value == 1, "setPivot must NOT refetch usageRuns")
    #expect(outliersCount.value == 1, "setPivot must NOT refetch outliers")
}

@MainActor @Test func setPivotSendsTheNewPivotAsGroupBy() async {
    let groupBys = Log<String?>()
    let store = makeStore(usageResult: { _, _, groupBy in groupBys.record(groupBy); return usageResponse() })
    await store.activate(range: .d30)
    await store.setPivot(.host)
    #expect(groupBys.snapshot == ["model", "host"])
}

// MARK: - setRange(_:) refetches all three and drops stale in-flight results

@MainActor @Test func setRangeRefetchesAllThreeBlocks() async {
    let usageCount = Counter()
    let runsCount = Counter()
    let outliersCount = Counter()
    let store = makeStore(
        usageResult: { _, _, _ in usageCount.increment(); return usageResponse() },
        usageRunsResult: { _, _ in runsCount.increment(); return [] },
        outliersResult: { _, _ in outliersCount.increment(); return [] }
    )
    await store.activate(range: .d30)
    await store.setRange(.d7)
    #expect(usageCount.value == 2)
    #expect(runsCount.value == 2)
    #expect(outliersCount.value == 2)
}

/// Same "stale in-flight result must not clobber a newer one" contract
/// `DashboardStore`'s own generation-guard test proves — a slow first
/// `activate` result landing AFTER a second `setRange` has already
/// resolved must not overwrite the newer content.
@MainActor @Test func aStaleInFlightUsageFetchIsDroppedNotAppliedAfterASecondSetRange() async {
    var releaseFirst: (() -> Void)?
    let firstReleased = AsyncStream<Void> { continuation in
        releaseFirst = { continuation.yield(); continuation.finish() }
    }
    let callCount = Counter()

    let store = makeStore(usageResult: { _, _, _ in
        let n = callCount.increment()
        if n == 1 {
            // Block the FIRST call until explicitly released, well after
            // the second `setRange` below has already completed.
            for await _ in firstReleased { break }
            return APIUsageResponse(
                summary: usageSummary(costUSD: 1),
                breakdown: [], unpriced: APIUnpricedGap(models: [], rows: 0), hosts: []
            )
        }
        return APIUsageResponse(
            summary: usageSummary(costUSD: 99),
            breakdown: [], unpriced: APIUnpricedGap(models: [], rows: 0), hosts: []
        )
    })

    let firstActivate = Task { await store.activate(range: .d30) }
    // Give the first call a chance to actually start (and block).
    try? await Task.sleep(for: .milliseconds(20))

    await store.setRange(.d7)
    #expect(store.usage.value?.summary.costUSD == 99, "the second, faster fetch's result must be live")

    releaseFirst?()
    _ = await firstActivate.value
    #expect(store.usage.value?.summary.costUSD == 99, "the stale first fetch's late-arriving result must be dropped, not reapplied")
}

// MARK: - deactivate()

@MainActor @Test func deactivateDropsAResultFromBeforeItWasCalled() async {
    var releaseFirst: (() -> Void)?
    let firstReleased = AsyncStream<Void> { continuation in
        releaseFirst = { continuation.yield(); continuation.finish() }
    }
    let store = makeStore(usageResult: { _, _, _ in
        for await _ in firstReleased { break }
        return usageResponse()
    })

    let activateTask = Task { await store.activate(range: .d30) }
    try? await Task.sleep(for: .milliseconds(20))
    store.deactivate()
    releaseFirst?()
    _ = await activateTask.value

    guard case .loading = store.usage else { Issue.record("expected .loading — deactivate must drop the late result"); return }
}
