import Testing
import Foundation
@testable import RupuMenuBar
import RupuAPI
import RupuStore
import RupuOverview

// MARK: - Test infra
//
// `DashboardStoreTests`'s own `DashboardStubURLProtocol` is `private`,
// file-scoped even under `@testable import` from a DIFFERENT test target
// (`RupuMenuBarTests` here, vs `RupuStoreTests` there) — re-declared here
// rather than widened, same rationale that file's own header comment gives
// for its own re-declaration of `CPClientTests.StubURLProtocol`.

/// Path-routing HTTP stub for `MenuBarStore`'s two polled endpoints
/// (`GET /api/dashboard`, `GET /api/runs`). Routes on `request.url?.path`.
///
/// Generation-token isolation (full-suite flake fix, same shape
/// `DashboardStubURLProtocol` uses): `reset(handler:)` mints a fresh
/// generation and every session `session()` hands out is stamped with
/// whichever generation was current at that call — a request whose stamped
/// generation doesn't match `currentGeneration` at the time it's served
/// belongs to an already-ended test and is failed as `.cancelled` rather
/// than corrupting the new test's hit counts.
///
/// `updateHandler(_:)` is this file's own addition beyond
/// `DashboardStubURLProtocol`'s shape: it swaps `handler` WITHOUT bumping
/// the generation, for the one test here (`failedPollKeepsLastGoodCounts`)
/// that needs to change server behavior partway through a single
/// still-running poll loop — a `reset(handler:)` there would invalidate the
/// generation stamped into the `URLSession` the loop is already using,
/// turning every subsequent request into a spurious `.cancelled` rather
/// than the deliberate 500 the test wants to exercise.
final class MenuBarStubURLProtocol: URLProtocol {
    private static let generationHeader = "X-MenuBar-Stub-Generation"
    private static let lock = NSLock()
    nonisolated(unsafe) private static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) private static var pathHits: [String: Int] = [:]
    nonisolated(unsafe) private static var currentGeneration = 0

    static func session() -> URLSession {
        let generation = lock.withLock { currentGeneration }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [MenuBarStubURLProtocol.self]
        config.httpAdditionalHeaders = [generationHeader: String(generation)]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        lock.withLock {
            currentGeneration += 1
            pathHits = [:]
            self.handler = handler
        }
    }

    static func updateHandler(_ handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        lock.withLock { self.handler = handler }
    }

    static func hits(_ path: String) -> Int {
        lock.withLock { pathHits[path, default: 0] }
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let requestGeneration = request.value(forHTTPHeaderField: Self.generationHeader).flatMap(Int.init) ?? -1

        let (isCurrent, activeHandler): (Bool, (@Sendable (URLRequest) -> (status: Int, body: Data))?) = Self.lock.withLock {
            (requestGeneration == Self.currentGeneration, Self.handler)
        }
        guard isCurrent else {
            client?.urlProtocol(self, didFailWithError: URLError(.cancelled))
            return
        }

        if let path = request.url?.path {
            Self.lock.withLock { Self.pathHits[path, default: 0] += 1 }
        }
        guard let handler = activeHandler else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let (status, body) = handler(request)
        let response = HTTPURLResponse(
            url: request.url!, statusCode: status, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"]
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

/// De-flakes "wait for something async to land" — same shape
/// `DashboardStoreTests`'s own copy uses.
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

/// Best-effort settle after `store.deactivate()` — gives an in-flight
/// request time to land (and, per the stub's generation isolation, be
/// harmlessly rejected as stale) before the next test's `reset(handler:)`
/// rotates the generation again.
@MainActor
private func drainAfterDeactivate(_ duration: Duration = .milliseconds(150)) async {
    try? await Task.sleep(for: duration)
}

private func dashboardJSON(running: Int, awaiting: Int, paused: Int, pending: Int) -> Data {
    let json = """
    {
      "hosts": [],
      "findings_partial": false,
      "cycles_partial": false,
      "fleet_partial": false,
      "active": {"running": \(running), "awaiting_approval": \(awaiting), "paused": \(paused), "pending": \(pending)},
      "active_longest": null,
      "terminal_buckets": [],
      "throughput_buckets": [],
      "cycles": {"total": 0, "clean": null, "with_failures": null},
      "findings_open": null,
      "fleet": {"repos": null, "providers_configured": null, "providers_unhealthy": null, "autoflows_enabled": null, "autoflows_disabled": null, "workers": null, "claims_active": null, "issues_pending": null, "issues_open": null, "issues_capped": false, "inventory_captured_at": null},
      "captured_at": null
    }
    """
    return Data(json.utf8)
}

/// Builds one `ActivityRow` the way `MenuBarStore.pollOnce` actually does —
/// through `APIRunListRow` → `ActivityRow.init(_:APIRunListRow)` — since
/// `ActivityRow` itself exposes no direct memberwise initializer.
private func makeRow(id: String, status: String, startedAt: String) -> ActivityRow {
    ActivityRow(APIRunListRow(
        id: id,
        workflowName: "wf-\(id)",
        status: status,
        startedAt: startedAt,
        finishedAt: nil,
        trigger: "manual",
        usage: APIUsageSummary(inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0, costUSD: nil, priced: false, runs: 1),
        turns: 1,
        durationMS: nil,
        hostID: "local"
    ))
}

// MARK: - `apply` (pure seam)

@MainActor @Test func applyStoresCountsVerbatim() {
    let store = MenuBarStore()
    let counts = APIActiveCounts(running: 2, awaitingApproval: 1, paused: 0, pending: 3)
    store.apply(counts: counts, rows: [], now: Date())

    #expect(store.counts == counts)
    #expect(store.needsYou.isEmpty)
    #expect(store.overflow == 0)
}

/// 3 gates + 4 failed rows = 7 candidates. `deriveNeedsYou` itself caps at
/// 6 (3 gates + the 3 most recent failures, dropping the oldest failure) —
/// `apply` then trims that down to the menu bar's own top-5 (3 gates + the
/// 2 most recent of those 3 failures), folding the extra dropped item into
/// `overflow` alongside `deriveNeedsYou`'s own count of 1.
@MainActor @Test func applyDerivesTop5AndComposesOverflow() {
    let store = MenuBarStore()
    let now = Date()

    let gates = (1...3).map { i in
        makeRow(id: "gate\(i)", status: "awaiting_approval", startedAt: iso(now.addingTimeInterval(TimeInterval(-i * 3600))))
    }
    // Newest-first by construction: fail1 is the most recent, fail4 the
    // oldest — `deriveNeedsYou`'s failed-side ordering keeps the newest.
    let failures = (1...4).map { i in
        makeRow(id: "fail\(i)", status: "failed", startedAt: iso(now.addingTimeInterval(TimeInterval(-i * 60))))
    }

    store.apply(counts: APIActiveCounts(running: 0, awaitingApproval: 3, paused: 0, pending: 0), rows: gates + failures, now: now)

    #expect(store.needsYou.count == 5)
    #expect(store.needsYou.filter { $0.kind == .gate }.count == 3)
    let failedIDsShown = store.needsYou.filter { $0.kind == .failedRun }.map(\.row.id)
    #expect(failedIDsShown == ["fail1", "fail2"], "the two most recent failures, in newest-first order")
    #expect(store.overflow == 2, "deriveNeedsYou's own 1 (7 candidates, cap 6) + this store's own trim from 6 to 5")
}

@MainActor @Test func applyWithNothingAwaitingOrFailedYieldsEmptyQueue() {
    let store = MenuBarStore()
    let rows = [makeRow(id: "r1", status: "running", startedAt: iso(Date()))]
    store.apply(counts: APIActiveCounts(running: 1, awaitingApproval: 0, paused: 0, pending: 0), rows: rows, now: Date())

    #expect(store.needsYou.isEmpty)
    #expect(store.overflow == 0)
}

private func iso(_ date: Date) -> String {
    ISO8601DateFormatter().string(from: date)
}

// MARK: - Poll loop
//
// `.serialized` (same rationale `DashboardStoreTests`/`ActivityStoreTests`/
// `LauncherStoreTests` already give): `MenuBarStubURLProtocol`'s `handler`/
// `pathHits`/`currentGeneration` are process-wide statics. Swift Testing may
// run separate `@Test` functions concurrently even though each is
// `@MainActor` — an `await Task.sleep` inside one test yields the actor,
// letting another test's synchronous prelude (including its own
// `reset(handler:)`, which bumps the generation) run in between. Without
// `.serialized`, a bump from a DIFFERENT test can permanently strand an
// already-in-flight test's session on a generation that will never be
// current again (the header is stamped once, at `session()` call time, and
// never changes afterward) — exactly the hang this suite saw before this
// trait was added.
@Suite(.serialized)
struct MenuBarStorePollTests {

@MainActor @Test func activateIsIdempotentAcrossRepeatedCalls() async {
    MenuBarStubURLProtocol.reset { req in
        guard let url = req.url else { return (200, Data("{}".utf8)) }
        if url.path == "/api/dashboard" {
            return (200, dashboardJSON(running: 1, awaiting: 0, paused: 0, pending: 0))
        }
        if url.path == "/api/runs" {
            return (200, Data("[]".utf8))
        }
        return (404, Data())
    }
    let client = CPClient(
        config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
        session: MenuBarStubURLProtocol.session()
    )
    // A long poll interval — long enough that a real second tick can never
    // land inside this test's timeout, so any hit count above 1 can only
    // mean the second `activate(client:)` call spawned an extra loop, not
    // that a legitimate second tick raced the assertion.
    let store = MenuBarStore(pollInterval: .seconds(60))

    store.activate(client: client)
    store.activate(client: client)

    await expectEventually("the first poll lands") {
        store.counts?.running == 1
    }
    // A buggy second loop would fire its own immediate poll right away, not
    // 60s later — a short settle window is enough to catch it.
    try? await Task.sleep(for: .milliseconds(150))

    #expect(MenuBarStubURLProtocol.hits("/api/dashboard") == 1)
    #expect(MenuBarStubURLProtocol.hits("/api/runs") == 1)

    store.deactivate()
    await drainAfterDeactivate()
}

@MainActor @Test func failedPollKeepsLastGoodCounts() async {
    MenuBarStubURLProtocol.reset { req in
        guard let url = req.url else { return (200, Data("{}".utf8)) }
        if url.path == "/api/dashboard" {
            return (200, dashboardJSON(running: 2, awaiting: 1, paused: 0, pending: 0))
        }
        if url.path == "/api/runs" {
            return (200, Data("[]".utf8))
        }
        return (404, Data())
    }
    let client = CPClient(
        config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
        session: MenuBarStubURLProtocol.session()
    )
    let store = MenuBarStore(pollInterval: .milliseconds(40))

    store.activate(client: client)
    await expectEventually("the first successful poll lands") {
        store.counts?.running == 2
    }

    // Every subsequent poll now fails outright.
    MenuBarStubURLProtocol.updateHandler { _ in (500, Data("{}".utf8)) }

    // Comfortably past at least one more 40ms tick.
    try? await Task.sleep(for: .milliseconds(200))

    #expect(store.counts?.running == 2, "a failed poll must never blank or alter the last good counts")
    #expect(store.needsYou.isEmpty, "unchanged from the first successful poll's empty runs list")

    store.deactivate()
    await drainAfterDeactivate()
}

@MainActor @Test func deactivateStopsFurtherPolling() async {
    MenuBarStubURLProtocol.reset { req in
        guard let url = req.url else { return (200, Data("{}".utf8)) }
        if url.path == "/api/dashboard" {
            return (200, dashboardJSON(running: 5, awaiting: 0, paused: 0, pending: 0))
        }
        if url.path == "/api/runs" {
            return (200, Data("[]".utf8))
        }
        return (404, Data())
    }
    let client = CPClient(
        config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
        session: MenuBarStubURLProtocol.session()
    )
    let store = MenuBarStore(pollInterval: .milliseconds(30))

    store.activate(client: client)
    await expectEventually("the first poll lands") {
        store.counts?.running == 5
    }
    store.deactivate()
    let hitsAtDeactivate = MenuBarStubURLProtocol.hits("/api/dashboard")

    try? await Task.sleep(for: .milliseconds(150))

    #expect(MenuBarStubURLProtocol.hits("/api/dashboard") == hitsAtDeactivate, "no further ticks after deactivate()")

    await drainAfterDeactivate()
}

}
