import Testing
import Foundation
@testable import RupuAPI

/// Write-surface coverage for `CPClient`: run-control routes, agent/workflow
/// launch, session send, definition reads, and the `post` error mapping.
///
/// Deliberately an `extension CPClientTests` rather than its own `@Suite`:
/// `StubURLProtocol`'s handler/last-request are class-level (shared) state,
/// and Swift Testing's `.serialized` trait only serializes tests *within*
/// one suite — two separate `.serialized` suites can still run concurrently
/// against each other and race on that shared state. Extending the existing
/// suite keeps every stub-based test under the same `.serialized` trait
/// declared once on `CPClientTests` in `CPClientTests.swift`.
extension CPClientTests {
    private func client() -> CPClient {
        CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: StubURLProtocol.session()
        )
    }

    private func query(of request: URLRequest?) -> [URLQueryItem] {
        request?.url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems } ?? []
    }

    private func jsonObject(_ request: URLRequest?) throws -> [String: Any]? {
        guard let body = request?.httpBodyStreamedOrDirect() else { return nil }
        return try JSONSerialization.jsonObject(with: body) as? [String: Any]
    }

    // MARK: - Approve

    @Test func approveRunHitsExpectedPathQueryAndBody() async throws {
        let fixture = try Fixtures.data("run_control_response.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let response = try await client().approveRun(id: "r1", gate: "g1", body: ApproveBody(mode: "bypass"))

        #expect(response.confirmedStatus == "cancelled")
        #expect(StubURLProtocol.lastRequest?.httpMethod == "POST")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/runs/r1/approve")
        let q = query(of: StubURLProtocol.lastRequest)
        #expect(q.contains(URLQueryItem(name: "gate", value: "g1")))
        #expect(!q.contains(where: { $0.name == "host" }))
        let body = try jsonObject(StubURLProtocol.lastRequest)
        #expect(body?["mode"] as? String == "bypass")
    }

    @Test func approveRunSendsNoBodyWhenBodyOmitted() async throws {
        let fixture = try Fixtures.data("run_control_response.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        _ = try await client().approveRun(id: "r1", gate: "g1")

        #expect(StubURLProtocol.lastRequest?.httpBodyStreamedOrDirect() == nil || StubURLProtocol.lastRequest?.httpBodyStreamedOrDirect()?.isEmpty == true)
    }

    // MARK: - Reject / Cancel / Pause / Resume / Archive / Restore

    @Test func rejectRunAlwaysSendsAJSONBodyEvenWithNoReason() async throws {
        let fixture = try Fixtures.data("run_control_response.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        _ = try await client().rejectRun(id: "r1", gate: "g1")

        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/runs/r1/reject")
        let body = try jsonObject(StubURLProtocol.lastRequest)
        #expect(body?.isEmpty == true)
    }

    @Test func cancelRunHitsCancelSubpath() async throws {
        let fixture = try Fixtures.data("run_control_response.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        _ = try await client().cancelRun(id: "r1", body: CancelBody(reason: "operator cancelled"))
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/runs/r1/cancel")
    }

    @Test func pauseResumeArchiveRestoreHitExpectedSubpaths() async throws {
        let fixture = try Fixtures.data("run_control_response.json")
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        StubURLProtocol.lastRequest = nil
        _ = try await client().pauseRun(id: "r1")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/runs/r1/pause")

        StubURLProtocol.lastRequest = nil
        _ = try await client().resumeRun(id: "r1")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/runs/r1/resume")

        StubURLProtocol.lastRequest = nil
        _ = try await client().archiveRun(id: "r1")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/runs/r1/archive")

        StubURLProtocol.lastRequest = nil
        _ = try await client().restoreRun(id: "r1")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/runs/r1/restore")
    }

    // MARK: - Agent / workflow launch, session send

    @Test func runWorkflowBodyCarriesHostField() async throws {
        let fixture = try Fixtures.data("launch_responses.json").firstJSONArrayElement()
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let response = try await client().runWorkflow(
            name: "nightly-health",
            body: WorkflowLaunchBody(inputs: ["branch": "main"], mode: "ask", host: "mini")
        )

        #expect(response.hostID == "local")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/workflows/nightly-health/run")
        #expect(query(of: StubURLProtocol.lastRequest).isEmpty)
        let body = try jsonObject(StubURLProtocol.lastRequest)
        #expect(body?["host"] as? String == "mini")
        #expect((body?["inputs"] as? [String: String])?["branch"] == "main")
    }

    @Test func launchAgentRunAndStartAgentSessionHitExpectedPaths() async throws {
        let fixture = try Fixtures.data("launch_responses.json").firstJSONArrayElement()
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        StubURLProtocol.lastRequest = nil
        _ = try await client().launchAgentRun(name: "code-reviewer", body: AgentLaunchBody(prompt: "go"))
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/agents/code-reviewer/run")

        StubURLProtocol.lastRequest = nil
        _ = try await client().startAgentSession(name: "code-reviewer", body: AgentLaunchBody(prompt: "go"))
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/agents/code-reviewer/session")
    }

    @Test func sendToSessionSendsHostAsQueryAndPromptInBody() async throws {
        let fixture = try Fixtures.data("launch_responses.json").firstJSONArrayElement()
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        _ = try await client().sendToSession(id: "sess-1", host: "mini", body: SendBody(prompt: "hello"))

        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/sessions/sess-1/send")
        let q = query(of: StubURLProtocol.lastRequest)
        #expect(q.contains(URLQueryItem(name: "host", value: "mini")))
        let body = try jsonObject(StubURLProtocol.lastRequest)
        #expect(body?["prompt"] as? String == "hello")
    }

    @Test func sendToSessionOmitsHostQueryWhenHostIsLocal() async throws {
        let fixture = try Fixtures.data("launch_responses.json").firstJSONArrayElement()
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        _ = try await client().sendToSession(id: "sess-1", host: "local", body: SendBody(prompt: "hello"))

        let q = query(of: StubURLProtocol.lastRequest)
        #expect(!q.contains(where: { $0.name == "host" }))
    }

    /// Phase 3, Task 6 addition — `archiveSession`/`restoreSession` weren't
    /// part of Task 2's original write surface (its brief only covered run
    /// archive/restore). Stub coverage mirrors `pauseResumeArchiveRestoreHitExpectedSubpaths`
    /// above: expected subpath, plus the `host` query placement.
    @Test func archiveSessionAndRestoreSessionHitExpectedSubpathsAndHostQuery() async throws {
        let fixture = try Fixtures.data("run_control_response.json")
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        StubURLProtocol.lastRequest = nil
        _ = try await client().archiveSession(id: "sess-1", host: "mini")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/sessions/sess-1/archive")
        #expect(query(of: StubURLProtocol.lastRequest).contains(URLQueryItem(name: "host", value: "mini")))

        StubURLProtocol.lastRequest = nil
        _ = try await client().restoreSession(id: "sess-1")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/sessions/sess-1/restore")
        #expect(!query(of: StubURLProtocol.lastRequest).contains(where: { $0.name == "host" }))
    }

    // MARK: - Autoflow definitions

    /// `enabled: true` hits `.../enable`, `enabled: false` hits
    /// `.../disable`; `scopeKind`/`scopeID` (when both given) become the
    /// `?scope_kind=&scope_id=` pinning pair, omitted entirely when both are
    /// `nil`.
    @Test func setAutoflowEnabledChoosesSubpathAndSendsScopeQuery() async throws {
        let enableFixture = Data(#"{"name":"nightly-health","enabled":true}"#.utf8)
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], enableFixture) }

        let enableResponse = try await client().setAutoflowEnabled(name: "nightly-health", enabled: true)
        #expect(enableResponse.enabled)
        #expect(StubURLProtocol.lastRequest?.httpMethod == "POST")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/autoflows/nightly-health/enable")
        #expect(query(of: StubURLProtocol.lastRequest).isEmpty)

        let disableFixture = Data(#"{"name":"issue-triage","enabled":false}"#.utf8)
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], disableFixture) }

        let disableResponse = try await client().setAutoflowEnabled(
            name: "issue-triage",
            scopeKind: "project",
            scopeID: "ws-1",
            enabled: false
        )
        #expect(!disableResponse.enabled)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/autoflows/issue-triage/disable")
        let q = query(of: StubURLProtocol.lastRequest)
        #expect(q.contains(URLQueryItem(name: "scope_kind", value: "project")))
        #expect(q.contains(URLQueryItem(name: "scope_id", value: "ws-1")))
    }

    // MARK: - Validate

    @Test func validateWorkflowReturnsTrueOn200() async throws {
        let body = Data(#"{"ok":true}"#.utf8)
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], body) }

        let ok = try await client().validateWorkflow(body: ValidateBody(raw: "name: demo\n"))
        #expect(ok)
    }

    @Test func validateWorkflowThrowsHTTPErrorWithSingleMessageOn400() async throws {
        let body = Data(#"{"error":"missing required field: steps"}"#.utf8)
        StubURLProtocol.requestHandler = { _ in (400, [:], body) }

        await #expect(throws: CPError.http(status: 400, body: #"{"error":"missing required field: steps"}"#)) {
            _ = try await client().validateWorkflow(body: ValidateBody(raw: "name: demo\n"))
        }
    }

    // MARK: - Definitions

    @Test func agentDefinitionsWorkflowDefinitionsWorkflowDetailAndToolsHitExpectedPaths() async throws {
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], try! Fixtures.data("agent_defs.json")) }
        StubURLProtocol.lastRequest = nil
        let agents = try await client().agentDefinitions()
        #expect(agents.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/agents")

        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], try! Fixtures.data("workflow_defs.json")) }
        StubURLProtocol.lastRequest = nil
        let workflows = try await client().workflowDefinitions()
        #expect(workflows.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/workflows")

        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], try! Fixtures.data("workflow_detail.json")) }
        StubURLProtocol.lastRequest = nil
        let detail = try await client().workflowDetail(name: "nightly-health")
        #expect(detail.name == "nightly-health")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/workflows/nightly-health")

        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], try! Fixtures.data("tools.json")) }
        StubURLProtocol.lastRequest = nil
        let tools = try await client().tools()
        #expect(tools.count == 2)
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/tools")
    }

    // MARK: - 501 mapping

    @Test func fiveOhOneStatusMapsToHTTPErrorPreservingMessage() async throws {
        let body = Data(#"{"error":"agent_launcher not configured"}"#.utf8)
        StubURLProtocol.requestHandler = { _ in (501, [:], body) }

        await #expect(throws: CPError.http(status: 501, body: #"{"error":"agent_launcher not configured"}"#)) {
            _ = try await client().launchAgentRun(name: "code-reviewer", body: AgentLaunchBody(prompt: "go"))
        }
    }
}

private extension URLRequest {
    /// `URLSession` converts a request's `httpBody` into an
    /// `httpBodyStream` internally before handing the request to a custom
    /// `URLProtocol` — `.httpBody` reads back `nil` at that point even
    /// though the original request set it directly. Read whichever one is
    /// populated.
    func httpBodyStreamedOrDirect() -> Data? {
        if let httpBody { return httpBody }
        guard let stream = httpBodyStream else { return nil }
        stream.open()
        defer { stream.close() }
        var data = Data()
        let bufferSize = 4096
        var buffer = [UInt8](repeating: 0, count: bufferSize)
        while stream.hasBytesAvailable {
            let read = stream.read(&buffer, maxLength: bufferSize)
            guard read > 0 else { break }
            data.append(buffer, count: read)
        }
        return data
    }
}

private extension Data {
    /// `launch_responses.json` holds all three response-shape variants as a
    /// JSON array; write-path stub tests only need one object per response,
    /// so this re-serializes the first element back to `Data`.
    func firstJSONArrayElement() -> Data {
        guard
            let array = try? JSONSerialization.jsonObject(with: self) as? [Any],
            let first = array.first,
            let data = try? JSONSerialization.data(withJSONObject: first)
        else {
            return self
        }
        return data
    }
}
