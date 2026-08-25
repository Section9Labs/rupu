import Testing
import Foundation
@testable import RupuMenuBar
import RupuBackend

/// Final-review fix (M4): `MenuBarStore.pollOnce` keeps its last good data
/// when a poll fails, so an unhealthy backend leaves the popover rendering
/// counts and gate rows that are merely the last thing that was true. The
/// view must say so ("Backend unreachable — showing last known") and
/// disable the inline gate Approve/Reject, whose "this run is still parked
/// on that gate" premise is precisely what can no longer be checked.
///
/// `@MainActor` because `MenuBarView` is a `View` (main-actor-isolated in
/// this project's concurrency settings); the assertion itself drives the
/// pure `isHealthy(_:)` seam rather than a `BackendController`, whose
/// `health` is `private(set)` and only movable by a real health monitor.
@MainActor
@Test func onlyTheHealthyBackendStateCountsAsLiveDataInTheMenuBar() {
    #expect(MenuBarView.isHealthy(.healthy(version: "0.74.0")))

    // Everything else is "showing last known": the caption appears and the
    // gate buttons disable.
    #expect(!MenuBarView.isHealthy(.starting))
    #expect(!MenuBarView.isHealthy(.degraded("slow")))
    #expect(!MenuBarView.isHealthy(.down("connection refused")))
    #expect(!MenuBarView.isHealthy(.incompatible(serverVersion: "0.1.0")))
}
