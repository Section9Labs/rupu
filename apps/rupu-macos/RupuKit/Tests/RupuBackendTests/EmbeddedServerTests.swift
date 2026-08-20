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
