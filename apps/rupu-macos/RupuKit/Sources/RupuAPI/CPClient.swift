import Foundation

/// Thin HTTP client for a `rupu cp serve` control-plane host.
public actor CPClient {
    let config: CPConfig
    let session: URLSession

    public init(config: CPConfig, session: URLSession = .shared) {
        self.config = config
        self.session = session
    }

    public func hostInfo() async throws -> HostInfo {
        try await get("api/host/info")
    }

    public func recentEvents(limit: Int = 200) async throws -> [CPEventRow] {
        try await get("api/events", query: [URLQueryItem(name: "limit", value: String(limit))])
    }

    public func runs(offset: Int, limit: Int) async throws -> [APIRunListRow] {
        try await get("api/runs", query: offsetLimitQuery(offset: offset, limit: limit))
    }

    public func workflowRuns(offset: Int, limit: Int) async throws -> [APIRunListRow] {
        try await get("api/runs/workflows", query: offsetLimitQuery(offset: offset, limit: limit))
    }

    public func agentRuns(offset: Int, limit: Int) async throws -> [APIAgentRunRow] {
        try await get("api/runs/agents", query: offsetLimitQuery(offset: offset, limit: limit))
    }

    public func autoflowEvents(offset: Int, limit: Int) async throws -> [APIAutoflowEventRow] {
        try await get("api/runs/autoflows/events", query: offsetLimitQuery(offset: offset, limit: limit))
    }

    public func sessions(offset: Int, limit: Int) async throws -> [APISessionRow] {
        try await get("api/sessions", query: offsetLimitQuery(offset: offset, limit: limit))
    }

    public func runDetail(id: String, host: String? = nil) async throws -> APIRunDetail {
        try await get("api/runs/\(id)", query: hostQuery(host))
    }

    public func runGraph(id: String, host: String? = nil) async throws -> APIRunGraph {
        try await get("api/runs/\(id)/graph", query: hostQuery(host))
    }

    /// `path` is the transcript file's full path (from a step result / run
    /// record / unit row / agent-run row), sent as a query parameter —
    /// `/api/transcript`, never a URL path segment.
    public func transcript(path: String, host: String? = nil) async throws -> APITranscriptPage {
        var query = [URLQueryItem(name: "path", value: path)]
        query.append(contentsOf: hostQuery(host))
        return try await get("api/transcript", query: query)
    }

    public func runNetflow(id: String) async throws -> APINetflow {
        try await get("api/runs/\(id)/netflow")
    }

    public func runFindings(id: String) async throws -> APIFindings {
        try await get("api/findings", query: [URLQueryItem(name: "run_id", value: id)])
    }

    public func sessionDetail(id: String) async throws -> APISessionRow {
        try await get("api/sessions/\(id)")
    }

    public func sessionRuns(id: String) async throws -> [APISessionRunRow] {
        try await get("api/sessions/\(id)/runs")
    }

    private func offsetLimitQuery(offset: Int, limit: Int) -> [URLQueryItem] {
        [
            URLQueryItem(name: "offset", value: String(offset)),
            URLQueryItem(name: "limit", value: String(limit)),
        ]
    }

    private func hostQuery(_ host: String?) -> [URLQueryItem] {
        guard let host else { return [] }
        return [URLQueryItem(name: "host", value: host)]
    }

    func get<T: Decodable>(_ path: String, query: [URLQueryItem] = []) async throws -> T {
        guard var components = URLComponents(url: config.baseURL, resolvingAgainstBaseURL: false) else {
            throw CPError.transport("invalid base URL: \(config.baseURL)")
        }
        let basePath = components.path.hasSuffix("/") ? String(components.path.dropLast()) : components.path
        let relativePath = path.hasPrefix("/") ? path : "/\(path)"
        components.path = basePath + relativePath
        if !query.isEmpty {
            components.queryItems = query
        }
        guard let url = components.url else {
            throw CPError.transport("could not build URL for path: \(path)")
        }

        var request = URLRequest(url: url)
        if let token = config.token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }

        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch let error as CPError {
            throw error
        } catch {
            throw CPError.transport(error.localizedDescription)
        }

        guard let httpResponse = response as? HTTPURLResponse else {
            throw CPError.transport("non-HTTP response for \(url)")
        }
        guard httpResponse.statusCode != 401 else {
            throw CPError.unauthorized
        }
        guard (200..<300).contains(httpResponse.statusCode) else {
            let body = String(data: data, encoding: .utf8) ?? ""
            throw CPError.http(status: httpResponse.statusCode, body: body)
        }

        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw CPError.decoding(String(describing: error))
        }
    }
}
