import Testing
import Foundation
@testable import RupuAPI

/// `CPClient.writeWorkflow` coverage (Task 9) — `PUT /api/workflows/:name`
/// with the optional `?scope_kind=&scope_id=` pinning pair and a
/// `{"raw": ...}` body. Deliberately an `extension CPClientTests` for the
/// same reason `CPClientWriteTests.swift` is — see that file's header doc
/// comment: `StubURLProtocol`'s handler/last-request are class-level shared
/// state, and Swift Testing's `.serialized` trait only serializes tests
/// *within* one suite.
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
        guard let body = request?.httpBody ?? request?.httpBodyStreamedOrDirectForWrite() else { return nil }
        return try JSONSerialization.jsonObject(with: body) as? [String: Any]
    }

    @Test func writeWorkflowSendsPutWithScopeQueryAndRawBody() async throws {
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], Data(#"{"ok":true}"#.utf8)) }

        try await client().writeWorkflow(
            name: "nightly",
            body: WorkflowWriteBody(raw: "name: nightly\n"),
            scopeKind: "project",
            scopeID: "ws1"
        )

        #expect(StubURLProtocol.lastRequest?.httpMethod == "PUT")
        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/workflows/nightly")
        let q = query(of: StubURLProtocol.lastRequest)
        #expect(q.contains(URLQueryItem(name: "scope_kind", value: "project")))
        #expect(q.contains(URLQueryItem(name: "scope_id", value: "ws1")))
        let body = try jsonObject(StubURLProtocol.lastRequest)
        #expect(body?["raw"] as? String == "name: nightly\n")
    }

    @Test func writeWorkflowOmitsQueryWhenScopeIsNil() async throws {
        StubURLProtocol.lastRequest = nil
        StubURLProtocol.requestHandler = { _ in (200, ["Content-Type": "application/json"], Data(#"{"ok":true}"#.utf8)) }

        try await client().writeWorkflow(name: "nightly", body: WorkflowWriteBody(raw: "name: nightly\n"))

        #expect(StubURLProtocol.lastRequest?.url?.path == "/api/workflows/nightly")
        #expect(query(of: StubURLProtocol.lastRequest).isEmpty)
    }

    @Test func writeWorkflowThrowsHTTPErrorOnNonTwoXX() async throws {
        let body = Data(#"{"error":"invalid workflow: missing required field: steps"}"#.utf8)
        StubURLProtocol.requestHandler = { _ in (400, [:], body) }

        await #expect(throws: CPError.http(status: 400, body: #"{"error":"invalid workflow: missing required field: steps"}"#)) {
            try await client().writeWorkflow(name: "nightly", body: WorkflowWriteBody(raw: "bad"))
        }
    }
}

private extension URLRequest {
    /// Same rationale as `CPClientWriteTests.swift`'s private extension of
    /// the same shape — `URLSession` converts a request's `httpBody` into
    /// an `httpBodyStream` internally before handing the request to a
    /// custom `URLProtocol`, so `.httpBody` reads back `nil` at that point
    /// even though the original request set it directly.
    func httpBodyStreamedOrDirectForWrite() -> Data? {
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
