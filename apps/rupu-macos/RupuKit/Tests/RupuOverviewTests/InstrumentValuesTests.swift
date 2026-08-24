import Testing
import RupuAPI
import RupuStore
@testable import RupuOverview

/// `InstrumentValues.compute(from:)` is the pure, testable seam behind
/// `InstrumentStrip` — a plain struct static, not a `View` member, so none
/// of these need `@MainActor` (CI rule: only tests touching a `View`-type
/// member do).

private func makeMerged(
    running: Int = 0,
    awaitingApproval: Int = 0,
    paused: Int = 0,
    pending: Int = 0,
    terminalBuckets: [APITerminalBucket] = [],
    findingsOpen: Int? = nil,
    findingsPartial: Bool = false
) -> MergedDashboard {
    MergedDashboard(
        active: APIActiveCounts(running: running, awaitingApproval: awaitingApproval, paused: paused, pending: pending),
        activeLongest: nil,
        terminalBuckets: terminalBuckets,
        throughputBuckets: [],
        cycles: APICycleCounts(total: 0, clean: nil, withFailures: nil),
        findingsOpen: findingsOpen,
        fleet: APIFleetCounts(
            repos: nil,
            providersConfigured: nil,
            providersUnhealthy: nil,
            autoflowsEnabled: nil,
            autoflowsDisabled: nil,
            workers: nil,
            claimsActive: nil,
            issuesPending: nil,
            issuesOpen: nil,
            issuesCapped: false,
            inventoryCapturedAt: nil
        ),
        findingsPartial: findingsPartial,
        cyclesPartial: false,
        fleetPartial: false,
        capturedAt: nil
    )
}

// MARK: - nil merged

@Test func computeReturnsAllEmDashesWhenMergedIsNil() {
    let values = InstrumentValues.compute(from: nil)
    #expect(values == InstrumentValues(
        awaiting: "—", activeNow: "—", paused: "—",
        failedTotal: "—", successRate: "—", openFindings: "—"
    ))
}

// MARK: - happy path

@Test func computeHappyPathSumsActiveCountsAndTerminalBuckets() {
    let buckets = [
        APITerminalBucket(ts: "2026-08-20T00:00:00Z", completed: 6, failed: 2, rejected: 1, cancelled: 1),
        APITerminalBucket(ts: "2026-08-21T00:00:00Z", completed: 3, failed: 1, rejected: 0, cancelled: 0),
    ]
    let merged = makeMerged(
        running: 3, awaitingApproval: 2, paused: 1,
        terminalBuckets: buckets, findingsOpen: 5, findingsPartial: false
    )
    let values = InstrumentValues.compute(from: merged)

    #expect(values.awaiting == "2")
    #expect(values.activeNow == "3")
    #expect(values.paused == "1")
    #expect(values.failedTotal == "3")
    // terminalTotal = (6+2+1+1) + (3+1+0+0) = 14; completedTotal = 6+3 = 9;
    // round(9/14*100) = round(64.28...) = 64.
    #expect(values.successRate == "64%")
    #expect(values.openFindings == "5")
}

// MARK: - zero denominator

@Test func computeSuccessRateIsEmDashWhenNoBuckets() {
    let merged = makeMerged(terminalBuckets: [])
    #expect(InstrumentValues.compute(from: merged).successRate == "—")
}

@Test func computeSuccessRateIsEmDashWhenBucketsSumToZero() {
    let buckets = [APITerminalBucket(ts: "2026-08-20T00:00:00Z", completed: 0, failed: 0, rejected: 0, cancelled: 0)]
    let merged = makeMerged(terminalBuckets: buckets)
    #expect(InstrumentValues.compute(from: merged).successRate == "—")
}

@Test func computeFailedOnlyBucketYieldsZeroPercentNotEmDash() {
    // `failed` still counts toward the denominator, so an all-failed bucket
    // produces a non-zero denominator and a genuine "0%" — never "—".
    let buckets = [APITerminalBucket(ts: "2026-08-20T00:00:00Z", completed: 0, failed: 4, rejected: 0, cancelled: 0)]
    let merged = makeMerged(terminalBuckets: buckets)
    let values = InstrumentValues.compute(from: merged)
    #expect(values.failedTotal == "4")
    #expect(values.successRate == "0%")
}

// MARK: - partial suffix (open findings)

@Test func computeOpenFindingsSuffixesPlusWhenPartial() {
    let merged = makeMerged(findingsOpen: 7, findingsPartial: true)
    #expect(InstrumentValues.compute(from: merged).openFindings == "7+")
}

@Test func computeOpenFindingsHasNoSuffixWhenNotPartial() {
    let merged = makeMerged(findingsOpen: 4, findingsPartial: false)
    #expect(InstrumentValues.compute(from: merged).openFindings == "4")
}

@Test func computeOpenFindingsIsEmDashWhenNilEvenIfPartialFlagIsSet() {
    // `findingsOpen == nil` means "nobody reported" — must render "—", not
    // a fabricated "0" or "0+", regardless of the partial flag's value.
    let merged = makeMerged(findingsOpen: nil, findingsPartial: true)
    #expect(InstrumentValues.compute(from: merged).openFindings == "—")
}
