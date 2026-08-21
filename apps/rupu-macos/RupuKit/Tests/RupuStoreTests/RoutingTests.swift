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
