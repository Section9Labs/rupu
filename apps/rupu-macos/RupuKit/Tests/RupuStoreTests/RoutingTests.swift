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
    model.route = .library(.agents)

    model.route = .agentDefinition(name: "code-reviewer")
    #expect(model.selectedSidebarItem == SidebarItem.library)

    model.route = .workflowDefinition(name: "nightly-health")
    #expect(model.selectedSidebarItem == SidebarItem.library)
}

/// `navigate(to:)`/`navigateBack()` from Library push/pop the same as every
/// other pushed detail route.
@MainActor @Test func navigateBackFromAgentDefinitionReturnsToLibrary() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .library(.agents)

    model.navigate(to: .agentDefinition(name: "code-reviewer"))
    #expect(model.route == .agentDefinition(name: "code-reviewer"))
    #expect(model.routeStack == [.library(.agents)])

    model.navigateBack()
    #expect(model.route == .library(.agents))
}

/// Final-review fix wave: `.agentDefinition`/`.workflowDefinition` gained
/// `scopeKind`/`scopeID` so a Library row's tap carries its own scope all
/// the way to the pushed detail route (`WorkflowDetailScreen.definitionRow`/
/// `AgentDetailScreen`'s Launch button both key off these, not off `name`
/// alone — see those types' doc comments). Two routes for the SAME name at
/// DIFFERENT scopes must compare unequal — this is the exact invariant that
/// keeps a same-named-but-differently-scoped definition from being silently
/// conflated, the same scope-collision class `ActionKey.autoflow(...)`
/// already fixes for the Library toggle.
@MainActor @Test func definitionRoutesCarryScopeAndDifferByIt() {
    let globalRoute = Route.agentDefinition(name: "code-reviewer", scopeKind: "global", scopeID: nil)
    let projectRoute = Route.agentDefinition(name: "code-reviewer", scopeKind: "project", scopeID: "ws-1")
    #expect(globalRoute != projectRoute)

    guard case .agentDefinition(let name, let scopeKind, let scopeID) = projectRoute else {
        Issue.record("expected .agentDefinition")
        return
    }
    #expect(name == "code-reviewer")
    #expect(scopeKind == "project")
    #expect(scopeID == "ws-1")

    let globalWorkflow = Route.workflowDefinition(name: "nightly-health", scopeKind: "global", scopeID: nil)
    let projectWorkflow = Route.workflowDefinition(name: "nightly-health", scopeKind: "project", scopeID: "ws-2")
    #expect(globalWorkflow != projectWorkflow)

    // Omitting scope entirely (legacy call shape) still defaults to nil/nil
    // and compares equal to an explicit nil/nil route.
    #expect(Route.agentDefinition(name: "x") == Route.agentDefinition(name: "x", scopeKind: nil, scopeID: nil))
}

/// A scoped push/pop round-trips exactly like the name-only case above —
/// the scope fields don't change `navigate(to:)`/`navigateBack()`'s stack
/// mechanics, only what the pushed route carries.
@MainActor @Test func navigateBackFromScopedWorkflowDefinitionReturnsToLibrary() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .library(.agents)

    model.navigate(to: .workflowDefinition(name: "issue-triage", scopeKind: "project", scopeID: "ws-1"))
    #expect(model.route == .workflowDefinition(name: "issue-triage", scopeKind: "project", scopeID: "ws-1"))
    #expect(model.routeStack == [.library(.agents)])

    model.navigateBack()
    #expect(model.route == .library(.agents))
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

/// Final-review fix wave, item 5: locks the sequencing contract
/// `LauncherSheet.activate()` now depends on — see that method's own doc
/// comment for the full bug this guards against. `LauncherSheet.activate()`
/// itself is `private` to a `View` and can't be called directly from a test
/// target, so this simulates its two relevant steps at the `AppModel` level:
/// (1) a first sheet open that requested a prefill drains it via
/// `consumeLauncherPrefill()` UNCONDITIONALLY, even when the client-
/// available guard immediately after would fail and no `LauncherStore`
/// ever gets built; (2) a second, later sheet open — a plain "+ New run"
/// that never calls `presentLauncher(...)` — must not inherit whatever the
/// first attempt drained but never got to apply.
@MainActor @Test func prefillDoesNotLeakAcrossSheetOpensWhenTheFirstOpenFoundNoClient() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.presentLauncher(kind: .agentRun, name: "code-reviewer", scopeKind: "project", scopeID: "ws-1")

    // First sheet open: activate() drains the prefill before its client
    // guard (the fix) — even though the simulated client check right after
    // this would fail (backend down) and no store gets built.
    let firstAttempt = model.consumeLauncherPrefill()
    #expect(firstAttempt == LauncherPrefillRequest(kind: .agentRun, name: "code-reviewer", scopeKind: "project", scopeID: "ws-1"))
    #expect(model.launcherPrefill == nil)

    // Second sheet open — plain, never calls presentLauncher(...) — finds
    // nothing left to inherit.
    #expect(model.consumeLauncherPrefill() == nil)
}

// MARK: - .coverageDetail (Phase 5B, Task 3)

/// `.coverageDetail` is pushed from `.security`'s Coverage tab (a row tap —
/// Task 4 builds the real destination screen, this task only stubs the
/// route + an honest `PlaceholderScreen`) and keeps the sidebar
/// highlighting Security, same "pushed, not directly sidebar-selectable"
/// contract `.projectDetail`/`.agentDefinition` already have for
/// Projects/Library.
@MainActor @Test func coverageDetailKeepsSidebarHighlightOnSecurity() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .security(.coverage)

    model.route = .coverageDetail(target: "auth-core", wsID: "ws-1")
    #expect(model.selectedSidebarItem == SidebarItem.security)
}

/// `navigate(to:)`/`navigateBack()` from Security push/pop the same as
/// every other pushed detail route.
@MainActor @Test func navigateBackFromCoverageDetailReturnsToSecurity() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    model.route = .security(.coverage)

    model.navigate(to: .coverageDetail(target: "auth-core", wsID: "ws-1"))
    #expect(model.route == .coverageDetail(target: "auth-core", wsID: "ws-1"))
    #expect(model.routeStack == [.security(.coverage)])

    model.navigateBack()
    #expect(model.route == .security(.coverage))
}

/// `Route` equality distinguishes `.coverageDetail` instances by both
/// associated values, same regression-guard shape `RouteTests`'
/// `agentRunDetailRoutesWithDifferentFieldsAreNotEqual` already establishes
/// for `.agentRunDetail`.
@MainActor @Test func coverageDetailRoutesWithDifferentFieldsAreNotEqual() {
    let base = Route.coverageDetail(target: "auth-core", wsID: "ws-1")
    #expect(base == Route.coverageDetail(target: "auth-core", wsID: "ws-1"))
    #expect(base != Route.coverageDetail(target: "web-api", wsID: "ws-1"))
    #expect(base != Route.coverageDetail(target: "auth-core", wsID: "ws-2"))
}

// MARK: - Tabbed routes (Task 0, sidebar disclosure sub-items)
//
// `.security`/`.library`/`.fleet` gained an associated tab enum
// (`SecurityTab`/`LibraryTab`/`FleetTab`) so `Route` can carry which of a
// composite screen's tabs is showing — the sidebar's disclosure children
// and each screen's own in-page tab picker both read/write it directly.
// Swift has no default-associated-value shorthand, so every construction
// site spells the default out explicitly; these tests pin down the
// equality/mapping contract that default relies on.

/// Constructing a route with its screen's explicit default tab equals
/// itself and any other route built the same way — the "default-tab
/// equality" the design-alignment amendment's ruling calls for in place of
/// `case security(SecurityTab = .findings)` (which Swift doesn't support).
@MainActor @Test func tabbedRoutesConstructedWithTheirExplicitDefaultAreEqual() {
    #expect(Route.security(.findings) == Route.security(.findings))
    #expect(Route.library(.agents) == Route.library(.agents))
    #expect(Route.fleet(.hosts) == Route.fleet(.hosts))
}

/// Two `.security`/`.library`/`.fleet` routes at different tabs are NOT
/// equal — a regression here would silently break SwiftUI's `.task(id:)`
/// re-triggering on a tab switch, the same class of bug
/// `agentRunDetailRoutesWithDifferentFieldsAreNotEqual` (`RouteTests`)
/// guards against for `.agentRunDetail`.
@MainActor @Test func tabbedRoutesAtDifferentTabsAreNotEqual() {
    #expect(Route.security(.findings) != Route.security(.coverage))
    #expect(Route.library(.agents) != Route.library(.workflows))
    #expect(Route.library(.workflows) != Route.library(.autoflows))
    #expect(Route.fleet(.hosts) != Route.fleet(.workers))
}

/// Every tab of `.security`/`.library`/`.fleet` maps back to the same
/// parent `SidebarItem` — `selectedSidebarItem`'s getter ignores which tab
/// is showing, same "every kind collapses to one row" contract
/// `allActivityKindsMapToTheSingleActivitySidebarItem` already establishes
/// for `.activity`.
@MainActor @Test func everyTabOfASecurityRouteMapsToTheSingleSecuritySidebarItem() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    for tab in SecurityTab.allCases {
        model.route = .security(tab)
        #expect(model.selectedSidebarItem == SidebarItem.security)
    }
}

@MainActor @Test func everyTabOfALibraryRouteMapsToTheSingleLibrarySidebarItem() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    for tab in LibraryTab.allCases {
        model.route = .library(tab)
        #expect(model.selectedSidebarItem == SidebarItem.library)
    }
}

@MainActor @Test func everyTabOfAFleetRouteMapsToTheSingleFleetSidebarItem() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    for tab in FleetTab.allCases {
        model.route = .fleet(tab)
        #expect(model.selectedSidebarItem == SidebarItem.fleet)
    }
}

/// Unlike `.activity` (whose parent-row click restores `lastActivityRoute`),
/// a bare `selectedSidebarItem = .security`/`.library`/`.fleet` assignment
/// always resets to that screen's fixed default tab — it does NOT remember
/// whichever tab was showing before the user navigated away. This is the
/// controller ruling's explicit amendment to the brief's original
/// `case security(SecurityTab = .findings)` shorthand: the default lives at
/// the setter, not on the enum case itself.
@MainActor @Test func selectingSecurityLibraryOrFleetFromTheSidebarAlwaysResetsToTheDefaultTab() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)

    model.route = .security(.coverage)
    model.selectedSidebarItem = .security
    #expect(model.route == .security(.findings))

    model.route = .library(.workflows)
    model.selectedSidebarItem = .library
    #expect(model.route == .library(.agents))

    model.route = .fleet(.workers)
    model.selectedSidebarItem = .fleet
    #expect(model.route == .fleet(.hosts))
}
