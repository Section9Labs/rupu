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
