import Testing
import Foundation
@testable import RupuAPI

// MARK: - Decode: config_view.json fixture

@Test func decodesConfigViewFixtureWithQuotedProvenanceKeyAndLockedEntry() throws {
    let view = try JSONDecoder().decode(APIConfigView.self, from: Fixtures.data("config_view.json"))

    // The quoted-key provenance entry (`providers."GLM-5.2-FP8".model`)
    // survives verbatim as a dictionary key — the canonical dotted-key
    // encoding (`crates/rupu-cp/src/api/config.rs`'s `quote_key_segment`)
    // is opaque to Swift's `Decodable`: it decodes `[String: APIKeyProvenance]`
    // by whatever the wire's JSON object keys literally are, quotes and all.
    let providerProv = try #require(view.provenance["providers.\"GLM-5.2-FP8\".model"])
    #expect(providerProv.source == .project)
    #expect(!providerProv.locked)

    // The locked entry.
    let lockProv = try #require(view.provenance["policy.lock"])
    #expect(lockProv.source == .global)
    #expect(lockProv.locked)

    // Every provenance source variant the fixture carries.
    #expect(view.provenance["default_model"]?.source == .global)
    #expect(view.provenance["log_level"]?.source == .default)
    #expect(view.provenance["cp.max_workspace_bytes"]?.source == .project)

    #expect(view.status.restartRequiredKeys == ["bind", "token"])
    #expect(view.status.bind == "127.0.0.1:7420")
    #expect(!view.status.tokenSet)

    #expect(view.rawProject != nil)
    #expect(view.rawProject?.contains("claude-opus-4-8") == true)
    #expect(view.rawGlobal.contains("claude-sonnet-4-6"))

    // `effective` is the untyped JSONValue tree — spot-check a nested path
    // survives, including the same quoted provider key under `providers`.
    guard case let .object(root) = view.effective else {
        Issue.record("effective must decode as a JSON object")
        return
    }
    guard case let .object(providers) = root["providers"] else {
        Issue.record("effective.providers must decode as a JSON object")
        return
    }
    guard case let .object(glm) = providers["GLM-5.2-FP8"] else {
        Issue.record("effective.providers[\"GLM-5.2-FP8\"] must decode as a JSON object")
        return
    }
    #expect(glm["model"] == .string("GLM-5.2-FP8"))
}

// MARK: - CPClient: fetch / write surface

/// Deliberately an `extension CPClientTests` — see `CPClientWriteTests.swift`'s
/// doc comment on why every stub-based test shares that suite's
/// `.serialized` trait rather than declaring a new one.
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
        guard let body = request?.httpBodyStreamedOrDirectForConfigTests() else { return nil }
        return try JSONSerialization.jsonObject(with: body) as? [String: Any]
    }

    @Test func fetchConfigHitsExpectedPathAndOmitsProjectQueryWhenNil() async throws {
        let fixture = try Fixtures.data("config_view.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        let view = try await client().fetchConfig()

        #expect(view.status.bind == "127.0.0.1:7420")
        #expect(StubURLProtocol.lastRequest?.httpMethod == "GET")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/config")
        #expect(query(of: StubURLProtocol.lastRequest).isEmpty)
    }

    @Test func fetchConfigSendsProjectQueryWhenGiven() async throws {
        let fixture = try Fixtures.data("config_view.json")
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], fixture) }

        _ = try await client().fetchConfig(project: "ws-1")

        #expect(query(of: StubURLProtocol.lastRequest).contains(URLQueryItem(name: "project", value: "ws-1")))
    }

    @Test func putConfigGlobalSendsPUTWithRawBody() async throws {
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], Data(#"{"ok":true,"restart_required":[]}"#.utf8)) }

        try await client().putConfigGlobal(raw: "default_model = \"sonnet\"\n")

        #expect(StubURLProtocol.lastRequest?.httpMethod == "PUT")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/config/global")
        let body = try jsonObject(StubURLProtocol.lastRequest)
        #expect(body?["raw"] as? String == "default_model = \"sonnet\"\n")
        #expect(body?["patch"] == nil)
    }

    @Test func putConfigProjectHitsExpectedPathWithId() async throws {
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], Data(#"{"ok":true}"#.utf8)) }

        try await client().putConfigProject(id: "ws-1", raw: "default_model = \"sonnet\"\n")

        #expect(StubURLProtocol.lastRequest?.httpMethod == "PUT")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/config/project/ws-1")
        let body = try jsonObject(StubURLProtocol.lastRequest)
        #expect(body?["raw"] as? String == "default_model = \"sonnet\"\n")
    }

    @Test func putConfigPolicySendsLockArrayBody() async throws {
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], Data(#"{"ok":true}"#.utf8)) }

        try await client().putConfigPolicy(lock: ["permission_mode", "policy.lock"])

        #expect(StubURLProtocol.lastRequest?.httpMethod == "PUT")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/config/policy")
        let body = try jsonObject(StubURLProtocol.lastRequest)
        #expect(body?["lock"] as? [String] == ["permission_mode", "policy.lock"])
    }

    /// A `cp` deployment with no launcher installed 501s every config PUT
    /// (`require_writable` in `crates/rupu-cp/src/api/config.rs`) — this
    /// must surface as `CPError.http(status: 501, ...)`, the same
    /// distinguishable shape `fiveOhOneStatusMapsToHTTPErrorPreservingMessage`
    /// in `CPClientWriteTests.swift` asserts for the launcher-gated launch
    /// routes, so a later read-only-mode mapping (Task 4) can match on
    /// `.status == 501` regardless of which write route produced it.
    @Test func putConfigGlobalFiveOhOneMapsToHTTPErrorPreservingStatus() async throws {
        let body = Data(#"{"error":"editing config requires `rupu cp serve`"}"#.utf8)
        StubURLProtocol.requestHandler = { _ in (501, [:], body) }

        await #expect(throws: CPError.http(status: 501, body: #"{"error":"editing config requires `rupu cp serve`"}"#)) {
            try await client().putConfigGlobal(raw: "default_model = \"sonnet\"\n")
        }
    }
}

private extension URLRequest {
    /// Local copy of `CPClientWriteTests.swift`'s private
    /// `httpBodyStreamedOrDirect()` helper (file-private there, so not
    /// reachable from this file) — see that declaration's doc comment for
    /// why `URLSession` requires reading `httpBodyStream` as a fallback.
    func httpBodyStreamedOrDirectForConfigTests() -> Data? {
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
