import Testing
import Foundation
import RupuAPI
import RupuDesign
@testable import RupuLibrary

/// Reorder-under-sort regression coverage for the backlog's row-20 finding:
/// `LibraryScreen`'s three definition tables used to key their `ForEach`
/// rows `id: \.offset` (an array-position id) rather than a stable,
/// data-derived one. That's fine only until a header-click sort reorders the
/// array under the rows — at that point row identity silently churns (state/
/// animation/selection tied to a row follow the OFFSET, not the definition
/// that used to be there).
///
/// This codebase's established convention (see `RupuUsageTests/
/// UsageScreenPureFunctionsTests.swift`'s file-header comment) is to test a
/// screen's pure seams, never the `View` body itself — so these tests
/// exercise exactly the two pieces the fix touches: `AgentDefinition.
/// rowIdentity`/`WorkflowDefinition.rowIdentity`/`AutoflowDefinition.
/// rowIdentity` (`RupuAPI/DefinitionModels.swift`) composed with
/// `sortRows`/`LibraryScreen.agentSortValue`/`workflowSortValue`/
/// `autoflowSortValue` (widened from `private` to internal for exactly this
/// `@testable import`) — the same two ingredients `LibraryScreen`'s three
/// tables actually feed into their `ForEach(sorted, id: \.rowIdentity)`.

// MARK: - Fixtures

private func agent(
    name: String, slug: String? = nil, scopeKind: String = "global", scopeID: String? = nil, runCount: Int = 0
) -> AgentDefinition {
    AgentDefinition(
        name: name, slug: slug ?? name, description: nil, provider: nil, model: nil, effort: nil, maxTokens: nil,
        mode: nil, tools: [], scope: scopeKind, scopeKind: scopeKind, scopeID: scopeID, runCount: runCount, lastRun: nil
    )
}

private func workflow(
    name: String, scopeKind: String = "global", scopeID: String? = nil, runCount: Int = 0
) -> WorkflowDefinition {
    WorkflowDefinition(
        name: name, scope: scopeKind, scopeKind: scopeKind, scopeID: scopeID, runCount: runCount, lastRun: nil,
        autoflowEnabled: nil
    )
}

private func autoflow(
    name: String, slug: String? = nil, trigger: String = "cron", scopeKind: String = "global", scopeID: String? = nil,
    enabled: Bool = true
) -> AutoflowDefinition {
    AutoflowDefinition(
        name: name, slug: slug ?? name, trigger: trigger, scope: scopeKind, scopeKind: scopeKind, scopeID: scopeID,
        enabled: enabled
    )
}

// MARK: - rowIdentity: composite, not name-alone

// The exact ambiguity `list_agents`'s Rust-side doc comment calls out: a
// project-scoped def and a same-named GLOBAL def both appear in one flat
// list. A bare `id: \.name` (or `\.offset`) can't distinguish them —
// `rowIdentity` must.

@Test func agentRowIdentityDistinguishesSameNameAcrossScope() {
    let globalReviewer = agent(name: "reviewer", scopeKind: "global", scopeID: nil)
    let projectReviewer = agent(name: "reviewer", scopeKind: "project", scopeID: "ws_1")
    #expect(globalReviewer.rowIdentity != projectReviewer.rowIdentity)
}

@Test func workflowRowIdentityDistinguishesSameNameAcrossScope() {
    let globalFlow = workflow(name: "dispatch-demo", scopeKind: "global", scopeID: nil)
    let projectFlow = workflow(name: "dispatch-demo", scopeKind: "project", scopeID: "ws_2")
    #expect(globalFlow.rowIdentity != projectFlow.rowIdentity)
}

@Test func autoflowRowIdentityDistinguishesSameNameAcrossScope() {
    let a = autoflow(name: "nightly", scopeKind: "project", scopeID: "ws_a")
    let b = autoflow(name: "nightly", scopeKind: "project", scopeID: "ws_b")
    #expect(a.rowIdentity != b.rowIdentity)
}

@Test func agentRowIdentityAgreesForTheSameRowRebuiltTwice() {
    // Same definition, two independently-constructed values (e.g. two
    // fetches of the same row) — identity must agree, or a `ForEach` would
    // treat every refetch as a brand new row.
    let a = agent(name: "reviewer", scopeKind: "project", scopeID: "ws_1")
    let b = agent(name: "reviewer", scopeKind: "project", scopeID: "ws_1")
    #expect(a.rowIdentity == b.rowIdentity)
}

// MARK: - Reorder-under-sort: identity follows the DATA, not the position

// `@MainActor` — `LibraryScreen.agentSortValue`/`workflowSortValue`/
// `autoflowSortValue` are static members of `LibraryScreen`, a `View`
// (implicitly `@MainActor`-isolated), so calling them requires this context
// even though the functions themselves touch no UI state.
@Test @MainActor func agentTableSortReorderKeepsIdentityWithItsRow() {
    let rows = [
        agent(name: "zeta", scopeKind: "global", runCount: 1),
        agent(name: "alpha", scopeKind: "global", runCount: 5),
        agent(name: "mid", scopeKind: "project", scopeID: "ws_1", runCount: 3),
    ]

    let ascending = sortRows(rows, sort: ListSort(key: AgentsSortKey.name, ascending: true), value: LibraryScreen.agentSortValue)
    let descending = sortRows(rows, sort: ListSort(key: AgentsSortKey.name, ascending: false), value: LibraryScreen.agentSortValue)

    // The sort actually reordered the array...
    #expect(ascending.map(\.name) == ["alpha", "mid", "zeta"])
    #expect(descending.map(\.name) == ["zeta", "mid", "alpha"])
    #expect(ascending.map(\.rowIdentity) != descending.map(\.rowIdentity))

    // ...but the `ForEach` identity SET is unchanged (a permutation) between
    // the two orders — a stable `id:` produces exactly this shape. An
    // offset-keyed `ForEach` would instead have the SAME id sequence
    // (0, 1, 2) both times, silently reattaching row 0's identity to
    // whichever definition happens to sort first.
    #expect(Set(ascending.map(\.rowIdentity)) == Set(descending.map(\.rowIdentity)))
    #expect(ascending.map(\.rowIdentity) == descending.map(\.rowIdentity).reversed())

    // And each identity still resolves to the SAME underlying definition
    // (by its own `runCount`, a stand-in for row-carried state) in both
    // orders — identity followed the row, not the slot.
    for def in rows {
        let inAscending = ascending.first { $0.rowIdentity == def.rowIdentity }
        let inDescending = descending.first { $0.rowIdentity == def.rowIdentity }
        #expect(inAscending?.runCount == def.runCount)
        #expect(inDescending?.runCount == def.runCount)
    }
}

// `@MainActor` — see `agentTableSortReorderKeepsIdentityWithItsRow`'s
// identical comment above.
@Test @MainActor func workflowTableSortReorderKeepsIdentityWithItsRow() {
    let rows = [
        workflow(name: "zeta", runCount: 1),
        workflow(name: "alpha", runCount: 9),
        workflow(name: "mid", scopeKind: "project", scopeID: "ws_1", runCount: 4),
    ]

    let ascending = sortRows(rows, sort: ListSort(key: WorkflowsSortKey.name, ascending: true), value: LibraryScreen.workflowSortValue)
    let descending = sortRows(rows, sort: ListSort(key: WorkflowsSortKey.name, ascending: false), value: LibraryScreen.workflowSortValue)

    #expect(ascending.map(\.name) == ["alpha", "mid", "zeta"])
    #expect(descending.map(\.name) == ["zeta", "mid", "alpha"])
    #expect(Set(ascending.map(\.rowIdentity)) == Set(descending.map(\.rowIdentity)))
    #expect(ascending.map(\.rowIdentity) == descending.map(\.rowIdentity).reversed())

    for def in rows {
        let inAscending = ascending.first { $0.rowIdentity == def.rowIdentity }
        let inDescending = descending.first { $0.rowIdentity == def.rowIdentity }
        #expect(inAscending?.runCount == def.runCount)
        #expect(inDescending?.runCount == def.runCount)
    }
}

// `@MainActor` — see `agentTableSortReorderKeepsIdentityWithItsRow`'s
// identical comment above.
@Test @MainActor func autoflowTableSortReorderKeepsIdentityWithItsRow() {
    let rows = [
        autoflow(name: "zeta", trigger: "cron", enabled: true),
        autoflow(name: "alpha", trigger: "event", enabled: false),
        autoflow(name: "mid", trigger: "cron", scopeKind: "project", scopeID: "ws_1", enabled: true),
    ]

    let ascending = sortRows(rows, sort: ListSort(key: AutoflowsSortKey.name, ascending: true), value: LibraryScreen.autoflowSortValue)
    let descending = sortRows(rows, sort: ListSort(key: AutoflowsSortKey.name, ascending: false), value: LibraryScreen.autoflowSortValue)

    #expect(ascending.map(\.name) == ["alpha", "mid", "zeta"])
    #expect(descending.map(\.name) == ["zeta", "mid", "alpha"])
    #expect(Set(ascending.map(\.rowIdentity)) == Set(descending.map(\.rowIdentity)))
    #expect(ascending.map(\.rowIdentity) == descending.map(\.rowIdentity).reversed())

    for def in rows {
        let inAscending = ascending.first { $0.rowIdentity == def.rowIdentity }
        let inDescending = descending.first { $0.rowIdentity == def.rowIdentity }
        #expect(inAscending?.trigger == def.trigger)
        #expect(inDescending?.trigger == def.trigger)
    }
}
