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

/// Thread-safe append-only log / counter — same rationale as
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

@Suite(.serialized)
struct ActivityStoreTests {
    // API row types are `Decodable` only (never round-tripped back to the
    // server), so the stub bodies below are hand-written JSON text matching
    // each type's `CodingKeys`, not `JSONEncoder` output.
    private static let usageJSON = #"{"cached_tokens":0,"cost_usd":null,"input_tokens":0,"output_tokens":0,"priced":false,"runs":0,"total_tokens":0}"#

    private static func runListRowJSON(id: String, startedAt: String, status: String, durationMS: String = "null", turns: Int = 1) -> String {
        #"{"duration_ms":\#(durationMS),"finished_at":null,"host_id":"local","id":"\#(id)","started_at":"\#(startedAt)","status":"\#(status)","trigger":"cron","turns":\#(turns),"usage":\#(usageJSON),"workflow_name":"nightly-health"}"#
    }

    /// One row per source, each with a distinct `startedAt`/`at`/`createdAt`
    /// so merge-sort order is unambiguous: agent (12:00) > autoflow (11:00)
    /// > workflow (10:00) > session (09:00).
    private static func fourSourceBody(for path: String) -> Data {
        switch path {
        case "/api/runs/workflows":
            return Data("[\(runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: "running"))]".utf8)
        case "/api/runs/agents":
            let row = #"{"agent":"rupuso","duration_ms":5000,"host_id":"local","run_id":"run-ag-1","session_id":null,"source":"session","started_at":"2026-08-20T12:00:00Z","status":"completed","transcript_path":null,"trigger_source":"session_turn","turns":1,"usage":\#(usageJSON)}"#
            return Data("[\(row)]".utf8)
        case "/api/runs/autoflows/events":
            let row = #"{"at":"2026-08-20T11:00:00Z","cycle_id":"cycle-1","detail":null,"duration_ms":null,"event_id":"evt-1","host_id":"local","issue_display_ref":null,"kind":"run_launched","run_id":"run-af-1","status":"failed","turns":1,"usage":\#(usageJSON),"worker_name":"worker-a","workflow":"nightly-health"}"#
            return Data("[\(row)]".utf8)
        case "/api/sessions":
            let row = #"{"active_run_id":null,"agent_name":"rupuso","created_at":"2026-08-20T09:00:00Z","host_id":"local","last_error":null,"model":"claude","provider_name":"anthropic","scope":"active","session_id":"sess-1","status":"idle","target":null,"total_tokens_cached":0,"total_tokens_in":0,"total_tokens_out":0,"total_turns":1,"updated_at":"2026-08-20T09:00:00Z","usage":null,"workspace_id":"ws-1"}"#
            return Data("[\(row)]".utf8)
        default:
            return Data("[]".utf8)
        }
    }

    @MainActor
    private func makeStore(
        respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data) = { req in
            (200, ActivityStoreTests.fourSourceBody(for: req.url?.path ?? ""))
        }
    ) -> (store: ActivityStore, continuation: AsyncStream<StreamSignal<CPEvent>>.Continuation) {
        ActivityStubURLProtocol.reset(handler: respond)
        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: ActivityStubURLProtocol.session()
        )
        let (signals, continuation) = AsyncStream<StreamSignal<CPEvent>>.makeStream()
        let store = ActivityStore(client: client, signals: signals)
        return (store, continuation)
    }

    // (a) activate(.all) merges the four fixture-derived source arrays
    // sorted by date, descending, most-recent first.
    @MainActor @Test func activateAllMergesFourSourcesSortedByStartedAtDescending() async {
        let (store, continuation) = makeStore()
        await store.activate(kind: .all)

        #expect(store.rows.map(\.id) == ["run-ag-1", "evt-1", "run-wf-1", "sess-1"])
        #expect(store.rows.map(\.kind) == [.agent, .autoflow, .workflow, .session])

        store.deactivate()
        continuation.finish()
    }

    // (b) statusFilter {.failed} filters the merged view, synchronously,
    // with no refetch needed (it's a client-side narrowing of rows already
    // in memory).
    @MainActor @Test func statusFilterNarrowsMergedRows() async {
        let (store, continuation) = makeStore()
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
        continuation.finish()
    }

    // (c) liveTail off: a runStarted event increments pendingNewRuns and
    // leaves rows untouched; applyPendingRefresh() refetches and zeroes it.
    @MainActor @Test func liveTailOffCountsPendingNewRunsWithoutMutatingRows() async {
        let (store, continuation) = makeStore()
        store.liveTail = false
        await store.activate(kind: .all)
        let before = store.rows
        let requestsAfterActivate = ActivityStubURLProtocol.requestCount

        continuation.yield(.connection(true))
        continuation.yield(.event(.runStarted(runID: "run-new-1", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:00Z")))
        try? await Task.sleep(for: .milliseconds(30))

        #expect(store.pendingNewRuns == 1)
        #expect(store.rows == before)
        // No refetch happened purely from the newRun delta while liveTail is off.
        #expect(ActivityStubURLProtocol.requestCount == requestsAfterActivate)

        await store.applyPendingRefresh()
        #expect(store.pendingNewRuns == 0)
        #expect(ActivityStubURLProtocol.requestCount > requestsAfterActivate)

        store.deactivate()
        continuation.finish()
    }

    // (d) liveTail on: a runCompleted event for a visible row patches its
    // status in place, without a refetch.
    @MainActor @Test func liveTailOnPatchesVisibleRowStatusInPlaceWithoutRefetch() async {
        let (store, continuation) = makeStore()
        store.liveTail = true
        await store.activate(kind: .all)
        let requestsAfterActivate = ActivityStubURLProtocol.requestCount
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .running)

        continuation.yield(.connection(true))
        continuation.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "2026-08-20T10:05:00Z")))
        try? await Task.sleep(for: .milliseconds(30))

        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed)
        // The other three rows are untouched by the patch.
        #expect(store.rows.filter { $0.id != "run-wf-1" }.count == 3)
        #expect(ActivityStubURLProtocol.requestCount == requestsAfterActivate)

        store.deactivate()
        continuation.finish()
    }

    // (e) deactivate() stops the stream: the scripted continuation's
    // consumer task is torn down (observed via onTermination).
    @MainActor @Test func deactivateStopsTheStream() async {
        ActivityStubURLProtocol.reset { req in (200, Self.fourSourceBody(for: req.url?.path ?? "")) }
        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: ActivityStubURLProtocol.session()
        )
        let (signals, continuation) = AsyncStream<StreamSignal<CPEvent>>.makeStream()
        let terminated = FlagBox()
        continuation.onTermination = { _ in terminated.value = true }

        let store = ActivityStore(client: client, signals: signals)
        await store.activate(kind: .all)
        continuation.yield(.connection(true))
        try? await Task.sleep(for: .milliseconds(20))
        #expect(terminated.value == false)

        store.deactivate()
        try? await Task.sleep(for: .milliseconds(20))

        #expect(terminated.value == true)

        // Events yielded after deactivate() are never applied.
        let before = store.rows
        continuation.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "x")))
        try? await Task.sleep(for: .milliseconds(20))
        #expect(store.rows == before)

        continuation.finish()
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
        ActivityStubURLProtocol.reset { req in
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
        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: ActivityStubURLProtocol.session()
        )
        let (signals, continuation) = AsyncStream<StreamSignal<CPEvent>>.makeStream()
        let store = ActivityStore(client: client, signals: signals)
        store.liveTail = true
        await store.activate(kind: .all)
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.durationMS == nil)

        // Reconnect (a connect that is not the pristine first signal) then
        // an event, back to back with zero delay — matching Task 4's own
        // adversarial StreamLifecycle test shape.
        continuation.yield(.connection(false))
        continuation.yield(.connection(true))
        continuation.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "2026-08-20T10:05:00Z")))
        try? await Task.sleep(for: .milliseconds(80))

        // The resnapshot's fresh row content (durationMS: 9999) must be
        // visible, proving the refresh actually ran and completed ...
        let final = store.rows.first(where: { $0.id == "run-wf-1" })
        #expect(final?.durationMS == 9999)
        // ... with the trailing event's patch layered on top of it, not lost.
        #expect(final?.status == .completed)
        #expect(log.snapshot == ["initial-fetch", "resnapshot-fetch", "resnapshot-fetch-done"])

        store.deactivate()
        continuation.finish()
    }
}
