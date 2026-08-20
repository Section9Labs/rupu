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

    static func session() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [StubURLProtocol.self]
        return URLSession(configuration: config)
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        StubURLProtocol.lastRequest = request
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
}
