import Foundation
import Observation
import RupuBackend

/// Top-level app state: current screen, time-range filter, backend health,
/// and live-stream counters. One instance is owned by the app scene and
/// threaded down through `RootView`.
@MainActor
@Observable
public final class AppModel {
    public var route: Route = .overview {
        didSet {
            // Tracks the most recent `.activity(kind)` route so
            // `navigateBack()` can return to it after a push to
            // `.runDetail`/`.sessionDetail`. A stored-property initializer
            // (see `lastActivityRoute` below) never runs observers, so this
            // deliberately doesn't fire for the `.overview` starting value —
            // `lastActivityRoute`'s own default (`.activity(.all)`) is what
            // backs a `navigateBack()` called before the user ever visits
            // Activity.
            if case .activity = route {
                lastActivityRoute = route
            }
            // Carry-over (Phase 3, Task 4): a *direct* `route =` assignment
            // (sidebar click, tab switch) is a new context, not a step
            // deeper into the current one — it clears whatever `navigate
            // (to:)` had pushed. `navigate(to:)`/`navigateBack()` below set
            // `route` too, but through this same `didSet`, so they flip
            // `isNavigatingViaStack` first to opt out of the clear for the
            // duration of their own assignment.
            if !isNavigatingViaStack {
                routeStack.removeAll()
            }
        }
    }

    /// The last `.activity(kind)` route visited — restored by
    /// `navigateBack()`. Defaults to `.activity(.all)` so a `navigateBack()`
    /// called with no prior Activity visit still lands somewhere sensible.
    public private(set) var lastActivityRoute: Route = .activity(.all)

    /// Carry-over (Phase 3, Task 4): the real navigation stack `navigate
    /// (to:)` pushes onto and `navigateBack()` pops from — closes the
    /// single-level `navigateBack()` quirk matt flagged (a run pushed atop
    /// a session detail used to skip straight past it back to Activity).
    /// A direct `route =` assignment (sidebar/tab switch) clears this —
    /// see `route`'s `didSet`.
    public private(set) var routeStack: [Route] = []

    /// Set for the duration of a `navigate(to:)`/`navigateBack()`-driven
    /// `route` assignment so `route`'s `didSet` doesn't treat it as a fresh
    /// sidebar-style context switch and clear `routeStack` out from under
    /// the very push/pop that's updating it.
    private var isNavigatingViaStack = false

    public var range: TimeRange = .d7
    public var backendHealth: BackendHealth = .starting
    public var liveConnected: Bool = false
    public var liveEventCount: Int = 0

    /// Whether the user has completed the Embedded/Remote onboarding sheet
    /// (Task 9). Persisted so a relaunch skips straight to the shell.
    public var onboardingComplete: Bool {
        didSet {
            defaults.set(onboardingComplete, forKey: Self.onboardingCompleteKey)
        }
    }

    private let defaults: UserDefaults

    private static let onboardingCompleteKey = "onboarding.complete"

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        self.onboardingComplete = defaults.bool(forKey: Self.onboardingCompleteKey)
    }

    /// The sidebar's selection, derived from/driving `route`. There is no
    /// separate stored selection: a sidebar click writes `route` through
    /// this setter, and anything that changes `route` programmatically
    /// (deep links, Phase 2 navigation) is reflected back into the sidebar
    /// through the getter. Sidebar selection and the Activity kind filter
    /// are the same state by construction.
    public var selectedSidebarItem: SidebarItem {
        get {
            switch route {
            case .overview: .overview
            case .activity(.all): .runs
            case .activity(let kind): .runsLeaf(kind)
            case .runDetail, .sessionDetail, .agentRunDetail: .runs
            case .projects: .projects
            case .security: .security
            case .library: .library
            case .fleet: .fleet
            case .usage: .usage
            }
        }
        set {
            switch newValue {
            case .overview: route = .overview
            case .runs: route = .activity(.all)
            case .runsLeaf(let kind): route = .activity(kind)
            case .projects: route = .projects
            case .security: route = .security
            case .library: route = .library
            case .fleet: route = .fleet
            case .usage: route = .usage
            }
        }
    }

    /// Pushes the *current* `route` onto `routeStack`, then sets `route` to
    /// `newRoute` — the row-activation half of the stack (`ActivityTable`/
    /// `SessionDetailScreen`'s child rows call this instead of assigning
    /// `route` directly, so a chain of pushes — e.g. session → run — pops
    /// back through each intermediate stop rather than jumping straight to
    /// Activity).
    public func navigate(to newRoute: Route) {
        isNavigatingViaStack = true
        routeStack.append(route)
        route = newRoute
        isNavigatingViaStack = false
    }

    /// Pops the most recently pushed route and returns to it. With nothing
    /// on `routeStack` (either nothing was ever pushed, or a direct `route =`
    /// assignment since cleared it — see `route`'s `didSet`), falls back to
    /// whichever `.activity(kind)` route was current before that, same as
    /// the single-level contract this replaces.
    ///
    /// Review fix: the empty-stack fallback is still a `navigateBack()`
    /// assignment, not a fresh sidebar-style context switch — it goes
    /// through `isNavigatingViaStack` the same as the pop branch below, for
    /// the same reason every other stack-driven `route` assignment does
    /// (`route`'s `didSet`). Harmless today (`routeStack` is already empty
    /// here, so clearing it either way is a no-op), but leaving this one
    /// assignment inconsistent would be a trap for whatever this method
    /// grows into next.
    public func navigateBack() {
        let target = routeStack.popLast() ?? lastActivityRoute
        isNavigatingViaStack = true
        route = target
        isNavigatingViaStack = false
    }
}
