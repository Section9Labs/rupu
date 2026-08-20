import Foundation
import Observation
import RupuBackend

/// Top-level app state: current screen, time-range filter, backend health,
/// and live-stream counters. One instance is owned by the app scene and
/// threaded down through `RootView`.
@MainActor
@Observable
public final class AppModel {
    public var route: Route = .overview
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
}
