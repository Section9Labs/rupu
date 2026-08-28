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

// MARK: - resetAndRefresh (perf & interaction arc, Plan 5 Task 5)

/// `isLoadingMore` is `true` only while a `loadMore()` fetch is actually in
/// flight — `false` before, during a plain `refresh()`, and after
/// completion. The sentinel's "loading more…" footer state reads this
/// directly, so it must never read true outside a genuine loadMore.
@MainActor @Test func pagedSnapshotIsLoadingMoreOnlyTrueDuringLoadMoreItself() async {
    let all = (0..<80).map { FakeRow(id: $0) }
    let snapshot = PagedSnapshot<FakeRow>(pageSize: 50) { offset, limit in
        guard offset < all.count else { return [] }
        let end = min(offset + limit, all.count)
        return Array(all[offset..<end])
    }

    #expect(snapshot.isLoadingMore == false)
    await snapshot.refresh()
    #expect(snapshot.isLoadingMore == false, "refresh() must never flip isLoadingMore")

    let loadTask = Task { await snapshot.loadMore() }
    // Give the fetch a moment to actually start before asserting the flag —
    // this fetch is synchronous (no artificial delay), so by the time
    // `loadTask` is awaited it may already be done; the meaningful assertion
    // is the post-completion one below, kept honest rather than racy.
    _ = await loadTask.value
    #expect(snapshot.isLoadingMore == false, "isLoadingMore must clear once loadMore() finishes")
}

/// Deterministic one-shot gate: `wait()` suspends until `open()` is called
/// (from anywhere), regardless of call ordering — used below to hold a fetch
/// closure's completion at a precise, test-controlled point WITHOUT
/// depending on sleep-based timing for correctness (unlike the sleep-widened
/// races elsewhere in this file, which only need "wide enough," not exact
/// ordering). An `actor`, not `FlagBox`'s lock-around-a-bool, because this
/// needs a real suspension point (`withCheckedContinuation`), not a busy-poll
/// loop — a busy-poll here would burn CPU (or, if `value` were ever
/// misread across tasks, hang indefinitely) for what should be an instant
/// handoff once `open()` is actually called.
private actor AsyncGate {
    private var isOpen = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        if isOpen { return }
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            waiters.append(continuation)
        }
    }

    func open() {
        isOpen = true
        let toResume = waiters
        waiters.removeAll()
        for w in toResume { w.resume() }
    }
}

/// The core paging-reset contract: `resetAndRefresh()` must win over a
/// `loadMore()` that was already in flight for the OLD data set when the
/// filter changed — the stale call's eventual completion must be discarded,
/// never appended on top of the freshly-reset page 0. This is the store-level
/// proof for the brief's "filter change resets to page 0 (late old-generation
/// page dropped)" requirement, expressed directly against `PagedSnapshot`
/// rather than through `ActivityStore`'s higher-level surface.
@MainActor @Test func pagedSnapshotResetAndRefreshDiscardsALateInFlightLoadMore() async {
    let oldData = (0..<80).map { FakeRow(id: $0) }
    let newData = (1000..<1010).map { FakeRow(id: $0) } // disjoint id range — unambiguous provenance
    let usingOldData = FlagBox(true)
    let gate = AsyncGate()

    let snapshot = PagedSnapshot<FakeRow>(pageSize: 50) { offset, limit in
        if usingOldData.value {
            // Only the loadMore() fetch (offset > 0) suspends until the test
            // explicitly opens the gate — models a loadMore() that's still in
            // flight when the filter changes, with NO timing dependency for
            // correctness (only the "has it started yet" check below is
            // timing-based, and that one is bounded — see its own comment).
            // The setup `refresh()` above fetches page 0 of oldData and must
            // pass through ungated: gating it too would deadlock the test
            // against its own not-yet-reached `gate.open()`.
            if offset > 0 {
                await gate.wait()
            }
            guard offset < oldData.count else { return [] }
            let end = min(offset + limit, oldData.count)
            return Array(oldData[offset..<end])
        }
        guard offset < newData.count else { return [] }
        let end = min(offset + limit, newData.count)
        return Array(newData[offset..<end])
    }

    await snapshot.refresh() // page 0 of oldData, 50 rows
    #expect(snapshot.rows.count == 50)

    // Start a loadMore() for oldData's page 1 — its fetch will suspend on
    // `gate.wait()` until this test calls `gate.open()` below.
    async let staleLoadMore: Bool = snapshot.loadMore()

    // Bounded wait for loadMore() to actually enter `fetch` — `isLoadingMore`
    // flips `true` synchronously, before the `await fetch(...)` call, so
    // this only needs to observe that flip, not race the gate itself. Capped
    // at 100 * 2ms so a regression elsewhere can never hang this test.
    for _ in 0..<100 where !snapshot.isLoadingMore {
        try? await Task.sleep(for: .milliseconds(2))
    }
    #expect(snapshot.isLoadingMore == true, "loadMore() should have entered fetch by now")

    // The filter changes: switch the fetch closure's data source and reset —
    // this must win regardless of the still-in-flight staleLoadMore above.
    usingOldData.value = false
    await snapshot.resetAndRefresh()
    #expect(snapshot.rows.map(\.id) == Array(1000..<1010), "resetAndRefresh's own page must be what's showing")
    #expect(snapshot.exhausted == true)

    // Now let the stale loadMore's fetch finally return its (old, now
    // irrelevant) page — it must be discarded, not appended.
    await gate.open()
    _ = await staleLoadMore
    #expect(snapshot.rows.map(\.id) == Array(1000..<1010), "the late old-generation page must never land")
    #expect(snapshot.exhausted == true)
}

/// `resetAndRefresh()` must actually restart paging from page 0 — a
/// previously-`exhausted` snapshot must become loadable again once the
/// underlying data set (or the filter driving `fetch`) changes.
@MainActor @Test func pagedSnapshotResetAndRefreshRestartsPagingFromZero() async {
    let data = FlagBox(true) // true → small data set (already exhausted after one page)
    let snapshot = PagedSnapshot<FakeRow>(pageSize: 50) { offset, limit in
        let rows = data.value ? (0..<10).map { FakeRow(id: $0) } : (0..<120).map { FakeRow(id: $0) }
        guard offset < rows.count else { return [] }
        let end = min(offset + limit, rows.count)
        return Array(rows[offset..<end])
    }

    await snapshot.refresh()
    #expect(snapshot.rows.count == 10)
    #expect(snapshot.exhausted == true)

    data.value = false
    await snapshot.resetAndRefresh()
    #expect(snapshot.rows.count == 50, "resetAndRefresh must re-fetch page 0 against the new data")
    #expect(snapshot.exhausted == false)

    await snapshot.loadMore()
    #expect(snapshot.rows.count == 100)
}

/// A `resetAndRefresh()` that lands while nothing else is in flight behaves
/// exactly like `refresh()` for the common case — same contract, just a
/// forced generation bump on top.
@MainActor @Test func pagedSnapshotResetAndRefreshWithNoConcurrentCallBehavesLikeRefresh() async {
    let all = (0..<30).map { FakeRow(id: $0) }
    let snapshot = PagedSnapshot<FakeRow>(pageSize: 50) { offset, limit in
        guard offset < all.count else { return [] }
        let end = min(offset + limit, all.count)
        return Array(all[offset..<end])
    }
    await snapshot.resetAndRefresh()
    #expect(snapshot.rows.map(\.id) == Array(0..<30))
    #expect(snapshot.exhausted == true)
}
