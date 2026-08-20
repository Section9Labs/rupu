import Testing
import Foundation
@testable import RupuBackend
import RupuAPI

/// Thread-safe bool box for flipping probe behavior from the test body
/// while the probe closure itself runs on `MainActor`/`Sendable` contexts.
final class LockedBox: @unchecked Sendable {
    private let lock = NSLock()
    private var v: Bool

    init(_ v: Bool) {
        self.v = v
    }

    var value: Bool {
        get { lock.withLock { v } }
        set { lock.withLock { v = newValue } }
    }
}

@MainActor @Test func healthMonitorTransitions() async {
    let flag = LockedBox(false)
    let monitor = HealthMonitor(probe: {
        if flag.value { return HostInfo(version: "0.71.0", capabilities: .init(backends: [], scmHosts: [], permissionModes: [])) }
        throw CPError.transport("refused")
    })
    #expect(monitor.health == .starting)
    await monitor.pollOnce()
    #expect(monitor.health == .down("refused"))
    flag.value = true
    await monitor.pollOnce()
    #expect(monitor.health == .healthy(version: "0.71.0"))
    flag.value = false
    await monitor.pollOnce()
    if case .degraded = monitor.health {} else { Issue.record("healthy → failure should be degraded, got \(monitor.health)") }
}
