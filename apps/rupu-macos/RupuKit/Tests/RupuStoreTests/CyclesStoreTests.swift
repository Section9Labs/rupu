import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test infra (generation-token-isolated stub — see
// `ClaimsStoreTests.ClaimsStubURLProtocol`'s doc comment for the full
// rationale; duplicated here because it's `internal`/file-scoped to that
// sibling test file, same as every other store-test file's own copy of
// this rig. NOT reusing `ActivityStubURLProtocol` — that one has no
// generation-token isolation and is shared class-level state already keyed
// to `ActivityStoreTests`'s own `@Suite(.serialized)`; two independent
// suites racing on the same un-guarded static handler under parallel-suite
// load would be exactly the hazard the generation-token rig exists to
// close.)
final class CyclesStubURLProtocol: URLProtocol {
    private static let generationHeader = "X-Cycles-Stub-Generation"
    private static let lock = NSLock()
    nonisolated(unsafe) private static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) private static var pathHits: [String: Int] = [:]
    nonisolated(unsafe) private static var currentGeneration = 0

    static func session() -> URLSession {
        let generation = lock.withLock { currentGeneration }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [CyclesStubURLProtocol.self]
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
        Self.lock.withLock {
            if let path = request.url?.path {
                Self.pathHits[path, default: 0] += 1
            }
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
    _ description: String,
    timeout: Duration = .seconds(5),
    sourceLocation: SourceLocation = #_sourceLocation,
    _ condition: () -> Bool
) async {
    let ok = await pollUntil(timeout: timeout, condition)
    if !ok {
        Issue.record("timed out waiting for: \(description)", sourceLocation: sourceLocation)
    }
}

@Suite(.serialized)
struct CyclesStoreTests {
    private static let usageJSON = #"{"cached_tokens":0,"cost_usd":null,"input_tokens":0,"output_tokens":0,"priced":false,"runs":0,"total_tokens":0}"#

    private static func cycleJSON(id: String, startedAt: String = "2026-08-20T12:00:00Z", hostID: String? = "local") -> String {
        let hostField = hostID.map { #","host_id":"\#($0)""# } ?? ""
        return #"{"cycle_id":"\#(id)","mode":"tick","worker_name":"worker-a","started_at":"\#(startedAt)","finished_at":"2026-08-20T12:05:00Z","workflow_count":1,"ran_cycles":1,"skipped_cycles":0,"failed_cycles":0,"run_ids":["run-1"],"usage":\#(usageJSON)\#(hostField)}"#
    }

    private static func hostsJSON(_ hosts: [(id: String, status: String)]) -> Data {
        let entries = hosts.map { #"{"id":"\#($0.id)","name":"\#($0.id)","transport_kind":"ssh","status":"\#($0.status)"}"# }.joined(separator: ",")
        return Data("[\(entries)]".utf8)
    }

    private static func queryHost(_ req: URLRequest) -> String? {
        URLComponents(url: req.url!, resolvingAgainstBaseURL: false)?
            .queryItems?.first(where: { $0.name == "host" })?.value
    }

    @MainActor
    private func makeStore(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) -> CyclesStore {
        CyclesStubURLProtocol.reset(handler: handler)
        let client = CPClient(config: CPConfig(baseURL: URL(string: "http://localhost:7777")!), session: CyclesStubURLProtocol.session())
        return CyclesStore(client: client)
    }

    @MainActor @Test func activateLoadsLocalCyclesAndSetsContentState() async {
        let store = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" { return (200, Data("[]".utf8)) }
            guard url.path == "/api/runs/autoflows" else { return (200, Data("[]".utf8)) }
            return (200, Data("[\(Self.cycleJSON(id: "cycle-local"))]".utf8))
        }

        await store.activate()

        guard case .content = store.state else {
            Issue.record("expected .content, got \(store.state)")
            return
        }
        #expect(store.rows.map(\.cycleID) == ["cycle-local"])
        store.deactivate()
    }

    @MainActor @Test func emptyLocalPageIsEmptyState() async {
        let store = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" { return (200, Data("[]".utf8)) }
            return (200, Data("[]".utf8))
        }

        await store.activate()

        guard case .empty = store.state else {
            Issue.record("expected .empty, got \(store.state)")
            return
        }
        store.deactivate()
    }

    @MainActor @Test func onlineRemoteHostMergesProgressivelyAfterLocalTruthAlreadyShows() async {
        let store = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("mini", "online")]))
            }
            guard url.path == "/api/runs/autoflows" else { return (200, Data("[]".utf8)) }
            if Self.queryHost(req) == "mini" {
                return (200, Data("[\(Self.cycleJSON(id: "cycle-remote", hostID: "mini"))]".utf8))
            }
            return (200, Data("[\(Self.cycleJSON(id: "cycle-local"))]".utf8))
        }

        await store.activate()
        // Local truth already showing before any remote host answers.
        #expect(store.rows.map(\.cycleID) == ["cycle-local"])

        await expectEventually("the remote host's cycle merges in and pendingHosts drains") {
            store.rows.map(\.cycleID).sorted() == ["cycle-local", "cycle-remote"] && store.pendingHosts == 0
        }
        #expect(store.pendingHosts == 0)
        store.deactivate()
    }

    @MainActor @Test func failingRemoteHostNeverFailsStateAndContributesNothing() async {
        let store = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("mini", "online")]))
            }
            guard url.path == "/api/runs/autoflows" else { return (200, Data("[]".utf8)) }
            if Self.queryHost(req) == "mini" {
                return (500, Data(#"{"error":"boom"}"#.utf8))
            }
            return (200, Data("[\(Self.cycleJSON(id: "cycle-local"))]".utf8))
        }

        await store.activate()
        await expectEventually("the failing remote host's fetch resolves and pendingHosts drains") {
            store.pendingHosts == 0
        }

        #expect(store.rows.map(\.cycleID) == ["cycle-local"])
        guard case .content = store.state else {
            Issue.record("a failing remote host must never flip state to .failed, got \(store.state)")
            return
        }
        store.deactivate()
    }

    @MainActor @Test func offlineHostIsSkippedEntirelyAndNeverFetchedFrom() async {
        let store = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("kuki", "offline")]))
            }
            return (200, Data("[\(Self.cycleJSON(id: "cycle-local"))]".utf8))
        }

        await store.activate()
        await expectEventually("host discovery completes with nothing pending (offline host skipped)") {
            store.pendingHosts == 0
        }

        #expect(store.pendingHosts == 0)
        #expect(store.rows.map(\.cycleID) == ["cycle-local"])
        // Only ever fetched with `host=local` — never from the offline host.
        #expect(CyclesStubURLProtocol.hits("/api/runs/autoflows") == 1)
        store.deactivate()
    }

    @MainActor @Test func deactivateResetsPendingHostsAndCancelsInFlightWork() async {
        let remoteGate = DispatchSemaphore(value: 0)
        let store = makeStore { req in
            guard let url = req.url else { return (200, Data("[]".utf8)) }
            if url.path == "/api/hosts" {
                return (200, Self.hostsJSON([("mini", "online")]))
            }
            guard url.path == "/api/runs/autoflows" else { return (200, Data("[]".utf8)) }
            if Self.queryHost(req) == "mini" {
                _ = remoteGate.wait(timeout: .now() + 10)
                return (200, Data("[\(Self.cycleJSON(id: "cycle-remote", hostID: "mini"))]".utf8))
            }
            return (200, Data("[\(Self.cycleJSON(id: "cycle-local"))]".utf8))
        }

        await store.activate()
        await expectEventually("remote host discovery registers pendingHosts") { store.pendingHosts == 1 }

        store.deactivate()
        #expect(store.pendingHosts == 0)

        // The gated remote fetch, once released, must not resurrect
        // `pendingHosts` or append its row after deactivation — its
        // generation no longer matches.
        remoteGate.signal()
        try? await Task.sleep(for: .milliseconds(50))
        #expect(store.pendingHosts == 0)
        #expect(store.rows.map(\.cycleID) == ["cycle-local"])
    }
}
