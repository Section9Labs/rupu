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
final class LauncherStubURLProtocol: URLProtocol, @unchecked Sendable {
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

    /// Final-review fix (fold-in, minor 4): `pathHits` increments
    /// synchronously, still on `startLoading()`'s own calling thread — the
    /// "dispatched" signal `hitCount`-based tests (e.g. `slowTarget
    /// NeverDelaysAnotherTargetsPOSTFromBeingSent`) poll for must land the
    /// instant a request is *submitted*, not once it's answered. The
    /// handler invocation itself, though, is dispatched onto a global
    /// concurrent queue rather than run inline: `URLSession` funnels every
    /// `startLoading()` call for a given custom `URLProtocol` class through
    /// a single shared queue, so a `Thread.sleep`-based "slow target" handler
    /// run *inline* here would block that shared queue from ever invoking
    /// `startLoading()` for a concurrently in-flight *second* request — an
    /// artifact of this stub's own delivery mechanism, not of
    /// `LauncherStore.launch()`'s genuinely-concurrent `TaskGroup` dispatch.
    /// Dispatching the handler (and thus any `Thread.sleep` inside it) off
    /// that shared queue is what lets two concurrent requests' completions
    /// actually race independently, matching what a real `URLSession` talking
    /// to a real server would do.
    override func startLoading() {
        if let path = request.url?.path {
            LauncherStubURLProtocol.pathHits[path, default: 0] += 1
        }
        guard let handler = LauncherStubURLProtocol.handler else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let capturedRequest = request
        DispatchQueue.global().async { [weak self] in
            guard let self else { return }
            let (status, body) = handler(capturedRequest)
            let response = HTTPURLResponse(
                url: capturedRequest.url!, statusCode: status, httpVersion: "HTTP/1.1",
                headerFields: ["Content-Type": "application/json"]
            )!
            self.client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            self.client?.urlProtocol(self, didLoad: body)
            self.client?.urlProtocolDidFinishLoading(self)
        }
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

    /// Minimal `workflowDetail` body whose single declared-input key is
    /// derived from `name` (`"<name>_input"`) — used by the
    /// `selectDefinition` race tests so the *content* of `workflowInputs`
    /// unambiguously identifies which definition's fetch actually won,
    /// independent of `selectedDefinition` (which is set synchronously and
    /// so is never itself racy).
    private static func racyWorkflowDetailJSON(name: String) -> String {
        #"""
        {"workflow":{"name":"\#(name)","inputs":{"\#(name)_input":{"type":"string","required":true,"enum":[]}}},"yaml":"name: \#(name)\n"}
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
        // De-flake (timed-stub sweep): `#expect(store.hosts.isEmpty)` below
        // is a MID-FLIGHT assertion — it only means anything while
        // discovery genuinely hasn't answered. An 80ms `Thread.sleep` made
        // that "probably true"; under parallel-suite load the sleep can
        // elapse while the main actor is starved, hosts fill in, and the
        // assertion fails on a store that behaved correctly. The response
        // is now held on a gate the test opens only after asserting.
        let hostsGate = DispatchSemaphore(value: 0)
        let store = makeStore { req in
            guard let path = req.url?.path else { return (200, Data("[]".utf8)) }
            switch path {
            case "/api/agents":
                return (200, Data("[\(Self.agentJSON(name: "rupuso"))]".utf8))
            case "/api/workflows":
                return (200, Data("[\(Self.workflowJSON(name: "nightly"))]".utf8))
            case "/api/hosts":
                // Held until the test has asserted that `activate()` came
                // back WITHOUT waiting on discovery (see the gate's
                // rationale at the call site). The 10s cap only bounds a
                // hung test; it is never a timing knob.
                _ = hostsGate.wait(timeout: .now() + 10)
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

        // Only now let discovery answer.
        hostsGate.signal()

        await expectEventually("the gated /api/hosts discovery call lands and fills in hosts") {
            !store.hosts.isEmpty
        }
        #expect(store.hosts.map(\.id) == ["local"])
    }

    // (b) `selectDefinition` for a workflow fetches its detail and seeds
    // `workflowInputs` + `inputValues` — required input with no default
    // seeds an empty string, non-required input with a default seeds that
    // default. `inputsLoadError` stays `nil` on a successful fetch.
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
        #expect(store.inputsLoadError == nil)
    }

    // (b2) Review fix (Important 1): a rapid A→B selection is
    // last-*called*-wins, not last-*resolved*-wins — A's slower fetch
    // landing after B's must never clobber B's already-applied inputs (nor
    // leave a mix of the two). `selectedDefinition == "B"` reads correctly
    // even without the fix (it's set synchronously); the fix is that
    // `workflowInputs` agrees with it.
    @MainActor @Test func rapidSelectDefinitionRaceOlderSlowerCallNeverClobbersNewerFasterOne() async {
        // De-flake (timed-stub sweep): this race was scripted with two
        // clocks — a 10ms head start for A and an 80ms "slow" A response —
        // and BOTH are load-bearing. `selectDefinition` bumps its
        // generation synchronously, so if A's `async let` hadn't actually
        // started within the head start, A became the NEWER call and
        // legitimately won (`selectedDefinition == "B"` then fails on a
        // correct store). Both are now signals: A's start is observed via
        // its request reaching the stub, and A's response is held on a gate
        // opened only once B has fully landed.
        let slowAGate = DispatchSemaphore(value: 0)
        let store = makeStore { req in
            guard let path = req.url?.path else { return (200, Data("[]".utf8)) }
            if path == "/api/workflows/A" {
                // The OLDER call — held until B has applied. 10s caps a
                // hung test only.
                _ = slowAGate.wait(timeout: .now() + 10)
                return (200, Data(Self.racyWorkflowDetailJSON(name: "A").utf8))
            }
            if path == "/api/workflows/B" {
                return (200, Data(Self.racyWorkflowDetailJSON(name: "B").utf8)) // fast, the NEWER call
            }
            return (200, Data("[]".utf8))
        }
        store.kind = .workflow

        async let first: Void = store.selectDefinition("A")
        // A's request reaching the stub proves `selectDefinition("A")` ran
        // (and captured the older generation) before B is ever called.
        await expectEventually("A's fetch is genuinely in flight before B is called") {
            LauncherStubURLProtocol.hitCount("/api/workflows/A") == 1
        }
        await store.selectDefinition("B") // the NEWER call — lands fully first

        // Only now let the older, superseded call answer, and await it.
        slowAGate.signal()
        await first

        #expect(store.selectedDefinition == "B")
        #expect(store.workflowInputs.keys.contains("B_input"))
        #expect(!store.workflowInputs.keys.contains("A_input"))
    }

    // (b3) Same race, names/timing reversed — the newer call (this time
    // "A", called second) still wins even though it's the fast one and the
    // older call ("B") is the slow one. Rules out any accidental bias
    // toward a specific name or call slot in the fix.
    @MainActor @Test func rapidSelectDefinitionRaceReversedOrderStillLastCallWins() async {
        // Same two-clock de-flake as (b2) above, names/roles reversed.
        let slowBGate = DispatchSemaphore(value: 0)
        let store = makeStore { req in
            guard let path = req.url?.path else { return (200, Data("[]".utf8)) }
            if path == "/api/workflows/B" {
                // The OLDER call this time — held until A has applied.
                _ = slowBGate.wait(timeout: .now() + 10)
                return (200, Data(Self.racyWorkflowDetailJSON(name: "B").utf8))
            }
            if path == "/api/workflows/A" {
                return (200, Data(Self.racyWorkflowDetailJSON(name: "A").utf8)) // fast, the NEWER call this time
            }
            return (200, Data("[]".utf8))
        }
        store.kind = .workflow

        async let first: Void = store.selectDefinition("B")
        await expectEventually("B's fetch is genuinely in flight before A is called") {
            LauncherStubURLProtocol.hitCount("/api/workflows/B") == 1
        }
        await store.selectDefinition("A") // the NEWER call this time

        slowBGate.signal()
        await first

        #expect(store.selectedDefinition == "A")
        #expect(store.workflowInputs.keys.contains("A_input"))
        #expect(!store.workflowInputs.keys.contains("B_input"))
    }

    // (b4) Review fix (fold 2): a `workflowDetail` fetch failure sets
    // `inputsLoadError` for the UI to render, while still failing open —
    // `workflowInputs`/`inputValues` land empty (not left stale) and
    // `canLaunch` is unaffected by the empty, unknown input set.
    @MainActor @Test func workflowDetailFetchFailureSetsInputsLoadErrorAndFailsOpen() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/workflows/deploy" else { return (200, Data("[]".utf8)) }
            return (500, Data(#"{"error":"boom"}"#.utf8))
        }
        store.kind = .workflow

        await store.selectDefinition("deploy")

        #expect(store.selectedDefinition == "deploy")
        #expect(store.workflowInputs.isEmpty)
        #expect(store.inputValues.isEmpty)
        #expect(store.inputsLoadError != nil)
        // Fail-open: no required inputs are known, so nothing blocks
        // `canLaunch` besides having a definition and a target host — both
        // satisfied by defaults (`selectedHosts == ["local"]`).
        #expect(store.canLaunch == true)
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
    // `Route` directly for auto-navigation. Final-review fix (Important 3):
    // an agent-run launch now maps to `.agentRunDetail` with `transcriptPath:
    // nil`, not `.runDetail` — a standalone agent run isn't an orchestrator
    // run and `GET /api/runs/:id` 404s for it (hotfix root cause C), so
    // `.runDetail` was a guaranteed dead end.
    @MainActor @Test func singleHostAgentLaunchReturnsAgentRunDetailRoute() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            return (200, Data(#"{"run_id":"run-123","host_id":"local"}"#.utf8))
        }
        store.kind = .agentRun
        store.selectedDefinition = "rupuso"
        store.prompt = "do the thing"

        let route = await store.launch()

        #expect(route == .agentRunDetail(id: "run-123", transcriptPath: nil, host: nil))
        #expect(store.launchResults == [LaunchOutcome(host: "local", result: .success(.agentRunDetail(id: "run-123", transcriptPath: nil, host: nil)))])
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

    // (e2) Phase 5A Task 4: a picked row's scope rides along in the launch
    // body when the target is local — `selectDefinition(_:scopeKind:scopeID:)`
    // (as `DefinitionPicker`'s row tap now calls it) captures the tapped
    // row's own scope, and `performLaunch` threads it through for a local
    // target (`hostField == nil`).
    @MainActor @Test func localTargetCarriesPickedRowScopeInLaunchBody() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            let json = LauncherStubURLProtocol.bodyJSON(req)
            #expect(json?["scope_kind"] as? String == "project")
            #expect(json?["scope_id"] as? String == "ws_a")
            return (200, Data(#"{"run_id":"run-1","host_id":"local"}"#.utf8))
        }
        store.kind = .agentRun
        await store.selectDefinition("rupuso", scopeKind: "project", scopeID: "ws_a")
        store.prompt = "hi"
        store.selectedHosts = ["local"]

        _ = await store.launch()
    }

    // (e3) A global-scope row has no `scopeID` (`AgentDefinition.scopeID` is
    // `nil` for a global-scoped definition) — the launch body must still
    // carry `scope_kind: "global"` while `scope_id` is nil-safe (omitted,
    // not an explicit `null`, and never crashes threading a `nil` through).
    @MainActor @Test func globalScopeRowSendsScopeKindWithNilScopeID() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            let json = LauncherStubURLProtocol.bodyJSON(req)
            #expect(json?["scope_kind"] as? String == "global")
            #expect(json?["scope_id"] == nil)
            return (200, Data(#"{"run_id":"run-1","host_id":"local"}"#.utf8))
        }
        store.kind = .agentRun
        await store.selectDefinition("rupuso", scopeKind: "global", scopeID: nil)
        store.prompt = "hi"
        store.selectedHosts = ["local"]

        _ = await store.launch()
    }

    // (e4) SERVER RULE (Phase 5A): scope is a LOCAL-ONLY affordance — a
    // remote-targeted launch must omit `scope_kind`/`scope_id` entirely
    // even though a scope was picked, since the server silently ignores
    // them there and sending them would misleadingly imply the remote host
    // honors the pin.
    @MainActor @Test func remoteTargetOmitsScopeEvenWhenPicked() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            let json = LauncherStubURLProtocol.bodyJSON(req)
            #expect(json?["scope_kind"] == nil)
            #expect(json?["scope_id"] == nil)
            #expect(json?["host"] as? String == "mini")
            return (200, Data(#"{"run_id":"run-1","host_id":"mini"}"#.utf8))
        }
        store.kind = .agentRun
        await store.selectDefinition("rupuso", scopeKind: "project", scopeID: "ws_a")
        store.prompt = "hi"
        store.selectedHosts = ["mini"]

        _ = await store.launch()
    }

    // (e5) A fan-out launch targeting both "local" and a remote host sends
    // scope on the local target's POST only — proves rule 1's "local
    // target gets scope, remote targets don't" per-target, not per-batch.
    @MainActor @Test func fanOutSendsScopeOnlyToLocalTarget() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            let json = LauncherStubURLProtocol.bodyJSON(req)
            let host = json?["host"] as? String
            if host == "mini" {
                #expect(json?["scope_kind"] == nil)
                return (200, Data(#"{"run_id":"run-mini","host_id":"mini"}"#.utf8))
            }
            // nil host == "local"
            #expect(json?["scope_kind"] as? String == "project")
            #expect(json?["scope_id"] as? String == "ws_a")
            return (200, Data(#"{"run_id":"run-local","host_id":"local"}"#.utf8))
        }
        store.kind = .agentRun
        await store.selectDefinition("rupuso", scopeKind: "project", scopeID: "ws_a")
        store.prompt = "hi"
        store.selectedHosts = ["local", "mini"]

        let route = await store.launch()

        #expect(route == nil) // multi-target
        #expect(store.launchResults.count == 2)
    }

    // (e6) A plain `selectedDefinition =` assignment (bypassing
    // `selectDefinition`, as several tests above and any future
    // programmatic caller might do) leaves scope unset — no scope fields
    // ride along, same as launches had before this phase.
    @MainActor @Test func directSelectedDefinitionAssignmentCarriesNoScope() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            let json = LauncherStubURLProtocol.bodyJSON(req)
            #expect(json?["scope_kind"] == nil)
            #expect(json?["scope_id"] == nil)
            return (200, Data(#"{"run_id":"run-1","host_id":"local"}"#.utf8))
        }
        store.kind = .agentRun
        store.selectedDefinition = "rupuso"
        store.prompt = "hi"
        store.selectedHosts = ["local"]

        _ = await store.launch()

        #expect(store.selectedScopeKind == nil)
        #expect(store.selectedScopeID == nil)
    }

    // (e7) Prefill seam (Phase 5A Task 7): `prefill(kind:name:scopeKind:
    // scopeID:)` selects the definition (and its scope) synchronously
    // enough that a subsequent `activate()` call never clobbers it — proven
    // here by calling `activate()` *after* `prefill`, matching the
    // presenter's real call order (prefill before the sheet's own
    // `.task { await store.activate() }` fires on appear).
    @MainActor @Test func prefillSelectsDefinitionAndScopeSurvivingSubsequentActivate() async {
        let store = makeStore { req in
            switch req.url?.path {
            case "/api/agents":
                return (200, Data("[\(Self.agentJSON(name: "rupuso"))]".utf8))
            case "/api/workflows":
                return (200, Data("[]".utf8))
            default:
                return (200, Data("[]".utf8))
            }
        }

        await store.prefill(kind: .agentRun, name: "rupuso", scopeKind: "project", scopeID: "ws_a")

        #expect(store.kind == .agentRun)
        #expect(store.selectedDefinition == "rupuso")
        #expect(store.selectedScopeKind == "project")
        #expect(store.selectedScopeID == "ws_a")

        await store.activate()

        // `activate()` only populates `agents`/`workflows`/`hosts` — the
        // prefilled selection is untouched.
        #expect(store.selectedDefinition == "rupuso")
        #expect(store.selectedScopeKind == "project")
        #expect(store.selectedScopeID == "ws_a")
        #expect(store.agents.value?.map(\.name) == ["rupuso"])
    }

    // (e8) Prefill of a workflow fetches its declared inputs exactly as a
    // picker tap would — the prefilled sheet's input form isn't empty.
    @MainActor @Test func prefillOfWorkflowSeedsDeclaredInputs() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/workflows/deploy" else { return (200, Data("[]".utf8)) }
            return (200, Data(Self.workflowDetailJSON(name: "deploy").utf8))
        }

        await store.prefill(kind: .workflow, name: "deploy", scopeKind: "global", scopeID: nil)

        #expect(store.kind == .workflow)
        #expect(store.selectedDefinition == "deploy")
        #expect(store.selectedScopeKind == "global")
        #expect(store.selectedScopeID == nil)
        #expect(store.workflowInputs["target"]?.required == true)
        #expect(store.inputValues["message"] == "hello")
    }

    /// Task 4 review carry-over, closed in Task 7: prefilling a name that
    /// isn't in the loaded lists (a stale/typo'd name, or a definition
    /// deleted between the Library row rendering and its Launch button
    /// firing) must never fabricate a selection. For `.workflow` — the only
    /// kind `prefill`/`selectDefinition` actually validates against the
    /// server, via `workflowDetail(name:)` — a 404 fails OPEN honestly
    /// (`selectDefinition`'s existing catch branch): `selectedDefinition`
    /// still records what was asked for (so the form doesn't silently
    /// revert to "nothing selected", which would hide what the user
    /// clicked), but `workflowInputs`/`inputValues` stay empty and
    /// `inputsLoadError` carries the real server message — no phantom
    /// input rows, no fabricated defaults. A subsequent `launch()` attempt
    /// then surfaces the SAME server truth (the workflow genuinely doesn't
    /// exist) rather than this store papering over it with an invented
    /// success path.
    @MainActor @Test func prefillWithMissingWorkflowNameFailsOpenWithoutFabricatingInputs() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/workflows/ghost" else { return (200, Data("[]".utf8)) }
            return (404, Data(#"{"error":"workflow ghost not found"}"#.utf8))
        }

        await store.prefill(kind: .workflow, name: "ghost", scopeKind: "global", scopeID: nil)

        // The honest record of what was asked for — not silently cleared.
        #expect(store.selectedDefinition == "ghost")
        // No fabricated declared-input state from a definition that was
        // never actually loaded.
        #expect(store.workflowInputs.isEmpty)
        #expect(store.inputValues.isEmpty)
        #expect(store.inputsLoadError?.contains("workflow ghost not found") == true)
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
        guard case .failure(let error) = miniOutcome?.result else {
            Issue.record("expected mini outcome to fail")
            return
        }
        #expect(!error.text.isEmpty)
    }

    // (f2) Review fix (Important 2): a fast target's `LaunchOutcome` lands
    // in `launchResults` while a slower target is still in flight — the
    // fan-out publishes progressively, not all-at-once after the last one
    // finishes.
    //
    // Timed-stub sweep (2026-08-25): "aslow" is held on a BOUNDED gate the
    // test opens once the mid-flight assertion has been made, not on a
    // `Thread.sleep` the test then races. The historical objection to a
    // gate here no longer applies: the earlier, genuinely-flaky version
    // parked its wait INLINE in `startLoading()`, on the single shared
    // queue `URLSession` funnels every custom-`URLProtocol` request
    // through, so the *other* target's request could not even be submitted
    // while it was held (verified: a blocked handler stalls the whole
    // session). `startLoading()` now dispatches the handler off that queue
    // (see its doc comment, fold-in minor 4), so a held response blocks
    // only its own target — which is exactly what this test needs — and
    // the wait is capped at 10s purely to bound a hung test.
    //
    // Final-review fix (fold-in, minor 4): the slow host is named "aslow",
    // not "slow" — `resolvedTargets()` returns `selectedHosts.sorted()`, so
    // "aslow" sorts *before* "local" (`"a" < "l"`). If a future regression
    // ever turned the fan-out back into a serial `for` loop over that sorted
    // order, "aslow" would be the *first* target dispatched and its held
    // response would block "local"'s request from ever being sent — this
    // test would then fail outright (on the gate's 10s cap) instead of
    // silently passing because the already-fast host happened to be visited
    // first.
    @MainActor @Test func fastTargetOutcomeAppearsInLaunchResultsWhileSlowTargetStillInFlight() async {
        let slowHostGate = DispatchSemaphore(value: 0)
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            let json = LauncherStubURLProtocol.bodyJSON(req)
            let host = (json?["host"] as? String) ?? "local"
            if host == "aslow" {
                // Held until the test has asserted that "local"'s outcome
                // published on its own. 10s caps a hung test only.
                _ = slowHostGate.wait(timeout: .now() + 10)
                return (200, Data(#"{"run_id":"run-slow","host_id":"aslow"}"#.utf8))
            }
            return (200, Data(#"{"run_id":"run-fast","host_id":"local"}"#.utf8))
        }
        store.kind = .agentRun
        store.selectedDefinition = "rupuso"
        store.prompt = "hi"
        store.selectedHosts = ["local", "aslow"]

        let launchTask = Task { await store.launch() }

        // "aslow" cannot possibly have answered — it is gated — so this can
        // only pass by "local" genuinely publishing on its own, and the
        // generous default ceiling never waits out a real failure.
        await expectEventually("the fast target's outcome lands while the slow one is still stuck") {
            store.launchResults.contains { $0.host == "local" }
        }
        #expect(store.launchResults.count == 1) // "aslow" hasn't landed yet
        #expect(store.launchResults.first?.host == "local")

        // Only now let the slow target answer.
        slowHostGate.signal()
        _ = await launchTask.value

        #expect(store.launchResults.count == 2)
        #expect(Set(store.launchResults.map(\.host)) == Set(["local", "aslow"]))
    }

    // (f3) Review fix (Important 2): a slow target must never delay another
    // target's own POST from being *sent* — proven here by both requests'
    // path hit count reaching 2 (both dispatched) while the slow target is
    // still unanswered, not merely by favorable overall timing. See (f2)'s
    // doc comment for why the slow host is held on a bounded gate (and why
    // the historical objection to gating here no longer applies), and on why
    // it is named "aslow" rather than "slow" (sorts first — a serial-loop
    // regression must fail this test, not accidentally pass it).
    //
    // Gating strictly strengthens this one: "aslow" is now guaranteed
    // unanswered while the dispatch count is observed, where the old 300ms
    // sleep merely made it likely. `!launchResults.contains { aslow }` is
    // therefore an assertion about the store, not about the clock.
    @MainActor @Test func slowTargetNeverDelaysAnotherTargetsPOSTFromBeingSent() async {
        let slowHostGate = DispatchSemaphore(value: 0)
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            let json = LauncherStubURLProtocol.bodyJSON(req)
            let host = (json?["host"] as? String) ?? "local"
            if host == "aslow" {
                // Held until both dispatches have been observed. 10s caps a
                // hung test only.
                _ = slowHostGate.wait(timeout: .now() + 10)
                return (200, Data(#"{"run_id":"run-slow","host_id":"aslow"}"#.utf8))
            }
            return (200, Data(#"{"run_id":"run-fast","host_id":"local"}"#.utf8))
        }
        store.kind = .agentRun
        store.selectedDefinition = "rupuso"
        store.prompt = "hi"
        store.selectedHosts = ["local", "aslow"]

        let launchTask = Task { await store.launch() }

        // Both requests dispatched (hit count 2) while "aslow" is provably
        // still unanswered — proves sending isn't serialized behind another
        // target's completion.
        await expectEventually("both requests are sent even though \"aslow\" hasn't answered yet") {
            LauncherStubURLProtocol.hitCount("/api/agents/rupuso/run") == 2
        }
        // "local"'s own completion is a separate event from its dispatch —
        // polled, not assumed to have already landed at the instant the
        // dispatch count was observed (that assumption was its own small
        // race in the pre-gate version of this test).
        await expectEventually("the fast target's outcome publishes") {
            store.launchResults.contains { $0.host == "local" }
        }
        #expect(!store.launchResults.contains { $0.host == "aslow" }) // gated — provably still in flight

        slowHostGate.signal()
        _ = await launchTask.value

        #expect(store.launchResults.count == 2)
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
        guard case .failure(let error) = store.launchResults.first?.result else {
            Issue.record("expected a failure outcome")
            return
        }
        #expect(error == .message("server lacks launch runtime — start with `rupu cp serve`"))
    }

    // (g2) Final-review fix (Important 2): a `.session` launch never
    // reaches a non-local target, even when `selectedHosts` explicitly
    // includes one — `resolvedTargets()` hard-filters to `"local"` for this
    // kind, defense in depth alongside `HostChips`'s `localOnly` mode (a
    // view-level guard this store-level test can't exercise directly).
    // Both an explicit multi-host selection and `fanOutAllHealthy` are
    // covered — either path into `resolvedTargets()` must land on the same
    // filtered result.
    @MainActor @Test func sessionLaunchHardFiltersToLocalEvenWithNonLocalHostsSelected() async {
        let store = makeStore { req in
            switch req.url?.path {
            case "/api/hosts":
                let hosts = [
                    Self.hostJSON(id: "local", status: "online"),
                    Self.hostJSON(id: "mini", status: "online"),
                ]
                return (200, Data("[\(hosts.joined(separator: ","))]".utf8))
            case "/api/agents/rupuso/session":
                let json = LauncherStubURLProtocol.bodyJSON(req)
                #expect(json?["host"] == nil) // "local" only, sent as nil
                return (200, Data(#"{"session_id":"sess-1","host_id":"local"}"#.utf8))
            default:
                return (200, Data("[]".utf8))
            }
        }
        store.kind = .session
        await store.activate()
        await expectEventually("hosts load") { store.hosts.count == 2 }

        store.selectedDefinition = "rupuso"
        store.prompt = "hi"
        store.selectedHosts = ["local", "mini"]

        let route = await store.launch()

        #expect(route == .sessionDetail(id: "sess-1"))
        #expect(store.launchResults == [LaunchOutcome(host: "local", result: .success(.sessionDetail(id: "sess-1")))])
        #expect(LauncherStubURLProtocol.hitCount("/api/agents/rupuso/session") == 1) // "mini" never targeted

        // Same hard filter applies via the fan-out path.
        LauncherStubURLProtocol.pathHits = [:]
        store.selectedHosts = []
        store.fanOutAllHealthy = true

        let fanOutRoute = await store.launch()

        #expect(fanOutRoute == .sessionDetail(id: "sess-1"))
        #expect(LauncherStubURLProtocol.hitCount("/api/agents/rupuso/session") == 1) // still just "local"
    }

    // (i) Dismiss-gating seam: `isLaunchInFlight` is the single
    // pending-state read the sheet's Cancel button AND its
    // `interactiveDismissDisabled` gate share — true exactly while the
    // batch's `ActionKey("launcher", .launch)` key is `.pending`, false
    // before any launch and false again once the batch resolves. The
    // Esc/click-outside gating itself is view-only (SwiftUI modifier), but
    // this proves the state it reads flips at the right moments.
    @MainActor @Test func isLaunchInFlightTrueWhileLaunchPendingFalseOnceResolved() async {
        // De-flake (timed-stub sweep): `isLaunchInFlight == true` is only
        // observable WHILE the POST is unresolved. A 300ms `Thread.sleep`
        // made that window probable; under parallel-suite load it can
        // elapse before the poll below ever runs, after which the flag is
        // (correctly) false and the poll times out on a healthy store. The
        // response is now held until the flag has actually been observed.
        let launchGate = DispatchSemaphore(value: 0)
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            _ = launchGate.wait(timeout: .now() + 10) // 10s caps a hung test only
            return (200, Data(#"{"run_id":"run-1","host_id":"local"}"#.utf8))
        }
        store.kind = .agentRun
        store.selectedDefinition = "rupuso"
        store.prompt = "hi"

        #expect(store.isLaunchInFlight == false) // nothing fired yet

        let launchTask = Task { await store.launch() }
        await expectEventually("the launch begins and reads as in flight") {
            store.isLaunchInFlight
        }

        // Only now let the POST resolve.
        launchGate.signal()
        _ = await launchTask.value
        #expect(store.isLaunchInFlight == false) // confirmed — no longer pending
    }

    // (i2) A failed launch resolves the pending key to `.failed`, which must
    // read as NOT in flight — the sheet becomes dismissable again so the
    // operator isn't trapped behind a launch that already errored out.
    @MainActor @Test func isLaunchInFlightFalseAfterFailedLaunch() async {
        let store = makeStore { req in
            guard req.url?.path == "/api/agents/rupuso/run" else { return (200, Data("[]".utf8)) }
            return (500, Data(#"{"error":"boom"}"#.utf8))
        }
        store.kind = .agentRun
        store.selectedDefinition = "rupuso"
        store.prompt = "hi"

        let route = await store.launch()

        #expect(route == nil)
        #expect(store.isLaunchInFlight == false) // failed — not pending, dismiss is allowed
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
