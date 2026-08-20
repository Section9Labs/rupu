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
