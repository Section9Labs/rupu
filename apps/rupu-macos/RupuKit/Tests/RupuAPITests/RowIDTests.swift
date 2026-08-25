import Testing
@testable import RupuAPI

/// `APIFinding.rowID`/`APICoverageSummary.rowID` — the composite,
/// content-derived identity added to fix a real Security-screen bug: the
/// Coverage table's `ForEach` originally keyed rows by a per-group
/// positional offset, which silently collapsed most rows across a real,
/// multi-workspace fleet (`targetID` collides across workspaces there —
/// both types' own doc comments already warned this was possible before it
/// actually happened). `rowID` is the pure "uniqueness seam" this incident
/// needs pinned down: the actual `ForEach`/`SwiftUI` rendering isn't
/// unit-testable, but the identity VALUES it's keyed on are.
@Suite struct RowIDTests {
    // MARK: - APICoverageSummary.rowID

    private func coverageRow(wsID: String, targetID: String) -> APICoverageSummary {
        APICoverageSummary(wsID: wsID, project: "proj", targetID: targetID, assertionLines: 1, hasCatalog: false, findings: 0)
    }

    /// The exact scenario the coordinator's diagnosis named: two summaries
    /// sharing a `targetID` under DIFFERENT workspaces must never collide.
    @Test func coverageRowIDDiffersForTheSameTargetIDUnderDifferentWorkspaces() {
        let a = coverageRow(wsID: "ws-1", targetID: "auth-core")
        let b = coverageRow(wsID: "ws-2", targetID: "auth-core")
        #expect(a.rowID != b.rowID)
    }

    /// Genuinely identical rows (same ws AND target) DO produce the same
    /// id — that's correct, not a bug: this is what makes `rowID` a real
    /// identity function rather than something that just always returns
    /// distinct strings regardless of input.
    @Test func coverageRowIDMatchesForTrulyIdenticalWorkspaceAndTarget() {
        let a = coverageRow(wsID: "ws-1", targetID: "auth-core")
        let b = coverageRow(wsID: "ws-1", targetID: "auth-core")
        #expect(a.rowID == b.rowID)
    }

    @Test func coverageRowIDDiffersForDifferentTargetIDsUnderTheSameWorkspace() {
        let a = coverageRow(wsID: "ws-1", targetID: "auth-core")
        let b = coverageRow(wsID: "ws-1", targetID: "web-api")
        #expect(a.rowID != b.rowID)
    }

    // MARK: - APIFinding.rowID

    private func finding(wsID: String, targetID: String, id: String) -> APIFinding {
        APIFinding(
            id: id, summary: "s", severity: "high", scope: "target", filePath: nil, lineRange: nil,
            wsID: wsID, project: "proj", targetID: targetID, workflowName: nil, permalink: nil, rationale: "r",
            declaredAt: "2026-08-20T12:00:00Z"
        )
    }

    @Test func findingRowIDDiffersForTheSameTargetIDUnderDifferentWorkspaces() {
        let a = finding(wsID: "ws-1", targetID: "auth-core", id: "fnd-1")
        let b = finding(wsID: "ws-2", targetID: "auth-core", id: "fnd-1")
        #expect(a.rowID != b.rowID)
    }

    /// `target_id`/`id` alone (the web's own key shape,
    /// `${f.target_id}/${f.id}`) is NOT enough once `target_id` is known to
    /// collide across workspaces — two findings sharing both `targetID` AND
    /// `id` under different workspaces would still collide without the
    /// `wsID` segment `rowID` adds on top.
    @Test func findingRowIDDiffersEvenWhenTargetIDAndFindingIDBothMatchAcrossWorkspaces() {
        let a = finding(wsID: "ws-1", targetID: "auth-core", id: "fnd-1")
        let b = finding(wsID: "ws-2", targetID: "auth-core", id: "fnd-1")
        #expect(a.rowID != b.rowID)
    }

    @Test func findingRowIDMatchesForTrulyIdenticalWorkspaceTargetAndFindingID() {
        let a = finding(wsID: "ws-1", targetID: "auth-core", id: "fnd-1")
        let b = finding(wsID: "ws-1", targetID: "auth-core", id: "fnd-1")
        #expect(a.rowID == b.rowID)
    }

    @Test func findingRowIDDiffersForDifferentFindingIDsUnderTheSameWorkspaceAndTarget() {
        let a = finding(wsID: "ws-1", targetID: "auth-core", id: "fnd-1")
        let b = finding(wsID: "ws-1", targetID: "auth-core", id: "fnd-2")
        #expect(a.rowID != b.rowID)
    }
}
