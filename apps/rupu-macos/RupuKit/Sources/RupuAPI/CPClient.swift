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

    /// `host` (default `nil`) sends `host=<value>` when set. With no `host`
    /// at all, the server fans this call out to *every* registered host
    /// (local + every Fleet node) sequentially server-side — fine for a
    /// single fast local backend, but a multi-host fleet with one slow or
    /// offline node turns a ~60ms call into several seconds (measured
    /// 2.5-4.0s against a fleet with one offline node vs 60-70ms with
    /// `host: "local"`). Callers that want progressive per-host loading
    /// (see `ActivityStore`) always pass an explicit `host`, never omit it.
    public func runs(offset: Int, limit: Int, host: String? = nil) async throws -> [APIRunListRow] {
        try await get("api/runs", query: offsetLimitQuery(offset: offset, limit: limit, host: host))
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`.
    public func workflowRuns(offset: Int, limit: Int, host: String? = nil) async throws -> [APIRunListRow] {
        try await get("api/runs/workflows", query: offsetLimitQuery(offset: offset, limit: limit, host: host))
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`.
    public func agentRuns(offset: Int, limit: Int, host: String? = nil) async throws -> [APIAgentRunRow] {
        try await get("api/runs/agents", query: offsetLimitQuery(offset: offset, limit: limit, host: host))
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`.
    public func autoflowEvents(offset: Int, limit: Int, host: String? = nil) async throws -> [APIAutoflowEventRow] {
        try await get("api/runs/autoflows/events", query: offsetLimitQuery(offset: offset, limit: limit, host: host))
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`.
    public func sessions(offset: Int, limit: Int, host: String? = nil) async throws -> [APISessionRow] {
        try await get("api/sessions", query: offsetLimitQuery(offset: offset, limit: limit, host: host))
    }

    /// `GET /api/hosts` — the registered fleet: `local` plus every attached
    /// Fleet node, each with a `status` ("online"/"offline"/...). Drives
    /// `ActivityStore`'s per-host progressive loading: only `status ==
    /// "online"` hosts other than `"local"` are worth fetching from at all.
    public func hosts() async throws -> [APIHostRow] {
        try await get("api/hosts")
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

    private func offsetLimitQuery(offset: Int, limit: Int, host: String? = nil) -> [URLQueryItem] {
        var items = [
            URLQueryItem(name: "offset", value: String(offset)),
            URLQueryItem(name: "limit", value: String(limit)),
        ]
        items.append(contentsOf: hostQuery(host))
        return items
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
        } catch is CancellationError {
            // Cancellation (e.g. a SwiftUI `.task(id:)` whose id just
            // changed) is a routine, expected way for an in-flight request
            // to end — never a transport failure. `CPError.cancelled` lets
            // every call site distinguish it from a real error and leave
            // its current state untouched rather than surfacing a "Retry"
            // failure box for something the user didn't cause and doesn't
            // need to act on.
            throw CPError.cancelled
        } catch let urlError as URLError where urlError.code == .cancelled {
            // `URLSession`'s async `data(for:)` wires a cancelled `Task`'s
            // continuation to `task.cancel()`, which surfaces here as
            // `URLError(.cancelled)` rather than Swift's own
            // `CancellationError` — same benign meaning, same mapping.
            throw CPError.cancelled
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
