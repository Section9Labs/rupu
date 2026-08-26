import Testing
import Foundation
@testable import RupuStore
import RupuAPI
import RupuUsageKit

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

/// A host row for `fetchHosts` stubs — same shape `ActivityStoreTests`/
/// `DashboardStoreTests` already build.
private func hostRow(id: String, status: String = "online") -> APIHostRow {
    APIHostRow(id: id, name: id, transportKind: "http", status: status)
}

// MARK: - makeStore

@MainActor
private func makeStore(
    usageResult: @escaping @Sendable (String?, String?, String?, String?) async throws -> APIUsageResponse = { _, _, _, _ in usageResponse() },
    hostsResult: @escaping @Sendable () async throws -> [APIHostRow] = { [] },
    usageRunsResult: @escaping @Sendable (String?, String?) async throws -> [APIUsageRunRow] = { _, _ in [] },
    outliersResult: @escaping @Sendable (String?, String?) async throws -> [APIOutlierRun] = { _, _ in [] }
) -> UsageStore {
    UsageStore(fetchUsage: usageResult, fetchHosts: hostsResult, fetchUsageRuns: usageRunsResult, fetchOutliers: outliersResult)
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
        usageResult: { _, _, _, _ in usageResponse() },
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
        usageResult: { since, until, _, _ in usageArgs.record((since, until)); return usageResponse() },
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
    let store = makeStore(usageResult: { _, _, groupBy, _ in groupBys.record(groupBy); return usageResponse() })
    await store.activate(range: .d30)
    #expect(groupBys.snapshot == ["model"])
}

// MARK: - Independence: one block's failure never blanks the others

@MainActor @Test func usageFailureLeavesUsageRunsAndOutliersUnaffected() async {
    let store = makeStore(
        usageResult: { _, _, _, _ in throw StubError(description: "usage boom") },
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
        usageResult: { _, _, _, _ in usageResponse() },
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
        usageResult: { _, _, _, _ in usageResponse() },
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
    let store = makeStore(usageResult: { _, _, _, _ in throw CancellationError() })
    await store.activate(range: .d30)
    guard case .loading = store.usage else { Issue.record("expected .loading (untouched by cancellation)"); return }
}

// MARK: - setPivot(_:) refetches usage ALONE

@MainActor @Test func setPivotRefetchesOnlyUsageLeavingUsageRunsAndOutliersUntouched() async {
    let usageCount = Counter()
    let runsCount = Counter()
    let outliersCount = Counter()
    let store = makeStore(
        usageResult: { _, _, _, _ in usageCount.increment(); return usageResponse() },
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
    let store = makeStore(usageResult: { _, _, groupBy, _ in groupBys.record(groupBy); return usageResponse() })
    await store.activate(range: .d30)
    await store.setPivot(.host)
    #expect(groupBys.snapshot == ["model", "host"])
}

/// Review fix (round 1, Important): a pivot change landing while
/// `usageRuns`/`outliers` are still in flight (from an `activate`/
/// `setRange` cycle that hasn't finished) must NOT strand either block at
/// `.loading` forever — `setPivot` only touches `usage`'s own generation
/// counter (see `UsageStore`'s doc comment), so `usageRuns`/`outliers`
/// keep the SAME `generation` their in-flight fetches were dispatched
/// under and their real results still apply once unblocked. Same "spins
/// forever" class PR #501 fixed on the run-status side.
@MainActor @Test func pivotChangeWhileUsageRunsAndOutliersAreInFlightLetsBothStillResolve() async {
    var releaseRuns: (() -> Void)?
    let runsReleased = AsyncStream<Void> { continuation in
        releaseRuns = { continuation.yield(); continuation.finish() }
    }
    var releaseOutliers: (() -> Void)?
    let outliersReleased = AsyncStream<Void> { continuation in
        releaseOutliers = { continuation.yield(); continuation.finish() }
    }

    let store = makeStore(
        usageResult: { _, _, _, _ in usageResponse() },
        usageRunsResult: { _, _ in
            for await _ in runsReleased { break }
            return [runRow()]
        },
        outliersResult: { _, _ in
            for await _ in outliersReleased { break }
            return [outlierRow()]
        }
    )

    let activateTask = Task { await store.activate(range: .d30) }
    // Give `activate`'s three concurrent fetches a chance to actually
    // start (and usageRuns/outliers to block).
    try? await Task.sleep(for: .milliseconds(20))

    // The pivot change lands while usageRuns/outliers are still blocked.
    await store.setPivot(.workflow)

    guard case .loading = store.usageRuns else {
        Issue.record("expected usageRuns still .loading (blocked, not yet stranded)")
        return
    }
    guard case .loading = store.outliers else {
        Issue.record("expected outliers still .loading (blocked, not yet stranded)")
        return
    }

    releaseRuns?()
    releaseOutliers?()
    _ = await activateTask.value

    #expect(store.usageRuns.value?.count == 1, "usageRuns must still resolve after the pivot change, not be stranded at .loading")
    #expect(store.outliers.value?.count == 1, "outliers must still resolve after the pivot change, not be stranded at .loading")
}

// MARK: - setRange(_:) refetches all three and drops stale in-flight results

@MainActor @Test func setRangeRefetchesAllThreeBlocks() async {
    let usageCount = Counter()
    let runsCount = Counter()
    let outliersCount = Counter()
    let store = makeStore(
        usageResult: { _, _, _, _ in usageCount.increment(); return usageResponse() },
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

    let store = makeStore(usageResult: { _, _, _, _ in
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
    let store = makeStore(usageResult: { _, _, _, _ in
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

// MARK: - Local-first, remote-progressive (perf & interaction arc, Plan 5 Task 2)
//
// Same de-flake/gating idioms `DashboardStoreTests`/`ActivityStoreTests`
// already establish (re-declared here, file-scoped `private`, per this
// module's existing "duplicate rather than widen access" convention) — a
// `DispatchSemaphore` gates the REMOTE host's response, never a timed
// sleep, so the "local already painted" assertion is made while the gate
// is provably still closed, not merely probably so.

@MainActor
private func pollUntil(
    timeout: Duration = .seconds(5),
    interval: Duration = .milliseconds(10),
    _ condition: () -> Bool
) async -> Bool {
    let deadline = ContinuousClock.now + timeout
    while true {
        if condition() { return true }
        if ContinuousClock.now >= deadline { return false }
        try? await Task.sleep(for: interval)
    }
}

@MainActor
private func expectEventually(
    timeout: Duration = .seconds(5),
    interval: Duration = .milliseconds(10),
    _ description: String,
    sourceLocation: SourceLocation = #_sourceLocation,
    _ condition: () -> Bool
) async {
    let succeeded = await pollUntil(timeout: timeout, interval: interval, condition)
    #expect(succeeded, "timed out waiting for: \(description)", sourceLocation: sourceLocation)
}

/// An async-native "semaphore" for gating an `async throws -> T` stub
/// closure — plain `DispatchSemaphore.wait` is unavailable from an async
/// context (correctly: it would block the cooperative thread pool), so this
/// gate blocks via `AsyncStream` instead, same idiom this file's own
/// `aStaleInFlightUsageFetchIsDroppedNotAppliedAfterASecondSetRange`/
/// `deactivateDropsAResultFromBeforeItWasCalled` already use inline —
/// pulled out once here since the local-first tests below need two of them.
/// `wait()` suspends until `release()` is called (from anywhere, including a
/// later point in the same test) — deterministic, never a timed sleep.
private final class AsyncGate: @unchecked Sendable {
    private var continuation: AsyncStream<Void>.Continuation?
    private let stream: AsyncStream<Void>

    init() {
        var c: AsyncStream<Void>.Continuation?
        stream = AsyncStream<Void> { c = $0 }
        continuation = c
    }

    func wait() async {
        for await _ in stream { break }
    }

    func release() {
        continuation?.yield()
        continuation?.finish()
        continuation = nil
    }
}

/// (a0) `activate(range:)` returns as soon as `"local"`'s own fetch lands —
/// fleet discovery (`GET /api/hosts`, here gated closed) runs entirely in
/// the background, never delaying `activate(range:)`'s own return. Same
/// fix (and same regression class) as `DashboardStoreTests`'
/// `activateNeverWaitsOnASlowGetApiHostsBeforePaintingLocal`.
@MainActor @Test func activateNeverWaitsOnSlowFleetDiscoveryBeforePaintingLocal() async {
    let hostsGate = AsyncGate()
    let store = makeStore(
        usageResult: { _, _, _, _ in usageResponse() },
        hostsResult: {
            await hostsGate.wait()
            return [hostRow(id: "local")]
        }
    )

    await store.activate(range: .d30)

    // Local truth already showing even though fleet discovery is still
    // gated closed.
    #expect(store.usage.value?.summary.totalTokens == 1500)
    #expect(store.pendingHosts == 0)

    hostsGate.release()
}

/// (a) `activate(range:)` returns once `"local"`'s fetch has landed —
/// `usage` already reflects local truth before a slow online remote host
/// has answered at all (asserted while a `DispatchSemaphore` provably still
/// gates it). The remote host's contribution then merges in progressively
/// once its gate opens.
@MainActor @Test func activatePaintsLocalImmediatelyThenMergesGatedRemoteHostProgressively() async {
    let miniGate = AsyncGate()
    let store = makeStore(
        usageResult: { _, _, _, host in
            if host == "mini" {
                await miniGate.wait()
                return APIUsageResponse(
                    summary: usageSummary(costUSD: 2), breakdown: [breakdownRow(model: "mini-model")],
                    unpriced: APIUnpricedGap(models: [], rows: 0), hosts: [hostFreshness(id: "mini")]
                )
            }
            return usageResponse() // "local"
        },
        hostsResult: { [hostRow(id: "local"), hostRow(id: "mini")] }
    )

    await store.activate(range: .d30)

    // Local truth already showing; the gated remote host hasn't been
    // waited on at all.
    #expect(store.usage.value?.summary.totalTokens == 1500, "local alone")
    #expect(store.usage.value?.breakdown.count == 1)

    miniGate.release()

    await expectEventually("mini's contribution merges into usage") {
        store.usage.value?.breakdown.count == 2
    }

    #expect(store.usage.value?.summary.totalTokens == 3000, "1500 (local) + 1500 (mini)")
    #expect(store.usage.value?.hosts.map(\.hostID).sorted() == ["local", "mini"])
    #expect(store.pendingHosts == 0)
}

/// (b) A remote host whose HTTP call itself fails never blanks `usage` —
/// it stays exactly what local alone produced, plus an `"unavailable"`
/// entry synthesized for the failing host in `hosts`.
@MainActor @Test func failingRemoteHostBecomesUnavailableAndUsageStaysLocalOnly() async {
    let store = makeStore(
        usageResult: { _, _, _, host in
            if host == "kuki" { throw StubError(description: "connection refused") }
            return usageResponse()
        },
        hostsResult: { [hostRow(id: "local"), hostRow(id: "kuki")] }
    )

    await store.activate(range: .d30)

    await expectEventually("kuki's synthesized freshness entry lands") {
        store.usage.value?.hosts.contains(where: { $0.hostID == "kuki" }) == true
    }

    #expect(store.usage.value?.summary.totalTokens == 1500, "the failing host must contribute nothing numerically")
    guard let kuki = store.usage.value?.hosts.first(where: { $0.hostID == "kuki" }) else {
        Issue.record("expected a synthesized kuki freshness entry")
        return
    }
    #expect(kuki.state == "unavailable")
    #expect(kuki.reason?.contains("connection refused") == true)
}

/// (c) A "local" failure skips remote discovery/fan-out entirely — no
/// reason to fan out to the fleet if the operator's own machine can't even
/// answer.
@MainActor @Test func localFailureNeverFansOutToRemoteHosts() async {
    let hostsCalls = Counter()
    let store = makeStore(
        usageResult: { _, _, _, _ in throw StubError(description: "local boom") },
        hostsResult: { hostsCalls.increment(); return [hostRow(id: "local"), hostRow(id: "mini")] }
    )

    await store.activate(range: .d30)

    guard case .failed = store.usage else { Issue.record("expected .failed"); return }
    // Give a wrongly-fired fan-out a moment to have shown up, if it were
    // going to.
    try? await Task.sleep(for: .milliseconds(50))
    #expect(hostsCalls.value == 0, "a local failure must never trigger fleet discovery")
}

/// (d) Generation guard: a remote host's response landing AFTER
/// `setPivot(_:)` has already bumped `usageGeneration` (and re-dispatched
/// `local` under the new pivot) must be dropped, not folded into the
/// newer cycle's merge — same "late arrival from a superseded cycle is
/// silently discarded" contract `DashboardStoreTests`/`ActivityStoreTests`
/// already prove for their own per-host progressive merges.
@MainActor @Test func staleRemoteHostResultFromBeforeASetPivotIsDroppedNotMerged() async {
    let miniGate = AsyncGate()
    let miniAttempts = Counter()
    let hostsCalls = Counter()
    let store = makeStore(
        usageResult: { _, _, groupBy, host in
            if host == "mini" {
                miniAttempts.increment()
                await miniGate.wait()
                return APIUsageResponse(
                    summary: usageSummary(costUSD: 2), breakdown: [breakdownRow(model: "mini-model")],
                    unpriced: APIUnpricedGap(models: [], rows: 0), hosts: [hostFreshness(id: "mini")]
                )
            }
            return APIUsageResponse(
                summary: usageSummary(costUSD: 1), breakdown: [breakdownRow(model: "local-\(groupBy ?? "")")],
                unpriced: APIUnpricedGap(models: [], rows: 0), hosts: [hostFreshness(id: "local")]
            )
        },
        // "mini" is only ever discovered on the FIRST fleet discovery (the
        // one `activate(range:)` kicks off) — the SECOND discovery cycle
        // (fired by `setPivot`'s own `loadUsageLocalFirst`) sees no remote
        // hosts at all. Combined with polling for `miniAttempts >= 1` below
        // BEFORE calling `setPivot`, this deterministically guarantees the
        // second `hostsResult()` call really is `setPivot`'s own discovery,
        // not a race against the first — so this test isolates exactly one
        // stale (generation 1) "mini" fetch rather than risking a second,
        // fresh one racing the same gate.
        hostsResult: {
            hostsCalls.increment() == 1 ? [hostRow(id: "local"), hostRow(id: "mini")] : [hostRow(id: "local")]
        }
    )

    await store.activate(range: .d30)
    // Local (under the .model pivot) has landed; mini is gated, still in
    // flight under generation 1.
    #expect(store.usage.value?.breakdown.first?.model == "local-model")

    // Wait for mini's stale fetch to have genuinely started (and be
    // blocked on the gate) before changing the pivot — otherwise `setPivot`
    // could race ahead of `activate`'s own background fleet discovery.
    await expectEventually("mini's stale (generation 1) fetch has started") {
        miniAttempts.value >= 1
    }

    // Pivot change bumps `usageGeneration`, cancels mini's in-flight task,
    // and re-dispatches local under the new pivot.
    await store.setPivot(.workflow)
    #expect(store.usage.value?.breakdown.first?.model == "local-workflow")

    // Only now release mini's stale (generation-1) response.
    miniGate.release()
    // Give the stale response a chance to have wrongly landed, if the
    // generation guard were broken.
    try? await Task.sleep(for: .milliseconds(50))

    #expect(miniAttempts.value == 1, "sanity: only the one stale mini fetch should ever have been dispatched")
    #expect(store.usage.value?.breakdown.count == 1, "mini's stale response must never merge into the new-pivot cycle")
    #expect(store.usage.value?.hosts.contains(where: { $0.hostID == "mini" }) == false)
}

// MARK: - Merge (pure — port of `crates/rupu-cp/src/api/usage.rs`)

@Test func rollupSumsTokensAndRunsAndSumsOnlyNonNilCosts() {
    let a = APIUsageSummary(inputTokens: 10, outputTokens: 5, cachedTokens: 1, totalTokens: 15, costUSD: 2, priced: true, runs: 1)
    let b = APIUsageSummary(inputTokens: 20, outputTokens: 0, cachedTokens: 0, totalTokens: 20, costUSD: nil, priced: false, runs: 1)
    let merged = UsageStore.rollup([a, b])
    #expect(merged.inputTokens == 30)
    #expect(merged.outputTokens == 5)
    #expect(merged.totalTokens == 35, "recomputed as inputTokens + outputTokens, not carried over")
    #expect(merged.costUSD == 2, "sums only the non-nil contribution, never fabricates 0 for the unpriced one")
    #expect(merged.priced == false, "one unpriced contributor poisons the merged flag")
    #expect(merged.runs == 2)
}

@Test func rollupOfNoSummariesIsAllZeroAndPricedTrue() {
    let merged = UsageStore.rollup([])
    #expect(merged.totalTokens == 0)
    #expect(merged.costUSD == nil)
    #expect(merged.priced == true, "matches the Rust source: priced starts true, only flipped by an actual unpriced contributor")
}

@Test func mergeUnpricedUnionsModelsAndSumsRows() {
    let a = APIUnpricedGap(models: ["gpt-x"], rows: 3)
    let b = APIUnpricedGap(models: ["gpt-x", "claude-y"], rows: 2)
    let merged = UsageStore.mergeUnpriced([a, b])
    #expect(merged.models == ["claude-y", "gpt-x"], "distinct union, sorted")
    #expect(merged.rows == 5)
}

@Test func mergeBreakdownRowsSumsSameKeyRowsAcrossHostsAndSortsByTotalTokensDescThenModelAsc() {
    let rowA = APIUsageBreakdownRow(
        provider: "anthropic", model: "claude", agent: "", workflow: "", hostID: "local", workspaceID: "",
        inputTokens: 100, outputTokens: 0, cachedTokens: 0, totalTokens: 100, costUSD: 1, priced: true, runs: 1
    )
    let rowB = APIUsageBreakdownRow(
        provider: "anthropic", model: "claude", agent: "", workflow: "", hostID: "mini", workspaceID: "",
        inputTokens: 50, outputTokens: 0, cachedTokens: 0, totalTokens: 50, costUSD: nil, priced: false, runs: 1
    )
    let rowC = APIUsageBreakdownRow(
        provider: "openai", model: "gpt", agent: "", workflow: "", hostID: "local", workspaceID: "",
        inputTokens: 200, outputTokens: 0, cachedTokens: 0, totalTokens: 200, costUSD: 4, priced: true, runs: 1
    )
    let merged = UsageStore.mergeBreakdownRows([rowA, rowB, rowC], groupBy: "model")

    #expect(merged.map(\.model) == ["gpt", "claude"], "totalTokens descending: gpt (200) before the merged claude row (150)")
    let claude = merged.first { $0.model == "claude" }
    #expect(claude?.totalTokens == 150)
    #expect(claude?.costUSD == 1, "sums only the non-nil contribution")
    #expect(claude?.priced == false, "one unpriced contributor poisons the merged row")
    #expect(claude?.runs == 2)
}

@Test func mergeBreakdownRowsGroupsByHostWhenGroupByIsHost() {
    let rowA = APIUsageBreakdownRow(
        provider: "", model: "claude", agent: "", workflow: "", hostID: "local", workspaceID: "",
        inputTokens: 10, outputTokens: 0, cachedTokens: 0, totalTokens: 10, costUSD: nil, priced: true, runs: 1
    )
    let rowB = APIUsageBreakdownRow(
        provider: "", model: "gpt", agent: "", workflow: "", hostID: "local", workspaceID: "",
        inputTokens: 5, outputTokens: 0, cachedTokens: 0, totalTokens: 5, costUSD: nil, priced: true, runs: 1
    )
    let merged = UsageStore.mergeBreakdownRows([rowA, rowB], groupBy: "host")
    #expect(merged.count == 1, "both rows share hostID \"local\" — one merged group")
    #expect(merged.first?.totalTokens == 15)
}

@Test func mergeUsageOfASingleResponseReproducesItsOwnFields() {
    let response = usageResponse()
    let merged = UsageStore.mergeUsage([response], groupBy: "model")
    #expect(merged.summary.totalTokens == response.summary.totalTokens)
    #expect(merged.breakdown == response.breakdown)
    #expect(merged.unpriced == response.unpriced)
    #expect(merged.hosts == response.hosts)
}
