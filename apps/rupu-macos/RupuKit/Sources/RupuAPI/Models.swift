import Foundation

/// `GET /api/host/info` response.
public struct HostInfo: Decodable, Equatable, Sendable {
    public let version: String
    public let capabilities: HostCapabilities

    public init(version: String, capabilities: HostCapabilities) {
        self.version = version
        self.capabilities = capabilities
    }
}

public struct HostCapabilities: Decodable, Equatable, Sendable {
    public let backends: [String]
    public let scmHosts: [String]
    public let permissionModes: [String]

    public init(backends: [String], scmHosts: [String], permissionModes: [String]) {
        self.backends = backends
        self.scmHosts = scmHosts
        self.permissionModes = permissionModes
    }

    private enum CodingKeys: String, CodingKey {
        case backends
        case scmHosts = "scm_hosts"
        case permissionModes = "permission_modes"
    }
}

/// One row from `GET /api/events`: the endpoint injects a stream position
/// (`pos`) and timestamp (`ts`) into the same JSON object as the event
/// payload, so this decodes both from that shared object. The server
/// injects both fields into every row unconditionally, so a row missing
/// either one fails to decode rather than silently defaulting.
public struct CPEventRow: Decodable, Equatable, Sendable {
    public let event: CPEvent
    public let ts: Int64
    public let pos: Int

    private enum CodingKeys: String, CodingKey {
        case ts
        case pos
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.ts = try container.decode(Int64.self, forKey: .ts)
        self.pos = try container.decode(Int.self, forKey: .pos)
        self.event = try CPEvent(from: decoder)
    }
}

/// Configuration for talking to a `rupu cp serve` host.
public struct CPConfig: Sendable {
    public var baseURL: URL
    public var token: String?

    public init(baseURL: URL, token: String? = nil) {
        self.baseURL = baseURL
        self.token = token
    }
}

public enum CPError: Error, Equatable {
    case http(status: Int, body: String)
    case transport(String)
    case decoding(String)
    case unauthorized
    /// The underlying request was cancelled (a `CancellationError` or
    /// `URLError(.cancelled)` from `URLSession`) — never a real transport
    /// failure. Every store's load path checks for this specifically and
    /// leaves its current state untouched rather than surfacing `.failed`.
    case cancelled
}

/// One row from `GET /api/hosts`: the registered fleet (`local` plus every
/// attached Fleet node). Only `id`/`name`/`transportKind`/`status` are
/// decoded — the endpoint returns more fields, ignored here (`Decodable`'s
/// default behavior already skips unknown keys, so no custom `init` is
/// needed for that).
public struct APIHostRow: Decodable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let transportKind: String
    public let status: String

    public init(id: String, name: String, transportKind: String, status: String) {
        self.id = id
        self.name = name
        self.transportKind = transportKind
        self.status = status
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case name
        case transportKind = "transport_kind"
        case status
    }
}
