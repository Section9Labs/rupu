import Foundation
import os

/// Attaches to (or, failing that, spawns) a `rupu cp serve` instance on
/// localhost.
///
/// `start()` never assumes ownership of a server that was already running
/// before the app launched: if the probe succeeds immediately, the origin
/// is `.attached` and `stop()` will never terminate it. Only a server this
/// instance itself spawned (`.spawned`) is eligible for termination, and
/// only when the caller doesn't ask to keep it running.
public actor EmbeddedServer {
    public enum Origin: Equatable, Sendable {
        case attached
        case spawned(pid: Int32)
    }

    private static let logger = Logger(subsystem: "com.section9labs.rupu", category: "backend")

    private let binaryPath: String
    private let port: Int
    private let probe: @Sendable (URL) async -> Bool

    private var origin: Origin?

    public init(binaryPath: String, port: Int, probe: @escaping @Sendable (URL) async -> Bool) {
        self.binaryPath = binaryPath
        self.port = port
        self.probe = probe
    }

    /// Idempotent: once this instance has attached to or spawned a
    /// server, a repeat call just returns the same `Origin` rather than
    /// re-probing (a re-probe after a successful spawn would hit this
    /// instance's own child and misclassify it as `.attached`, orphaning
    /// it from `stop()`).
    public func start() async throws -> Origin {
        if let origin { return origin }

        if await probe(probeURL) {
            origin = .attached
            return .attached
        }

        let pid = try spawn()
        guard await pollUntilHealthy() else {
            terminateProcessGroup(pid: pid)
            throw EmbeddedServerError.startTimedOut
        }

        let result = Origin.spawned(pid: pid)
        origin = result
        return result
    }

    public func stop(keepRunning: Bool) {
        guard case .spawned(let pid) = origin, !keepRunning else { return }
        terminateProcessGroup(pid: pid)
        origin = nil
    }

    private var probeURL: URL {
        URL(string: "http://127.0.0.1:\(port)/api/host/info")!
    }

    /// Launches `binaryPath cp serve --port <port>` in its own process
    /// group so `stop()` can tear down the whole tree (the server may fork
    /// helper processes) without touching the app's own group.
    ///
    /// `process` is intentionally not stored on `self`: once `run()`
    /// succeeds, Foundation's `Process` keeps the underlying task alive
    /// until it terminates even after the local value goes out of scope
    /// (Darwin's `Process` self-retains for the life of the child), so we
    /// only need the `pid` going forward.
    private func spawn() throws -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: binaryPath)
        process.arguments = ["cp", "serve", "--port", String(port)]
        try process.run()

        let pid = process.processIdentifier
        setpgid(pid, pid)
        return pid
    }

    /// Polls the probe every 500ms until it succeeds or 20s elapse.
    private func pollUntilHealthy() async -> Bool {
        let deadline = Date().addingTimeInterval(20)
        while Date() < deadline {
            if await probe(probeURL) { return true }
            try? await Task.sleep(for: .milliseconds(500))
        }
        return false
    }

    /// Terminates the process group `spawn()` put the child in. Falls back
    /// to signaling just the pid if the group signal fails — e.g. if the
    /// `setpgid` in `spawn()` lost its race with the child's `exec` and
    /// the child ended up in some other group, `killpg` targeting an
    /// empty/wrong group would otherwise no-op silently and leak the
    /// spawned server.
    private func terminateProcessGroup(pid: Int32) {
        if killpg(pid, SIGTERM) != 0 {
            let groupErrno = errno
            Self.logger.error("killpg(\(pid, privacy: .public)) failed (errno \(groupErrno, privacy: .public)); falling back to kill(pid)")
            if kill(pid, SIGTERM) != 0 {
                Self.logger.error("kill(\(pid, privacy: .public)) also failed (errno \(errno, privacy: .public)); spawned rupu cp serve process may have leaked")
            }
        }
    }
}

public enum EmbeddedServerError: Error, Equatable {
    case startTimedOut
}
