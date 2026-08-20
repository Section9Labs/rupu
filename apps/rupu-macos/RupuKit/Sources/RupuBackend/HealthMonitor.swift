import Foundation
import Observation
import RupuAPI

/// Polls a backend's `/api/host/info` on an interval and tracks the
/// resulting `BackendHealth` state machine:
///
/// - `starting` / `down` / `degraded` + probe ok & version-compatible → `healthy`
/// - probe ok but version too old → `incompatible`
/// - `healthy` + probe failure → `degraded` (one bad poll doesn't declare
///   the backend down outright)
/// - any other state + probe failure → `down`
@MainActor
@Observable
public final class HealthMonitor {
    public private(set) var health: BackendHealth = .starting

    private let probe: @Sendable () async throws -> HostInfo
    private let interval: Duration
    private var task: Task<Void, Never>?

    public init(probe: @escaping @Sendable () async throws -> HostInfo, interval: Duration = .seconds(5)) {
        self.probe = probe
        self.interval = interval
    }

    /// Starts polling on `interval`. A second call while already running
    /// is a no-op, so `start()` can be called defensively without ever
    /// accumulating more than one polling task.
    public func start() {
        guard task == nil else { return }
        task = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                await self.pollOnce()
                try? await Task.sleep(for: self.interval)
            }
        }
    }

    public func stop() {
        task?.cancel()
        task = nil
    }

    public func pollOnce() async {
        do {
            let info = try await probe()
            health = VersionGate.compatible(info.version)
                ? .healthy(version: info.version)
                : .incompatible(serverVersion: info.version)
        } catch {
            let message = Self.message(for: error)
            if case .healthy = health {
                health = .degraded(message)
            } else {
                health = .down(message)
            }
        }
    }

    /// `CPError` carries a human-readable payload directly; anything else
    /// falls back to its debug description.
    private static func message(for error: Error) -> String {
        switch error {
        case let cpError as CPError:
            switch cpError {
            case .transport(let message): return message
            case .decoding(let message): return message
            case .http(let status, let body): return "http \(status): \(body)"
            case .unauthorized: return "unauthorized"
            }
        default:
            return String(describing: error)
        }
    }
}
