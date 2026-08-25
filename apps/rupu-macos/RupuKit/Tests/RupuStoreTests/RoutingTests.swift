import Foundation
import Testing
@testable import RupuStore

@MainActor @Test func allActivityKindsMapToTheSingleActivitySidebarItem() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    #expect(model.route == .overview)
    for kind in RunKindFilter.allCases {
        model.route = .activity(kind)
        #expect(model.selectedSidebarItem == SidebarItem.activity)
    }
}

@MainActor @Test func selectingActivityRestoresLastActivityKind() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .activity(.workflows)
    model.route = .projects

    model.selectedSidebarItem = .activity

    #expect(model.route == .activity(.workflows))
}

@MainActor @Test func onboardingFlagPersists() {
    let suite = "test-\(UUID())"
    let d = UserDefaults(suiteName: suite)!
    AppModel(defaults: d).onboardingComplete = true
    #expect(AppModel(defaults: d).onboardingComplete)
}

@MainActor @Test func rangeAndScopePersistAcrossAppModelInstances() {
    let suite = "test-\(UUID())"
    let d = UserDefaults(suiteName: suite)!

    // Defaults before anything is ever written: no scope (all projects),
    // whatever `range`'s own default is.
    #expect(AppModel(defaults: d).scopeWsID == nil)

    let first = AppModel(defaults: d)
    first.range = .d30
    first.scopeWsID = "ws-1"

    let second = AppModel(defaults: d)
    #expect(second.range == .d30)
    #expect(second.scopeWsID == "ws-1")

    // "All projects" (nil) persists too, not just a non-nil scope.
    second.scopeWsID = nil
    let third = AppModel(defaults: d)
    #expect(third.scopeWsID == nil)
}

@MainActor @Test func runDetailAndSessionDetailKeepSidebarHighlightOnRuns() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .activity(.agents)

    model.route = .runDetail(id: "run-1", host: "mini")
    #expect(model.selectedSidebarItem == SidebarItem.activity)

    model.route = .sessionDetail(id: "sess-1")
    #expect(model.selectedSidebarItem == SidebarItem.activity)
}

@MainActor @Test func projectDetailKeepsSidebarHighlightOnProjects() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .projects

    model.route = .projectDetail(wsID: "ws-1")
    #expect(model.selectedSidebarItem == SidebarItem.projects)
}

/// Phase 5A, Task 7: `.agentDefinition`/`.workflowDefinition` are pushed from
/// `.library` (either agents/workflows tab, or an autoflows-tab row — an
/// autoflow definition IS a workflow definition, no separate detail route)
/// and keep the sidebar highlighting Library, same "pushed, not directly
/// sidebar-selectable" contract `.projectDetail` already has for Projects.
@MainActor @Test func agentAndWorkflowDefinitionKeepSidebarHighlightOnLibrary() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .library

    model.route = .agentDefinition(name: "code-reviewer")
    #expect(model.selectedSidebarItem == SidebarItem.library)

    model.route = .workflowDefinition(name: "nightly-health")
    #expect(model.selectedSidebarItem == SidebarItem.library)
}

/// `navigate(to:)`/`navigateBack()` from Library push/pop the same as every
/// other pushed detail route.
@MainActor @Test func navigateBackFromAgentDefinitionReturnsToLibrary() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .library

    model.navigate(to: .agentDefinition(name: "code-reviewer"))
    #expect(model.route == .agentDefinition(name: "code-reviewer"))
    #expect(model.routeStack == [.library])

    model.navigateBack()
    #expect(model.route == .library)
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

// MARK: - Launcher prefill seam (Phase 5A, Task 7)
//
// `AppModel.presentLauncher(...)`/`consumeLauncherPrefill()` — the Library
// screen's per-row/page Launch seam. See `AppModel.launcherPrefill`'s doc
// comment for the produce/consume contract `LauncherSheet.activate()`
// depends on.

@MainActor @Test func presentLauncherSetsShowLauncherAndTheRequestedPrefill() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    #expect(!model.showLauncher)
    #expect(model.launcherPrefill == nil)

    model.presentLauncher(kind: .agentRun, name: "code-reviewer", scopeKind: "project", scopeID: "ws-1")

    #expect(model.showLauncher)
    #expect(model.launcherPrefill == LauncherPrefillRequest(kind: .agentRun, name: "code-reviewer", scopeKind: "project", scopeID: "ws-1"))
}

/// `consumeLauncherPrefill()` reads and clears in one step — a second call
/// with nothing new set returns `nil`, never re-delivering the same
/// request. This is what keeps a later plain "+ New run" open (toolbar/⌘N,
/// neither of which calls `presentLauncher`) from silently inheriting a
/// stale prior per-row Launch request.
@MainActor @Test func consumeLauncherPrefillReadsAndClearsInOneStep() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.presentLauncher(kind: .workflow, name: "nightly-health")

    let first = model.consumeLauncherPrefill()
    #expect(first == LauncherPrefillRequest(kind: .workflow, name: "nightly-health"))
    #expect(model.launcherPrefill == nil)

    let second = model.consumeLauncherPrefill()
    #expect(second == nil)
}

/// `showLauncher` toggled directly (the toolbar/⌘N path) never sets a
/// prefill — `presentLauncher(...)` is the only producer.
@MainActor @Test func settingShowLauncherDirectlyNeverSetsAPrefill() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.showLauncher = true
    #expect(model.launcherPrefill == nil)
    #expect(model.consumeLauncherPrefill() == nil)
}
