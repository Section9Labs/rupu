import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Fixture builders

private func projectRow(
    wsID: String = "ws-1",
    name: String = "rupu",
    runCount: Int? = 14,
    lastRunAt: String? = "2026-08-20T12:00:00Z",
    costUSD: Double? = 0.85
) -> APIProjectRow {
    APIProjectRow(
        wsID: wsID, name: name, runCount: runCount, lastRunAt: lastRunAt,
        usage: APIUsageSummary(
            inputTokens: 5000, outputTokens: 1200, cachedTokens: 300, totalTokens: 6200,
            costUSD: costUSD, priced: costUSD != nil, runs: 2
        )
    )
}

private struct StubError: Error, CustomStringConvertible {
    let description: String
}

// MARK: - ProjectsStore

@MainActor @Test func projectsStoreActivateLoadsRowsIntoContent() async {
    let store = ProjectsStore(fetchProjects: { [projectRow(wsID: "ws-1"), projectRow(wsID: "ws-2", name: "other")] })
    await store.activate()

    guard case .content(let rows) = store.rows else {
        Issue.record("expected .content")
        return
    }
    #expect(rows.map(\.wsID) == ["ws-1", "ws-2"])
}

@MainActor @Test func projectsStoreActivateWithEmptyResultSetsEmpty() async {
    let store = ProjectsStore(fetchProjects: { [] })
    await store.activate()
    guard case .empty = store.rows else {
        Issue.record("expected .empty")
        return
    }
}

@MainActor @Test func projectsStoreActivateFailuresSurfaceAsFailed() async {
    let store = ProjectsStore(fetchProjects: { throw StubError(description: "boom") })
    await store.activate()
    guard case .failed(let message) = store.rows else {
        Issue.record("expected .failed")
        return
    }
    #expect(message.contains("boom"))
}

@MainActor @Test func projectsStoreCancellationLeavesStateUntouched() async {
    let store = ProjectsStore(fetchProjects: { throw CancellationError() })
    await store.activate()
    guard case .loading = store.rows else {
        Issue.record("expected .loading (untouched)")
        return
    }
}

/// Thread-safe call counter — same rationale as `ActivityStoreTests.Counter`
/// / `RunDetailStoreTests.Counter`: a plain captured `var` can't cross into a
/// `@Sendable` fetch closure under Swift 6 strict concurrency.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
}

@MainActor @Test func projectsStoreActivateIsRepeatableAndReloadsFromScratch() async {
    let counter = Counter()
    let store = ProjectsStore(fetchProjects: {
        counter.increment() == 1 ? [projectRow(wsID: "ws-1")] : [projectRow(wsID: "ws-1"), projectRow(wsID: "ws-2")]
    })
    await store.activate()
    #expect(store.rows.value?.count == 1)
    await store.activate()
    #expect(store.rows.value?.count == 2)
}
