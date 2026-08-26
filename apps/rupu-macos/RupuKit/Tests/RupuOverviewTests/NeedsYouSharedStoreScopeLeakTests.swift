import Testing
import Foundation
@testable import RupuOverview
@testable import RupuStore
import RupuAPI

/// Reviewer's exact repro (perf & interaction arc, Plan 5 Task 2 fix-round-1
/// — the shared-`ActivityStore` review's Critical): before `ActivityStore`
/// was shared between `ActivityScreen` and `OverviewScreen`, `OverviewScreen`
/// held a PRIVATE instance that never had `scopeFilter`/`statusFilter` set —
/// the fleet-wide needs-you invariant `NeedsYouCard` documents held "for
/// free." Sharing broke it: `ActivityScreen.activate(kind:)` sets
/// `scopeFilter = model.scopeWsID` on every activation, and nothing resets
/// it when the operator navigates back to Overview, so a project-scoped
/// Activity visit silently under-reported every OTHER project's gates on
/// Overview afterward.
///
/// This test builds a REAL `ActivityStore` (network-stubbed, same idiom
/// `RupuStoreTests/ActivityStoreTests.swift` already establishes — that
/// type's own stub is `internal` to a different SPM test target and not
/// visible here, so this is a deliberate, minimal, self-contained copy, not
/// a missed reuse), sets `scopeFilter` exactly the way `ActivityScreen`
/// would, and proves the FULL pipeline `NeedsYouCard.body` actually runs —
/// `store.unscopedRows` piped into `deriveNeedsYou(rows:range:now:)` — still
/// surfaces the out-of-scope gate, while `store.rows` (the Activity table's
/// own projection) still correctly narrows away from it.

/// Minimal path-routing HTTP stub for `ActivityStore`'s four source
/// endpoints plus `GET /api/hosts` (returns `[]` — no remote fan-out needed
/// for this test). `.serialized` below since `handler` is class-level state
/// shared across the whole `URLProtocol` subclass, same as the
/// `RupuStoreTests` original.
private final class NeedsYouLeakStubURLProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?

    static func session() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [NeedsYouLeakStubURLProtocol.self]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        self.handler = handler
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let handler = NeedsYouLeakStubURLProtocol.handler else {
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

@Suite(.serialized)
struct NeedsYouSharedStoreScopeLeakTests {
    private static let usageJSON = #"{"cached_tokens":0,"cost_usd":null,"input_tokens":0,"output_tokens":0,"priced":false,"runs":0,"total_tokens":0}"#

    /// A workflow (gate) row — `ActivityRow.init(_: APIRunListRow)` always
    /// sets `project = nil` (only session rows carry a workspace id), so
    /// this can NEVER pass a non-nil `scopeFilter` — exactly the shape that
    /// exposed the leak.
    private static func awaitingWorkflowRowJSON(id: String, startedAt: String) -> String {
        #"{"duration_ms":null,"finished_at":null,"host_id":"local","id":"\#(id)","started_at":"\#(startedAt)","status":"awaiting_approval","trigger":"cron","turns":1,"usage":\#(usageJSON),"workflow_name":"nightly-health"}"#
    }

    @MainActor
    private func makeStore() -> ActivityStore {
        NeedsYouLeakStubURLProtocol.reset(handler: { req in
            switch req.url?.path {
            case "/api/runs/workflows":
                let gate = Self.awaitingWorkflowRowJSON(id: "run-gate-1", startedAt: "2026-08-20T13:00:00Z")
                return (200, Data("[\(gate)]".utf8))
            case "/api/hosts":
                return (200, Data("[]".utf8))
            default:
                return (200, Data("[]".utf8))
            }
        })
        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: NeedsYouLeakStubURLProtocol.session()
        )
        return ActivityStore(
            client: client,
            signalsFactory: { AsyncStream<StreamSignal<CPEvent>> { _ in } }
        )
    }

    @MainActor @Test func scopedActivityScreenVisitNeverHidesTheGateFromNeedsYouDerivation() async {
        let store = makeStore()
        await store.activate(kind: .all)
        #expect(store.rows.map(\.id).contains("run-gate-1"), "sanity: the gate is fetched before any scope is applied")

        // Exactly what `ActivityScreen.activate(kind:)` does on a
        // project-scoped visit — no other screen resets this once the
        // operator navigates back to Overview.
        store.scopeFilter = "some-other-project"

        // The Activity table's own projection correctly narrows away from
        // the out-of-scope gate — unchanged behavior, still correct.
        #expect(!store.rows.map(\.id).contains("run-gate-1"))

        // `NeedsYouCard.body`'s actual call: `deriveNeedsYou(rows: store.
        // unscopedRows, range:, now:)`.
        let result = deriveNeedsYou(rows: store.unscopedRows, range: .d7, now: Date())
        #expect(result.items.contains(where: { $0.row.id == "run-gate-1" && $0.kind == .gate }), "the out-of-scope gate must still reach the needs-you derivation")

        store.deactivate()
    }
}
