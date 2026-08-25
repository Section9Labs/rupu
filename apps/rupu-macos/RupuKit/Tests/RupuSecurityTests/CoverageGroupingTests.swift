import Testing
import RupuAPI
import RupuDesign
@testable import RupuSecurity

/// `CoverageTabView.groupedByProject(_:sort:)` — the pure transform that
/// actually feeds the Coverage table's `ForEach` (the render itself isn't
/// unit-testable; this is). Review fix's "uniqueness seam" test: two
/// `APICoverageSummary` rows sharing the SAME `targetID` under DIFFERENT
/// `wsID`s — the exact real-fleet shape that collapsed most of the
/// Coverage table's rows when the `ForEach` was keyed by a per-group
/// positional offset instead of `rowID` — must both survive this transform
/// intact, distinguishable, and neither dropped nor merged.
@MainActor
@Suite struct CoverageGroupingTests {
    private func row(wsID: String, project: String, targetID: String, findings: Int = 0) -> APICoverageSummary {
        APICoverageSummary(wsID: wsID, project: project, targetID: targetID, assertionLines: 1, hasCatalog: false, findings: findings)
    }

    private let sort = ListSort<CoverageSortKey>(key: .target, ascending: true)

    /// Same `targetID`, same `project` display name (two different
    /// workspaces can share a project basename too), different `wsID` —
    /// both rows land in the SAME group and must both be present, keyed
    /// distinctly by `rowID`.
    @Test func sameTargetIDAndProjectNameUnderDifferentWorkspacesBothSurviveInTheSameGroup() {
        let rows = [
            row(wsID: "ws-1", project: "rupu", targetID: "auth-core", findings: 1),
            row(wsID: "ws-2", project: "rupu", targetID: "auth-core", findings: 2),
        ]

        let groups = CoverageTabView.groupedByProject(rows, sort: sort)

        #expect(groups.count == 1)
        let group = groups[0]
        #expect(group.rows.count == 2, "both same-targetID rows from different workspaces must survive, not collapse to one")
        #expect(Set(group.rows.map(\.rowID)).count == 2, "rowID must distinguish them")
        #expect(Set(group.rows.map(\.wsID)) == ["ws-1", "ws-2"])
        // Both rows' own distinguishing data (findings count) survives too —
        // not just a count-matches coincidence.
        #expect(Set(group.rows.map(\.findings)) == [1, 2])
    }

    /// Same `targetID`, DIFFERENT `project` display names (the more common
    /// real-world shape — two distinct workspaces, each with its own
    /// project name, that happen to reuse a target id) — both rows survive,
    /// now in their own separate groups.
    @Test func sameTargetIDUnderDifferentWorkspacesAndProjectNamesBothSurviveInSeparateGroups() {
        let rows = [
            row(wsID: "ws-1", project: "rupu", targetID: "auth-core"),
            row(wsID: "ws-2", project: "phi-cell", targetID: "auth-core"),
        ]

        let groups = CoverageTabView.groupedByProject(rows, sort: sort)

        #expect(groups.count == 2)
        let allRows = groups.flatMap(\.rows)
        #expect(allRows.count == 2)
        #expect(Set(allRows.map(\.rowID)).count == 2)
    }

    /// Sanity check on the grouping itself: a genuinely distinct target
    /// within the same project group is neither dropped nor merged with
    /// its sibling.
    @Test func distinctTargetsInTheSameProjectAllSurvive() {
        let rows = [
            row(wsID: "ws-1", project: "rupu", targetID: "auth-core"),
            row(wsID: "ws-1", project: "rupu", targetID: "web-api"),
            row(wsID: "ws-1", project: "rupu", targetID: "ml-pipeline"),
        ]

        let groups = CoverageTabView.groupedByProject(rows, sort: sort)

        #expect(groups.count == 1)
        #expect(groups[0].rows.count == 3)
        #expect(Set(groups[0].rows.map(\.rowID)).count == 3)
    }
}
