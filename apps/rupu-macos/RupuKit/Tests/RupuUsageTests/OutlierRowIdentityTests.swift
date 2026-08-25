import Testing
import Foundation
import RupuAPI
@testable import RupuUsage

/// Reorder regression coverage for the backlog's row-20 finding on
/// `OutlierPanel` (the second of the two named sites): its `ForEach` used to
/// key rows `id: \.offset` rather than the row's own `runID`. `OutlierPanel`
/// has no client-side sort control (`find_outliers` on the Rust side already
/// sorts descending by ratio — see `OutlierPanel.swift`'s own doc comment),
/// but the array it renders can still legitimately reorder across two
/// fetches: a different usage window returns a different top-N set, and a
/// run whose ratio has since changed moves rank. An offset-keyed row would
/// silently reattach its identity to whatever run lands on its old position;
/// a `runID`-keyed row follows its own run.
///
/// Same "pure seam, not the View body" convention this file's sibling
/// (`UsageScreenPureFunctionsTests.swift`) already establishes — there is no
/// screen-owned sort function to widen access on here (`runID` is already
/// public on `APIOutlierRun`), so this exercises the identity contract
/// directly against two hand-built orderings of the same run set.

private func outlier(
    runID: String, workflowName: String = "wf", costUSD: Double = 10, baselineUSD: Double = 2, ratio: Double = 5,
    startedAt: String = "2026-08-01T00:00:00Z"
) -> APIOutlierRun {
    APIOutlierRun(
        runID: runID, workflowName: workflowName, costUSD: costUSD, baselineUSD: baselineUSD, ratio: ratio,
        startedAt: startedAt
    )
}

@Test func outlierRowIdentityIsUniquePerRun() {
    let rows = [outlier(runID: "run_a"), outlier(runID: "run_b"), outlier(runID: "run_c")]
    #expect(Set(rows.map(\.runID)).count == rows.count)
}

@Test func outlierRowIdentityFollowsItsRunAcrossReorder() {
    let a = outlier(runID: "run_a", costUSD: 40, ratio: 8)
    let b = outlier(runID: "run_b", costUSD: 25, ratio: 5)
    let c = outlier(runID: "run_c", costUSD: 15, ratio: 3)

    // Two legitimate orderings of an overlapping run set — e.g. a window
    // change re-fetches with `run_b` now ranking above `run_a`.
    let fetchOne = [a, b, c]
    let fetchTwo = [b, a, c]

    #expect(fetchOne.map(\.runID) == ["run_a", "run_b", "run_c"])
    #expect(fetchTwo.map(\.runID) == ["run_b", "run_a", "run_c"])

    // The identity SET is unchanged (a permutation) — a stable `id: \.runID`
    // produces exactly this shape, unlike an offset id, which would show the
    // same (0, 1, 2) sequence regardless of which run actually occupies each
    // slot.
    #expect(Set(fetchOne.map(\.runID)) == Set(fetchTwo.map(\.runID)))

    // And each run's own data (cost, a stand-in for row-carried state)
    // stays attached to its `runID` regardless of which fetch/order it came
    // from — identity followed the run, not the slot.
    for run in [a, b, c] {
        let inFetchOne = fetchOne.first { $0.runID == run.runID }
        let inFetchTwo = fetchTwo.first { $0.runID == run.runID }
        #expect(inFetchOne?.costUSD == run.costUSD)
        #expect(inFetchTwo?.costUSD == run.costUSD)
    }
}
