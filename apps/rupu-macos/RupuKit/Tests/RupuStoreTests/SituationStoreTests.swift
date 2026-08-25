import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test infra (duplicated from `DashboardStoreTests.swift`/
// `ActivityStoreTests.swift` — see those files' own header comments: a
// `private` type/function is file-scoped, so it isn't visible here even
// though every file lives in the same `RupuStoreTests` target; re-declared
// locally rather than widening another file's access just for this one.
// Same generation-token isolation `DashboardStubURLProtocol` documents in
// full — every test in a full-suite `swift test` run shares this one
// process-wide `URLProtocol` subclass, so a straggling request from an
// already-ended test must never land against (or get counted by) the next
// one's handler.

final class SituationStubURLProtocol: URLProtocol {
    private static let generationHeader = "X-Situation-Stub-Generation"
    private static let lock = NSLock()
    nonisolated(unsafe) private static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) private static var currentGeneration = 0

    static func session() -> URLSession {
        let generation = lock.withLock { currentGeneration }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [SituationStubURLProtocol.self]
        config.httpAdditionalHeaders = [generationHeader: String(generation)]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        lock.withLock {
            currentGeneration += 1
            self.handler = handler
        }
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

/// Fakes `SituationStore`'s `signalsFactory` seam — each `activate()` call
/// needs a *fresh* `AsyncStream` (see `SituationStore.restartStream`), so
/// this hands back a new one per `factory()` call while keeping every
/// continuation reachable for the test to drive directly. Same shape as
/// `DashboardStoreTests`'/`ActivityStoreTests`' own private copies.
private final class SignalsFactoryBox: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [AsyncStream<StreamSignal<CPEvent>>.Continuation] = []

    func factory() -> AsyncStream<StreamSignal<CPEvent>> {
        let (stream, continuation) = AsyncStream<StreamSignal<CPEvent>>.makeStream()
        lock.withLock { continuations.append(continuation) }
        return stream
    }

    var latest: AsyncStream<StreamSignal<CPEvent>>.Continuation {
        lock.withLock { continuations[continuations.count - 1] }
    }
}

/// Thread-safe call counter — same rationale as `DashboardStoreTests`'s own
/// `HitCounter` (file-scoped `private`, so re-declared here rather than
/// shared): lets a `@Sendable` stub handler count/branch on request order
/// without a `var` capture, which strict concurrency checking forbids.
private final class HitCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    /// Increments and returns the NEW count (1 on the first call).
    func incrementAndGet() -> Int { lock.withLock { v += 1; return v } }
}

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

@Suite(.serialized)
struct SituationStoreTests {
    private static func encodeEvents(_ rows: [[String: Any]]) -> Data {
        // swiftlint-safe: JSONSerialization over hand-built dictionaries —
        // simplest way to produce a valid flat `{ts, pos, type, ...}` event
        // row (`CPEventRow`'s decode shape — see `RupuAPI/Models.swift`)
        // without hand-escaping JSON strings.
        try! JSONSerialization.data(withJSONObject: rows) // swiftlint:disable:this force_try
    }

    private static func emptyFindingsJSON() -> Data {
        Data(#"{"findings":[],"summary":{"total":0,"critical":0,"high":0,"medium":0,"low":0,"info":0}}"#.utf8)
    }

    private static func emptyProjectsJSON() -> Data { Data("[]".utf8) }

    private static func emptyDashboardJSON() -> Data {
        Data(
            """
            {"hosts":[],"findings_partial":false,"cycles_partial":false,"fleet_partial":false,
             "active":{"running":0,"awaiting_approval":0,"paused":0,"pending":0},"active_longest":null,
             "terminal_buckets":[],"throughput_buckets":[],"cycles":{"total":0,"clean":null,"with_failures":null},
             "findings_open":null,
             "fleet":{"repos":null,"providers_configured":null,"providers_unhealthy":null,"autoflows_enabled":null,\
            "autoflows_disabled":null,"workers":null,"claims_active":null,"issues_pending":null,"issues_open":null,\
            "issues_capped":false,"inventory_captured_at":null},
             "captured_at":null}
            """.utf8
        )
    }

    /// Routes the three aggregate poll endpoints to empty-but-valid
    /// responses, so a test focused on the event backlog doesn't need to
    /// restate them. Returns `nil` for any other path so the caller's own
    /// handler can take over (e.g. `/api/events`).
    private static func defaultAggregateResponse(_ req: URLRequest) -> (status: Int, body: Data)? {
        switch req.url?.path {
        case "/api/findings": (200, emptyFindingsJSON())
        case "/api/projects": (200, emptyProjectsJSON())
        case "/api/dashboard": (200, emptyDashboardJSON())
        default: nil
        }
    }

    @MainActor
    private func makeStore(
        maxEventRows: Int = 5_000,
        respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)
    ) -> (store: SituationStore, box: SignalsFactoryBox) {
        SituationStubURLProtocol.reset(handler: respond)
        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: SituationStubURLProtocol.session()
        )
        let box = SignalsFactoryBox()
        let store = SituationStore(client: client, signalsFactory: { box.factory() }, maxEventRows: maxEventRows)
        return (store, box)
    }

    // MARK: - Step 1 requirements

    /// Backfill delivers one `run_paused` row; the live tail then replays
    /// the SAME event content (a reconnect replaying `events.jsonl` from
    /// offset 0 — see `SituationStore`'s doc comment) — must render once,
    /// not twice.
    @MainActor @Test func backfillAndLiveEventFoldDedupsSameEventRenderedOnce() async {
        // Encoded to `Data` up front (not captured as `[String: Any]`) — a
        // dictionary literal isn't `Sendable`, and `makeStore`'s `respond`
        // closure is `@escaping @Sendable`.
        let backfillBody = Self.encodeEvents([["ts": 1_000, "pos": 0, "type": "run_paused", "run_id": "run-1"]])
        let (store, box) = makeStore { req in
            if let hit = Self.defaultAggregateResponse(req) { return hit }
            guard req.url?.path == "/api/events" else { return (200, Data("[]".utf8)) }
            return (200, backfillBody)
        }

        await store.activate()
        #expect(store.eventRows.count == 1, "backfill should have delivered the one history row")
        #expect(store.eventRows.first?.event == .runPaused(runID: "run-1"))

        box.latest.yield(.event(.runPaused(runID: "run-1")))
        try? await Task.sleep(for: .milliseconds(80))

        #expect(store.eventRows.count == 1, "a content-identical replay must not double the backlog")

        store.deactivate()
        box.latest.finish()
    }

    /// Five DISTINCT live events (never seen in backfill) against a cap of
    /// 3: every one is processed (no false-positive dedup), but the backlog
    /// never exceeds the cap and keeps the most-recently-arrived rows —
    /// same `maxEventRows` mechanism the production default (5,000,
    /// `Events.tsx`'s `MAX_EVENTS`) uses, just small enough to assert on
    /// directly rather than needing 5,000+ live events.
    @MainActor @Test func capIsEnforcedOnTheLiveBacklog() async {
        let (store, box) = makeStore(maxEventRows: 3) { req in
            if let hit = Self.defaultAggregateResponse(req) { return hit }
            return (200, Data("[]".utf8)) // empty /api/events backfill
        }

        await store.activate()
        #expect(store.eventRows.isEmpty)

        for i in 0..<5 {
            box.latest.yield(.event(.stepFailed(runID: "r", stepID: "s\(i)", error: "e\(i)")))
        }

        await expectEventually("the cap trims the backlog to maxEventRows") {
            store.eventRows.count == 3
        }
        #expect(store.eventRows.count == 3, "cap (3) must hold even though 5 distinct events were pushed")
        #expect(
            store.eventRows.first?.event == .stepFailed(runID: "r", stepID: "s4", error: "e4"),
            "newest-first: the retained rows are the three most recently pushed"
        )

        store.deactivate()
        box.latest.finish()
    }

    /// Review fix round 1, ruling 1 (HIGH): a reconnect's resnapshot
    /// (`StreamLifecycle`'s own "resnapshot on any connect after an
    /// earlier signal" rule) must MERGE the fresh backfill page into
    /// `eventRows`, not replace it wholesale — the first pass at this
    /// collapsed up to 5,000 accumulated rows down to the last 200 on
    /// every SSE reconnect. Accumulates past the 200-row page size (one
    /// backfill row + one live-only row), simulates a reconnect, and
    /// asserts BOTH the pre-reconnect tail survives AND the reconnect
    /// page's genuinely-new "gap" row lands.
    @MainActor @Test func reconnectMergesFreshBackfillWithoutTruncatingAccumulatedRows() async {
        let firstPageRow: [String: Any] = ["ts": 1_000, "pos": 0, "type": "run_paused", "run_id": "run-first-page"]
        let firstPageBody = Self.encodeEvents([firstPageRow])
        // The reconnect's fresh backfill page: one row already known (must
        // be a no-op) alongside one genuinely new "gap" row — content that
        // arrived while this store was offline, which a reconnect backfill
        // is exactly meant to recover (strictly better than the web, which
        // has no re-backfill on reconnect at all).
        let gapRow: [String: Any] = ["ts": 5_000, "pos": 0, "type": "run_paused", "run_id": "run-gap"]
        let secondPageBody = Self.encodeEvents([gapRow, firstPageRow])

        let eventsRequestCount = HitCounter()
        let (store, box) = makeStore { req in
            if let hit = Self.defaultAggregateResponse(req) { return hit }
            guard req.url?.path == "/api/events" else { return (200, Data("[]".utf8)) }
            return (200, eventsRequestCount.incrementAndGet() == 1 ? firstPageBody : secondPageBody)
        }

        await store.activate()
        #expect(store.eventRows.count == 1)

        box.latest.yield(.event(.stepFailed(runID: "r", stepID: "live-only", error: "e")))
        await expectEventually("the live-only event lands") { store.eventRows.count == 2 }

        // Simulate a reconnect: `.connection(false)` then a SECOND
        // `.connection(true)` is what `StreamLifecycle` treats as a real
        // reconnect (see that type's doc comment) — the shape that
        // triggers `resnapshot()`.
        box.latest.yield(.connection(false))
        box.latest.yield(.connection(true))

        await expectEventually("the reconnect backfill's gap row merges in") {
            store.eventRows.contains { $0.event == .runPaused(runID: "run-gap") }
        }

        #expect(
            store.eventRows.count == 3,
            "the live-only row and the original backfill row must both survive the reconnect merge"
        )
        #expect(store.eventRows.contains { $0.event == .runPaused(runID: "run-first-page") })
        #expect(store.eventRows.contains { $0.event == .stepFailed(runID: "r", stepID: "live-only", error: "e") })

        store.deactivate()
        box.latest.finish()
    }

    /// Review fix round 1, ruling 2: the incremental `seenEventKeys` dedup
    /// ledger must stay in exact sync with `eventRows` — a row trimmed off
    /// by the cap is forgotten, not permanently blacklisted, so the SAME
    /// content re-arriving later (a legitimate later occurrence, not a
    /// stale reconnect replay) is welcomed back as new rather than
    /// silently dropped forever.
    @MainActor @Test func trimmedRowsAreForgottenSoTheSameContentCanReappearAsNewAfterEviction() async {
        let (store, box) = makeStore(maxEventRows: 2) { req in
            if let hit = Self.defaultAggregateResponse(req) { return hit }
            return (200, Data("[]".utf8))
        }
        await store.activate()

        box.latest.yield(.event(.stepFailed(runID: "r", stepID: "s0", error: "e0")))
        await expectEventually("s0 lands") { store.eventRows.count == 1 }

        box.latest.yield(.event(.stepFailed(runID: "r", stepID: "s1", error: "e1")))
        box.latest.yield(.event(.stepFailed(runID: "r", stepID: "s2", error: "e2")))
        await expectEventually("s0 is evicted by the cap (maxEventRows: 2)") {
            store.eventRows.count == 2
                && !store.eventRows.contains { $0.event == .stepFailed(runID: "r", stepID: "s0", error: "e0") }
        }

        box.latest.yield(.event(.stepFailed(runID: "r", stepID: "s0", error: "e0")))
        await expectEventually("the evicted content is welcomed back as new, not dropped as a duplicate") {
            store.eventRows.first?.event == .stepFailed(runID: "r", stepID: "s0", error: "e0")
        }

        store.deactivate()
        box.latest.finish()
    }

    /// The 60s aggregate poll (findings/projects/dashboard) fires once
    /// immediately as part of `activate()` — the screen must never render
    /// blank vitals/roster while waiting out the first full interval.
    @MainActor @Test func activatePopulatesFindingsProjectsAndDashboardImmediately() async {
        let (store, _) = makeStore { req in
            if let hit = Self.defaultAggregateResponse(req) { return hit }
            return (200, Data("[]".utf8))
        }

        await store.activate()

        #expect(store.findingsSummary?.total == 0)
        #expect(store.projects.isEmpty)
        #expect(store.dashboard != nil, "the dashboard poll must have landed synchronously within activate()")

        store.deactivate()
    }

    /// `deactivate()` must stop the live tail outright, not merely leave it
    /// running unobserved — a signal yielded AFTER `deactivate()` must never
    /// be applied. Relies on `StreamLifecycle`'s own `[weak self]`
    /// unwind-on-next-tick contract (see that type's doc comment): dropping
    /// `SituationStore.lifecycle`'s only strong reference deallocates the
    /// `StreamLifecycle` synchronously, so the still-suspended consumer
    /// task's next `guard let self else { return }` exits without calling
    /// `apply`.
    @MainActor @Test func deactivateStopsTheLiveStream() async {
        let (store, box) = makeStore { req in
            if let hit = Self.defaultAggregateResponse(req) { return hit }
            return (200, Data("[]".utf8))
        }

        await store.activate()
        #expect(store.eventRows.isEmpty)

        store.deactivate()
        box.latest.yield(.event(.runPaused(runID: "run-after-deactivate")))
        try? await Task.sleep(for: .milliseconds(100))

        #expect(store.eventRows.isEmpty, "deactivate() must stop the stream, not just leave it running unobserved")
        box.latest.finish()
    }

    // MARK: - Mutations

    /// `approve(runID:stepID:)` begins a gate-scoped pending key immediately
    /// and confirms it only once the store's own live tail observes the run
    /// leaving `.awaiting` — never off the POST response alone (Phase 3's
    /// pending-state contract, CLAUDE.md rule 9).
    @MainActor @Test func approveBeginsPendingAndConfirmsOnlyOnceTheLiveTailObservesTheRunLeavingAwaiting() async {
        let (store, box) = makeStore { req in
            if let hit = Self.defaultAggregateResponse(req) { return hit }
            if req.url?.path == "/api/events" { return (200, Data("[]".utf8)) }
            if req.url?.path.hasSuffix("/approve") == true { return (200, Data(#"{"ok":true}"#.utf8)) }
            return (200, Data("{}".utf8))
        }
        await store.activate()

        let key = ActionKey.gate(runID: "run-1", stepID: "step-a", verb: .approve)
        #expect(store.pendingActions.state(key) == .idle)

        await store.approve(runID: "run-1", stepID: "step-a")
        guard case .pending = store.pendingActions.state(key) else {
            Issue.record("expected .pending immediately after approve()'s POST resolves")
            store.deactivate()
            return
        }

        // Still pending — a status patch for a DIFFERENT run must not
        // confirm it: `PendingActions.resolve(runID:observedStatus:)` only
        // touches keys whose `entityID` is exactly `runID` or the
        // `"\(runID):"`-prefixed gate form (see that method's doc comment),
        // so "run-2" completing must leave "run-1:step-a" untouched.
        box.latest.yield(.event(.runCompleted(runID: "run-2", status: "completed", finishedAt: "2026-08-25T00:00:00Z")))
        try? await Task.sleep(for: .milliseconds(60))
        guard case .pending = store.pendingActions.state(key) else {
            Issue.record("an unrelated run's event must not confirm this key")
            store.deactivate()
            return
        }

        // The run actually leaves the gate — confirms. `ActivityDelta.reduce`
        // only maps RUN-level lifecycle events to a `.statusPatch` (a
        // step-level `.stepResumed` reduces to `.none` — it's step-granular,
        // not run-granular; see that function's doc comment), so this uses
        // `.runResumed`, not `.stepResumed`.
        box.latest.yield(.event(.runResumed(runID: "run-1")))
        await expectEventually("approve confirms once the run is observed leaving .awaiting") {
            store.pendingActions.state(key) == .confirmed
        }

        store.deactivate()
        box.latest.finish()
    }
}
