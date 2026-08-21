import Testing
import Foundation
@testable import RupuStore

/// Routing coverage for `.agentRunDetail` (hotfix root cause C): a
/// standalone agent-run row now pushes this route instead of `.runDetail`
/// (which 404s for it) — these tests pin down the same sidebar-highlight
/// and back-navigation contract `.runDetail`/`.sessionDetail` already have,
/// so the new route doesn't silently fall through to "no selection" or
/// break `navigateBack()`.
@Suite(.serialized)
struct RouteTests {
    @MainActor private func makeModel() -> AppModel {
        AppModel(defaults: UserDefaults(suiteName: "test-\(UUID())")!)
    }

    // (a) `.agentRunDetail` keeps the sidebar highlighting the Runs section,
    // same as `.runDetail`/`.sessionDetail` — never "no selection", never a
    // guessed kind leaf.
    @MainActor @Test func agentRunDetailKeepsSidebarHighlightingRuns() {
        let model = makeModel()
        model.route = .agentRunDetail(id: "run-11", transcriptPath: nil, host: nil)
        #expect(model.selectedSidebarItem == .runs)
    }

    // (b) navigateBack() from a pushed `.agentRunDetail` returns to whichever
    // `.activity(kind)` route was current before the push — the same
    // contract `.runDetail`/`.sessionDetail` already have.
    @MainActor @Test func navigateBackFromAgentRunDetailReturnsToPriorActivityRoute() {
        let model = makeModel()
        model.route = .activity(.agents)
        model.route = .agentRunDetail(id: "run-11", transcriptPath: "t/run-11.jsonl", host: "local")

        model.navigateBack()

        #expect(model.route == .activity(.agents))
    }

    // (c) with no prior Activity visit, navigateBack() from `.agentRunDetail`
    // still lands somewhere sensible (`.activity(.all)`, `lastActivityRoute`'s
    // default) rather than nowhere.
    @MainActor @Test func navigateBackFromAgentRunDetailWithNoPriorActivityVisitDefaultsToAll() {
        let model = makeModel()
        model.route = .agentRunDetail(id: "run-11", transcriptPath: nil, host: nil)

        model.navigateBack()

        #expect(model.route == .activity(.all))
    }

    // (d) `Route` equality distinguishes `.agentRunDetail` instances by all
    // three associated values — a `Hashable`/`Equatable` regression here
    // would silently break SwiftUI's `.task(id:)` re-triggering.
    @MainActor @Test func agentRunDetailRoutesWithDifferentFieldsAreNotEqual() {
        let base = Route.agentRunDetail(id: "run-1", transcriptPath: "t/1.jsonl", host: "local")
        #expect(base == Route.agentRunDetail(id: "run-1", transcriptPath: "t/1.jsonl", host: "local"))
        #expect(base != Route.agentRunDetail(id: "run-2", transcriptPath: "t/1.jsonl", host: "local"))
        #expect(base != Route.agentRunDetail(id: "run-1", transcriptPath: nil, host: "local"))
        #expect(base != Route.agentRunDetail(id: "run-1", transcriptPath: "t/1.jsonl", host: nil))
    }
}
