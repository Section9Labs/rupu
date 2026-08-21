import RupuStore

/// Display strings and Phase-1 placeholder numbering for `Route`. Kept
/// module-internal to RupuShell: `RupuStore` stays UI-string-free, and this
/// mapping only exists to drive chrome (toolbar title, placeholder banner)
/// that Phase 2+ deletes screen-by-screen as real content lands.
extension Route {
    var screenTitle: String {
        switch self {
        case .overview: "Overview"
        case .activity(let kind): kind.screenTitle
        case .runDetail: "Run"
        case .sessionDetail: "Session"
        case .agentRunDetail: "Agent run"
        case .projects: "Projects"
        case .security: "Security"
        case .library: "Library"
        case .fleet: "Fleet"
        case .usage: "Usage"
        }
    }

    /// The phase (per spec §8) that first replaces this route's
    /// `PlaceholderScreen` with real content.
    var placeholderPhase: Int {
        switch self {
        case .overview: 4
        case .activity, .runDetail, .sessionDetail, .agentRunDetail: 2
        case .projects, .security, .library, .fleet, .usage: 5
        }
    }
}

extension RunKindFilter {
    var screenTitle: String {
        switch self {
        case .all: "Activity"
        case .agents: "Agent runs"
        case .workflows: "Workflows"
        case .autoflows: "Autoflows"
        case .sessions: "Sessions"
        }
    }
}
