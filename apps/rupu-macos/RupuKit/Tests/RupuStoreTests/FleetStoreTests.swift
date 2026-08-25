import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Fixture builders

private func hostRow(
    id: String = "local",
    name: String = "local",
    transportKind: String = "local",
    status: String = "online",
    version: String? = "0.74.0",
    activeRunCount: Int? = 2,
    lastSeenAt: String? = nil
) -> APIHostRow {
    APIHostRow(
        id: id, name: name, transportKind: transportKind, status: status,
        version: version, activeRunCount: activeRunCount, lastSeenAt: lastSeenAt
    )
}

private func workerRow(
    workerID: String = "w-1",
    name: String = "matt-mbp",
    host: String = "local",
    activeRunCount: UInt64 = 1,
    totalRunCount: UInt64 = 42,
    lastRunAt: String? = "2026-08-20T12:00:00Z",
    lastSeenAt: String = "2026-08-24T00:00:00Z"
) -> APIWorkerRow {
    APIWorkerRow(
        version: 1, workerID: workerID, kind: "cli", name: name, host: host,
        capabilities: APIWorkerCapabilities(backends: [], scmHosts: [], permissionModes: []),
        registeredAt: "2026-08-01T00:00:00Z", lastSeenAt: lastSeenAt,
        activeRunCount: activeRunCount, totalRunCount: totalRunCount, lastRunAt: lastRunAt
    )
}

private struct StubError: Error, CustomStringConvertible {
    let description: String
}

/// Thread-safe call counter — same rationale as every other store test's
/// `Counter` in this file (`ProjectsStoreTests`, `ActivityStoreTests`, ...):
/// a plain captured `var` can't cross into a `@Sendable` fetch closure under
/// Swift 6 strict concurrency.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
}

@MainActor
private func makeStore(
    fetchHosts: @escaping @Sendable () async throws -> [APIHostRow] = { [hostRow()] },
    fetchWorkers: @escaping @Sendable () async throws -> [APIWorkerRow] = { [workerRow()] },
    postRemoveHost: @escaping @Sendable (String) async throws -> Void = { _ in },
    pendingActions: PendingActions = PendingActions()
) -> FleetStore {
    FleetStore(
        fetchHosts: fetchHosts, fetchWorkers: fetchWorkers, postRemoveHost: postRemoveHost,
        pendingActions: pendingActions
    )
}

// MARK: - activate(): merge of hosts + workers blocks

@MainActor @Test func activateLoadsHostsAndWorkersIntoContent() async {
    let store = makeStore(
        fetchHosts: { [hostRow(id: "local"), hostRow(id: "mini", name: "mini")] },
        fetchWorkers: { [workerRow(workerID: "w-1"), workerRow(workerID: "w-2")] }
    )
    await store.activate()

    guard case .content(let hosts) = store.hosts else {
        Issue.record("expected hosts .content")
        return
    }
    #expect(hosts.map(\.id) == ["local", "mini"])

    guard case .content(let workers) = store.workers else {
        Issue.record("expected workers .content")
        return
    }
    #expect(workers.map(\.workerID) == ["w-1", "w-2"])
}

@MainActor @Test func activateWithEmptyResultsSetsEmptyIndependently() async {
    let store = makeStore(fetchHosts: { [] }, fetchWorkers: { [] })
    await store.activate()

    guard case .empty = store.hosts else {
        Issue.record("expected hosts .empty")
        return
    }
    guard case .empty = store.workers else {
        Issue.record("expected workers .empty")
        return
    }
}

/// Independence per block (same convention `ProjectDetailStore`'s tabs
/// already establish): a failing hosts fetch must not blank an
/// already-successful workers block, and vice versa.
@MainActor @Test func hostsFailureDoesNotBlankWorkersBlock() async {
    let store = makeStore(
        fetchHosts: { throw StubError(description: "hosts down") },
        fetchWorkers: { [workerRow()] }
    )
    await store.activate()

    guard case .failed(let message) = store.hosts else {
        Issue.record("expected hosts .failed")
        return
    }
    #expect(message.contains("hosts down"))
    guard case .content = store.workers else {
        Issue.record("expected workers .content, untouched by the hosts failure")
        return
    }
}

@MainActor @Test func workersFailureDoesNotBlankHostsBlock() async {
    let store = makeStore(
        fetchHosts: { [hostRow()] },
        fetchWorkers: { throw StubError(description: "workers down") }
    )
    await store.activate()

    guard case .content = store.hosts else {
        Issue.record("expected hosts .content, untouched by the workers failure")
        return
    }
    guard case .failed(let message) = store.workers else {
        Issue.record("expected workers .failed")
        return
    }
    #expect(message.contains("workers down"))
}

@MainActor @Test func activateCancellationLeavesBothBlocksUntouched() async {
    let store = makeStore(
        fetchHosts: { throw CancellationError() },
        fetchWorkers: { throw CancellationError() }
    )
    await store.activate()

    guard case .loading = store.hosts else {
        Issue.record("expected hosts .loading (untouched)")
        return
    }
    guard case .loading = store.workers else {
        Issue.record("expected workers .loading (untouched)")
        return
    }
}

@MainActor @Test func activateIsRepeatableAndReloadsFromScratch() async {
    let counter = Counter()
    let store = makeStore(fetchHosts: {
        counter.increment() == 1 ? [hostRow(id: "local")] : [hostRow(id: "local"), hostRow(id: "mini")]
    })
    await store.activate()
    #expect(store.hosts.value?.count == 1)
    await store.activate()
    #expect(store.hosts.value?.count == 2)
}

// MARK: - applyHosts(_:): the pure, testable-without-timing seam

/// Mirrors `HostsFooterStoreTests`' "the poll loop itself is deliberately
/// not timing-tested, only the pure mapping" convention: `applyHosts` is
/// exercised directly, never through the 60s loop.
@MainActor @Test func applyHostsUpdatesHostsBlock() {
    let store = makeStore()
    store.applyHosts([hostRow(id: "local"), hostRow(id: "mini", name: "mini")])

    guard case .content(let hosts) = store.hosts else {
        Issue.record("expected .content")
        return
    }
    #expect(hosts.map(\.id) == ["local", "mini"])
}

@MainActor @Test func applyHostsWithEmptyRowsSetsEmpty() {
    let store = makeStore()
    store.applyHosts([hostRow()])
    store.applyHosts([])

    guard case .empty = store.hosts else {
        Issue.record("expected .empty")
        return
    }
}

/// The "row disappearing IS the confirmation" contract: a pending `.remove`
/// key for a host id no longer present in a fresh batch is confirmed by
/// `applyHosts` itself — never by the removal POST's own response.
@MainActor @Test func applyHostsConfirmsAPendingRemoveWhenTheHostIsGone() {
    let pendingActions = PendingActions()
    let store = makeStore(pendingActions: pendingActions)
    store.applyHosts([hostRow(id: "local"), hostRow(id: "mini", name: "mini")])

    let key = ActionKey("mini", .remove)
    pendingActions.begin(key)
    #expect(pendingActions.state(key) != .idle)

    store.applyHosts([hostRow(id: "local")])

    #expect(pendingActions.state(key) == .confirmed)
}

/// The host is STILL present in the fresh batch (removal hasn't taken
/// effect yet, or failed silently server-side) — the pending key must stay
/// pending, not confirm just because a reconcile ran.
@MainActor @Test func applyHostsLeavesRemovePendingWhileTheHostIsStillPresent() {
    let pendingActions = PendingActions()
    let store = makeStore(pendingActions: pendingActions)
    store.applyHosts([hostRow(id: "local"), hostRow(id: "mini", name: "mini")])

    let key = ActionKey("mini", .remove)
    pendingActions.begin(key)

    store.applyHosts([hostRow(id: "local"), hostRow(id: "mini", name: "mini")])

    guard case .pending = pendingActions.state(key) else {
        Issue.record("expected .pending, got \(pendingActions.state(key))")
        return
    }
}

/// A key that isn't currently `.pending` (never begun, or already
/// `.confirmed`/`.failed`) must be left exactly as it is — same "resolve
/// only ever touches `.pending` keys" contract `PendingActions.resolve`
/// itself documents.
@MainActor @Test func applyHostsNeverTouchesANonPendingRemoveKey() {
    let pendingActions = PendingActions()
    let store = makeStore(pendingActions: pendingActions)
    store.applyHosts([hostRow(id: "local"), hostRow(id: "mini", name: "mini")])

    let key = ActionKey("mini", .remove)
    // Never begun — stays .idle even once the host is gone.
    store.applyHosts([hostRow(id: "local")])
    #expect(pendingActions.state(key) == .idle)
}

/// First `applyHosts` call ever (no previous batch to diff against) must
/// not spuriously "confirm" every host in the very first response — there
/// is no `previousIDs` set yet, so the diff is empty by construction.
@MainActor @Test func applyHostsFirstCallNeverConfirmsAnythingSpuriously() {
    let pendingActions = PendingActions()
    let store = makeStore(pendingActions: pendingActions)
    let key = ActionKey("mini", .remove)
    pendingActions.begin(key)

    store.applyHosts([hostRow(id: "local")]) // "mini" never appeared at all

    guard case .pending = pendingActions.state(key) else {
        Issue.record("expected .pending, got \(pendingActions.state(key))")
        return
    }
}

// MARK: - removeHost(id:): confirm-first mutation

@MainActor @Test func removeHostBeginsPendingImmediately() async {
    let pendingActions = PendingActions()
    let store = makeStore(
        fetchHosts: { [hostRow(id: "local"), hostRow(id: "mini", name: "mini")] },
        postRemoveHost: { _ in try await Task.sleep(for: .seconds(60)) }, // never returns in this test's window
        pendingActions: pendingActions
    )
    await store.activate()

    let key = ActionKey("mini", .remove)
    let task = Task { await store.removeHost(id: "mini") }
    // Give the mutation a moment to fire `begin()` before we check —
    // `postRemoveHost` above never completes, so `removeHost` is still
    // suspended inside it right after `begin()`.
    try? await Task.sleep(for: .milliseconds(20))
    guard case .pending = pendingActions.state(key) else {
        Issue.record("expected .pending, got \(pendingActions.state(key))")
        task.cancel()
        return
    }
    task.cancel()
}

/// The success path: the DELETE succeeds, `removeHost` refetches, and the
/// refreshed hosts list (now missing `"mini"`) is what actually confirms
/// the key — never the DELETE's own 2xx alone.
@MainActor @Test func removeHostSuccessConfirmsOnceTheHostIsGoneFromTheRefetch() async {
    let pendingActions = PendingActions()
    let counter = Counter()
    let store = makeStore(
        fetchHosts: {
            // First fetch (activate): mini is present. Second fetch (the
            // post-removal reconcile removeHost triggers): mini is gone.
            counter.increment() == 1
                ? [hostRow(id: "local"), hostRow(id: "mini", name: "mini")]
                : [hostRow(id: "local")]
        },
        postRemoveHost: { _ in },
        pendingActions: pendingActions
    )
    await store.activate()

    await store.removeHost(id: "mini")

    let key = ActionKey("mini", .remove)
    #expect(pendingActions.state(key) == .confirmed)
    #expect(store.hosts.value?.map(\.id) == ["local"])
}

/// The DELETE request itself fails (network / non-2xx) — `fail`s the key
/// with the error message; never attempts a refetch-based confirmation for
/// a mutation that never actually fired.
@MainActor @Test func removeHostFailureFailsTheKeyWithTheErrorMessage() async {
    let pendingActions = PendingActions()
    let store = makeStore(
        fetchHosts: { [hostRow(id: "local"), hostRow(id: "mini", name: "mini")] },
        postRemoveHost: { _ in throw StubError(description: "cannot remove the built-in local host") },
        pendingActions: pendingActions
    )
    await store.activate()

    await store.removeHost(id: "mini")

    let key = ActionKey("mini", .remove)
    guard case .failed(let message) = pendingActions.state(key) else {
        Issue.record("expected .failed, got \(pendingActions.state(key))")
        return
    }
    #expect(message.contains("cannot remove the built-in local host"))
    // The failed DELETE must not have silently dropped "mini" from the
    // in-memory hosts block either — nothing removed it server-side.
    #expect(store.hosts.value?.map(\.id) == ["local", "mini"])
}

// MARK: - Reconcile loop (activate + deactivate)

/// Mirrors `HostsFooterStoreTests`' own note: the loop's *timing* isn't
/// tested (no sleeping 60s in a unit test), only that `activate()` performs
/// an immediate load and `deactivate()` is safe to call (idempotent,
/// doesn't crash) whether or not a loop is running.
@MainActor @Test func deactivateIsSafeBeforeAndAfterActivate() async {
    let store = makeStore()
    store.deactivate() // never activated — must not crash
    await store.activate()
    store.deactivate()
    store.deactivate() // idempotent
}
