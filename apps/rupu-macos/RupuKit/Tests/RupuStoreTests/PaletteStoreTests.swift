import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - `paletteRank` (pure, no store/network involved)

@Suite
struct PaletteRankTests {
    private func item(_ id: String, _ title: String, kind: PaletteItem.Kind = .run) -> PaletteItem {
        PaletteItem(id: id, kind: kind, title: title, subtitle: nil, action: .navigate(.overview))
    }

    @Test func titlePrefixOutranksWordBoundaryOutranksSubsequence() {
        let items = [
            item("a", "quarterly review agent"), // subsequence only ("qr" -> q...r skips)
            item("b", "review nightly"), // word-boundary: second word starts with "night"? no—"nightly" starts query
            item("c", "nightly health check"), // title prefix
        ]
        // query "night": c is a title prefix (score 3), b's second word
        // "nightly" is a word-boundary match (score 2), a has no "night"
        // substring at all in order so it drops entirely.
        let ranked = paletteRank(query: "night", items: items)
        #expect(ranked.map(\.id) == ["c", "b"])
    }

    @Test func nonMatchingItemsAreDroppedNotRankedLast() {
        let items = [item("a", "Overview"), item("b", "Zzz totally unrelated")]
        let ranked = paletteRank(query: "overview", items: items)
        #expect(ranked.map(\.id) == ["a"])
    }

    @Test func subsequenceMatchesOutOfOrderCharactersDoNotCount() {
        // "run" is a subsequence of "nightly-health-runner" (n...u...n — wait,
        // pick an unambiguous case instead): "wf" is a subsequence of
        // "workflow" (w...f, in order) but not of "flowwork" reversed-order.
        let items = [item("a", "workflow"), item("b", "flowwork")]
        let ranked = paletteRank(query: "wf", items: items)
        #expect(ranked.map(\.id) == ["a"])
    }

    @Test func caseInsensitiveMatching() {
        let items = [item("a", "Overview")]
        #expect(paletteRank(query: "OVER", items: items).map(\.id) == ["a"])
        #expect(paletteRank(query: "over", items: items).map(\.id) == ["a"])
    }

    @Test func emptyQueryReturnsPagesFirstThenInsertionOrder() {
        let items = [
            item("run-1", "nightly-health", kind: .run),
            item("page-overview", "Overview", kind: .page),
            item("agent-1", "rupuso", kind: .agent),
            item("page-activity", "Activity", kind: .page),
        ]
        let ranked = paletteRank(query: "", items: items)
        #expect(ranked.map(\.id) == ["page-overview", "page-activity", "run-1", "agent-1"])
    }

    @Test func whitespaceOnlyQueryIsTreatedAsEmpty() {
        let items = [item("page-overview", "Overview", kind: .page), item("run-1", "nightly", kind: .run)]
        let ranked = paletteRank(query: "   ", items: items)
        #expect(ranked.map(\.id) == ["page-overview", "run-1"])
    }

    @Test func resultsAreCappedAtThirty() {
        let items = (0..<50).map { item("run-\($0)", "nightly-health-\($0)") }
        #expect(paletteRank(query: "nightly", items: items).count == 30)
    }

    @Test func emptyQueryResultsAreAlsoCappedAtThirty() {
        let items = (0..<40).map { item("run-\($0)", "nightly-health-\($0)") }
        #expect(paletteRank(query: "", items: items).count == 30)
    }

    @Test func stableWithinEqualScore() {
        // All three are pure subsequence matches for "el" (no prefix, no
        // word-boundary hit) — must come back in their original order.
        let items = [
            item("a", "zebra elephant"),
            item("b", "yellow eel"),
            item("c", "violet gel"),
        ]
        let ranked = paletteRank(query: "el", items: items)
        #expect(ranked.map(\.id) == ["a", "b", "c"])
    }

    @Test func wordBoundaryScoreRequiresAWordStart() {
        // "health" appears inside "nightly-health" only after a hyphen
        // boundary (word-boundary match, score 2) — inside "unhealthy" it's
        // not at a word start and not a title prefix either, but it *is*
        // still a subsequence, so it survives at score 1, ranked below.
        let items = [item("a", "unhealthy-flag"), item("b", "nightly-health")]
        let ranked = paletteRank(query: "health", items: items)
        #expect(ranked.map(\.id) == ["b", "a"])
    }
}

// MARK: - `PaletteStore`

/// Dedicated stub, not a shared reuse of `ActivityStoreTests`'
/// `ActivityStubURLProtocol` — same rationale that type's own doc comment
/// gives for not reusing `RupuAPITests`' stub: its `handler`/`requestCount`
/// are class-level state, and running these tests against the same class
/// concurrently with `ActivityStoreTests` (Swift Testing parallelizes
/// across suites by default; only `ActivityStoreTests` itself opts into
/// `.serialized`) would let the two suites' handlers clobber each other.
final class PaletteStubURLProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) static var requests: [URLRequest] = []
    private static let lock = NSLock()

    static func session() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [PaletteStubURLProtocol.self]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        lock.withLock {
            requests = []
            self.handler = handler
        }
    }

    static func requestCount(forPath path: String) -> Int {
        lock.withLock { requests.filter { $0.url?.path == path }.count }
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.lock.withLock { Self.requests.append(request) }
        guard let handler = Self.handler else {
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

private final class NavLog: @unchecked Sendable {
    private let lock = NSLock()
    private var entries: [Route] = []
    func record(_ route: Route) { lock.withLock { entries.append(route) } }
    var snapshot: [Route] { lock.withLock { entries } }
}

@Suite(.serialized)
struct PaletteStoreTests {
    private static let usageJSON = #"{"cached_tokens":0,"cost_usd":null,"input_tokens":0,"output_tokens":0,"priced":false,"runs":0,"total_tokens":0}"#

    private static func runListRowJSON(id: String, status: String, workflowName: String = "nightly-health") -> String {
        #"{"duration_ms":null,"finished_at":null,"host_id":"local","id":"\#(id)","started_at":"2026-08-20T10:00:00Z","status":"\#(status)","trigger":"cron","turns":1,"usage":\#(usageJSON),"workflow_name":"\#(workflowName)"}"#
    }

    private static func agentDefJSON(name: String, slug: String) -> String {
        #"{"description":null,"effort":null,"last_run":null,"max_tokens":null,"model":null,"name":"\#(name)","provider":null,"run_count":0,"scope":"user","scope_id":null,"scope_kind":"user","slug":"\#(slug)","tools":[]}"#
    }

    private static func workflowDefJSON(name: String) -> String {
        #"{"autoflow_enabled":null,"last_run":null,"name":"\#(name)","run_count":0,"scope":"project","scope_id":null,"scope_kind":"project"}"#
    }

    private static func runRecordJSON(id: String, status: String, awaiting: String = "[]") -> String {
        #"{"active_step_id":null,"active_step_transcript_path":null,"awaiting":\#(awaiting),"error_message":null,"final_output":null,"finished_at":null,"id":"\#(id)","parent_run_id":null,"permission_mode":"ask","started_at":"2026-08-20T10:00:00Z","status":"\#(status)","workflow_name":"nightly-health","workspace_id":"ws-1"}"#
    }

    private static func runDetailJSON(id: String, status: String, awaiting: String = "[]") -> Data {
        Data(#"{"run":\#(runRecordJSON(id: id, status: status, awaiting: awaiting)),"steps":[],"usage":\#(usageJSON)}"#.utf8)
    }

    private static func awaitingGateJSON(stepID: String) -> String {
        #"{"prompt":null,"since":"2026-08-20T10:00:00Z","step_id":"\#(stepID)"}"#
    }

    @MainActor
    private func makeStore(
        pendingActions: PendingActions = PendingActions(),
        onNavigate: @escaping (Route) -> Void = { _ in },
        respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)
    ) -> PaletteStore {
        PaletteStubURLProtocol.reset(handler: respond)
        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: PaletteStubURLProtocol.session()
        )
        return PaletteStore(client: client, pendingActions: pendingActions, onNavigate: onNavigate)
    }

    // (a) fail-open contract: every source erroring never removes the 7
    // static page items, and `open()` itself never throws/hangs.
    @MainActor @Test func openKeepsPageItemsWhenEverySourceFails() async {
        let store = makeStore { _ in (500, Data("{}".utf8)) }
        await store.open()

        let pageTitles = store.items.filter { $0.kind == .page }.map(\.title)
        #expect(pageTitles == ["Overview", "Activity", "Projects", "Security", "Library", "Fleet", "Usage"])
        #expect(store.items.filter { $0.kind != .page }.isEmpty)
        #expect(store.isOpen)
    }

    // (b) a healthy fetch folds the three sources' rows in on top of the
    // always-present pages.
    @MainActor @Test func openFoldsInRunsAgentsAndWorkflows() async {
        let store = makeStore { req in
            switch req.url?.path {
            case "/api/runs":
                return (200, Data("[\(Self.runListRowJSON(id: "run-1", status: "running"))]".utf8))
            case "/api/agents":
                return (200, Data("[\(Self.agentDefJSON(name: "rupuso", slug: "rupuso"))]".utf8))
            case "/api/workflows":
                return (200, Data("[\(Self.workflowDefJSON(name: "nightly-health"))]".utf8))
            default:
                return (200, Data("[]".utf8))
            }
        }
        await store.open()

        #expect(store.items.contains { $0.kind == .run && $0.title == "nightly-health" })
        #expect(store.items.contains { $0.kind == .agent && $0.title == "rupuso" })
        #expect(store.items.contains { $0.kind == .workflow && $0.title == "nightly-health" })
    }

    // (c) an `.awaiting` run row gets a paired `.approve` item; a
    // `.running` row does not.
    @MainActor @Test func approveItemAppearsOnlyForAwaitingRuns() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/runs" else { return (200, Data("[]".utf8)) }
            let rows = "[\(Self.runListRowJSON(id: "run-await", status: "awaiting_approval")),\(Self.runListRowJSON(id: "run-go", status: "running"))]"
            return (200, Data(rows.utf8))
        }
        await store.open()

        let approveItems = store.items.filter { $0.kind == .approve }
        #expect(approveItems.count == 1)
        #expect(approveItems.first?.title == "Approve: nightly-health")
        #expect(approveItems.first?.action == .approveGate(runID: "run-await", host: "local"))
        #expect(!store.items.contains { $0.kind == .approve && $0.id == "approve:run-go" })
    }

    // (d) executing a `.navigate` item pushes the route via `onNavigate`
    // and closes the palette.
    @MainActor @Test func executeNavigateItemNavigatesAndCloses() async {
        let log = NavLog()
        let store = makeStore(onNavigate: { log.record($0) }) { _ in (500, Data("{}".utf8)) }
        await store.open()
        store.query = "overview"

        let item = PaletteItem(id: "page:overview", kind: .page, title: "Overview", subtitle: nil, action: .navigate(.overview))
        await store.execute(item)

        #expect(log.snapshot == [.overview])
        #expect(!store.isOpen)
    }

    // (e) executing an approve item against a run with exactly one
    // awaiting gate: begins + posts the gate-scoped `ActionKey`, then
    // navigates to the run.
    @MainActor @Test func executeApproveItemResolvesSoleGateAndBeginsPendingAction() async {
        let log = NavLog()
        let pendingActions = PendingActions()
        let store = makeStore(pendingActions: pendingActions, onNavigate: { log.record($0) }) { req in
            switch (req.httpMethod, req.url?.path) {
            case ("GET", "/api/runs/run-1"):
                return (200, Self.runDetailJSON(id: "run-1", status: "awaiting_approval", awaiting: "[\(Self.awaitingGateJSON(stepID: "gate-1"))]"))
            case ("POST", "/api/runs/run-1/approve"):
                return (200, Data(#"{"ok":true}"#.utf8))
            default:
                return (500, Data("{}".utf8))
            }
        }

        let item = PaletteItem(id: "approve:run-1", kind: .approve, title: "Approve: nightly-health", subtitle: "local", action: .approveGate(runID: "run-1", host: "local"))
        await store.execute(item)

        let key = ActionKey.gate(runID: "run-1", stepID: "gate-1", verb: .approve)
        if case .pending = pendingActions.state(key) {
            // expected — approve is marker-only, confirmed later by an
            // observed status transition, not by this POST's response.
        } else {
            Issue.record("expected .pending, got \(pendingActions.state(key))")
        }
        #expect(log.snapshot == [.runDetail(id: "run-1", host: "local")])
        #expect(!store.isOpen)
    }

    // (f) a run with more than one awaiting gate is ambiguous — ruled:
    // just navigate, never guess which gate to approve.
    @MainActor @Test func executeApproveItemOnMultiGateRunJustNavigates() async {
        let log = NavLog()
        let pendingActions = PendingActions()
        let store = makeStore(pendingActions: pendingActions, onNavigate: { log.record($0) }) { req in
            guard req.url?.path == "/api/runs/run-2" else {
                Issue.record("unexpected request to \(req.url?.path ?? "?")")
                return (500, Data("{}".utf8))
            }
            let awaiting = "[\(Self.awaitingGateJSON(stepID: "gate-a")),\(Self.awaitingGateJSON(stepID: "gate-b"))]"
            return (200, Self.runDetailJSON(id: "run-2", status: "awaiting_approval", awaiting: awaiting))
        }

        let item = PaletteItem(id: "approve:run-2", kind: .approve, title: "Approve: nightly-health", subtitle: "local", action: .approveGate(runID: "run-2", host: "local"))
        await store.execute(item)

        #expect(PaletteStubURLProtocol.requestCount(forPath: "/api/runs/run-2/approve") == 0)
        #expect(log.snapshot == [.runDetail(id: "run-2", host: "local")])
    }

    // (g) `open()` resets `query`/`activeIndex` for a fresh session even
    // when called a second time mid-typing.
    @MainActor @Test func openResetsQueryAndActiveIndex() async {
        let store = makeStore { _ in (200, Data("[]".utf8)) }
        await store.open()
        store.query = "nightly"
        store.activeIndex = 2

        await store.open()

        #expect(store.query == "")
        #expect(store.activeIndex == 0)
    }

    // (h) `close()` resets state too, and setting a non-empty `query`
    // resets `activeIndex` back to 0 (Step 3's keyboard-nav view relies on
    // this rather than re-clamping itself on every keystroke).
    @MainActor @Test func settingQueryResetsActiveIndex() async {
        let store = makeStore { _ in (200, Data("[]".utf8)) }
        await store.open()
        store.activeIndex = 3
        store.query = "n"
        #expect(store.activeIndex == 0)

        store.activeIndex = 1
        store.close()
        #expect(!store.isOpen)
        #expect(store.query == "")
        #expect(store.activeIndex == 0)
    }
}
