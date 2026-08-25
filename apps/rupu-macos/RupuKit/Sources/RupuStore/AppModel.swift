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

    /// The time window applied to counts/aggregates across screens that
    /// show them. Persisted (Flows-composition Task 2) the same manual way
    /// as `onboardingComplete` — `TimeRange`'s `rawValue` is the stored
    /// string; an unrecognized/missing stored value falls back to `.d7`.
    /// No inline default (unlike its pre-Task-2 declaration): a stored
    /// property with a declaration-site default already counts as
    /// "initialized" by the time `init` reassigns it, so `didSet` would
    /// fire a second, redundant (if harmless) write-back of the value
    /// `init` just read — same reasoning `onboardingComplete` already
    /// follows by leaving no default here either.
    public var range: TimeRange {
        didSet {
            defaults.set(range.rawValue, forKey: Self.rangeKey)
        }
    }

    /// The project the top bar's scope picker has narrowed to — `nil`
    /// means "All projects". Persisted the same manual way as
    /// `onboardingComplete`/`range`. Drives `ActivityStore.scopeFilter`
    /// (client-side row narrowing, no refetch).
    public var scopeWsID: String? {
        didSet {
            defaults.set(scopeWsID, forKey: Self.scopeWsIDKey)
        }
    }

    public var backendHealth: BackendHealth = .starting
    public var liveConnected: Bool = false
    public var liveEventCount: Int = 0

    /// Whether `RootView`'s Launcher sheet (`.sheet(isPresented:)`) is
    /// presented. Lifted from a `RootView`-local `@State` (Phases 1–5) into
    /// `AppModel` in Phase 5A, Task 7 so a screen below the toolbar
    /// (Library's per-row/page Launch) can open the sheet itself, not just
    /// the toolbar button / hidden ⌘N button `RootView` already wires —
    /// every opener now flips this one flag instead of a `RootView`-private
    /// `@State` only `RootView` itself could reach.
    public var showLauncher = false

    /// Set by `presentLauncher(...)` alongside `showLauncher = true` —
    /// `LauncherSheet.activate()` is the sole reader, via
    /// `consumeLauncherPrefill()` (which clears this the instant it's read,
    /// see that method's doc comment). The toolbar/⌘N openers never touch
    /// this field at all, so a plain "+ New run" open is never affected by
    /// whatever a prior per-row Launch last set here.
    public private(set) var launcherPrefill: LauncherPrefillRequest?

    /// Opens the Launcher sheet pre-selected to one definition, carrying its
    /// scope — the Library screen's per-row and detail-page Launch
    /// affordance (Phase 5A, Task 7). `scopeKind`/`scopeID` should be the
    /// row's own values (`AgentDefinition`/`WorkflowDefinition`/
    /// `AutoflowDefinition`'s `scopeKind`/`scopeID`, or `AgentDetail`'s),
    /// same "carry the row's own scope, never re-derive by name" rule
    /// `DefinitionPicker`'s row-tap already follows (see
    /// `LauncherStore.prefill(kind:name:scopeKind:scopeID:)`'s doc comment).
    public func presentLauncher(kind: LaunchKind, name: String, scopeKind: String? = nil, scopeID: String? = nil) {
        launcherPrefill = LauncherPrefillRequest(kind: kind, name: name, scopeKind: scopeKind, scopeID: scopeID)
        showLauncher = true
    }

    /// Reads and clears `launcherPrefill` in one step — `LauncherSheet.
    /// activate()`'s only call site, right after it builds a fresh
    /// `LauncherStore` for this sheet presentation. Consuming (not just
    /// reading) here is what keeps a later plain "+ New run" open honest:
    /// without the clear, a per-row Launch's prefill would silently leak
    /// into the next unrelated sheet presentation.
    public func consumeLauncherPrefill() -> LauncherPrefillRequest? {
        defer { launcherPrefill = nil }
        return launcherPrefill
    }

    /// Whether the user has completed the Embedded/Remote onboarding sheet
    /// (Task 9). Persisted so a relaunch skips straight to the shell.
    public var onboardingComplete: Bool {
        didSet {
            defaults.set(onboardingComplete, forKey: Self.onboardingCompleteKey)
        }
    }

    private let defaults: UserDefaults

    private static let onboardingCompleteKey = "onboarding.complete"
    private static let rangeKey = "range"
    private static let scopeWsIDKey = "scope.wsID"

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        self.onboardingComplete = defaults.bool(forKey: Self.onboardingCompleteKey)
        self.range = defaults.string(forKey: Self.rangeKey).flatMap(TimeRange.init(rawValue:)) ?? .d7
        self.scopeWsID = defaults.string(forKey: Self.scopeWsIDKey)
    }

    /// The sidebar's selection, derived from/driving `route`. There is no
    /// separate stored selection: a sidebar click writes `route` through
    /// this setter, and anything that changes `route` programmatically
    /// (deep links, Phase 2 navigation) is reflected back into the sidebar
    /// through the getter.
    ///
    /// Flows-composition Task 1: every `.activity(_:)` kind (and every
    /// pushed detail route reached from one) maps to the single
    /// `SidebarItem.activity` — the v2 rail no longer has a kind-per-row
    /// "Runs" section, kind selection lives in the Activity screen's own
    /// `FilterBar`. Selecting `.activity` from the rail restores
    /// `lastActivityRoute` rather than forcing `.all`, so the user's kind
    /// tab survives a round trip through another sidebar item.
    public var selectedSidebarItem: SidebarItem {
        get {
            switch route {
            case .overview: .overview
            case .activity: .activity
            case .runDetail, .sessionDetail, .agentRunDetail: .activity
            case .projects, .projectDetail: .projects
            case .security: .security
            case .library, .agentDefinition, .workflowDefinition: .library
            case .fleet: .fleet
            case .usage: .usage
            }
        }
        set {
            switch newValue {
            case .overview: route = .overview
            case .activity: route = lastActivityRoute
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
