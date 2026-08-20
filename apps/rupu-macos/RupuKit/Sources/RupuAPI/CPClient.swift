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
