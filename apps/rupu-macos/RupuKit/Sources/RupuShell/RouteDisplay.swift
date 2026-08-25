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
        case .projectDetail: "Project"
        case .security: "Security"
        case .coverageDetail: "Coverage"
        case .library: "Library"
        case .agentDefinition: "Agent"
        case .workflowDefinition: "Workflow"
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
        // `.security`/`.coverageDetail` are both real as of Phase 5B (Task 3
        // and Task 4 respectively) — this value is now unused for either
        // case, `RootView` no longer routes them through `PlaceholderScreen`
        // — but they stay listed for exhaustiveness, same as every other
        // now-real route above.
        case .projects, .projectDetail, .security, .coverageDetail, .fleet, .usage: 5
        case .library, .agentDefinition, .workflowDefinition: 5
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
