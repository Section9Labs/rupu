import Foundation

/// How the app is reaching a `rupu cp serve` control plane: an embedded
/// server the app itself attached to or spawned on localhost, or a remote
/// host the user configured explicitly.
public enum BackendMode: Codable, Equatable, Sendable {
    case embedded(port: Int)
    case remote(url: URL)

    /// The base URL to build a `CPClient`/`CPConfig` against.
    public var baseURL: URL {
        switch self {
        case .embedded(let port):
            // Force-unwrap is safe: this URL string is a fixed, valid literal.
            return URL(string: "http://127.0.0.1:\(port)/")!
        case .remote(let url):
            return url
        }
    }
}

/// Observed health of the backend a `HealthMonitor` is polling.
public enum BackendHealth: Equatable, Sendable {
    case starting
    case healthy(version: String)
    case degraded(String)
    case down(String)
    case incompatible(serverVersion: String)
}
