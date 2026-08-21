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
        }
    }

    /// The last `.activity(kind)` route visited — restored by
    /// `navigateBack()`. Defaults to `.activity(.all)` so a `navigateBack()`
    /// called with no prior Activity visit still lands somewhere sensible.
    public private(set) var lastActivityRoute: Route = .activity(.all)

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

    /// Returns from a pushed `.runDetail`/`.sessionDetail` route to
    /// whichever `.activity(kind)` route was current before the push (or
    /// `.activity(.all)`, `lastActivityRoute`'s default, if Activity was
    /// never visited this session).
    public func navigateBack() {
        route = lastActivityRoute
    }
}
