/// Which slice of run history the Activity screen (Phase 2) is filtered to.
///
/// The Activity screen's own `FilterBar` segmented control was, for a while
/// (flows-composition Task 1), the only UI that picked a specific kind — the
/// sidebar had collapsed to a single `SidebarItem.activity` entry that
/// always selected whichever kind was last active (`AppModel.
/// lastActivityRoute`). Design-alignment Task 0 restores a sidebar-level
/// picker too (`RupuShell.Sidebar`'s disclosure children, one row per case
/// here) — but `SidebarItem` granularity is unchanged, see that enum's own
/// doc comment.
public enum RunKindFilter: String, CaseIterable, Sendable {
    case all, agents, workflows, autoflows, sessions
}

/// Which tab the Security screen is showing (Task 0, sidebar disclosure
/// sub-items — moved here from `RupuSecurity/SecurityScreen.swift` so
/// `Route` can carry it without an upward dependency on that module).
/// Segmented control, same idiom `LibraryTab`/`ProjectDetailTab` already use.
///
/// **No `.network` case** — deliberately deferred: the app has no netflow
/// aggregate surface yet, and a sidebar child that navigates to nothing
/// would be worse than one that's simply absent.
public enum SecurityTab: String, CaseIterable, Sendable {
    case findings, coverage

    public var title: String {
        switch self {
        case .findings: "Findings"
        case .coverage: "Coverage"
        }
    }
}

/// Which definition tab the Library screen is showing (Task 0 — moved here
/// from `RupuLibrary/LibraryScreen.swift`, same rationale as `SecurityTab`
/// above). Segmented control, same idiom `FilterBar`'s kind picker uses for
/// Activity.
public enum LibraryTab: String, CaseIterable, Sendable {
    case agents, workflows, autoflows

    public var title: String {
        switch self {
        case .agents: "Agents"
        case .workflows: "Workflows"
        case .autoflows: "Autoflows"
        }
    }
}

/// Which tab the Fleet screen is showing (Task 0 — `FleetScreen` previously
/// had no tab at all, stacking hosts and workers content in one scroll view;
/// this gives it the same route-driven tab idiom `SecurityTab`/`LibraryTab`
/// already follow).
public enum FleetTab: String, CaseIterable, Sendable {
    case hosts, workers

    public var title: String {
        switch self {
        case .hosts: "Hosts"
        case .workers: "Workers"
        }
    }
}

/// Every screen the shell can show in its detail pane.
///
/// `.runDetail`/`.sessionDetail`/`.agentRunDetail` (Phase 2 / hotfix) are
/// *pushed* states reached from an `ActivityRow.Navigation` — not directly
/// sidebar-selectable, unlike `.activity(_:)`. `AppModel.selectedSidebarItem`'s
/// getter maps all three (plus every `.activity(_:)` kind) to the single
/// `SidebarItem.activity` (the sidebar keeps highlighting the Activity row
/// rather than showing no selection or guessing a kind leaf). Row activation
/// pushes via `AppModel.navigate(to:)`, so a chain of
/// pushes (e.g. session → run) can nest; `AppModel.navigateBack()` pops one
/// level at a time, falling back to whichever `.activity(kind)` route was
/// current before the first push once `routeStack` is empty (Phase 3, Task
/// 4 — replaces the earlier single-level `navigateBack()`).
public enum Route: Hashable, Sendable {
    case overview
    case activity(RunKindFilter)
    case runDetail(id: String, host: String?)
    case sessionDetail(id: String)
    /// A standalone agent run (hotfix root cause C: `ActivityRow.Navigation
    /// .agentRun` — a row `GET /api/runs/:id` can never serve, so this
    /// route never reaches `RunDetailScreen`). `transcriptPath` is carried
    /// straight from the row that navigated here; `nil` renders an honest
    /// "no transcript recorded" state rather than attempting a fetch.
    case agentRunDetail(id: String, transcriptPath: String?, host: String?)
    case projects
    /// One project's detail (Phase 5A, Task 5) — pushed from a `.projects`
    /// row tap via `AppModel.navigate(to:)`, same "pushed, not directly
    /// sidebar-selectable" contract `.runDetail`/`.sessionDetail` already
    /// follow. `selectedSidebarItem`'s getter maps this to
    /// `SidebarItem.projects`, same as those two map to `.activity`.
    case projectDetail(wsID: String)
    /// The Security screen's tab (Task 0, sidebar disclosure sub-items) —
    /// there is no default-associated-value shorthand in Swift, so every
    /// construction site spells the default out explicitly:
    /// `.security(.findings)`. `AppModel.selectedSidebarItem`'s setter is the
    /// canonical "what does a bare parent-row click mean" answer.
    case security(SecurityTab)
    /// One coverage target's detail view (Phase 5B, Task 3 stubs the route;
    /// Task 4 builds `CoverageDetailScreen`) — pushed from a `.security`
    /// Coverage-tab row tap via `AppModel.navigate(to:)`, same "pushed, not
    /// directly sidebar-selectable" contract `.projectDetail` documents for
    /// Projects. `wsID` is `APICoverageSummary.wsID` (never optional on that
    /// row — see its doc comment), carried straight through to
    /// `CPClient.coverageDetail(target:wsID:)`'s disambiguation query param.
    case coverageDetail(target: String, wsID: String)
    /// The Library screen's tab (Task 0) — same "spell the default out at
    /// every construction site" contract `.security` documents above;
    /// default is `.library(.agents)`.
    case library(LibraryTab)
    /// One agent definition's detail view (Phase 5A, Task 7) — pushed from
    /// a `.library` agents-tab row tap via `AppModel.navigate(to:)`, same
    /// "pushed, not directly sidebar-selectable" contract `.projectDetail`
    /// documents above. `selectedSidebarItem`'s getter maps this to
    /// `SidebarItem.library`.
    ///
    /// **`scopeKind`/`scopeID` (final-review fix wave)**: the tapped row's
    /// own scope, carried alongside `name` so a same-named definition at a
    /// DIFFERENT scope never gets pinned to the wrong row — the same
    /// scope-collision class `ActionKey.autoflow(...)` (`RupuStore`) already
    /// fixes for the Library toggle. `AgentDetailScreen`/`WorkflowDetailScreen`
    /// use these to pin their own row-derived chrome/toggle/launch to the
    /// tapped row; the underlying `GET /api/agents/:name` / `GET /api/
    /// workflows/:name` calls themselves take no scope query params (a
    /// shared, tracked web gap — see those screens' own doc comments for the
    /// exact residual each one documents).
    case agentDefinition(name: String, scopeKind: String? = nil, scopeID: String? = nil)
    /// One workflow definition's detail view — also reached from a
    /// `.library` autoflows-tab row tap (an autoflow definition IS a
    /// workflow definition; there is no separate autoflow detail route),
    /// same push/sidebar-highlight contract as `.agentDefinition` above.
    /// `scopeKind`/`scopeID`: see `.agentDefinition`'s doc comment.
    case workflowDefinition(name: String, scopeKind: String? = nil, scopeID: String? = nil)
    /// The Fleet screen's tab (Task 0) — same "no default-associated-value
    /// shorthand, spell it out at every construction site" contract
    /// `.security`/`.library` document above; default is `.fleet(.hosts)`.
    case fleet(FleetTab)
    case usage
}

/// The time window applied to counts/aggregates across screens that show
/// them (Phase 2+). Persisted independently of `Route`.
public enum TimeRange: String, CaseIterable, Sendable {
    case d7 = "7d"
    case d30 = "30d"
    case all
}

/// The sidebar's own selection identity. Derived from/driving `Route` via
/// `AppModel.selectedSidebarItem` — never stored independently, so a
/// sidebar click and a programmatic `route` change can never disagree about
/// what's selected.
///
/// Flows-composition Task 1 collapsed the old `.runs`/`.runsLeaf(_:)` pair
/// (a "Runs" section with one row per `RunKindFilter`) into the single
/// `.activity` row per the v2 flat rail — kind selection lived entirely in
/// the Activity screen's own `FilterBar`, not the sidebar.
///
/// **Design-alignment Task 0 amendment**: `RupuShell.Sidebar` restores
/// sidebar-level reachability via disclosure children (Activity's `.all`/
/// `.agents`/`.workflows`/`.autoflows`/`.sessions`; Security's/Library's/
/// Fleet's own tab enums) — but the *sidebar item* granularity here is
/// unchanged: every tab of a composite screen still maps to that screen's
/// one `SidebarItem` (`selectedSidebarItem`'s getter ignores the associated
/// tab), the same way every `.activity(kind)` already collapses to
/// `.activity`. Only the rail's own child rows discriminate tabs.
public enum SidebarItem: Hashable {
    case overview
    case activity
    case projects
    case security
    case library
    case fleet
    case usage
}
