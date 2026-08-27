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
    /// Per-path hit counts, alongside the plain `requestCount` total —
    /// `ActivityStore.activate(kind:)` now also fires a `GET /api/hosts`
    /// discovery call in the background (progressive per-host loading; see
    /// `ActivityStore.loadRemoteHosts`), so a test asserting "no refetch
    /// happened" against the plain total would be racy against that
    /// unrelated, independently-timed call. `requestCount(forPaths:)` below
    /// lets a test scope its assertion to only the source endpoints it
    /// actually cares about.
    nonisolated(unsafe) static var pathHits: [String: Int] = [:]

    static func session() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [ActivityStubURLProtocol.self]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        requestCount = 0
        pathHits = [:]
        self.handler = handler
    }

    /// Sum of hits across just `paths` — see `pathHits`'s doc comment.
    static func requestCount(forPaths paths: Set<String>) -> Int {
        pathHits.filter { paths.contains($0.key) }.values.reduce(0, +)
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        ActivityStubURLProtocol.requestCount += 1
        if let path = request.url?.path {
            ActivityStubURLProtocol.pathHits[path, default: 0] += 1
        }
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

/// De-flakes "wait a fixed duration, then assert something async landed"
/// (CI regression: two tests below asserted right after a fixed
/// `Task.sleep`, which was long enough on a fast dev machine but too short
/// on the slower `macos-15` CI runner). Polls `condition` every `interval`
/// until it returns `true` or `timeout` elapses, checking `condition` once
/// more at the deadline in case it just became true. `timeout` defaults
/// generously relative to every debounce/settle window in this file (all
/// well under a second) without risking a genuinely-stuck condition hanging
/// a test run forever.
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

/// `pollUntil` plus a descriptive failure on timeout, so a genuine
/// regression reads as "timed out waiting for: ..." rather than a bare
/// boolean mismatch at some unrelated line below.
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

    /// The four federated source endpoints — used with
    /// `ActivityStubURLProtocol.requestCount(forPaths:)` to assert "no
    /// refetch happened" without being racy against the independently-timed
    /// `GET /api/hosts` background discovery call every `activate(kind:)`
    /// also fires (see that type's `pathHits` doc comment).
    private static let sourcePaths: Set<String> = [
        "/api/runs/workflows", "/api/runs/agents", "/api/runs/autoflows/events", "/api/sessions",
    ]

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

    // (b2) scopeFilter narrows the merged view to rows whose `project`
    // (the field `ActivityRow.project`/the table's PROJECT column reads)
    // matches, synchronously, with no refetch — same mechanism as
    // `statusFilter`. Of the four fixture-derived sources, only the session
    // row carries a workspace id (`ActivityRow.init(_: APISessionRow)` is
    // the only initializer that sets `project`), so scoping to that id
    // narrows to exactly that row and every other row (whose `project` is
    // `nil`) is excluded even though `scopeFilter` names a real project —
    // "honest narrowing": a row with no provable workspace never passes a
    // non-nil scope. `scopeFilter = nil` restores every row.
    @MainActor @Test func scopeFilterNarrowsMergedRowsAndNilRestores() async {
        let (store, box) = makeStore()
        await store.activate(kind: .all)
        #expect(store.rows.count == 4)
        let requestsAfterActivate = ActivityStubURLProtocol.requestCount

        store.scopeFilter = "ws-1"

        #expect(store.rows.map(\.id) == ["sess-1"])
        #expect(store.rows.allSatisfy { $0.project == "ws-1" })
        #expect(ActivityStubURLProtocol.requestCount == requestsAfterActivate)

        store.scopeFilter = nil
        #expect(store.rows.count == 4)

        store.deactivate()
        box.latest.finish()
    }

    // (b3) perf & interaction arc, Plan 5 Task 2 fix-round-1 (shared-store
    // review Critical): `unscopedRows` is the projection `NeedsYouCard`
    // reads instead of `rows` — it must NEVER be narrowed by
    // `scopeFilter`/`statusFilter`, since `ActivityScreen` (which shares
    // this exact instance with `OverviewScreen` — see `RootView.
    // activityStore`) sets both on every activation with no reset when the
    // operator navigates back to Overview. This is the regression repro: an
    // awaiting workflow row (a gate — `project` is always `nil` for
    // non-session rows, so it can never pass a non-nil scope) must still be
    // visible in `unscopedRows` even while `scopeFilter`/`statusFilter` are
    // set exactly the way `ActivityScreen.activate(kind:)` would leave them
    // — while `rows` (the Activity table's own projection) still narrows
    // exactly as before this fix.
    @MainActor @Test func unscopedRowsIgnoresScopeAndStatusFilterWhileRowsStaysNarrowed() async {
        let (store, box) = makeStore(respond: { req in
            switch req.url?.path {
            case "/api/runs/workflows":
                let gate = Self.runListRowJSON(id: "run-gate-1", startedAt: "2026-08-20T13:00:00Z", status: "awaiting_approval")
                return (200, Data("[\(gate)]".utf8))
            default:
                return (200, ActivityStoreTests.fourSourceBody(for: req.url?.path ?? ""))
            }
        })
        await store.activate(kind: .all)
        #expect(store.unscopedRows.map(\.id).contains("run-gate-1"))
        #expect(store.rows.map(\.id).contains("run-gate-1"))

        // Exactly what `ActivityScreen.activate(kind:)` does on a
        // project-scoped visit — no other screen resets this.
        store.scopeFilter = "some-other-project"
        store.statusFilter = [.completed]

        #expect(!store.rows.map(\.id).contains("run-gate-1"), "sanity: rows correctly narrows away the out-of-scope, wrong-status gate — Activity's own table behavior is unchanged")
        #expect(store.unscopedRows.map(\.id).contains("run-gate-1"), "the gate must still be visible in unscopedRows regardless of scope/status")
        guard let gateRow = store.unscopedRows.first(where: { $0.id == "run-gate-1" }) else {
            Issue.record("expected run-gate-1 in unscopedRows")
            return
        }
        #expect(gateRow.status == .awaiting)

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
        let requestsAfterActivate = ActivityStubURLProtocol.requestCount(forPaths: Self.sourcePaths)

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runStarted(runID: "run-new-1", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:00Z")))
        await expectEventually("pendingNewRuns reaches 1 after the newRun delta") { store.pendingNewRuns == 1 }

        #expect(store.pendingNewRuns == 1)
        #expect(store.rows == before)
        // No refetch happened purely from the newRun delta while liveTail is
        // off. Scoped to the four source paths (not the plain global
        // `requestCount`) so this isn't racy against the independently-timed
        // `GET /api/hosts` background discovery call — see `sourcePaths`'s
        // doc comment.
        #expect(ActivityStubURLProtocol.requestCount(forPaths: Self.sourcePaths) == requestsAfterActivate)

        await store.applyPendingRefresh()
        #expect(store.pendingNewRuns == 0)
        #expect(ActivityStubURLProtocol.requestCount(forPaths: Self.sourcePaths) > requestsAfterActivate)

        store.deactivate()
        box.latest.finish()
    }

    // (d) liveTail on: a runCompleted event for a visible row patches its
    // status in place, without a refetch.
    @MainActor @Test func liveTailOnPatchesVisibleRowStatusInPlaceWithoutRefetch() async {
        let (store, box) = makeStore()
        store.liveTail = true
        await store.activate(kind: .all)
        let requestsAfterActivate = ActivityStubURLProtocol.requestCount(forPaths: Self.sourcePaths)
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .running)

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "2026-08-20T10:05:00Z")))
        await expectEventually("run-wf-1's status patches to .completed") {
            store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed
        }

        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed)
        // The other three rows are untouched by the patch.
        #expect(store.rows.filter { $0.id != "run-wf-1" }.count == 3)
        // Scoped to the four source paths — see `sourcePaths`'s doc comment
        // on why the plain global `requestCount` would be racy here.
        #expect(ActivityStubURLProtocol.requestCount(forPaths: Self.sourcePaths) == requestsAfterActivate)

        store.deactivate()
        box.latest.finish()
    }

    // (d2) perf & interaction arc, Plan 5 Task 2 fix-round-1 (shared-store
    // review): `liveTail` gates only whether a genuinely NEW run (a
    // `.newRun` delta for a run this store has never fetched) triggers an
    // eager refresh vs. just incrementing `pendingNewRuns` — see
    // `ActivityStore.apply(_:)`'s `.newRun` branch. It does NOT gate
    // `.statusPatch` (an in-place correction to an ALREADY-visible row),
    // which `patchRow` applies unconditionally. This matters for the shared
    // store: `OverviewScreen`'s needs-you queue must never go stale on an
    // existing gate's status just because `ActivityScreen` last paused live
    // tail. Proven here with `liveTail = false`: `unscopedRows` (and
    // `rows`) both still patch in place, immediately, no refetch.
    @MainActor @Test func liveTailOffStillPatchesAlreadyVisibleRowsInUnscopedRowsAndRows() async {
        let (store, box) = makeStore()
        store.liveTail = false
        await store.activate(kind: .all)
        let requestsAfterActivate = ActivityStubURLProtocol.requestCount(forPaths: Self.sourcePaths)
        #expect(store.unscopedRows.first(where: { $0.id == "run-wf-1" })?.status == .running)

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "2026-08-20T10:05:00Z")))
        await expectEventually("run-wf-1's status patches to .completed in unscopedRows even with liveTail off") {
            store.unscopedRows.first(where: { $0.id == "run-wf-1" })?.status == .completed
        }

        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed)
        #expect(store.unscopedRows.first(where: { $0.id == "run-wf-1" })?.status == .completed)
        // No refetch — the patch alone did this, exactly like the
        // liveTail-on case; liveTail's cadence gate never applied here at
        // all, since `.statusPatch` isn't the `.newRun` branch it guards.
        #expect(ActivityStubURLProtocol.requestCount(forPaths: Self.sourcePaths) == requestsAfterActivate)

        store.deactivate()
        box.latest.finish()
    }

    // (d3) perf & interaction arc, Plan 5 Task 3: sort now lives on the
    // store — `toggleSort(_:)` both flips `sort` and re-sorts `rows`
    // immediately, from `rows`' own current contents (no view-side
    // `sortedRows` computed property needed anymore, and no re-run of the
    // whole merge/filter pipeline either).
    @MainActor @Test func toggleSortSetsActiveKeyAndReSortsRowsImmediately() async {
        let (store, box) = makeStore()
        await store.activate(kind: .all)
        #expect(store.sort == ActivitySort(key: .started, ascending: false))
        #expect(store.rows.map(\.id) == ["run-ag-1", "evt-1", "run-wf-1", "sess-1"])

        // First tap on a new column: `defaultAscending` for `.subject` is
        // `true`. Subjects: run-ag-1/sess-1 = "rupuso", evt-1/run-wf-1 =
        // "nightly-health" — ties keep the pre-toggle relative order.
        store.toggleSort(.subject)
        #expect(store.sort == ActivitySort(key: .subject, ascending: true))
        #expect(store.rows.map(\.id) == ["evt-1", "run-wf-1", "run-ag-1", "sess-1"])

        // Tapping the already-active column flips direction rather than
        // resetting to `defaultAscending`.
        store.toggleSort(.subject)
        #expect(store.sort == ActivitySort(key: .subject, ascending: false))
        #expect(store.rows.map(\.id) == ["run-ag-1", "sess-1", "evt-1", "run-wf-1"])

        store.deactivate()
        box.latest.finish()
    }

    // (d4) `patchRow`'s re-sort guard: a live status patch only re-sorts
    // `rows` when the active sort key actually participates (`.status`/
    // `.duration`) — proven here by sorting on `.status` first, then
    // patching `run-wf-1` (Running -> Completed) and confirming it moves to
    // rejoin the other `.completed` rows rather than staying pinned at its
    // pre-patch index.
    @MainActor @Test func liveStatusPatchReSortsRowsWhenActiveSortKeyIsStatus() async {
        let (store, box) = makeStore()
        await store.activate(kind: .all)
        store.toggleSort(.status)
        // Ascending by status display label: Completed (run-ag-1, sess-1,
        // in their pre-sort relative order) < Failed (evt-1) < Running
        // (run-wf-1).
        #expect(store.rows.map(\.id) == ["run-ag-1", "sess-1", "evt-1", "run-wf-1"])

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "2026-08-20T10:05:00Z")))
        await expectEventually("run-wf-1 rejoins the Completed group after its live status patch") {
            store.rows.map(\.id) == ["run-ag-1", "sess-1", "run-wf-1", "evt-1"]
        }

        store.deactivate()
        box.latest.finish()
    }

    // (d5) review fix (Important, task-3 follow-up): the sort-guard's OTHER
    // half — a patch under a NON-participating key (`.started`, the
    // default) must leave row ORDER byte-identical, not just "didn't
    // crash." Checked positionally (`rows.map(\.id)` before/after), not by
    // per-id lookup, since a lookup-based check can't distinguish "stayed
    // put" from "moved but is still findable."
    @MainActor @Test func liveStatusPatchNeverReordersRowsWhenActiveSortKeyDoesNotParticipate() async {
        let (store, box) = makeStore()
        await store.activate(kind: .all)
        #expect(store.sort == ActivitySort(key: .started, ascending: false))
        let idsBefore = store.rows.map(\.id)
        let unscopedIDsBefore = store.unscopedRows.map(\.id)
        #expect(idsBefore == ["run-ag-1", "evt-1", "run-wf-1", "sess-1"])

        box.latest.yield(.connection(true))
        // Patches the MIDDLE row (index 2 of 4) — a reorder bug that only
        // ever moves a patched row to the front or back wouldn't show up on
        // an edge row.
        box.latest.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "2026-08-20T10:05:00Z")))
        await expectEventually("run-wf-1 patches to .completed") {
            store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed
        }

        #expect(
            store.rows.map(\.id) == idsBefore,
            "a patch under a non-participating sort key (.started) must never reorder rows"
        )
        #expect(
            store.unscopedRows.map(\.id) == unscopedIDsBefore,
            "unscopedRows is never governed by this table's sort at all — must never reorder either"
        )

        store.deactivate()
        box.latest.finish()
    }

    // (d6) review fix (Important, task-3 follow-up): the `.duration` half
    // of the guard was previously untested — only `.status` had live
    // coverage. `ActivityDelta.reduce(_:)` never produces a non-nil
    // `durationMS` from any live `CPEvent` today (every case it reduces
    // sets `durationMS: nil` — see `patchRow`'s own doc comment on why it's
    // `internal`), so this drives the store's `patchRow(runID:status:
    // durationMS:)` seam directly via `@testable import` rather than
    // through a live event, the same way `statusOverrides` is already
    // reached directly for its own tests.
    @MainActor @Test func liveDurationChangingPatchRepositionsRowsWhenActiveSortKeyIsDuration() async {
        let (store, box) = makeStore()
        await store.activate(kind: .all)
        store.toggleSort(.duration)
        // Descending (defaultAscending == false for `.duration`). Only
        // run-ag-1 has a non-nil duration (5000ms) in the fixture; the
        // other three are nil, which sorts last regardless of direction —
        // so the order is unchanged from the default merge order, with
        // exactly one row having anything to compare against.
        #expect(store.sort == ActivitySort(key: .duration, ascending: false))
        #expect(store.rows.map(\.id) == ["run-ag-1", "evt-1", "run-wf-1", "sess-1"])

        // Gives run-wf-1 (currently nil duration) a duration far larger
        // than run-ag-1's 5000ms — under descending `.duration`, it must
        // now sort AHEAD of run-ag-1, not stay pinned at its old index.
        store.patchRow(runID: "run-wf-1", status: .completed, durationMS: 999_999)

        #expect(
            store.rows.map(\.id) == ["run-wf-1", "run-ag-1", "evt-1", "sess-1"],
            "a duration-changing patch under an active .duration sort must reposition the row, not leave it in place"
        )
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.durationMS == 999_999)

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
        await expectEventually("the scripted stream's consumer task terminates after deactivate()") {
            terminated.value == true
        }

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
    // resnapshot) is deliberately gated and returns a materially different
    // row than the first, so the final merged state can only be explained
    // by resnapshot completing (new row content visible) before the
    // trailing event's patch is layered on top of it.
    @MainActor @Test func reconnectResnapshotsBeforeApplyingFurtherDeltas() async {
        let log = EventLog()
        let workflowCallCount = CountBox()
        // De-flake (same class as the two gates below — a timed stub only
        // *probably* brackets the state it is sized for): the resnapshot's
        // response is held until the test has yielded the trailing event,
        // so the adversarial window (an event queued behind an
        // still-unresolved resnapshot) is established deterministically
        // rather than by a 30ms `Thread.sleep` that parallel-suite load can
        // let elapse first.
        let resnapshotGate = DispatchSemaphore(value: 0)
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
                // Held open until the test has yielded the trailing event
                // (below) — that is exactly the window a bug (event applied
                // before the resnapshot completes) would have to fall into,
                // and it is now guaranteed rather than sized. The 10s cap
                // only bounds a hung test; it is not a timing knob.
                _ = resnapshotGate.wait(timeout: .now() + 10)
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
        // The trailing event is now queued behind the still-unresolved
        // resnapshot — release it. (Signalling before the resnapshot's
        // handler is even reached is harmless: the semaphore's count is
        // held, so the wait returns immediately when it does run, and the
        // event was still yielded first either way.)
        resnapshotGate.signal()
        // CI regression (macos-15 runner, slower than a dev machine): a
        // fixed post-event sleep here wasn't always long enough for the
        // resnapshot's fetch plus the trailing patch to land — poll for the
        // fully-settled end state instead of asserting at one fixed instant.
        await expectEventually("run-wf-1 reflects both the resnapshot's fresh row and the trailing patch") {
            let row = store.rows.first(where: { $0.id == "run-wf-1" })
            return row?.durationMS == 9999 && row?.status == .completed
        }

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
        await expectEventually("run-wf-1 patches to .completed on the first stream") {
            store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed
        }
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
        await expectEventually("run-wf-1 patches to .completed on the second (rebuilt) stream") {
            store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed
        }
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
        // Timed-sleep sweep: this is one of the few stubs in the suite whose
        // sleep is NOT convertible to a semaphore gate — what's under test
        // IS the debounce window's own arithmetic (a fire landing inside an
        // in-flight fetch, its retry landing after that fetch), so the
        // fetch's duration is the discriminator, not scaffolding around one.
        // A gate would have to be released at a wall-clock instant chosen by
        // the test — the same clock, just moved. The de-flaking that IS
        // available here is already applied: wide margins (below) plus a
        // condition-based, not duration-based, final wait.
        //
        // Margins deliberately wide (were 10/25/47 — first-fire margin 13ms,
        // retry margin 3ms): on loaded macos-15 CI runners timer jitter can
        // shift the retry INSIDE the slow fetch's window, where it collides
        // again and gives up (one-retry guard) — fetches stuck at 2 forever.
        // With 10/150/200 the first fire (~160) sits 50ms inside the window
        // (10..210) and the retry (~310) lands 100ms after it — both margins
        // comfortably above observed CI jitter, same discriminator.
        let headStartMS = 10
        let debounceMS = 150
        let slowFetchMS = 200 // strictly between (headStart+debounce)=160 and (headStart+2*debounce)=310

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

        // `manualTask`'s own completion is a real signal (not a fixed
        // sleep) that fetch #2's collision window has closed.
        _ = await manualTask.value

        // CI regression (macos-15 runner, slower than a dev machine — final
        // review already called this the flakiest wait in the file): the
        // collision-retried debounced refresh still needs its own
        // wall-clock time (another `debounceMS` wait plus its own
        // `slowFetchMS` fetch) after `manualTask` resolves, and a fixed
        // total-duration sleep sized for a fast machine wasn't always long
        // enough on a slower one. Poll for the exact landed count instead —
        // burst/collision *timing* (headStartMS, the debounce/slow-fetch
        // sizing above) stays exactly as adversarial as before; only the
        // final "did it land yet" wait is condition-based now.
        await expectEventually("the collision-retried debounced refresh lands (exactly 3 fetches)") {
            workflowFetches.value == 3
        }

        // Exactly 3 fetches: activate's, manualTask's, and the debounce
        // retry's. The *first* debounce attempt contributes 0 — it was
        // skipped (in-flight collision) before ever calling `fetch`.
        #expect(workflowFetches.value == 3)

        // Settle past the retry's own `isRetry` guard (it gives up after
        // one attempt — see `ActivityStore.scheduleDebouncedRefresh`) and
        // confirm the count never overshoots to a runaway 4th fetch.
        try? await Task.sleep(for: .milliseconds(100))
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

        // The burst itself stays back-to-back with zero delay between
        // events — that tight timing is the scenario under test (proving
        // coalescing, not just eventual consistency).
        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runStarted(runID: "run-a", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:00Z")))
        box.latest.yield(.event(.runStarted(runID: "run-b", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:01Z")))
        box.latest.yield(.event(.runStarted(runID: "run-c", workflowPath: "p.yml", startedAt: "2026-08-20T13:00:02Z")))

        // CI regression (macos-15 runner): a fixed post-burst sleep here
        // wasn't always long enough for the debounced refresh to have
        // fired yet. Poll for it to land instead of asserting at one fixed
        // instant.
        await expectEventually("the burst's single coalesced debounced refresh lands") { fetches.value == 2 }

        // One coalesced refresh from the burst, on top of activate's own.
        #expect(fetches.value == 2)

        // Settle further and confirm it never overshoots — the whole point
        // of coalescing is that three rapid `newRun`s produce exactly one
        // extra fetch, not three.
        try? await Task.sleep(for: .milliseconds(100))
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
        await expectEventually("run-wf-1 patches to .completed before the filter toggle") {
            store.rows.first(where: { $0.id == "run-wf-1" })?.status == .completed
        }
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

    // MARK: - Infinite scroll + custom date range (perf & interaction arc, Plan 5 Task 5)

    /// Parses `offset`/`limit`/`since`/`until` off a stubbed request's query
    /// string — shared by the tests below.
    private static func queryParams(_ req: URLRequest) -> (offset: Int, limit: Int, since: String?, until: String?) {
        guard let url = req.url else { return (0, 20, nil, nil) }
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
        func value(_ name: String) -> String? { items.first(where: { $0.name == name })?.value }
        let offset = value("offset").flatMap(Int.init) ?? 0
        let limit = value("limit").flatMap(Int.init) ?? 20
        return (offset, limit, value("since"), value("until"))
    }

    // (l) a KIND page (not `.all`) uses the web-parity page size of 20 —
    // `hasMore`/`loadedCount` track the single active source's own
    // `PagedSnapshot`, and `hasMore` flips false once a short page (fewer
    // than 20 rows) comes back.
    @MainActor @Test func kindPageUsesPageSizeTwentyAndHasMoreTracksExhaustion() async {
        let allRows = (0..<25).map { i in
            Self.runListRowJSON(id: "wf-\(i)", startedAt: String(format: "2026-08-20T10:%02d:00Z", i), status: "running")
        }
        let requestedLimits = Counter()
        let (store, box) = makeStore { req in
            guard req.url?.path == "/api/runs/workflows" else { return (200, Data("[]".utf8)) }
            let (offset, limit, _, _) = Self.queryParams(req)
            requestedLimits.increment()
            guard offset < allRows.count else { return (200, Data("[]".utf8)) }
            let end = min(offset + limit, allRows.count)
            return (200, Data(("[" + allRows[offset..<end].joined(separator: ",") + "]").utf8))
        }

        await store.activate(kind: .workflows)
        #expect(store.rows.count == 20, "kind pages page at 20, not the .all merged size of 50")
        #expect(store.loadedCount == 20)
        #expect(store.hasMore == true)
        #expect(store.isLoadingMore == false)

        await store.loadMore()
        #expect(store.rows.count == 25, "only 5 rows remained — a short page")
        #expect(store.loadedCount == 25)
        #expect(store.hasMore == false, "a page shorter than the page size marks the source exhausted")

        store.deactivate()
        box.latest.finish()
    }

    // (m) `setDateRange(since:until:)` sends `since`/`until` on the NEXT
    // fetch, resets the active source back to offset 0, and — the
    // generation-guard contract — discards a `loadMore()` that was already
    // in flight for the OLD (unfiltered) data when the range changed, rather
    // than letting its stale page land on top of the freshly-reset one.
    @MainActor @Test func setDateRangeSendsBoundsResetsOffsetAndDropsAStaleInFlightLoadMore() async {
        let unfiltered = (0..<40).map { i in
            Self.runListRowJSON(id: "wf-\(i)", startedAt: String(format: "2026-08-20T10:%02d:00Z", i), status: "running")
        }
        let filtered = (0..<3).map { i in
            Self.runListRowJSON(id: "narrow-\(i)", startedAt: String(format: "2026-08-15T10:%02d:00Z", i), status: "running")
        }
        // `URLProtocol.startLoading()` is synchronous (the stub handler
        // can't `await`), and — as found live in this exact test (an earlier
        // indefinite-`DispatchSemaphore`-block version starved OTHER
        // concurrently-running suites' own stubbed fetches, timing them out)
        // — blocking one request's handler INDEFINITELY is unsafe here: it
        // can hold onto a thread/slot the whole `URLSession`-stub machinery
        // (shared across every test in this parallel-testing run) needs.
        // A bounded `Thread.sleep`, same fixed-delay "widen the race window"
        // technique `debouncedRefreshCollidingWithInFlightRefreshRetriesOnceAfterItCompletes`
        // already uses for two concurrent same-path fetches, avoids that:
        // the stale fetch is slow, not stuck.
        let sawSince = FlagBox()

        let (store, box) = makeStore { req in
            guard req.url?.path == "/api/runs/workflows" else { return (200, Data("[]".utf8)) }
            let (offset, limit, since, _) = Self.queryParams(req)
            if since != nil {
                sawSince.value = true
                guard offset < filtered.count else { return (200, Data("[]".utf8)) }
                let end = min(offset + limit, filtered.count)
                return (200, Data(("[" + filtered[offset..<end].joined(separator: ",") + "]").utf8))
            }
            // Unfiltered fetches beyond page 0 (i.e. a `loadMore()`) are
            // slowed down — models a loadMore() still in flight when the
            // date range changes, and gives the reset's own (fast,
            // since-bound) fetch above time to land first. Page 0 itself
            // (the initial `activate`) stays instant so the test can get to
            // a known starting state.
            if offset > 0 {
                Thread.sleep(forTimeInterval: 0.3)
            }
            guard offset < unfiltered.count else { return (200, Data("[]".utf8)) }
            let end = min(offset + limit, unfiltered.count)
            return (200, Data(("[" + unfiltered[offset..<end].joined(separator: ",") + "]").utf8))
        }

        await store.activate(kind: .workflows)
        #expect(store.rows.count == 20)

        async let staleLoadMore: Void = store.loadMore()
        await expectEventually("loadMore() enters its (slow) fetch") { store.isLoadingMore }

        let since = Date(timeIntervalSince1970: 1_755_000_000) // 2026-08-12ish, exact value irrelevant
        await store.setDateRange(since: since, until: nil)

        #expect(sawSince.value == true, "the reset fetch must carry the since bound")
        #expect(Set(store.rows.map(\.id)) == Set((0..<3).map { "narrow-\($0)" }))
        #expect(store.rows.count == 3, "offset reset to 0 against the narrowed 3-row result, not appended onto 20")
        #expect(store.since == since)

        // Let the stale loadMore's (slow, unfiltered, now irrelevant) fetch
        // finally land — it must be discarded, never appended.
        _ = await staleLoadMore
        #expect(Set(store.rows.map(\.id)) == Set((0..<3).map { "narrow-\($0)" }), "the late unfiltered page must be discarded")
        #expect(store.rows.count == 3)

        store.deactivate()
        box.latest.finish()
    }

    // (n) the live-tail path (`refreshActiveSources(preservingScrolledPages:
    // true)`, exercised here via `applyPendingRefresh()`) must SPLICE a
    // fresh page 0 onto whatever `loadMore()` already appended, not replace
    // `rows` wholesale — the brief's "SSE head splice must not reset scroll
    // or drop appended pages" contract.
    @MainActor @Test func applyPendingRefreshPreservesLoadMoreAppendedPagesViaHeadSplice() async {
        let page0 = (0..<20).map { i in
            Self.runListRowJSON(id: "wf-\(i)", startedAt: String(format: "2026-08-20T10:%02d:00Z", i), status: "running")
        }
        let page1 = (20..<25).map { i in
            Self.runListRowJSON(id: "wf-\(i)", startedAt: String(format: "2026-08-20T09:%02d:00Z", i), status: "running")
        }
        let refreshedPage0 = [Self.runListRowJSON(id: "wf-new", startedAt: "2026-08-20T11:00:00Z", status: "running")] + page0.dropLast()

        let servingRefresh = FlagBox()
        let (store, box) = makeStore { req in
            guard req.url?.path == "/api/runs/workflows" else { return (200, Data("[]".utf8)) }
            let (offset, _, _, _) = Self.queryParams(req)
            if offset == 0 && servingRefresh.value {
                return (200, Data(("[" + refreshedPage0.joined(separator: ",") + "]").utf8))
            }
            if offset == 0 { return (200, Data(("[" + page0.joined(separator: ",") + "]").utf8)) }
            return (200, Data(("[" + page1.joined(separator: ",") + "]").utf8))
        }

        await store.activate(kind: .workflows)
        #expect(store.rows.count == 20)
        await store.loadMore()
        #expect(store.rows.count == 25, "loadMore() appended page 1 (5 more rows)")

        // A live refresh lands (the "N new runs" pill, or the debounced
        // live-tail path it shares `refreshActiveSources` with) — page 0 has
        // changed (one new row at the head, one old row fell off it), but
        // the operator's scrolled-in page 1 rows must survive untouched.
        servingRefresh.value = true
        await store.applyPendingRefresh()

        #expect(store.rows.contains { $0.id == "wf-new" }, "the fresh head row must appear")
        for id in (20..<25).map({ "wf-\($0)" }) {
            #expect(store.rows.contains { $0.id == id }, "page-1 row \(id) must survive the head refresh")
        }
        // Fresh head (20: wf-new + wf-0...wf-18) + preserved tail (6: wf-19,
        // which fell off the fresh head, plus wf-20...wf-24 from page 1) —
        // never collapsed back down to a bare fresh page 0 (20).
        #expect(store.rows.count == 26)

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

    // MARK: - Per-host progressive loading (hotfix: matt's fleet directive —
    // "lazy load things as you gather them ... you should not fail all for
    // a single host")

    private static func hostsJSON(_ hosts: [(id: String, status: String)]) -> Data {
        let items = hosts.map { #"{"id":"\#($0.id)","name":"\#($0.id)","transport_kind":"ssh","status":"\#($0.status)"}"# }
        return Data("[\(items.joined(separator: ","))]".utf8)
    }

    private static func queryHost(_ req: URLRequest) -> String? {
        guard let url = req.url else { return nil }
        return URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first(where: { $0.name == "host" })?.value
    }

    // (n) `activate(kind:)` returns once the *local* load lands — `state`
    // is already `.content` from `host=local` alone, before a slow online
    // remote host has answered at all. `pendingHosts` then drains to 0 and
    // the remote host's row merges in once its (artificially delayed)
    // response actually arrives — proving the merge is progressive, not a
    // single all-or-nothing wait.
    @MainActor @Test func activateRendersLocalImmediatelyThenMergesSlowOnlineRemoteHostProgressively() async {
        // De-flake (the long-hunted ~1/20 full-suite failure, finally
        // attributed on main's CI): the old shape raced two TIMERS — a 60ms
        // poll window for observing `pendingHosts == 1` against an 80ms
        // `Thread.sleep` "slow remote". Under parallel-suite load either
        // could lose (discovery landing late, or the sleep elapsing while
        // the main actor was starved, draining 1→0 unobserved). The remote
        // response now blocks on a semaphore the TEST releases only after
        // it has asserted the mid-flight state — the pending window is held
        // open deterministically, so the poll below can use the generous
        // default timeout without ever waiting out a real failure.
        let remoteGate = DispatchSemaphore(value: 0)
        let (store, box) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("mini", "online")]))
            }
            guard url.path == "/api/runs/workflows" else { return (200, Data("[]".utf8)) }
            if Self.queryHost(req) == "mini" {
                // Held by the test until the pending state is asserted; the
                // 10s cap only bounds a hung test, it is not a timing knob.
                _ = remoteGate.wait(timeout: .now() + 10)
                let row = Self.runListRowJSON(id: "run-remote-1", startedAt: "2026-08-20T09:00:00Z", status: "running")
                return (200, Data("[\(row)]".utf8))
            }
            let row = Self.runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: "running")
            return (200, Data("[\(row)]".utf8))
        }

        await store.activate(kind: .workflows)

        // Local truth is already showing — the gated remote host hasn't
        // been waited on at all.
        guard case .content = store.state else {
            remoteGate.signal()
            Issue.record("expected .content from the local load alone, got \(store.state)")
            return
        }
        #expect(store.rows.map(\.id) == ["run-wf-1"])

        // Discovery registers the one online remote host as pending. The
        // remote row fetch is gated on the semaphore, so this state cannot
        // drain early — the default (generous) poll timeout is safe.
        await expectEventually(
            "GET /api/hosts discovery lands and registers the one online remote host as pending"
        ) { store.pendingHosts == 1 }
        #expect(store.pendingHosts == 1)
        #expect(store.rows.map(\.id) == ["run-wf-1"]) // still local-only

        // Only now let the remote host answer.
        remoteGate.signal()

        // Poll to the terminal state — bounded generously (default 5s).
        await expectEventually("the gated remote host's row merges in and pendingHosts drains to 0") {
            store.pendingHosts == 0 && store.rows.count == 2
        }

        #expect(store.pendingHosts == 0)
        #expect(Set(store.rows.map(\.id)) == Set(["run-wf-1", "run-remote-1"]))
        // `state` never depended on the remote host at all — still `.content`.
        guard case .content = store.state else {
            Issue.record("expected .content to persist through the remote merge, got \(store.state)")
            return
        }

        store.deactivate()
        box.latest.finish()
    }

    // (o) An erroring (or offline, or simply absent) remote host never
    // fails the table — `state` stays `.content` from local truth alone,
    // and `pendingHosts` still drains to 0 once the failure is known.
    @MainActor @Test func failingRemoteHostNeverFailsTableAndPendingHostsStillDrains() async {
        let (store, box) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("mini", "online")]))
            }
            guard url.path == "/api/runs/workflows" else { return (200, Data("[]".utf8)) }
            if Self.queryHost(req) == "mini" {
                return (500, Data(#"{"error":"boom"}"#.utf8))
            }
            let row = Self.runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: "running")
            return (200, Data("[\(row)]".utf8))
        }

        await store.activate(kind: .workflows)
        guard case .content = store.state else {
            Issue.record("expected .content from the local load, got \(store.state)")
            return
        }

        await expectEventually("the failing remote host's fetch resolves and pendingHosts drains to 0") {
            store.pendingHosts == 0
        }

        #expect(store.pendingHosts == 0)
        #expect(store.rows.map(\.id) == ["run-wf-1"]) // the failing host contributed nothing
        guard case .content = store.state else {
            Issue.record("a failing remote host must never flip state to .failed, got \(store.state)")
            return
        }

        store.deactivate()
        box.latest.finish()
    }

    // (p) An offline host (status != "online") is skipped entirely — never
    // fetched from, never counted in `pendingHosts`.
    @MainActor @Test func offlineHostIsSkippedEntirelyAndNeverCountedAsPending() async {
        let (store, box) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("kuki", "offline")]))
            }
            let row = Self.runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: "running")
            return (200, Data("[\(row)]".utf8))
        }

        await store.activate(kind: .workflows)
        await expectEventually("host discovery completes and finds nothing pending (offline host skipped)") {
            store.pendingHosts == 0
        }

        #expect(store.pendingHosts == 0)
        #expect(store.rows.map(\.id) == ["run-wf-1"])
        // Never fetched from the offline host at all — only ever `host=local`.
        #expect(ActivityStubURLProtocol.pathHits["/api/runs/workflows"] == 1)

        store.deactivate()
        box.latest.finish()
    }

    // MARK: - Client-side date filtering of remote-host rows (review fix,
    // perf & interaction arc Plan 5 Task 5 — the server's single-remote-host
    // proxy branch has no `since`/`until` params on any endpoint, so
    // `Source.remoteFetch` never sends them; the active range must instead
    // be enforced client-side against remote rows at merge time, in
    // `ActivityStore.dateFilteredForDisplay(_:)`, or a date-ranged Activity
    // view would silently show unfiltered remote-host history alongside
    // correctly-narrowed local history.)

    // (r) A remote host's row outside the active `since`/`until` bounds
    // never reaches the operator — excluded from BOTH `rows` (the table)
    // AND `unscopedRows` (the needs-you feed, which bypasses status/scope
    // narrowing but must still respect an active date range — see
    // `dateFilteredForDisplay`'s doc comment). A remote row inside the
    // bounds merges in normally.
    @MainActor @Test func remoteHostRowsOutsideActiveDateRangeAreExcludedFromRowsAndUnscopedRows() async {
        let (store, box) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("mini", "online")]))
            }
            guard url.path == "/api/runs/workflows" else { return (200, Data("[]".utf8)) }
            if Self.queryHost(req) == "mini" {
                // Never asked for since/until — the fix under test is that
                // this app-side stub receiving no date params at all is
                // exactly what SHOULD happen (see `Source.remoteFetch`'s
                // doc comment); a regression back to sending them would
                // still pass this particular assertion (the stub ignores
                // extra query items), which is why the request-count-based
                // test below is the one that actually pins "no refetch".
                let inRange = Self.runListRowJSON(id: "run-remote-in", startedAt: "2026-08-10T12:00:00Z", status: "running")
                let outOfRange = Self.runListRowJSON(id: "run-remote-out", startedAt: "2026-08-01T12:00:00Z", status: "running")
                return (200, Data("[\(inRange),\(outOfRange)]".utf8))
            }
            let row = Self.runListRowJSON(id: "run-wf-local", startedAt: "2026-08-12T12:00:00Z", status: "running")
            return (200, Data("[\(row)]".utf8))
        }

        await store.activate(kind: .workflows)
        await expectEventually("the remote host's two rows land") {
            store.pendingHosts == 0 && store.unscopedRows.count == 3
        }
        // Before any date range is set, every row (local + both remote) is visible.
        #expect(Set(store.rows.map(\.id)) == Set(["run-wf-local", "run-remote-in", "run-remote-out"]))

        // Narrow to a range that includes the local row and the "in" remote
        // row, but excludes the "out" remote row.
        await store.setDateRange(
            since: ISO8601Parsing.parse("2026-08-10T00:00:00Z")!,
            until: nil
        )

        #expect(Set(store.rows.map(\.id)) == Set(["run-wf-local", "run-remote-in"]), "the out-of-range remote row must not reach `rows`")
        #expect(Set(store.unscopedRows.map(\.id)) == Set(["run-wf-local", "run-remote-in"]), "…nor `unscopedRows` — a date range narrows this feed too, not just status/scope")

        store.deactivate()
        box.latest.finish()
    }

    // (s) Narrowing (or widening) the active date range re-filters
    // remote-host rows ALREADY sitting in `remoteRowsBySource` — no second
    // fetch to the remote host is needed for the narrowing itself to take
    // effect, since it's enforced client-side at `recompute()` time, not by
    // asking the server again with different bounds.
    @MainActor @Test func dateRangeChangeReFiltersAlreadyMergedRemoteRowsWithoutRefetchingTheRemoteHost() async {
        let (store, box) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("mini", "online")]))
            }
            guard url.path == "/api/runs/workflows" else { return (200, Data("[]".utf8)) }
            if Self.queryHost(req) == "mini" {
                let inRange = Self.runListRowJSON(id: "run-remote-in", startedAt: "2026-08-10T12:00:00Z", status: "running")
                let outOfRange = Self.runListRowJSON(id: "run-remote-out", startedAt: "2026-08-01T12:00:00Z", status: "running")
                return (200, Data("[\(inRange),\(outOfRange)]".utf8))
            }
            let row = Self.runListRowJSON(id: "run-wf-local", startedAt: "2026-08-12T12:00:00Z", status: "running")
            return (200, Data("[\(row)]".utf8))
        }

        await store.activate(kind: .workflows)
        await expectEventually("the remote host's two rows land, unfiltered (no range active yet)") {
            store.unscopedRows.count == 3
        }
        let remoteFetchCountBeforeRangeChange = ActivityStubURLProtocol.pathHits["/api/runs/workflows"] ?? 0

        await store.setDateRange(since: ISO8601Parsing.parse("2026-08-10T00:00:00Z")!, until: nil)

        #expect(Set(store.rows.map(\.id)) == Set(["run-wf-local", "run-remote-in"]), "the narrowing must apply immediately, client-side")
        // The local snapshot's own `resetAndRefresh()` (inside `setDateRange`)
        // legitimately adds ONE more request (host=local, now carrying the
        // new since/until) — but the REMOTE host must not be asked again at
        // all; its already-merged rows are just re-filtered in place.
        let remoteFetchCountAfterRangeChange = ActivityStubURLProtocol.pathHits["/api/runs/workflows"] ?? 0
        #expect(remoteFetchCountAfterRangeChange == remoteFetchCountBeforeRangeChange + 1, "exactly one new request (host=local's reset) — never a second remote-host fetch")

        store.deactivate()
        box.latest.finish()
    }

    // MARK: - statusOverrides pruning (Phase 3, Task 4 carry-over)

    // (q) A live status-patch override for a runID is dropped once the
    // freshly *merged* (unpatched) data for that same runID already shows
    // the identical status — "server caught up" — even when the
    // `recompute()` that notices this isn't a full `refreshActiveSources()`
    // (which already clears every override unconditionally regardless of
    // this fix; see that method's doc comment). `loadRemoteHost`'s
    // merge-then-`recompute()` step is the one production path that calls
    // `recompute()` without a full clear, so it's the vehicle here: one of
    // the two rows sharing a run id (local vs. the remote host's own copy
    // of it) already reads "completed" by the time they're both merged in —
    // exactly the case per-key pruning is for.
    //
    // Review fix: which of the two rows already matches — `matchingRowIsLocal`
    // — controls which one `recompute()`'s merge encounters *first* (local
    // rows always precede remote rows in `merged`; see that method). Pruning
    // must not depend on that order: a single combined prune-or-patch pass
    // would prune as soon as it hit the matching row and, if that happened
    // to be the *first* one, leave a later mismatched row for the same run
    // id force-patched or not depending purely on which row won the race —
    // this was previously masked entirely because the one committed test
    // only ever exercised the remote-matches-last case. Returns the final
    // set of statuses shown for `run-wf-1` and whether the override was
    // pruned, so the caller can assert both orderings land on the identical
    // outcome.
    @MainActor
    private func pruningOutcome(matchingRowIsLocal: Bool) async -> (statuses: Set<ActivityStatus>, overridePruned: Bool) {
        let localStatus = matchingRowIsLocal ? "completed" : "running"
        let remoteStatus = matchingRowIsLocal ? "running" : "completed"
        // De-flake (same class as the progressive-merge test above, and
        // attributed on the same CI runs): the old 80ms `Thread.sleep` only
        // *probably* held the remote fetch back past the live-patch below —
        // under parallel-suite load the sleep could elapse before the
        // override was recorded, recompute() ran with nothing standing to
        // prune, and `overridePruned` read false. The remote response now
        // blocks on a semaphore released only after the override is
        // OBSERVED standing — deterministic, no timing knob.
        let remoteGate = DispatchSemaphore(value: 0)
        let (store, box) = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("mini", "online")]))
            }
            guard url.path == "/api/runs/workflows" else { return (200, Data("[]".utf8)) }
            if Self.queryHost(req) == "mini" {
                // Held until the override stands; 10s caps a hung test only.
                _ = remoteGate.wait(timeout: .now() + 10)
                let row = Self.runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: remoteStatus)
                return (200, Data("[\(row)]".utf8))
            }
            let row = Self.runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: localStatus)
            return (200, Data("[\(row)]".utf8))
        }

        await store.activate(kind: .workflows)

        // Live patch: records the "completed" override ahead of whichever
        // side (local or remote) hasn't caught up to it yet.
        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runCompleted(runID: "run-wf-1", status: "completed", finishedAt: "2026-08-20T10:05:00Z")))
        await expectEventually("run-wf-1 has a standing .completed override") {
            store.statusOverrides["run-wf-1"] != nil
        }

        // Only now let the remote host answer.
        remoteGate.signal()

        // The remote host answers with its (deliberately delayed) row —
        // `loadRemoteHost`'s recompute() is the non-full-clear vehicle this
        // test exercises.
        await expectEventually("the remote host's row merges in and pendingHosts drains to 0") {
            store.pendingHosts == 0
        }

        let statuses = Set(store.rows.filter { $0.id == "run-wf-1" }.map(\.status))
        let overridePruned = store.statusOverrides["run-wf-1"] == nil

        store.deactivate()
        box.latest.finish()
        return (statuses, overridePruned)
    }

    @MainActor @Test func statusOverridePrunedOncePerKeyMergedStatusMatchesWithoutAFullClear() async {
        let outcome = await pruningOutcome(matchingRowIsLocal: false)

        #expect(outcome.overridePruned)
        // The row that already read "completed" on its own keeps doing so;
        // the other (still raw "running" underneath) is no longer
        // force-patched now that the override was pruned wholesale rather
        // than selectively — see `recompute()`'s two-pass doc comment.
        #expect(outcome.statuses == [.completed, .running])
    }

    // (r) Order independence (review fix): the matching row can be either
    // side of the merge — the outcome must be identical either way. A
    // single-pass prune-or-patch loop was safe only because local rows
    // happen to precede remote rows in `merged`, an undocumented invariant;
    // this asserts the fix no longer depends on it by running the exact
    // same scenario with the matching row on the *other* side and comparing
    // results directly.
    @MainActor @Test func statusOverridePruningIsOrderIndependentAcrossWhichRowMatchesFirst() async {
        let matchingLast = await pruningOutcome(matchingRowIsLocal: false) // remote (2nd in merge order) matches
        let matchingFirst = await pruningOutcome(matchingRowIsLocal: true) // local (1st in merge order) matches

        #expect(matchingLast.overridePruned)
        #expect(matchingFirst.overridePruned)
        #expect(matchingFirst.statuses == matchingLast.statuses)
        #expect(matchingFirst.statuses == [.completed, .running])
    }

    // (r) A full `refreshActiveSources()` (here, `applyPendingRefresh()`)
    // still clears every override wholesale, matching status or not — the
    // per-key pruning above is additive, not a replacement for this.
    @MainActor @Test func fullRefreshStillClearsOverridesRegardlessOfMatchingStatus() async {
        let (store, box) = makeStore()
        store.liveTail = false
        await store.activate(kind: .all)

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runCompleted(runID: "run-wf-1", status: "failed", finishedAt: "2026-08-20T10:05:00Z")))
        await expectEventually("run-wf-1 patches to .failed via the live override") {
            store.rows.first(where: { $0.id == "run-wf-1" })?.status == .failed
        }
        #expect(store.statusOverrides["run-wf-1"] != nil)

        // The stub's own REST truth is still "running" (never "failed") —
        // a full refresh must clear the override even though it doesn't
        // match, not just leave it standing because nothing "caught up".
        await store.applyPendingRefresh()

        #expect(store.statusOverrides.isEmpty)
        #expect(store.rows.first(where: { $0.id == "run-wf-1" })?.status == .running)

        store.deactivate()
        box.latest.finish()
    }

    // MARK: - Mutations (Phase 3, Task 5)

    /// `approve(runID:gate:host:)` begins the pending key, POSTs, and
    /// schedules a debounced refresh on success — the row's own status
    /// column catches up once that refresh lands (here, simulated by the
    /// stub returning a different status on the second hit), and the key
    /// itself confirms via the live-tail `.statusPatch` reduction once a
    /// matching event arrives, not from the refresh directly.
    @MainActor @Test func activityApprovePatchesRowStatusAfterDebouncedRefreshAndConfirmsViaLiveEvent() async {
        let workflowHits = Counter()
        let (store, box) = makeStore(
            debounceInterval: .milliseconds(20),
            respond: { req in
                if req.url?.path == "/api/runs/run-wf-1/approve" {
                    // Marker-only response — no `run` body, so `approve`
                    // never auto-confirms from it; it stays `.pending`
                    // until the live `runResumed` event below.
                    return (200, Data(#"{"ok":true,"host_id":"local"}"#.utf8))
                }
                guard req.url?.path == "/api/runs/workflows" else {
                    return (200, Data("[]".utf8))
                }
                workflowHits.increment()
                let status = workflowHits.value == 1 ? "awaiting_approval" : "running"
                let row = Self.runListRowJSON(id: "run-wf-1", startedAt: "2026-08-20T10:00:00Z", status: status)
                return (200, Data("[\(row)]".utf8))
            }
        )
        await store.activate(kind: .workflows)
        #expect(store.rows.first?.status == .awaiting)

        let key = ActionKey.gate(runID: "run-wf-1", stepID: "gate-1", verb: .approve)
        await store.approve(runID: "run-wf-1", gate: "gate-1", host: "local")
        guard case .pending = store.pendingActions.state(key) else {
            Issue.record("expected .pending immediately after approve(), got \(store.pendingActions.state(key))")
            return
        }

        // The debounced refresh (20ms) lands the row's fresh "running"
        // status from the stub.
        await expectEventually("row status becomes running after approve's debounced refresh") {
            store.rows.first(where: { $0.id == "run-wf-1" })?.status == .running
        }

        // The key itself only confirms off an observed live event — the
        // refresh alone (REST, no event) never touches `pendingActions`.
        #expect(store.pendingActions.state(key) != .confirmed)

        box.latest.yield(.connection(true))
        box.latest.yield(.event(.runResumed(runID: "run-wf-1")))
        await expectEventually("approve confirms once the live runResumed event lands") {
            store.pendingActions.state(key) == .confirmed
        }

        store.deactivate()
        box.latest.finish()
    }

    /// `reject(runID:gate:host:)` fails cleanly on a POST error, same
    /// `mutationErrorMessage` mapping every mutation method shares.
    @MainActor @Test func activityRejectFailsWithMessageOnPOSTError() async {
        let (store, box) = makeStore(respond: { req in
            if req.url?.path == "/api/runs/run-wf-1/reject" {
                return (409, Data(#"{"error":"gate already resolved"}"#.utf8))
            }
            return (200, ActivityStoreTests.fourSourceBody(for: req.url?.path ?? ""))
        })
        await store.activate(kind: .workflows)

        let key = ActionKey.gate(runID: "run-wf-1", stepID: "gate-1", verb: .reject)
        await store.reject(runID: "run-wf-1", gate: "gate-1", host: "local")

        guard case .failed(let message) = store.pendingActions.state(key) else {
            Issue.record("expected .failed, got \(store.pendingActions.state(key))")
            return
        }
        #expect(!message.isEmpty)

        store.deactivate()
        box.latest.finish()
    }

    /// Cross-store shared-key visibility: `BackendController.pendingActions`
    /// is ONE instance both `ActivityStore` and `RunDetailStore` are handed
    /// at construction — a key one store begins must read as `.pending` to
    /// the other immediately, with no refetch or event needed to see it.
    @MainActor @Test func activityStoreApproveIsVisibleAsPendingToASeparateRunDetailStoreSharingTheSameLedger() async {
        let shared = PendingActions()
        let (store, box) = makeStore(
            pendingActions: shared,
            respond: { req in
                if req.url?.path == "/api/runs/run-wf-1/approve" {
                    // Marker-only response, remote-proxy shape — no `run`
                    // body, so this never auto-confirms via
                    // `handleImmediateResponse`-style logic (approve doesn't
                    // have one; it stays `.pending` unconditionally).
                    return (200, Data(#"{"ok":true,"host_id":"local"}"#.utf8))
                }
                return (200, ActivityStoreTests.fourSourceBody(for: req.url?.path ?? ""))
            }
        )

        // A second, independent `RunDetailStore` for the same run — built
        // directly against the shared ledger rather than through
        // `ActivityStore`'s own HTTP-backed `client`, matching the "fake
        // client closures" seam `RunDetailStoreTests` already establishes.
        let runDetailStore = RunDetailStore(
            runID: "run-wf-1",
            host: nil,
            isRemote: false,
            fetchDetail: { throw StubHTTPError() },
            fetchGraph: { throw StubHTTPError() },
            fetchNetflow: { throw StubHTTPError() },
            fetchFindings: { throw StubHTTPError() },
            fetchTranscript: { _ in throw StubHTTPError() },
            runSignalsFactory: nil,
            transcriptTailFactory: nil,
            pendingActions: shared
        )

        await store.activate(kind: .workflows)

        let key = ActionKey.gate(runID: "run-wf-1", stepID: "gate-1", verb: .approve)
        #expect(runDetailStore.pendingActions.state(key) == .idle)

        await store.approve(runID: "run-wf-1", gate: "gate-1", host: "local")

        guard case .pending = runDetailStore.pendingActions.state(key) else {
            Issue.record("expected the RunDetailStore's view of the shared ledger to read .pending too, got \(runDetailStore.pendingActions.state(key))")
            return
        }

        store.deactivate()
        box.latest.finish()
    }

    /// Overload of `makeStore` accepting an explicit `pendingActions` — kept
    /// separate from the file's primary `makeStore` (used by every other
    /// test) rather than adding yet another optional parameter there, since
    /// this is the only test in the file that cares about sharing the
    /// ledger with a second, independently-constructed store.
    @MainActor
    private func makeStore(
        debounceInterval: Duration = .milliseconds(500),
        pendingActions: PendingActions,
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
        let store = ActivityStore(client: client, signalsFactory: { box.factory() }, debounceInterval: debounceInterval, pendingActions: pendingActions)
        return (store, box)
    }
}

/// Stand-in error for `RunDetailStore` fetch closures that must never
/// actually be called in a test scoped purely to `PendingActions` sharing —
/// see `activityStoreApproveIsVisibleAsPendingToASeparateRunDetailStoreSharingTheSameLedger`.
private struct StubHTTPError: Error, CustomStringConvertible {
    let description = "unused in this test"
}
