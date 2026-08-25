import Testing
import Foundation
@testable import RupuAPI

/// Stubs every request made through a `URLSession` configured with it,
/// so `CPClientTests` never touches the network. Tests in this file run
/// serialized (`@Suite(.serialized)`) because the handler/last-request are
/// class-level state shared across the whole `URLProtocol` subclass.
final class StubURLProtocol: URLProtocol {
    nonisolated(unsafe) static var requestHandler: (@Sendable (URLRequest) -> (status: Int, headers: [String: String], body: Data))?
    nonisolated(unsafe) static var lastRequest: URLRequest?
    /// When set, `startLoading()` fails the request with this error instead
    /// of running `requestHandler` — used by the cancellation-mapping test
    /// to simulate `URLSession` surfacing a cancelled request as
    /// `URLError(.cancelled)`.
    nonisolated(unsafe) static var failureError: Error?

    static func session() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [StubURLProtocol.self]
        return URLSession(configuration: config)
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        StubURLProtocol.lastRequest = request
        if let failureError = StubURLProtocol.failureError {
            client?.urlProtocol(self, didFailWithError: failureError)
            return
        }
        guard let handler = StubURLProtocol.requestHandler else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let (status, headers, body) = handler(request)
        let response = HTTPURLResponse(
            url: request.url!,
            statusCode: status,
            httpVersion: "HTTP/1.1",
            headerFields: headers
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

@Suite(.serialized)
struct CPClientTests {
    @Test func hostInfoHitsExpectedPathAndDecodesFixture() async throws {
        let fixture = try Fixtures.data("host_info.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let info = try await client.hostInfo()

        #expect(info.version == "0.71.0")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/host/info")
    }

    @Test func bearerHeaderPresentOnlyWhenTokenConfigured() async throws {
        let fixture = try Fixtures.data("host_info.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, [:], fixture) }

        let noTokenClient = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        _ = try await noTokenClient.hostInfo()
        #expect(StubURLProtocol.lastRequest?.value(forHTTPHeaderField: "Authorization") == nil)

        StubURLProtocol.lastRequest = nil
        let tokenClient = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!, token: "secret-token"),
            session: StubURLProtocol.session()
        )
        _ = try await tokenClient.hostInfo()
        #expect(StubURLProtocol.lastRequest?.value(forHTTPHeaderField: "Authorization") == "Bearer secret-token")
    }

    @Test func unauthorizedResponseMapsToUnauthorizedError() async throws {
        StubURLProtocol.requestHandler = { _ in (401, [:], Data()) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )

        await #expect(throws: CPError.unauthorized) {
            _ = try await client.hostInfo()
        }
    }

    @Test func serverErrorMapsToHTTPErrorWithStatusAndBody() async throws {
        let body = Data(#"{"error":"boom"}"#.utf8)
        StubURLProtocol.requestHandler = { _ in (500, [:], body) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )

        await #expect(throws: CPError.http(status: 500, body: #"{"error":"boom"}"#)) {
            _ = try await client.hostInfo()
        }
    }

    @Test func runsHitsExpectedPathAndQueryAndDecodesFixture() async throws {
        let fixture = try Fixtures.data("run_list_row.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let rows = try await client.runs(offset: 0, limit: 50)

        #expect(rows.count == 2)
        let url = StubURLProtocol.lastRequest?.url
        #expect(url?.path == "/api/runs")
        let query = url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }
        #expect(query?.contains(URLQueryItem(name: "offset", value: "0")) == true)
        #expect(query?.contains(URLQueryItem(name: "limit", value: "50")) == true)
    }

    @Test func runDetailHitsRunIDPathAndDecodesFixture() async throws {
        let fixture = try Fixtures.data("run_detail.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let detail = try await client.runDetail(id: "run-01")

        #expect(detail.run.id == "run-01")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/runs/run-01")
    }

    @Test func runGraphHitsGraphSubpathAndDecodesFixture() async throws {
        let fixture = try Fixtures.data("run_graph.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let graph = try await client.runGraph(id: "run-02", host: "mini")

        #expect(graph.run.id == "run-02")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/runs/run-02/graph")
        let query = StubURLProtocol.lastRequest?.url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }
        #expect(query?.contains(URLQueryItem(name: "host", value: "mini")) == true)
    }

    @Test func transcriptSendsPathAsQueryParameterNotURLSegment() async throws {
        let fixture = Data(#"{"events":[],"summary":null}"#.utf8)
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let page = try await client.transcript(path: "t/plan.jsonl")

        #expect(page.events.isEmpty)
        #expect(page.summary == nil)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/transcript")
        let query = StubURLProtocol.lastRequest?.url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }
        #expect(query?.contains(URLQueryItem(name: "path", value: "t/plan.jsonl")) == true)
    }

    @Test func runFindingsSendsRunIDAsQueryParameter() async throws {
        let fixture = try Fixtures.data("findings_run.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let findings = try await client.runFindings(id: "run-40")

        #expect(findings.findings.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/findings")
        let query = StubURLProtocol.lastRequest?.url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }
        #expect(query?.contains(URLQueryItem(name: "run_id", value: "run-40")) == true)
    }

    /// Phase 5A's Projects Findings tab: `findings(wsID:)` sends `ws_id`,
    /// never `run_id` — a distinct query key from `runFindings(id:)` above,
    /// not an overload of the same one. Reuses `findings_run.json` — the
    /// response envelope shape (`{findings, summary}`) is identical
    /// regardless of which query key scoped it server-side.
    @Test func findingsWsIDSendsWsIDAsQueryParameter() async throws {
        let fixture = try Fixtures.data("findings_run.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let findings = try await client.findings(wsID: "ws-1")

        #expect(findings.findings.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/findings")
        let query = StubURLProtocol.lastRequest?.url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }
        #expect(query?.contains(URLQueryItem(name: "ws_id", value: "ws-1")) == true)
        #expect(query?.contains(where: { $0.name == "run_id" }) == false)
    }

    @Test func runNetflowHitsNetflowSubpathAndDecodesFixture() async throws {
        let fixture = try Fixtures.data("netflow_run.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let netflow = try await client.runNetflow(id: "run-40")

        #expect(netflow.flows.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/runs/run-40/netflow")
    }

    @Test func sessionDetailAndSessionRunsHitExpectedPaths() async throws {
        // `GET /api/sessions/:id` returns a bare object in the same
        // `SessionDto` shape as the list endpoint (session_rows.json),
        // just not wrapped in an array.
        let singleSession = Data(#"{"session_id":"sess-1","agent_name":"rupuso","model":"claude","provider_name":"anthropic","total_turns":1,"total_tokens_in":1,"total_tokens_out":1,"total_tokens_cached":0,"created_at":"2026-08-20T12:00:00Z","updated_at":"2026-08-20T12:00:00Z","workspace_id":"ws-1"}"#.utf8)
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], singleSession) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let detail = try await client.sessionDetail(id: "sess-1")
        #expect(detail.sessionID == "sess-1")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/sessions/sess-1")

        let runsBody = Data(#"[{"run_id":"run-1","prompt":"do x","transcript_path":"t/r1.jsonl","status":"completed","started_at":"2026-08-20T12:00:00Z","completed_at":"2026-08-20T12:05:00Z","tokens_in":100,"tokens_out":50,"duration_ms":5000,"error":null}]"#.utf8)
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], runsBody) }
        let runs = try await client.sessionRuns(id: "sess-1")
        #expect(runs.count == 1)
        #expect(runs[0].runID == "run-1")
        #expect(runs[0].tokensIn == 100)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/sessions/sess-1/runs")
    }

    // MARK: - Fleet & project detail (Phase 5A)

    @Test func workersHitsExpectedPathAndDecodesFixture() async throws {
        let fixture = try Fixtures.data("workers.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let rows = try await client.workers()

        #expect(rows.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/workers")
    }

    @Test func autoflowDefinitionsHitsExpectedPathAndDecodesFixture() async throws {
        let fixture = try Fixtures.data("autoflow_defs.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let defs = try await client.autoflowDefinitions()

        #expect(defs.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/autoflows")
    }

    @Test func projectDetailHitsWsIDPathAndDecodesFixture() async throws {
        let fixture = try Fixtures.data("project_detail.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let detail = try await client.projectDetail(wsID: "ws-1")

        #expect(detail.project.wsID == "ws-1")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/projects/ws-1")
    }

    /// `projectRuns`/`projectSessions` are local-only (no `?host=`) — this
    /// proves the offset/limit query and that no `host` param is ever sent,
    /// unlike the fleet-wide `runs`/`sessions` methods.
    @Test func projectRunsAndProjectSessionsSendOffsetLimitAndNoHostQuery() async throws {
        let runsFixture = try Fixtures.data("project_runs.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], runsFixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let runs = try await client.projectRuns(wsID: "ws-1", offset: 0, limit: 25)
        #expect(runs.count == 1)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/projects/ws-1/runs")
        var query = StubURLProtocol.lastRequest?.url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }
        #expect(query?.contains(URLQueryItem(name: "offset", value: "0")) == true)
        #expect(query?.contains(URLQueryItem(name: "limit", value: "25")) == true)
        #expect(query?.contains(where: { $0.name == "host" }) == false)

        let sessionsFixture = try Fixtures.data("project_sessions.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], sessionsFixture) }
        let sessions = try await client.projectSessions(wsID: "ws-1", offset: 0, limit: 25)
        #expect(sessions.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/projects/ws-1/sessions")
        query = StubURLProtocol.lastRequest?.url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }
        #expect(query?.contains(where: { $0.name == "host" }) == false)
    }

    /// `projectAgents`/`projectWorkflows`/`projectAutoflows` reuse the same
    /// `AgentDefinition`/`WorkflowDefinition`/`AutoflowDefinition` shapes the
    /// global `GET /api/agents` / `/api/workflows` / `/api/autoflows` routes
    /// decode — this proves both the subpath and the shared decode.
    @Test func projectAgentsWorkflowsAndAutoflowsHitExpectedSubpaths() async throws {
        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )

        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], try! Fixtures.data("agent_defs.json")) }
        StubURLProtocol.lastRequest = nil
        let agents = try await client.projectAgents(wsID: "ws-1")
        #expect(agents.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/projects/ws-1/agents")

        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], try! Fixtures.data("workflow_defs.json")) }
        StubURLProtocol.lastRequest = nil
        let workflows = try await client.projectWorkflows(wsID: "ws-1")
        #expect(workflows.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/projects/ws-1/workflows")

        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], try! Fixtures.data("autoflow_defs.json")) }
        StubURLProtocol.lastRequest = nil
        let autoflows = try await client.projectAutoflows(wsID: "ws-1")
        #expect(autoflows.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/projects/ws-1/autoflows")
    }

    // MARK: - Per-host progressive loading (hotfix)

    /// The four federated list endpoints all accept an optional `host`,
    /// sent as `host=<value>` when set and omitted entirely otherwise (an
    /// omitted `host` fans the call out server-side to every registered
    /// host — see each method's doc comment). One representative endpoint
    /// (`workflowRuns`) proves the wiring; the other three share the exact
    /// same `offsetLimitQuery(offset:limit:host:)` helper.
    @Test func workflowRunsSendsHostQueryParameterOnlyWhenProvided() async throws {
        let fixture = try Fixtures.data("run_list_row.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )

        _ = try await client.workflowRuns(offset: 0, limit: 50)
        var query = StubURLProtocol.lastRequest?.url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }
        #expect(query?.contains(where: { $0.name == "host" }) == false)

        StubURLProtocol.lastRequest = nil
        _ = try await client.workflowRuns(offset: 0, limit: 50, host: "mini")
        query = StubURLProtocol.lastRequest?.url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }
        #expect(query?.contains(URLQueryItem(name: "host", value: "mini")) == true)
    }

    @Test func hostsHitsExpectedPathAndDecodesInlineJSON() async throws {
        // Fixture-free per the hotfix brief — matches the live `GET
        // /api/hosts` shape observed against a real `rupu cp serve`
        // (`{id, name, transport_kind, status, ...}`, extra fields ignored).
        let body = Data(#"""
        [
            {"id":"local","name":"local","transport_kind":"embedded","status":"online","extra_field_ignored":true},
            {"id":"mini","name":"mini","transport_kind":"ssh","status":"online"},
            {"id":"kuki","name":"kuki","transport_kind":"tunnel","status":"offline"}
        ]
        """#.utf8)
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], body) }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
        let hosts = try await client.hosts()

        #expect(hosts.count == 3)
        #expect(hosts[0] == APIHostRow(id: "local", name: "local", transportKind: "embedded", status: "online"))
        #expect(hosts[1] == APIHostRow(id: "mini", name: "mini", transportKind: "ssh", status: "online"))
        #expect(hosts[2] == APIHostRow(id: "kuki", name: "kuki", transportKind: "tunnel", status: "offline"))
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/hosts")
    }

    // MARK: - Cancellation mapping (hotfix)

    /// `URLSession`'s async `data(for:)` surfaces a cancelled `Task` as
    /// `URLError(.cancelled)` (not Swift's own `CancellationError`) — `get`
    /// maps both to the same `CPError.cancelled`, so every store's load
    /// path can treat cancellation uniformly regardless of which shape it
    /// actually arrives as.
    @Test func urlErrorCancelledMapsToCPErrorCancelled() async throws {
        StubURLProtocol.failureError = URLError(.cancelled)
        defer { StubURLProtocol.failureError = nil }

        let client = CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )

        await #expect(throws: CPError.cancelled) {
            _ = try await client.hostInfo()
        }
    }
}
