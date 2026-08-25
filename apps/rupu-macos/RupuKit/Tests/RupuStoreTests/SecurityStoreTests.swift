import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test doubles

/// Thread-safe call counter — same rationale as every other store test's
/// `Counter` in this module (`LibraryStoreTests`/`ProjectDetailStoreTests`):
/// a plain captured `var` can't cross into a `@Sendable` fetch closure under
/// Swift 6 strict concurrency.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
    var value: Int { lock.withLock { v } }
}

private struct StubError: Error, CustomStringConvertible {
    let description: String
}

// MARK: - Fixture builders

private func finding(id: String = "f-1", severity: String = "high", wsID: String = "ws-1", project: String = "rupu") -> APIFinding {
    APIFinding(
        id: id, summary: "finding \(id)", severity: severity, scope: "target", filePath: nil, lineRange: nil,
        wsID: wsID, project: project, targetID: "auth-core", workflowName: "nightly-security", permalink: nil,
        rationale: "because", declaredAt: "2026-08-20T12:00:00Z"
    )
}

private func findingsPayload(_ rows: [APIFinding] = [finding()]) -> APIFindings {
    APIFindings(
        findings: rows,
        summary: APIFindingsSummary(total: rows.count, critical: 0, high: rows.count, medium: 0, low: 0, info: 0)
    )
}

private func coverageRow(target: String = "auth-core", wsID: String = "ws-1", project: String = "rupu") -> APICoverageSummary {
    APICoverageSummary(wsID: wsID, project: project, targetID: target, assertionLines: 12, hasCatalog: true, findings: 1)
}

// MARK: - makeStore

@MainActor
private func makeStore(
    fetchFindings: @escaping @Sendable () async throws -> APIFindings = { findingsPayload() },
    fetchCoverage: @escaping @Sendable () async throws -> [APICoverageSummary] = { [coverageRow()] }
) -> SecurityStore {
    SecurityStore(fetchFindings: fetchFindings, fetchCoverage: fetchCoverage)
}

// MARK: - Initial state

@MainActor @Test func bothBlocksStartLoadingBeforeAnyFetch() {
    let store = makeStore()
    if case .loading = store.findings {} else { Issue.record("expected findings .loading") }
    if case .loading = store.coverage {} else { Issue.record("expected coverage .loading") }
}

// MARK: - Lazy per-tab loading

@MainActor @Test func loadFindingsIfNeededFetchesOnceAndIsANoOpOnASecondCall() async {
    let counter = Counter()
    let store = makeStore(fetchFindings: {
        counter.increment()
        return findingsPayload()
    })

    await store.loadFindingsIfNeeded()
    await store.loadFindingsIfNeeded()

    #expect(counter.value == 1, "a second loadFindingsIfNeeded() must not refetch")
}

@MainActor @Test func loadCoverageIfNeededFetchesOnceAndIsANoOpOnASecondCall() async {
    let counter = Counter()
    let store = makeStore(fetchCoverage: {
        counter.increment()
        return [coverageRow()]
    })

    await store.loadCoverageIfNeeded()
    await store.loadCoverageIfNeeded()

    #expect(counter.value == 1, "a second loadCoverageIfNeeded() must not refetch")
}

@MainActor @Test func loadFindingsIfNeededNeverTouchesCoverage() async {
    let coverageCounter = Counter()
    let store = makeStore(fetchCoverage: {
        coverageCounter.increment()
        return [coverageRow()]
    })

    await store.loadFindingsIfNeeded()

    #expect(store.findings.value != nil)
    if case .loading = store.coverage {} else { Issue.record("expected coverage to still be .loading") }
    #expect(coverageCounter.value == 0)
}

@MainActor @Test func loadCoverageIfNeededNeverTouchesFindings() async {
    let findingsCounter = Counter()
    let store = makeStore(fetchFindings: {
        findingsCounter.increment()
        return findingsPayload()
    })

    await store.loadCoverageIfNeeded()

    #expect(store.coverage.value != nil)
    if case .loading = store.findings {} else { Issue.record("expected findings to still be .loading") }
    #expect(findingsCounter.value == 0)
}

/// `loadFindings()`/`loadCoverage()` force a refetch regardless of the
/// lazy flag — the screen's own retry affordance for a `.failed` tab.
@MainActor @Test func loadFindingsForcesARefetchEvenWhenAlreadyRequested() async {
    let counter = Counter()
    let store = makeStore(fetchFindings: {
        counter.increment()
        return findingsPayload()
    })

    await store.loadFindingsIfNeeded()
    await store.loadFindings()

    #expect(counter.value == 2)
}

@MainActor @Test func loadCoverageForcesARefetchEvenWhenAlreadyRequested() async {
    let counter = Counter()
    let store = makeStore(fetchCoverage: {
        counter.increment()
        return [coverageRow()]
    })

    await store.loadCoverageIfNeeded()
    await store.loadCoverage()

    #expect(counter.value == 2)
}

/// `activate()` resets both lazy flags so a screen re-appearance (same
/// store instance) lets each tab refetch fresh on next selection — same
/// contract `ProjectDetailStore.activate()` documents.
@MainActor @Test func activateResetsLazyFlagsSoATabRefetchesOnNextSelection() async {
    let counter = Counter()
    let store = makeStore(fetchFindings: {
        counter.increment()
        return findingsPayload()
    })

    await store.loadFindingsIfNeeded()
    #expect(counter.value == 1)

    await store.activate()
    await store.loadFindingsIfNeeded()
    #expect(counter.value == 2)
}

@MainActor @Test func activateFetchesNothingItself() async {
    let findingsCounter = Counter()
    let coverageCounter = Counter()
    let store = makeStore(
        fetchFindings: { findingsCounter.increment(); return findingsPayload() },
        fetchCoverage: { coverageCounter.increment(); return [coverageRow()] }
    )

    await store.activate()

    #expect(findingsCounter.value == 0)
    #expect(coverageCounter.value == 0)
}

// MARK: - Per-block independence

/// One tab failing never blanks the other — same per-block-independence
/// contract `ProjectDetailStore`/`FleetStore`/`LibraryStore` already carry.
@MainActor @Test func findingsFailureLeavesCoverageUnaffected() async {
    let store = makeStore(fetchFindings: { throw StubError(description: "findings down") })

    await store.loadFindingsIfNeeded()
    await store.loadCoverageIfNeeded()

    guard case .failed(let message) = store.findings else {
        Issue.record("expected findings .failed")
        return
    }
    #expect(message.contains("findings down"))
    #expect(store.coverage.value != nil)
}

@MainActor @Test func coverageFailureLeavesFindingsUnaffected() async {
    let store = makeStore(fetchCoverage: { throw StubError(description: "coverage down") })

    await store.loadFindingsIfNeeded()
    await store.loadCoverageIfNeeded()

    guard case .failed(let message) = store.coverage else {
        Issue.record("expected coverage .failed")
        return
    }
    #expect(message.contains("coverage down"))
    #expect(store.findings.value != nil)
}

// MARK: - Empty-state semantics

/// `findings` is always `.content`, even when the payload's `findings`
/// array is empty — the summary counts are still real content to show. See
/// `SecurityStore`'s type doc comment.
@MainActor @Test func findingsStaysContentWhenThePayloadHasZeroRows() async {
    let store = makeStore(fetchFindings: { findingsPayload([]) })

    await store.loadFindingsIfNeeded()

    guard case .content(let value) = store.findings else {
        Issue.record("expected findings .content even with zero rows")
        return
    }
    #expect(value.findings.isEmpty)
}

/// `coverage` follows the ordinary list-block rule: an empty array is
/// `.empty`, not `.content([])`.
@MainActor @Test func coverageSetsEmptyStateForAZeroRowResult() async {
    let store = makeStore(fetchCoverage: { [] })

    await store.loadCoverageIfNeeded()

    if case .empty = store.coverage {} else { Issue.record("expected coverage .empty, got \(store.coverage)") }
}

// MARK: - Content decoding sanity

@MainActor @Test func loadFindingsDecodesEveryRowAndTheSummary() async {
    let rows = [finding(id: "f-1", severity: "critical"), finding(id: "f-2", severity: "low")]
    let store = makeStore(fetchFindings: { findingsPayload(rows) })

    await store.loadFindingsIfNeeded()

    #expect(store.findings.value?.findings.map(\.id) == ["f-1", "f-2"])
    #expect(store.findings.value?.summary.total == 2)
}

@MainActor @Test func loadCoverageDecodesEveryRow() async {
    let rows = [coverageRow(target: "auth-core"), coverageRow(target: "web-api", wsID: "ws-2", project: "phi-cell")]
    let store = makeStore(fetchCoverage: { rows })

    await store.loadCoverageIfNeeded()

    #expect(store.coverage.value?.map(\.targetID) == ["auth-core", "web-api"])
}
