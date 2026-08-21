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
        if flag.value { return HostInfo(version: "0.75.0", capabilities: .init(backends: [], scmHosts: [], permissionModes: [])) }
        throw CPError.transport("refused")
    })
    #expect(monitor.health == .starting)
    await monitor.pollOnce()
    #expect(monitor.health == .down("refused"))
    flag.value = true
    await monitor.pollOnce()
    #expect(monitor.health == .healthy(version: "0.75.0"))
    flag.value = false
    await monitor.pollOnce()
    if case .degraded = monitor.health {} else { Issue.record("healthy → failure should be degraded, got \(monitor.health)") }
}

/// Regression test for the self→task→closure→self retain cycle: `start()`
/// used to bind `self` strongly once above the polling loop, so the task
/// (retained by `self.task`) kept `self` alive for as long as it ran —
/// i.e. forever, since nothing but `stop()` ever cancelled it. A caller
/// that drops its last strong reference without calling `stop()` must
/// still let the monitor deallocate once the in-flight loop iteration
/// notices `self` is gone.
@MainActor @Test func droppingWithoutStopDoesNotLeak() async {
    weak var weakMonitor: HealthMonitor?

    do {
        let monitor = HealthMonitor(
            probe: { HostInfo(version: "0.75.0", capabilities: .init(backends: [], scmHosts: [], permissionModes: [])) },
            interval: .milliseconds(5)
        )
        weakMonitor = monitor
        monitor.start()
        // Let a few poll iterations actually run before we drop the
        // reference, so the task is mid-loop rather than never started.
        try? await Task.sleep(for: .milliseconds(20))
    }
    // `monitor` just went out of scope with no `stop()` call. Give the
    // task's next loop iteration a chance to notice `self` is nil.
    try? await Task.sleep(for: .milliseconds(50))

    #expect(weakMonitor == nil)
}
