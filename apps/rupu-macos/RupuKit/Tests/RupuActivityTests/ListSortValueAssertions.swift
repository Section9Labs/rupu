import Testing
import Foundation
import RupuDesign

/// Shared `ListSortValue` pattern-matching assertions for every kind
/// table's `sortValue` tests (`WorkflowRunsTableTests`/`AgentRunsTableTests`/
/// `AutoflowRunsTableTests`/`SessionsTableTests`) — `ListSortValue` has no
/// `Equatable` conformance of its own (`RupuDesign` deliberately keeps it to
/// `Sendable` only), so these pattern-match each case directly rather than
/// adding a retroactive conformance from a test target: `RupuActivityTests`
/// links into the same `RupuKitPackageTests` executable as every other test
/// target, and a second retroactive `Equatable` for the same type from a
/// sibling test target later would collide. Internal (not `private`) so
/// every file in this test target can share one copy.
func expectText(_ value: ListSortValue, _ expected: String?, sourceLocation: SourceLocation = #_sourceLocation) {
    guard case .text(let actual) = value else {
        Issue.record("expected .text, got \(value)", sourceLocation: sourceLocation)
        return
    }
    #expect(actual == expected, sourceLocation: sourceLocation)
}

func expectNumber(_ value: ListSortValue, _ expected: Double?, sourceLocation: SourceLocation = #_sourceLocation) {
    guard case .number(let actual) = value else {
        Issue.record("expected .number, got \(value)", sourceLocation: sourceLocation)
        return
    }
    #expect(actual == expected, sourceLocation: sourceLocation)
}

func expectDate(_ value: ListSortValue, _ expected: Date?, sourceLocation: SourceLocation = #_sourceLocation) {
    guard case .date(let actual) = value else {
        Issue.record("expected .date, got \(value)", sourceLocation: sourceLocation)
        return
    }
    #expect(actual == expected, sourceLocation: sourceLocation)
}
