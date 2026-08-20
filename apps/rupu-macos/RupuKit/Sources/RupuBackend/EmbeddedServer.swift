import Foundation

/// Attaches to (or, failing that, spawns) a `rupu cp serve` instance on
/// localhost.
///
/// `start()` never assumes ownership of a server that was already running
/// before the app launched: if the probe succeeds immediately, the origin
/// is `.attached` and `stop()` will never terminate it. Only a server this
/// instance itself spawned (`.spawned`) is eligible for termination, and
/// only when the caller doesn't ask to keep it running.
public actor EmbeddedServer {
    public enum Origin: Equatable {
        case attached
        case spawned(pid: Int32)
    }

    private let binaryPath: String
    private let port: Int
    private let probe: @Sendable (URL) async -> Bool

    private var origin: Origin?

    public init(binaryPath: String, port: Int, probe: @escaping @Sendable (URL) async -> Bool) {
        self.binaryPath = binaryPath
        self.port = port
        self.probe = probe
    }

    public func start() async throws -> Origin {
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

    private func terminateProcessGroup(pid: Int32) {
        killpg(pid, SIGTERM)
    }
}

public enum EmbeddedServerError: Error, Equatable {
    case startTimedOut
}
