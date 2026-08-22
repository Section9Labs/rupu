import Testing
import Foundation
@testable import RupuStore
import RupuAPI

/// Path-routing HTTP stub for `LauncherStore`'s endpoints. Duplicated from
/// `ActivityStubURLProtocol` (see that type's doc comment for the
/// rationale — it's `internal` to a sibling test file, so a fresh copy is
/// needed here too). Tests run `.serialized` because `handler`/
/// `requestCount`/`pathHits` are class-level state shared across the whole
/// `URLProtocol` subclass.
final class LauncherStubURLProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) static var pathHits: [String: Int] = [:]

    static func session() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [LauncherStubURLProtocol.self]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        pathHits = [:]
        self.handler = handler
    }

    static func hitCount(_ path: String) -> Int {
        pathHits[path] ?? 0
    }

    /// Reads the POST body regardless of whether `URLSession` relayed it as
    /// `httpBody` or converted it to `httpBodyStream` on the way through —
    /// a documented quirk of intercepting requests via `URLProtocol`.
    static func bodyJSON(_ request: URLRequest) -> [String: Any]? {
        var data = request.httpBody
        if data == nil, let stream = request.httpBodyStream {
            stream.open()
            defer { stream.close() }
            var collected = Data()
            let bufferSize = 4096
            var buffer = [UInt8](repeating: 0, count: bufferSize)
            while stream.hasBytesAvailable {
                let read = stream.read(&buffer, maxLength: bufferSize)
                if read <= 0 { break }
                collected.append(buffer, count: read)
            }
            data = collected
        }
        guard let data, !data.isEmpty else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        if let path = request.url?.path {
            LauncherStubURLProtocol.pathHits[path, default: 0] += 1
        }
        guard let handler = LauncherStubURLProtocol.handler else {
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

/// De-flakes "wait for an async background effect to land" — same recipe
/// (and rationale) as `ActivityStoreTests.pollUntil`/`expectEventually`.
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
struct LauncherStoreTests {
    private static func agentJSON(name: String) -> String {
        #"{"name":"\#(name)","slug":"\#(name)","description":null,"provider":null,"model":null,"effort":null,"max_tokens":null,"tools":[],"scope":"project","scope_kind":"project","scope_id":null,"run_count":0,"last_run":null}"#
    }

    private static func workflowJSON(name: String) -> String {
        #"{"name":"\#(name)","scope":"project","scope_kind":"project","scope_id":null,"run_count":0,"last_run":null,"autoflow_enabled":null}"#
    }

    private static func workflowDetailJSON(name: String) -> String {
        #"""
        {"workflow":{"name":"\#(name)","inputs":{"target":{"type":"string","required":true,"enum":[]},"message":{"type":"string","required":false,"default":"hello","enum":[]}}},"yaml":"name: \#(name)\n","usage":{"cached_tokens":0,"cost_usd":null,"input_tokens":0,"output_tokens":0,"priced":false,"runs":0,"total_tokens":0},"scope":"project","scope_kind":"project","scope_id":null}
        """#
    }

    private static func hostJSON(id: String, status: String) -> String {
        #"{"id":"\#(id)","name":"\#(id)","transport_kind":"ssh","status":"\#(status)"}"#
    }

    @MainActor
    private func makeStore(
        respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data) = { _ in (200, Data("[]".utf8)) }
    ) -> LauncherStore {
        LauncherStubURLProtocol.reset(handler: respond)
        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: LauncherStubURLProtocol.session()
        )
        return LauncherStore(client: client)
    }

    // (a) `activate()` loads `agents`/`workflows` in parallel and returns as
    // soon as they land — the `GET /api/hosts` discovery call is fired in
    // the background and must never delay that return, even when it's
    // artificially slow.
    @MainActor @Test func activateLoadsDefinitionsInParallelAndHostsProgressivelyWithoutBlocking() async {
        let store = makeStore { req in
            guard let path = req.url?.path else { return (200, Data("[]".utf8)) }
            switch path {
            case "/api/agents":
                return (200, Data("[\(Self.agentJSON(name: "rupuso"))]".utf8))
            case "/api/workflows":
                return (200, Data("[\(Self.workflowJSON(name: "nightly"))]".utf8))
            case "/api/hosts":
                Thread.sleep(forTimeInterval: 0.08) // artificially slow
                return (200, Data("[\(Self.hostJSON(id: "local", status: "online"))]".utf8))
            default:
                return (200, Data("[]".utf8))
            }
        }

        await store.activate()

        // Definitions are already showing — the slow `/api/hosts` call has
        // not been waited on at all.
        #expect(store.agents.value?.map(\.name) == ["rupuso"])
        #expect(store.workflows.value?.map(\.name) == ["nightly"])
        #expect(store.hosts.isEmpty)

        await expectEventually("the slow /api/hosts discovery call lands and fills in hosts") {
            !store.hosts.isEmpty
        }
        #expect(store.hosts.map(\.id) == ["local"])
    }

    // (b) `selectDefinition` for a workflow fetches its detail and seeds
    // `workflowInputs` + `inputValues` — required input with no default
    // seeds an empty string, non-required input with a default seeds that
    // default.
    @MainActor @Test func selectDefinitionForWorkflowSeedsInputsWithDefaults() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/workflows/deploy" else { return (200, Data("[]".utf8)) }
            return (200, Data(Self.workflowDetailJSON(name: "deploy").utf8))
        }
        store.kind = .workflow

        await store.selectDefinition("deploy")

        #expect(store.selectedDefinition == "deploy")
        #expect(store.workflowInputs["target"]?.required == true)
        #expect(store.workflowInputs["message"]?.required == false)
        #expect(store.inputValues["target"] == "")
        #expect(store.inputValues["message"] == "hello")
    }

    // (c) `canLaunch` matrix: no definition blocks; a missing required
    // workflow input blocks; an empty prompt blocks agent/session; no
    // target host blocks even with everything else satisfied.
    @MainActor @Test func canLaunchMatrix() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/workflows/deploy" else { return (200, Data("[]".utf8)) }
            return (200, Data(Self.workflowDetailJSON(name: "deploy").utf8))
        }
        store.kind = .workflow
        #expect(store.canLaunch == false) // no definition selected

        await store.selectDefinition("deploy")
        #expect(store.canLaunch == false) // "target" required, still empty

        store.inputValues["target"] = "prod"
        #expect(store.canLaunch == true) // required input filled, "local" host default

        store.selectedHosts = []
        #expect(store.canLaunch == false) // no target host, and no fan-out
        store.fanOutAllHealthy = true
        // fan-out with zero known online hosts still resolves to zero
        // targets — canLaunch stays false until hosts are known.
        #expect(store.canLaunch == false)
        store.fanOutAllHealthy = false
        store.selectedHosts = ["local"]
        #expect(store.canLaunch == true)

        // Agent-run kind: prompt drives canLaunch instead of inputs.
        let agentStore = makeStore()
        agentStore.kind = .agentRun
        agentStore.selectedDefinition = "rupuso"
        #expect(agentStore.canLaunch == false) // empty prompt
        agentStore.prompt = "do the thing"
        #expect(agentStore.canLaunch == true)
    }

    // (d) A single-target launch POSTs once and returns the destination
    // `Route` directly for auto-navigation.
    @MainActor @Test func singleHostAgentLaunchReturnsRunDetailRoute() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            return (200, Data(#"{"run_id":"run-123","host_id":"local"}"#.utf8))
        }
        store.kind = .agentRun
        store.selectedDefinition = "rupuso"
        store.prompt = "do the thing"

        let route = await store.launch()

        #expect(route == .runDetail(id: "run-123", host: nil))
        #expect(store.launchResults == [LaunchOutcome(host: "local", result: .success(.runDetail(id: "run-123", host: nil)))])
        #expect(LauncherStubURLProtocol.hitCount("/api/agents/rupuso/run") == 1)
    }

    // (d2) Session kind maps its response's `session_id` to `.sessionDetail`.
    @MainActor @Test func singleHostSessionLaunchReturnsSessionDetailRoute() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/session" else { return (200, Data("[]".utf8)) }
            return (200, Data(#"{"session_id":"sess-9","host_id":"local"}"#.utf8))
        }
        store.kind = .session
        store.selectedDefinition = "rupuso"
        store.prompt = "hello"

        let route = await store.launch()

        #expect(route == .sessionDetail(id: "sess-9"))
    }

    // (e) A "local" target sends `host: null` in the body, not the literal
    // string `"local"`.
    @MainActor @Test func localTargetSendsNilHostInBody() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            let json = LauncherStubURLProtocol.bodyJSON(req)
            #expect(json?["host"] == nil)
            return (200, Data(#"{"run_id":"run-1","host_id":"local"}"#.utf8))
        }
        store.kind = .agentRun
        store.selectedDefinition = "rupuso"
        store.prompt = "hi"
        store.selectedHosts = ["local"]

        _ = await store.launch()
    }

    // (f) Fan-out over the online hosts only: 2 online + 1 offline yields
    // exactly 2 POSTs (the offline host is skipped outright), with outcomes
    // recorded per host including one failure.
    @MainActor @Test func fanOutSkipsOfflineHostsAndRecordsOutcomesIncludingOneFailure() async {
        let store = makeStore { req in
            switch req.url?.path {
            case "/api/hosts":
                let hosts = [
                    Self.hostJSON(id: "local", status: "online"),
                    Self.hostJSON(id: "mini", status: "online"),
                    Self.hostJSON(id: "kuki", status: "offline"),
                ]
                return (200, Data("[\(hosts.joined(separator: ","))]".utf8))
            case "/api/workflows/deploy/run":
                let json = LauncherStubURLProtocol.bodyJSON(req)
                let host = json?["host"] as? String
                if host == "mini" {
                    return (500, Data(#"{"error":"boom"}"#.utf8))
                }
                // nil host == "local"
                return (200, Data(#"{"run_id":"run-local-1","host_id":"local"}"#.utf8))
            default:
                return (200, Data("[]".utf8))
            }
        }
        store.kind = .workflow
        await store.activate()
        await expectEventually("hosts load") { store.hosts.count == 3 }

        store.selectedDefinition = "deploy" // no `selectDefinition` fetch — `workflowInputs` stays empty, no required inputs to gate on
        store.fanOutAllHealthy = true

        let route = await store.launch()

        #expect(route == nil) // multi-target: caller navigates per outcome row
        #expect(LauncherStubURLProtocol.hitCount("/api/workflows/deploy/run") == 2) // kuki skipped
        #expect(store.launchResults.count == 2)
        #expect(Set(store.launchResults.map(\.host)) == Set(["local", "mini"]))

        let localOutcome = store.launchResults.first { $0.host == "local" }
        let miniOutcome = store.launchResults.first { $0.host == "mini" }
        guard case .success(let route) = localOutcome?.result else {
            Issue.record("expected local outcome to succeed")
            return
        }
        #expect(route == .runDetail(id: "run-local-1", host: nil))
        guard case .failure(let message) = miniOutcome?.result else {
            Issue.record("expected mini outcome to fail")
            return
        }
        #expect(!message.isEmpty)
    }

    // (g) A 501 (no launch runtime configured) surfaces via the shared
    // `mutationErrorMessage` convention, both in the outcome and by leaving
    // `launch()` returning `nil` for a single failed target.
    @MainActor @Test func fiveOhOneSurfacesLaunchRuntimeMessageInOutcome() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            return (501, Data(#"{"error":"no launcher configured"}"#.utf8))
        }
        store.kind = .agentRun
        store.selectedDefinition = "rupuso"
        store.prompt = "hi"
        store.selectedHosts = ["local"]

        let route = await store.launch()

        #expect(route == nil)
        #expect(store.launchResults.count == 1)
        guard case .failure(let message) = store.launchResults.first?.result else {
            Issue.record("expected a failure outcome")
            return
        }
        #expect(message == "server lacks launch runtime — start with `rupu cp serve`")
    }

    // (h) The required-input client-side gap surfaces via `validationError`
    // rather than firing any request at all.
    @MainActor @Test func missingRequiredInputSetsValidationErrorWithoutPosting() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/workflows/deploy" else { return (200, Data("[]".utf8)) }
            return (200, Data(Self.workflowDetailJSON(name: "deploy").utf8))
        }
        store.kind = .workflow
        await store.selectDefinition("deploy")

        let route = await store.launch()

        #expect(route == nil)
        #expect(store.validationError != nil)
        #expect(store.launchResults.isEmpty)
        #expect(LauncherStubURLProtocol.hitCount("/api/workflows/deploy/run") == 0)
    }
}
