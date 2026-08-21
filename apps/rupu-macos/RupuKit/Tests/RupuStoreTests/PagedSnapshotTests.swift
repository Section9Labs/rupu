import Testing
import Foundation
@testable import RupuStore

private struct FakeRow: Identifiable, Sendable, Equatable {
    let id: Int
}

private struct FakeFetchError: Error, Equatable {}

/// Thread-safe mutable flag, mirroring `RupuBackendTests.HealthMonitorTests`'
/// `LockedBox`: a plain captured `var` can't cross into the `@Sendable`
/// `fetch` closure under Swift 6 strict concurrency, so the fake fetches
/// below toggle behavior through this instead.
private final class FlagBox: @unchecked Sendable {
    private let lock = NSLock()
    private var v: Bool
    init(_ v: Bool) { self.v = v }
    var value: Bool {
        get { lock.withLock { v } }
        set { lock.withLock { v = newValue } }
    }
}

/// Thread-safe call counter, same rationale as `FlagBox` above.
private final class CountBox: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    func increment() { lock.withLock { count += 1 } }
    var value: Int { lock.withLock { count } }
}

@MainActor @Test func pagedSnapshotPagesThroughAllRowsAndMarksExhausted() async {
    let all = (0..<120).map { FakeRow(id: $0) }
    let snapshot = PagedSnapshot<FakeRow> { offset, limit in
        guard offset < all.count else { return [] }
        let end = min(offset + limit, all.count)
        return Array(all[offset..<end])
    }

    await snapshot.refresh()
    #expect(snapshot.rows.count == 50)
    #expect(snapshot.exhausted == false)
    #expect(snapshot.state.value != nil)

    await snapshot.loadMore()
    #expect(snapshot.rows.count == 100)
    #expect(snapshot.exhausted == false)

    await snapshot.loadMore()
    #expect(snapshot.rows.count == 120)
    #expect(snapshot.exhausted == true)
    #expect(snapshot.rows.map(\.id) == all.map(\.id))

    // A no-op once exhausted — a further loadMore must not re-invoke fetch
    // or change anything.
    await snapshot.loadMore()
    #expect(snapshot.rows.count == 120)
}

@MainActor @Test func pagedSnapshotRefreshOnEmptyFirstPageSetsEmptyState() async {
    let snapshot = PagedSnapshot<FakeRow> { _, _ in [] }
    await snapshot.refresh()
    #expect(snapshot.rows.isEmpty)
    #expect(snapshot.exhausted == true)
    guard case .empty = snapshot.state else {
        Issue.record("expected .empty, got \(snapshot.state)")
        return
    }
}

@MainActor @Test func pagedSnapshotFetchFailureKeepsPriorRowsThenRecoversOnRefresh() async {
    let all = (0..<80).map { FakeRow(id: $0) }
    let shouldFail = FlagBox(false)
    let snapshot = PagedSnapshot<FakeRow>(pageSize: 50) { offset, limit in
        if shouldFail.value {
            throw FakeFetchError()
        }
        guard offset < all.count else { return [] }
        let end = min(offset + limit, all.count)
        return Array(all[offset..<end])
    }

    await snapshot.refresh()
    #expect(snapshot.rows.count == 50)

    // loadMore's fetch (page 1) throws: prior 50 rows must stay intact,
    // never partially appended, and state must reflect the failure.
    shouldFail.value = true
    await snapshot.loadMore()
    #expect(snapshot.rows.count == 50)
    guard case .failed(let message) = snapshot.state else {
        Issue.record("expected .failed, got \(snapshot.state)")
        return
    }
    #expect(!message.isEmpty)

    // refresh() after the failure recovers once fetch succeeds again.
    shouldFail.value = false
    await snapshot.refresh()
    guard case .content = snapshot.state else {
        Issue.record("expected recovery to .content, got \(snapshot.state)")
        return
    }
    #expect(snapshot.rows.count == 50)
    #expect(snapshot.rows.map(\.id) == Array(0..<50))
}

@MainActor @Test func pagedSnapshotRefreshFailureBeforeAnyLoadLeavesRowsEmpty() async {
    let snapshot = PagedSnapshot<FakeRow> { _, _ in throw FakeFetchError() }
    await snapshot.refresh()
    #expect(snapshot.rows.isEmpty)
    guard case .failed = snapshot.state else {
        Issue.record("expected .failed, got \(snapshot.state)")
        return
    }
}

/// Reentrancy regression: two `loadMore()` calls started concurrently (e.g.
/// two views both triggering a load) used to each read the same stale
/// `rows.count` and both append their own page, duplicating rows. `fetch`
/// sleeps briefly to widen the window a real double-invocation would race
/// in; `inFlight` must make the second call a no-op regardless of which one
/// happens to win the MainActor scheduling race.
@MainActor @Test func pagedSnapshotConcurrentLoadMoreCallsDoNotDuplicateRowsOrDoubleFetch() async {
    let all = (0..<80).map { FakeRow(id: $0) }
    let fetchCount = CountBox()
    let snapshot = PagedSnapshot<FakeRow>(pageSize: 50) { offset, limit in
        fetchCount.increment()
        try? await Task.sleep(for: .milliseconds(20))
        guard offset < all.count else { return [] }
        let end = min(offset + limit, all.count)
        return Array(all[offset..<end])
    }

    await snapshot.refresh()
    #expect(snapshot.rows.count == 50)
    #expect(fetchCount.value == 1)

    // `loadMore()` is now `@discardableResult Bool` (review fix: it reports
    // performed-vs-skipped for `ActivityStore`'s debounce-retry logic) —
    // the annotation just needs to match that; this test still only cares
    // about the row/fetch-count side effects, not the returned Bools.
    async let first: Bool = snapshot.loadMore()
    async let second: Bool = snapshot.loadMore()
    _ = await (first, second)

    #expect(fetchCount.value == 2) // one for refresh() above, one for whichever loadMore() won
    #expect(snapshot.rows.count == 80)
    #expect(Set(snapshot.rows.map(\.id)).count == snapshot.rows.count) // no duplicate rows
}

/// `refresh()` and `loadMore()` share the same `inFlight` guard: a
/// `refresh()` that arrives while a `loadMore()` is still in flight is also
/// a no-op, not a queued/coalesced follow-up — documenting that choice
/// concretely rather than just in a comment.
@MainActor @Test func pagedSnapshotRefreshWhileLoadMoreInFlightIsANoOp() async {
    let all = (0..<80).map { FakeRow(id: $0) }
    let fetchCount = CountBox()
    let snapshot = PagedSnapshot<FakeRow>(pageSize: 50) { offset, limit in
        fetchCount.increment()
        try? await Task.sleep(for: .milliseconds(20))
        guard offset < all.count else { return [] }
        let end = min(offset + limit, all.count)
        return Array(all[offset..<end])
    }

    await snapshot.refresh()
    #expect(fetchCount.value == 1)

    async let loading: Bool = snapshot.loadMore()
    async let refreshing: Bool = snapshot.refresh()
    _ = await (loading, refreshing)

    #expect(fetchCount.value == 2) // the initial refresh() + exactly one of {loadMore, refresh}
}

/// Hotfix root cause B: cancellation is benign. A `fetch` closure throwing
/// `CancellationError` (the shape a superseded SwiftUI `.task(id:)`
/// produces) must leave `state` exactly as it already was — `.loading`,
/// set synchronously at the top of `refresh()` before this — never
/// `.failed`. `rows` also stays untouched (still empty from before this
/// call).
@MainActor @Test func pagedSnapshotRefreshCancellationLeavesStateAsLoadingNeverFailed() async {
    let snapshot = PagedSnapshot<FakeRow> { _, _ in throw CancellationError() }
    await snapshot.refresh()
    guard case .loading = snapshot.state else {
        Issue.record("expected cancellation to leave state as .loading, got \(snapshot.state)")
        return
    }
    #expect(snapshot.rows.isEmpty)
}

/// Same contract for `loadMore()`, against prior content that must survive
/// untouched — proving cancellation never blanks or fails existing rows
/// either.
@MainActor @Test func pagedSnapshotLoadMoreCancellationLeavesPriorRowsAndStateUntouched() async {
    let all = (0..<80).map { FakeRow(id: $0) }
    let shouldCancel = FlagBox(false)
    let snapshot = PagedSnapshot<FakeRow>(pageSize: 50) { offset, limit in
        if shouldCancel.value { throw CancellationError() }
        guard offset < all.count else { return [] }
        let end = min(offset + limit, all.count)
        return Array(all[offset..<end])
    }
    await snapshot.refresh()
    #expect(snapshot.rows.count == 50)
    guard case .content = snapshot.state else {
        Issue.record("expected .content after the successful refresh, got \(snapshot.state)")
        return
    }

    shouldCancel.value = true
    await snapshot.loadMore()
    #expect(snapshot.rows.count == 50) // untouched — no partial append, no blanking
    guard case .content = snapshot.state else {
        Issue.record("expected state to stay .content through a cancelled loadMore, got \(snapshot.state)")
        return
    }
}
