import Testing
import Foundation
@testable import RupuBackend

/// Thread-safe counter used to assert how many times the probe closure
/// ran, mirroring `LockedBox` in `HealthMonitorTests.swift`.
final class CallCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    @discardableResult
    func increment() -> Int {
        lock.withLock {
            count += 1
            return count
        }
    }

    var value: Int { lock.withLock { count } }
}

/// Covers `EmbeddedServer.start()`'s idempotency guard on the attach path
/// only: the probe succeeds immediately, so this never spawns a real
/// process (the spawn path stays untested here per the brief — spawning
/// is integration/flaky in CI and is verified manually in Task 9's
/// smoke test). A second `start()` call must short-circuit on the cached
/// `Origin` instead of re-probing.
@Test func startIsIdempotentOnAttachPath() async throws {
    let probeCalls = CallCounter()
    let server = EmbeddedServer(binaryPath: "/nonexistent/rupu", port: 65535, probe: { _ in
        probeCalls.increment()
        return true
    })

    let first = try await server.start()
    let second = try await server.start()

    #expect(first == .attached)
    #expect(second == .attached)
    #expect(probeCalls.value == 1)
}

// MARK: - Fast cold-start probe race (perf & interaction arc, Plan 5 Task 2)

/// The overwhelmingly common case — a `cp serve` already listening —
/// resolves `start()` via the race's "probe answered" branch without ever
/// waiting out the fast-path deadline. Proven by ordering, not wall-clock
/// (a `< 250ms` elapsed assertion here was flaky on loaded shared CI
/// runners — 284ms observed under scheduler noise): the injected deadline
/// is minutes long, so `start()` returning within the 1-minute time limit
/// at all is only possible if the answered branch won the race.
@Test(.timeLimit(.minutes(1)))
func startAttachesQuicklyWhenTheProbeAnswersFast() async throws {
    let probeCalls = CallCounter()
    let server = EmbeddedServer(
        binaryPath: "/nonexistent/rupu",
        port: 65535,
        fastPathDeadline: .seconds(600),
        probe: { _ in
            probeCalls.increment()
            return true
        }
    )

    let origin = try await server.start()

    #expect(origin == .attached)
    #expect(probeCalls.value == 1)
}

/// A probe that takes LONGER than the 300ms fast-path deadline to answer
/// (but still well within its own real timeout) must still resolve
/// correctly — via the SAME in-flight call, never a second, redundant
/// probe invocation — rather than being misclassified as "not running"
/// just because it didn't answer within the first 300ms.
@Test func startStillAttachesCorrectlyWhenTheProbeAnswersSlowerThanTheFastDeadline() async throws {
    let probeCalls = CallCounter()
    let server = EmbeddedServer(binaryPath: "/nonexistent/rupu", port: 65535, probe: { _ in
        probeCalls.increment()
        try? await Task.sleep(for: .milliseconds(600))
        return true
    })

    let origin = try await server.start()

    #expect(origin == .attached)
    #expect(probeCalls.value == 1, "the fast deadline elapsing must fall through to awaiting the SAME call, never invoke the probe a second time")
}

/// The false/"not running" case still works exactly as before the fast-path
/// race was added — a probe that quickly reports "nothing there" leads to
/// a real spawn attempt (which fails immediately against a nonexistent
/// binary path here), never misreported as `.attached`.
@Test func startAttemptsSpawnWhenTheProbeReportsNotRunning() async {
    let probeCalls = CallCounter()
    let server = EmbeddedServer(binaryPath: "/nonexistent/rupu", port: 65535, probe: { _ in
        probeCalls.increment()
        return false
    })

    await #expect(throws: (any Error).self) {
        try await server.start()
    }
    #expect(probeCalls.value == 1)
}
