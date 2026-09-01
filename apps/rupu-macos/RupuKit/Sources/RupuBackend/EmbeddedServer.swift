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
    private let fastPathDeadline: Duration

    private var origin: Origin?

    public init(binaryPath: String, port: Int, probe: @escaping @Sendable (URL) async -> Bool) {
        self.init(binaryPath: binaryPath, port: port, fastPathDeadline: .milliseconds(300), probe: probe)
    }

    /// Test seam: the fast-path deadline is injectable so tests can prove
    /// the race resolves on the answered branch by ordering (a deadline of
    /// minutes that the test never waits out) instead of asserting
    /// wall-clock elapsed time, which is flaky on loaded CI runners.
    init(binaryPath: String, port: Int, fastPathDeadline: Duration, probe: @escaping @Sendable (URL) async -> Bool) {
        self.binaryPath = binaryPath
        self.port = port
        self.fastPathDeadline = fastPathDeadline
        self.probe = probe
    }

    /// Idempotent: once this instance has attached to or spawned a
    /// server, a repeat call just returns the same `Origin` rather than
    /// re-probing (a re-probe after a successful spawn would hit this
    /// instance's own child and misclassify it as `.attached`, orphaning
    /// it from `stop()`).
    public func start() async throws -> Origin {
        if let origin { return origin }

        if await isAlreadyRunning() {
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

    /// Fast cold start (perf & interaction arc, Plan 5 Task 2): races the
    /// injected `probe` against a short deadline (`fastPathDeadline`,
    /// 300ms in production) before falling back
    /// to just awaiting it out for as long as its own (much longer — 3s in
    /// production, `BackendController.defaultEmbeddedProbe`) timeout
    /// allows. The overwhelmingly common case — a `cp serve` already
    /// listening, whether attached from an earlier app launch or started
    /// manually — answers in single-digit milliseconds on loopback, so this
    /// resolves `start()` far faster than always paying the probe's full
    /// timeout before ever getting an answer.
    ///
    /// **Never a second, redundant probe call**: `probe(probeURL)` is
    /// invoked exactly once, as an unstructured `Task` (`probeTask`) created
    /// OUTSIDE the racing `withTaskGroup` below — the race only decides
    /// which of "the probe answered" or "300ms elapsed" to act on FIRST; if
    /// the deadline wins, this falls through to simply awaiting
    /// `probeTask.value` (the SAME in-flight call, still running), never
    /// starting a fresh one. This is what keeps a server that's merely slow
    /// to answer (as opposed to genuinely absent, which fails fast with a
    /// loopback connection refusal) from being misclassified as needing a
    /// spawn — the probe still gets its full timeout to answer, just not
    /// on this call's own critical path once 300ms have passed.
    ///
    /// `group.cancelAll()` below only cancels the race's own two child
    /// tasks (the wrapper awaiting `probeTask` and the sleep) — `probeTask`
    /// itself is a sibling `Task`, not a structured child of the group, so
    /// cancelling the group's race can never cancel the real probe still
    /// running in the background.
    private func isAlreadyRunning() async -> Bool {
        let probeTask = Task { await probe(probeURL) }

        enum RaceOutcome { case answered(Bool), deadlineElapsed }
        let outcome = await withTaskGroup(of: RaceOutcome.self) { group -> RaceOutcome in
            group.addTask { .answered(await probeTask.value) }
            group.addTask { [fastPathDeadline] in
                try? await Task.sleep(for: fastPathDeadline)
                return .deadlineElapsed
            }
            let first = await group.next() ?? .deadlineElapsed
            group.cancelAll()
            return first
        }

        switch outcome {
        case .answered(let ok):
            return ok
        case .deadlineElapsed:
            return await probeTask.value
        }
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
        // `rupu cp serve` takes `--bind <addr:port>`, not a standalone
        // `--port` flag (confirmed against the installed 0.74.0-beta.3
        // CLI's `--help` during Task 9's integration smoke — the app never
        // spawned a reachable server without this). `--no-open` keeps a
        // headlessly-spawned server from popping a browser tab: the app is
        // the UI here, not the served web UI.
        process.arguments = ["cp", "serve", "--bind", "127.0.0.1:\(port)", "--no-open"]
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
