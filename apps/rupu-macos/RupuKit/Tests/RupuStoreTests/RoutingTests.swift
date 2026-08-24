import Foundation
import Testing
@testable import RupuStore

@MainActor @Test func sidebarLeavesAndKindFilterAreSameState() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    #expect(model.route == .overview)
    model.route = .activity(.workflows)
    #expect(model.selectedSidebarItem == SidebarItem.runsLeaf(.workflows))
    model.route = .activity(.all)
    #expect(model.selectedSidebarItem == SidebarItem.runs)
}
@MainActor @Test func onboardingFlagPersists() {
    let suite = "test-\(UUID())"
    let d = UserDefaults(suiteName: suite)!
    AppModel(defaults: d).onboardingComplete = true
    #expect(AppModel(defaults: d).onboardingComplete)
}

@MainActor @Test func runDetailAndSessionDetailKeepSidebarHighlightOnRuns() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .activity(.agents)

    model.route = .runDetail(id: "run-1", host: "mini")
    #expect(model.selectedSidebarItem == SidebarItem.runs)

    model.route = .sessionDetail(id: "sess-1")
    #expect(model.selectedSidebarItem == SidebarItem.runs)
}

@MainActor @Test func navigateBackRestoresPriorActivityFilterDefaultingToAll() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)

    // No Activity route visited yet: navigateBack() still lands somewhere
    // sensible rather than nowhere.
    model.navigateBack()
    #expect(model.route == .activity(.all))

    model.route = .activity(.workflows)
    model.route = .runDetail(id: "run-1", host: nil)
    model.navigateBack()
    #expect(model.route == .activity(.workflows))

    // A second push/pop with a different kind updates the restored target
    // too, not just the first one remembered.
    model.route = .activity(.sessions)
    model.route = .sessionDetail(id: "sess-1")
    model.navigateBack()
    #expect(model.route == .activity(.sessions))
}

// MARK: - Navigation stack (Phase 3, Task 4 carry-over)
//
// `navigate(to:)`/`navigateBack()` replace the single-level
// push-then-restore contract above with a real stack. The tests above stay
// green unchanged because they only ever assign `route` directly (never call
// `navigate(to:)`), so `routeStack` stays empty throughout them and
// `navigateBack()`'s empty-stack fallback (`lastActivityRoute`) is exactly
// the old behavior — this section covers the stack itself.

// (a) The quirk matt flagged, now fixed: two chained `navigate(to:)` pushes
// (session → run) pop back through *each* intermediate stop — a
// `navigateBack()` from the run lands on the session detail, not straight
// past it to Activity.
@MainActor @Test func navigateToPushesOntoStackAndNavigateBackPopsThroughEachLevel() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .activity(.sessions)

    model.navigate(to: .sessionDetail(id: "sess-1"))
    #expect(model.route == .sessionDetail(id: "sess-1"))
    #expect(model.routeStack == [.activity(.sessions)])

    model.navigate(to: .runDetail(id: "run-1", host: nil))
    #expect(model.route == .runDetail(id: "run-1", host: nil))
    #expect(model.routeStack == [.activity(.sessions), .sessionDetail(id: "sess-1")])

    model.navigateBack()
    #expect(model.route == .sessionDetail(id: "sess-1"))
    #expect(model.routeStack == [.activity(.sessions)])

    model.navigateBack()
    #expect(model.route == .activity(.sessions))
    #expect(model.routeStack == [])
}

// (b) A direct `route =` assignment (sidebar/tab switch) is a new context —
// it clears whatever `navigate(to:)` had pushed, so a `navigateBack()` after
// it falls straight to `lastActivityRoute`, not to the discarded stack.
@MainActor @Test func settingRouteDirectlyClearsTheNavigationStack() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .activity(.sessions)
    model.navigate(to: .sessionDetail(id: "sess-1"))
    #expect(model.routeStack == [.activity(.sessions)])

    model.route = .activity(.agents)
    #expect(model.routeStack == [])

    model.navigateBack()
    #expect(model.route == .activity(.agents))
}

// (c) `navigateBack()` with an empty stack (nothing ever pushed via
// `navigate(to:)`) falls back to `lastActivityRoute`, same contract as
// before the stack existed.
@MainActor @Test func navigateBackWithEmptyStackFallsBackToLastActivityRoute() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.navigateBack()
    #expect(model.route == .activity(.all))

    model.route = .activity(.workflows)
    model.navigateBack()
    #expect(model.route == .activity(.workflows))
    #expect(model.routeStack == [])
}
