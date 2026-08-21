import Testing
import Foundation
@testable import RupuStore
import RupuAPI

/// Path-routing HTTP stub for `ActivityStore`'s four source endpoints.
/// Duplicated from `RupuAPITests.CPClientTests.StubURLProtocol` (that type
/// is `internal` to a different SPM test target, so it isn't visible here —
/// same rationale as `Fixtures.swift`'s local copy of `FixtureLoader`).
/// Routes on `request.url?.path` so one instance can serve
/// `/api/runs/workflows`, `/api/runs/agents`, `/api/runs/autoflows/events`,
/// and `/api/sessions` with distinct bodies in the same test. Tests run
/// `.serialized` because `handler`/`requestCount` are class-level state
/// shared across the whole `URLProtocol` subclass.
final class ActivityStubURLProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) static var requestCount = 0

    static func session() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [ActivityStubURLProtocol.self]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        requestCount = 0
        self.handler = handler
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        ActivityStubURLProtocol.requestCount += 1
        guard let handler = ActivityStubURLProtocol.handler else {
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

/// Thread-safe append-only log — same rationale as
/// `StreamLifecycleTests.EventLog` and `PagedSnapshotTests.CountBox`: a
/// plain captured `var` can't cross into the `@Sendable` request-handler
/// closure under Swift 6 strict concurrency.
private final class EventLog: @unchecked Sendable {
    private let lock = NSLock()
    private var entries: [String] = []
    func record(_ entry: String) { lock.withLock { entries.append(entry) } }
    var snapshot: [String] { lock.withLock { entries } }
}

private final class FlagBox: @unchecked Sendable {
    private let lock = NSLock()
    private var v = false
    var value: Bool {
        get { lock.withLock { v } }
        set { lock.withLock { v = newValue } }
    }
}

private final class CountBox: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    /// Increments and returns whether this was the first increment.
    func incrementAndCheckFirst() -> Bool {
        lock.withLock {
            count += 1
            return count == 1
        }
    }
}

/// Thread-safe call counter — same rationale as `CountBox` above, just a
/// plain running total rather than a "was this the first?" check.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    func increment() { lock.withLock { v += 1 } }
    var value: Int { lock.withLock { v } }
}

/// Fakes `ActivityStore`'s `signalsFactory` seam: each call builds and
/// records a brand-new `AsyncStream<StreamSignal<CPEvent>>` +
/// `Continuation` pair, so a test can assert how many times `activate()`
/// actually rebuilt the stream (`callCount`) and reach into any specific
/// cycle's continuation (`continuation(at:)`/`latest`) to drive it.
private final class SignalsFactoryBox: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [AsyncStream<StreamSignal<CPEvent>>.Continuation] = []

    func factory() -> AsyncStream<StreamSignal<CPEvent>> {
        let (stream, continuation) = AsyncStream<StreamSignal<CPEvent>>.makeStream()
        lock.withLock { continuations.append(continuation) }
        return stream
    }

    var callCount: Int { lock.withLock { continuations.count } }

    func continuation(at index: Int) -> AsyncStream<StreamSignal<CPEvent>>.Continuation {
        lock.withLock { continuations[index] }
    }

    /// The continuation from the most recent `factory()` call — the one
    /// backing whichever stream the store's current `activate()` cycle is
    /// actually consuming.
    var latest: AsyncStream<StreamSignal<CPEvent>>.Continuation {
        lock.withLock { continuations[continuations.count - 1] }
    }
}

@Suite(.serialized)
struct ActivityStoreTests {
    // API row types are `Decodable` only (never round-tripped back to the
    // server), so the stub bodies below are hand-written JSON text matching
    // each type's `CodingKeys`, not `JSONEncoder` output.
    private static let usageJSON = #"{"cached_tokens":0,"cost_usd":null,"input_tokens":0,"output_tokens":0,"priced":false,"runs":0,"total_tokens":0}"#

    private static func runListRowJSON(id: String, startedAt: String, status: String, durationMS: String = "null", turns: Int = 1) -> String {
        #"{"duration_ms":\#(durationMS),"finished_at":null,"host_id":"local","id":"\#(id)","started_at":"\#(startedAt)","status":"\#(status)","trigger":"cron","turns":\#(turns),"usage":\#(usageJSON),"workflow_name":"nightly-health"}"#
    }

    private static func agentRowJSON(id: String, startedAt: String, status: String = "running", durationMS: String = "null") -> String {
        #"{"agent":"rupuso","duration_ms":\#(durationMS),"host_id":"local","run_id":"\#(id)","session_id":null,"source":"session","started_at":"\#(startedAt)","status":"\#(status)","transcript_path":null,"trigger_source":"session_turn","turns":1,"usage":\#(usageJSON)}"#
    }

    private static func autoflowRowJSON(id: String, at: String, status: String = "running", runID: String? = nil) -> String {
        let runIDJSON = runID.map { "\"\($0)\"" } ?? "null"
        return #"{"at":"\#(at)","cycle_id":"cycle-1","detail":null,"duration_ms":null,"event_id":"\#(id)","host_id":"local","issue_display_ref":null,"kind":"run_launched","run_id":\#(runIDJSON),"status":"\#(status)","turns":1,"usage":\#(usageJSON),"worker_name":"worker-a","workflow":"nightly-health"}"#
    }

    private static func sessionRowJSON(id: String, createdAt: String) -> String {
        #"{"active_run_id":null,"agent_name":"rupuso","created_at":"\#(createdAt)","host_id":"local","last_error":null,"model":"claude","provider_name":"anthropic","scope":"active","session_id":"\#(id)","target":null,"total_tokens_cached":0,"total_tokens_in":0,"total_tokens_out":0,"total_turns":1,"updated_at":"\#(createdAt)","usage":null,"workspace_id":"ws-1"}"#
    }

    /// One row per source, each with a distinct `startedAt`/`at`/`createdAt`
    /// so merge-sort order is unambiguous: agent (12:00) > autoflow (11:00)
    /// > workflow (10:00) > session (09:00).
    private static func fourSourceBody(for path: String) -> Data {
        switch path {
        case "/api/runs/workflows":
            return Data("[\(runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: "running"))]".utf8)
        case "/api/runs/agents":
            return Data("[\(agentRowJSON(id: "run-ag-1", startedAt: "2026-08-20T12:00:00Z", status: "completed", durationMS: "5000"))]".utf8)
        case "/api/runs/autoflows/events":
            return Data("[\(autoflowRowJSON(id: "evt-1", at: "2026-08-20T11:00:00Z", status: "failed", runID: "run-af-1"))]".utf8)
        case "/api/sessions":
            return Data("[\(sessionRowJSON(id: "sess-1", createdAt: "2026-08-20T09:00:00Z"))]".utf8)
        default:
            return Data("[]".utf8)
        }
    }

    @MainActor
    private func makeStore(
        debounceInterval: Duration = .milliseconds(500),
        respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data) = { req in
            (200, ActivityStoreTests.fourSourceBody(for: req.url?.path ?? ""))
        }
    ) -> (store: ActivityStore, box: SignalsFactoryBox) {
        ActivityStubURLProtocol.reset(handler: respond)
        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: ActivityStubURLProtocol.session()
        )
        let box = SignalsFactoryBox()
        let store = ActivityStore(client: client, signalsFactory: { box.factory() }, debounceInterval: debounceInterval)
        return (store, box)
    }

    // (a) activate(.all) merges the four fixture-derived source arrays
    // sorted by date, descending, most-recent first.
    @MainActor @Test func activateAllMergesFourSourcesSortedByStartedAtDescending() async {
        let (store, box) = makeStore()
        await store.activate(kind: .all)

        #expect(store.rows.map(\.id) == ["run-ag-1", "evt-1", "run-wf-1", "sess-1"])
        #expect(store.rows.map(\.kind) == [.agent, .autoflow, .workflow, .session])

        store.deactivate()
        box.latest.finish()
    }

    // (b) statusFilter {.failed} filters the merged view, synchronously,
    // with no refetch needed (it's a client-side narrowing of rows already
    // in memory).
    @MainActor @Test func statusFilterNarrowsMergedRows() async {
        let (store, box) = makeStore()
        await store.activate(kind: .all)
        #expect(store.rows.count == 4)
        let requestsAfterActivate = ActivityStubURLProtocol.requestCount

        store.statusFilter = [.failed]

        #expect(store.rows.map(\.id) == ["evt-1"])
        #expect(store.rows.allSatisfy { $0.status == .failed })
        #expect(ActivityStubURLProtocol.requestCount == requestsAfterActivate)

        store.statusFilter = []
        #expect(store.rows.count == 4)

        store.deactivate()
        box.latest.finish()
    }

    // (c) liveTail off: a runStarted event increments pendingNewRuns and
    // leaves rows untouched; applyPendingRefresh() refetches and zeroes it.
    @MainActor @Test func liveTailOffCountsPendingNewRunsWithoutMutatingRows() async {
        let (store, box) = makeStore()
        store.liveTail = false
        await store.activate(kind: .all)
        let before = store.rows
        let requestsAfterActivate = ActivityStubURLProtocol.requestCount

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runStarted(runID: "run-new-1", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:00Z")))
        try? await Task.sleep(for: .milliseconds(30))

        #expect(store.pendingNewRuns == 1)
        #expect(store.rows == before)
        // No refetch happened purely from the newRun delta while liveTail is off.
        #expect(ActivityStubURLProtocol.requestCount == requestsAfterActivate)

        await store.applyPendingRefresh()
        #expect(store.pendingNewRuns == 0)
        #expect(ActivityStubURLProtocol.requestCount > requestsAfterActivate)

        store.deactivate()
        box.latest.finish()
    }

    // (d) liveTail on: a runCompleted event for a visible row patches its
    // status in place, without a refetch.
    @MainActor @Test func liveTailOnPatchesVisibleRowStatusInPlaceWithoutRefetch() async {
        let (store, box) = makeStore()
        store.liveTail = true
        await store.activate(kind: .all)
        let requestsAfterActivate = ActivityStubURLProtocol.requestCount
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .running)

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "2026-08-20T10:05:00Z")))
        try? await Task.sleep(for: .milliseconds(30))

        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed)
        // The other three rows are untouched by the patch.
        #expect(store.rows.filter { $0.id != "run-wf-1" }.count == 3)
        #expect(ActivityStubURLProtocol.requestCount == requestsAfterActivate)

        store.deactivate()
        box.latest.finish()
    }

    // (e) deactivate() stops the stream: the scripted continuation's
    // consumer task is torn down (observed via onTermination).
    @MainActor @Test func deactivateStopsTheStream() async {
        let (store, box) = makeStore()
        let terminated = FlagBox()

        await store.activate(kind: .all)
        box.latest.onTermination = { _ in terminated.value = true }
        box.latest.yield(.connection(true))
        try? await Task.sleep(for: .milliseconds(20))
        #expect(terminated.value == false)

        store.deactivate()
        try? await Task.sleep(for: .milliseconds(20))

        #expect(terminated.value == true)

        // Events yielded after deactivate() are never applied.
        let before = store.rows
        box.latest.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "x")))
        try? await Task.sleep(for: .milliseconds(20))
        #expect(store.rows == before)

        box.latest.finish()
    }

    // (f) reconnect (scripted disconnect+connect) triggers a refresh before
    // further deltas apply. The workflow endpoint's second response (the
    // resnapshot) is deliberately slow and returns a materially different
    // row than the first, so the final merged state can only be explained
    // by resnapshot completing (new row content visible) before the
    // trailing event's patch is layered on top of it.
    @MainActor @Test func reconnectResnapshotsBeforeApplyingFurtherDeltas() async {
        let log = EventLog()
        let workflowCallCount = CountBox()
        let (store, box) = makeStore { req in
            guard req.url?.path == "/api/runs/workflows" else {
                return (200, Self.fourSourceBody(for: req.url?.path ?? ""))
            }
            let isFirstCall = workflowCallCount.incrementAndCheckFirst()
            if isFirstCall {
                log.record("initial-fetch")
                let row = Self.runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: "running")
                return (200, Data("[\(row)]".utf8))
            } else {
                log.record("resnapshot-fetch")
                // Simulate the resnapshot's REST round trip being slow, to
                // widen the window a bug (event applied before resnapshot
                // completes) would need to fall into.
                Thread.sleep(forTimeInterval: 0.03)
                let row = Self.runListRowJSON(
                    id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: "running",
                    durationMS: "9999", turns: 2
                )
                log.record("resnapshot-fetch-done")
                return (200, Data("[\(row)]".utf8))
            }
        }
        store.liveTail = true
        await store.activate(kind: .all)
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.durationMS == nil)

        // Reconnect (a connect that is not the pristine first signal) then
        // an event, back to back with zero delay — matching Task 4's own
        // adversarial StreamLifecycle test shape.
        box.latest.yield(.connection(false))
        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "2026-08-20T10:05:00Z")))
        try? await Task.sleep(for: .milliseconds(80))

        // The resnapshot's fresh row content (durationMS: 9999) must be
        // visible, proving the refresh actually ran and completed ...
        let final = store.rows.first(where: { $0.id == "run-wf-1" })
        #expect(final?.durationMS == 9999)
        // ... with the trailing event's patch layered on top of it, not lost.
        #expect(final?.status == .completed)
        #expect(log.snapshot == ["initial-fetch", "resnapshot-fetch", "resnapshot-fetch-done"])

        store.deactivate()
        box.latest.finish()
    }

    // (g) review fix #1: activate -> deactivate -> activate must fully
    // rebuild the stream (a fresh `signalsFactory()` call each time), not
    // leave the second `activate()` a silent no-op. Proven two ways: the
    // factory is called twice, and an event fed through the *second*
    // stream still reaches `apply` (patches a row) — not just that a
    // stream object exists, but that it's actually wired up and live.
    @MainActor @Test func activateDeactivateActivateRebuildsStreamAndEventsStillReachApply() async {
        let (store, box) = makeStore()
        store.liveTail = true

        await store.activate(kind: .all)
        #expect(box.callCount == 1)
        box.continuation(at: 0).yield(.connection(true))
        box.continuation(at: 0).yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "x")))
        try? await Task.sleep(for: .milliseconds(30))
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed)

        store.deactivate()
        box.continuation(at: 0).finish()

        // Re-fetching on the second activate() resets the row to the
        // stub's REST truth ("running") — patches are transient, not
        // persisted into the source of truth.
        await store.activate(kind: .all)
        #expect(box.callCount == 2)
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .running)

        // An event fed through the *second* factory-built stream must
        // still reach `apply` — proving the second `activate()` isn't a
        // no-op against an already-consumed stream.
        box.continuation(at: 1).yield(.connection(true))
        box.continuation(at: 1).yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "x")))
        try? await Task.sleep(for: .milliseconds(30))
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed)

        store.deactivate()
        box.continuation(at: 1).finish()
    }

    // (h) review fix #2: a debounced refresh that collides with some other
    // refresh already in flight on the same source must not be silently
    // dropped — it retries once, after the collision clears. Sized so the
    // *first* debounce fire (headStart + debounceInterval) lands strictly
    // inside the slow in-flight fetch's window, and the *retry*
    // (headStart + 2*debounceInterval) lands strictly after it — see the
    // arithmetic in the comments below.
    @MainActor @Test func debouncedRefreshCollidingWithInFlightRefreshRetriesOnceAfterItCompletes() async {
        let headStartMS = 10
        let debounceMS = 25
        let slowFetchMS = 47 // strictly between (headStart+debounce)=35 and (headStart+2*debounce)=60

        let workflowFetches = Counter()
        let (store, box) = makeStore(debounceInterval: .milliseconds(debounceMS)) { req in
            guard req.url?.path == "/api/runs/workflows" else {
                return (200, Data("[]".utf8))
            }
            workflowFetches.increment()
            Thread.sleep(forTimeInterval: Double(slowFetchMS) / 1000)
            let row = Self.runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: "running")
            return (200, Data("[\(row)]".utf8))
        }
        store.liveTail = true
        await store.activate(kind: .workflows) // fetch #1 (awaited fully — no collision here)
        #expect(workflowFetches.value == 1)

        // Start a second, concurrent, slow refresh — this is the "already
        // in flight" refresh the debounced one will collide with.
        let manualTask = Task { await store.applyPendingRefresh() } // fetch #2, in flight for slowFetchMS

        try? await Task.sleep(for: .milliseconds(headStartMS)) // let manualTask actually start & set inFlight
        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runStarted(runID: "run-new-1", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:00Z")))

        // Wait past: manualTask's fetch, the first (colliding, skipped)
        // debounce fire, and the retry's (successful) fetch.
        try? await Task.sleep(for: .milliseconds(headStartMS + 2 * debounceMS + slowFetchMS + 60))
        _ = await manualTask.value

        // Exactly 3 fetches: activate's, manualTask's, and the debounce
        // retry's. The *first* debounce attempt contributes 0 — it was
        // skipped (in-flight collision) before ever calling `fetch`.
        #expect(workflowFetches.value == 3)

        store.deactivate()
        box.latest.finish()
    }

    // (i) review fix #3: `CPEvent.runStarted` carries no kind tag, so a
    // `.sessions`-scoped store (which can never contain an
    // orchestrator-run row) must not count it toward `pendingNewRuns` —
    // that would promise a pill for a run the sessions view can never
    // show.
    @MainActor @Test func newRunIgnoredWhenKindCannotContainOrchestratorRuns() async {
        let (store, box) = makeStore()
        store.liveTail = false
        await store.activate(kind: .sessions)
        let before = store.rows

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runStarted(runID: "run-new-1", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:00Z")))
        try? await Task.sleep(for: .milliseconds(30))

        #expect(store.pendingNewRuns == 0)
        #expect(store.rows == before)

        store.deactivate()
        box.latest.finish()
    }

    // (j) coverage gap: a burst of several rapid `newRun` deltas while
    // `liveTail` is on must coalesce into exactly one debounced refresh,
    // not one per event.
    @MainActor @Test func burstOfRapidNewRunEventsCoalescesToOneDebouncedRefresh() async {
        let fetches = Counter()
        let (store, box) = makeStore(debounceInterval: .milliseconds(20)) { req in
            guard req.url?.path == "/api/runs/workflows" else {
                return (200, Data("[]".utf8))
            }
            fetches.increment()
            let row = Self.runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: "running")
            return (200, Data("[\(row)]".utf8))
        }
        store.liveTail = true
        await store.activate(kind: .workflows)
        #expect(fetches.value == 1)

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runStarted(runID: "run-a", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:00Z")))
        box.latest.yield(.event(.runStarted(runID: "run-b", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:01Z")))
        box.latest.yield(.event(.runStarted(runID: "run-c", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:02Z")))
        try? await Task.sleep(for: .milliseconds(80))

        // One coalesced refresh from the burst, on top of activate's own.
        #expect(fetches.value == 2)

        store.deactivate()
        box.latest.finish()
    }

    // (l) review fix: a live status patch (`patchRow`) must survive an
    // unrelated `recompute()` — here, toggling `statusFilter`, which calls
    // `recompute()` synchronously from its `didSet`. Before the
    // `statusOverrides` overlay fix, `recompute()` rebuilt `rows` straight
    // from the (unpatched) `PagedSnapshot`s, silently reverting the patch
    // back to "running" the moment anything re-triggered it.
    @MainActor @Test func statusPatchSurvivesRecomputeTriggeredByStatusFilterToggle() async {
        let (store, box) = makeStore()
        await store.activate(kind: .all)
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .running)

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "2026-08-20T10:05:00Z")))
        try? await Task.sleep(for: .milliseconds(30))
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed)

        // Toggling `statusFilter` recomputes `rows` from scratch. (Not
        // asserting the exact filtered set here — the fixture's agent/
        // session rows are already `.completed` by default, independent of
        // this patch — just that the patched row survives the recompute.)
        store.statusFilter = [.completed]
        #expect(store.rows.contains { $0.id == "run-wf-1" })

        store.statusFilter = []
        // The patch must still be visible — not reverted to the
        // snapshot's original "running" truth.
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed)

        store.deactivate()
        box.latest.finish()
    }

    // (m) review fix: the server firehose replays every active run's
    // `events.jsonl` from offset 0 on each (re)connect, so a `.newRun` for a
    // run that's already visible in `rows` must not inflate
    // `pendingNewRuns` — it isn't actually new.
    @MainActor @Test func newRunEventForAlreadyVisibleRowDoesNotInflatePendingNewRuns() async {
        let (store, box) = makeStore()
        store.liveTail = false
        await store.activate(kind: .all)
        #expect(store.rows.contains { $0.id == "run-wf-1" })

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runStarted(runID: "run-wf-1", workflowPath: "p.yml", startedAt: "2026-08-20T10:00:00Z")))
        try? await Task.sleep(for: .milliseconds(30))

        #expect(store.pendingNewRuns == 0)

        store.deactivate()
        box.latest.finish()
    }

    // (k) coverage gap: `loadMore()` advances every active source
    // independently (each keeps its own offset) with no duplicate rows
    // across pages, for a merged `.all` view.
    @MainActor @Test func loadMoreAdvancesEachActiveSourceIndependentlyWithNoDuplicateRows() async {
        let perSourceCount = 55 // > the 50 default page size, so exactly one loadMore() drains the rest
        let workflowRows = (0..<perSourceCount).map { i in
            Self.runListRowJSON(id: "wf-\(i)", startedAt: String(format: "2026-08-20T10:%02d:00Z", i), status: "running")
        }
        let agentRows = (0..<perSourceCount).map { i in
            Self.agentRowJSON(id: "ag-\(i)", startedAt: String(format: "2026-08-20T09:%02d:00Z", i))
        }
        let autoflowRows = (0..<perSourceCount).map { i in
            Self.autoflowRowJSON(id: "af-\(i)", at: String(format: "2026-08-20T08:%02d:00Z", i))
        }
        let sessionRows = (0..<perSourceCount).map { i in
            Self.sessionRowJSON(id: "se-\(i)", createdAt: String(format: "2026-08-20T07:%02d:00Z", i))
        }

        let (store, box) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
            let offset = items.first(where: { $0.name == "offset" }).flatMap { $0.value.flatMap(Int.init) } ?? 0
            let limit = items.first(where: { $0.name == "limit" }).flatMap { $0.value.flatMap(Int.init) } ?? 50
            let source: [String]
            switch url.path {
            case "/api/runs/workflows": source = workflowRows
            case "/api/runs/agents": source = agentRows
            case "/api/runs/autoflows/events": source = autoflowRows
            case "/api/sessions": source = sessionRows
            default: source = []
            }
            guard offset < source.count else { return (200, Data("[]".utf8)) }
            let end = min(offset + limit, source.count)
            return (200, Data(("[" + source[offset..<end].joined(separator: ",") + "]").utf8))
        }

        await store.activate(kind: .all)
        #expect(store.rows.count == 200) // 4 sources x page size 50
        #expect(store.rows.filter { $0.kind == .workflow }.count == 50)
        #expect(store.rows.filter { $0.kind == .agent }.count == 50)
        #expect(store.rows.filter { $0.kind == .autoflow }.count == 50)
        #expect(store.rows.filter { $0.kind == .session }.count == 50)

        await store.loadMore()
        #expect(store.rows.count == perSourceCount * 4) // each source advanced by its own remaining 5
        #expect(store.rows.filter { $0.kind == .workflow }.count == perSourceCount)
        #expect(store.rows.filter { $0.kind == .agent }.count == perSourceCount)
        #expect(store.rows.filter { $0.kind == .autoflow }.count == perSourceCount)
        #expect(store.rows.filter { $0.kind == .session }.count == perSourceCount)
        #expect(Set(store.rows.map(\.id)).count == store.rows.count) // no duplicate rows across pages

        store.deactivate()
        box.latest.finish()
    }
}
