/// Which slice of run history the Activity screen (Phase 2) is filtered to.
///
/// Doubles as the discriminator for the Runs section's sidebar leaves — see
/// `SidebarItem.runsLeaf(_:)` — so the sidebar selection and the Activity
/// screen's kind filter are always the same piece of state, never two that
/// can drift apart.
public enum RunKindFilter: String, CaseIterable, Sendable {
    case all, agents, workflows, autoflows, sessions
}

/// Every screen the shell can show in its detail pane.
///
/// `.runDetail`/`.sessionDetail` (Phase 2) are *pushed* states reached from
/// an `ActivityRow.Navigation` — not directly sidebar-selectable, unlike
/// `.activity(_:)`. `AppModel.selectedSidebarItem`'s getter maps both to
/// plain `.runs` (the sidebar keeps highlighting the Runs section rather
/// than showing no selection or guessing a kind leaf), and
/// `AppModel.navigateBack()` is how the shell returns from either of them
/// to whichever `.activity(kind)` route was current before the push.
public enum Route: Hashable, Sendable {
    case overview
    case activity(RunKindFilter)
    case runDetail(id: String, host: String?)
    case sessionDetail(id: String)
    case projects
    case security
    case library
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
public enum SidebarItem: Hashable {
    case overview
    case runs
    case runsLeaf(RunKindFilter)
    case projects
    case security
    case library
    case fleet
    case usage
}
