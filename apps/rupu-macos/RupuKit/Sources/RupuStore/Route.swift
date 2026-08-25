/// Which slice of run history the Activity screen (Phase 2) is filtered to.
///
/// The Activity screen's own `FilterBar` segmented control is the only UI
/// that picks a specific kind — the sidebar (v2 rail, flows-composition
/// Task 1) collapsed to a single `SidebarItem.activity` entry that always
/// selects whichever kind was last active (`AppModel.lastActivityRoute`),
/// so this enum is no longer a sidebar discriminator, only the Activity
/// screen's own filter state.
public enum RunKindFilter: String, CaseIterable, Sendable {
    case all, agents, workflows, autoflows, sessions
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
    case security
    /// One coverage target's detail view (Phase 5B, Task 3 stubs the route;
    /// Task 4 builds `CoverageDetailScreen`) — pushed from a `.security`
    /// Coverage-tab row tap via `AppModel.navigate(to:)`, same "pushed, not
    /// directly sidebar-selectable" contract `.projectDetail` documents for
    /// Projects. `wsID` is `APICoverageSummary.wsID` (never optional on that
    /// row — see its doc comment), carried straight through to
    /// `CPClient.coverageDetail(target:wsID:)`'s disambiguation query param.
    /// Until Task 4 lands, `RootView` renders this as an honest
    /// `PlaceholderScreen` — see that switch arm's own comment.
    case coverageDetail(target: String, wsID: String)
    case library
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
    case fleet
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
/// `.activity` row per the v2 flat rail — kind selection now lives entirely
/// in the Activity screen's own `FilterBar`, not the sidebar.
public enum SidebarItem: Hashable {
    case overview
    case activity
    case projects
    case security
    case library
    case fleet
    case usage
}
