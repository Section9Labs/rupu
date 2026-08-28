import Testing
@testable import RupuBuilder
import RupuAPI
import RupuStore

@Suite
struct RunOverlayTests {
    // MARK: - Fixtures

    private static func result(stepID: String, success: Bool = true, skipped: Bool = false) -> APIStepResult {
        APIStepResult(
            stepID: stepID, runID: "run-1", transcriptPath: "t/\(stepID)", output: "",
            success: success, skipped: skipped, kind: "step", iterations: 1
        )
    }

    private static func unit(stepID: String, index: Int, success: Bool?) -> APIUnitRow {
        APIUnitRow(
            stepID: stepID, index: index, runID: "run-1", transcriptPath: "t/\(stepID)/\(index)",
            success: success, host: nil, item: nil
        )
    }

    private static func row(id: String, workflowName: String, hostID: String? = nil) -> APIRunListRow {
        APIRunListRow(
            id: id, workflowName: workflowName, status: "running", startedAt: "2026-08-28T00:00:00Z",
            finishedAt: nil, trigger: "manual", usage: usage(), turns: 0, durationMS: nil, hostID: hostID
        )
    }

    private static func usage() -> APIUsageSummary {
        APIUsageSummary(inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0, costUSD: nil, priced: false, runs: 0)
    }

    // MARK: - Priority merge: live beats result beats pending

    @Test func liveStateWinsOverAFinishedResultForTheSameStep() {
        let overlay = runOverlay(
            results: [Self.result(stepID: "a", success: true)],
            units: [],
            liveStates: ["a": .running]
        )

        #expect(overlay.states["a"] == .running, "a live entry must win outright, even over a finished REST result")
    }

    @Test func resultSettlesAStepWithNoLiveEntry() {
        let overlay = runOverlay(
            results: [Self.result(stepID: "a", success: true)],
            units: [],
            liveStates: [:]
        )

        #expect(overlay.states["a"] == .done(success: true))
    }

    @Test func resultCarryingFailureSettlesAsDoneFailure() {
        let overlay = runOverlay(
            results: [Self.result(stepID: "a", success: false)],
            units: [],
            liveStates: [:]
        )

        #expect(overlay.states["a"] == .done(success: false))
    }

    @Test func aStepWithNeitherLiveNorResultHasNoEntryAtAll() {
        let overlay = runOverlay(results: [], units: [], liveStates: [:])

        #expect(overlay.states["never-reached"] == nil, "the caller treats a missing key as .pending, not this function")
    }

    // MARK: - Skipped mapping

    @Test func aSkippedResultMapsToSkippedRegardlessOfItsSuccessFlag() {
        let overlay = runOverlay(
            results: [Self.result(stepID: "a", success: true, skipped: true)],
            units: [],
            liveStates: [:]
        )

        #expect(overlay.states["a"] == .skipped)
    }

    // MARK: - for_each in-flight promotion (mirrors layoutGraph's own rule)

    @Test func aStepWithNoLiveOrResultButAnInFlightUnitPromotesToRunning() {
        let overlay = runOverlay(
            results: [],
            units: [Self.unit(stepID: "fanout", index: 0, success: nil)],
            liveStates: [:]
        )

        #expect(overlay.states["fanout"] == .running)
    }

    @Test func aFinishedForEachIsNotPromotedPastItsOwnResult() {
        // Every unit has finished, and the step's own APIStepResult already
        // settled it .done — the promotion rule must never override an
        // existing result-derived state, only fill in a true gap.
        let overlay = runOverlay(
            results: [Self.result(stepID: "fanout", success: true)],
            units: [Self.unit(stepID: "fanout", index: 0, success: true)],
            liveStates: [:]
        )

        #expect(overlay.states["fanout"] == .done(success: true))
    }

    @Test func aLiveGatePendingStateIsNeverOverriddenByTheForEachPromotion() {
        let overlay = runOverlay(
            results: [],
            units: [Self.unit(stepID: "fanout", index: 0, success: nil)],
            liveStates: ["fanout": .gatePending]
        )

        #expect(overlay.states["fanout"] == .gatePending)
    }

    // MARK: - Unit progress counts

    @Test func unitProgressCountsAUnitAsDoneWheneverSuccessIsNonNil() {
        let overlay = runOverlay(
            results: [],
            units: [
                Self.unit(stepID: "fanout", index: 0, success: true),
                Self.unit(stepID: "fanout", index: 1, success: false),
                Self.unit(stepID: "fanout", index: 2, success: nil),
            ],
            liveStates: [:]
        )

        let progress = overlay.unitProgress["fanout"]
        #expect(progress?.done == 2, "both a passed AND a failed unit count as done — only success == nil is still running")
        #expect(progress?.total == 3)
    }

    @Test func aStepWithNoUnitsHasNoUnitProgressEntry() {
        let overlay = runOverlay(results: [Self.result(stepID: "a")], units: [], liveStates: [:])

        #expect(overlay.unitProgress["a"] == nil)
    }

    // MARK: - RunOverlay Equatable (manual ==, since a tuple map isn't
    // synthesized-Equatable)

    @Test func equalOverlaysWithMatchingUnitProgressCompareEqual() {
        let a = RunOverlay(states: ["x": .running], unitProgress: ["f": (done: 1, total: 2)])
        let b = RunOverlay(states: ["x": .running], unitProgress: ["f": (done: 1, total: 2)])
        let c = RunOverlay(states: ["x": .running], unitProgress: ["f": (done: 2, total: 2)])

        #expect(a == b)
        #expect(a != c)
    }

    // MARK: - latestRunID

    @Test func latestRunIDPicksTheFirstRowMatchingTheWorkflowName() {
        let rows = [
            Self.row(id: "run-other", workflowName: "unrelated"),
            Self.row(id: "run-newest", workflowName: "nightly", hostID: "kuki"),
            Self.row(id: "run-older", workflowName: "nightly"),
        ]

        let picked = latestRunID(rows: rows, workflowName: "nightly")

        #expect(picked?.id == "run-newest")
        #expect(picked?.host == "kuki")
    }

    @Test func latestRunIDIsNilWhenNoRowMatches() {
        let rows = [Self.row(id: "run-other", workflowName: "unrelated")]

        #expect(latestRunID(rows: rows, workflowName: "nightly") == nil)
    }
}
