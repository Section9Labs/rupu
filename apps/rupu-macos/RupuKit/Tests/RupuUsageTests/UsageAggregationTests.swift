import Testing
import Foundation
import RupuAPI
@testable import RupuUsage

// Test tables ported from `crates/rupu-cp/web/src/lib/usage/buildTimeline.test.ts`
// (`buildTimeline`/`aggregateRuns`'s observable behavior) plus new tables for
// `buildSpendTimeline`'s gap-fill, which has no web counterpart (see
// `UsageAggregation.swift`'s file header on why this function is a composite
// port of the web's client-side bucketing PLUS the Rust server's window
// gap-fill).

private func row(
    runID: String = "run_1",
    startedAt: String = "2026-07-01T10:00:00Z",
    workflowName: String = "wf",
    agent: String = "agent_1",
    provider: String = "anthropic",
    model: String = "claude",
    workspaceID: String = "ws_1",
    hostID: String = "local",
    inputTokens: UInt64 = 100,
    outputTokens: UInt64 = 50,
    cachedTokens: UInt64 = 0,
    totalTokens: UInt64 = 150,
    costUSD: Double? = 1,
    priced: Bool = true
) -> APIUsageRunRow {
    APIUsageRunRow(
        runID: runID, startedAt: startedAt, workflowName: workflowName, agent: agent, provider: provider,
        model: model, workspaceID: workspaceID, hostID: hostID, inputTokens: inputTokens,
        outputTokens: outputTokens, cachedTokens: cachedTokens, totalTokens: totalTokens,
        costUSD: costUSD, priced: priced
    )
}

private func iso(_ s: String) -> Date {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime]
    return f.date(from: s)!
}

// MARK: - aggregateRows (port of `aggregateRuns`)

@Test func aggregateRowsSumsAllRowsForAPivotKeyRegardlessOfDay() {
    let rows = [
        row(runID: "run_a", startedAt: "2026-07-01T00:00:00Z", model: "claude", totalTokens: 10, costUSD: 1),
        row(runID: "run_b", startedAt: "2026-07-09T00:00:00Z", model: "claude", totalTokens: 20, costUSD: 2),
    ]
    let out = aggregateRows(rows: rows, pivot: .model)
    #expect(out.count == 1)
    #expect(out[0].model == "claude")
    #expect(out[0].totalTokens == 30)
    #expect(out[0].costUSD == 3)
    #expect(out[0].runs == 2)
}

@Test func aggregateRowsGroupsByTheActivePivotOnlyThatFieldPopulated() {
    let rows = [
        row(runID: "run_a", workflowName: "nightly-scan", costUSD: 1),
        row(runID: "run_b", workflowName: "pr-review", costUSD: 2),
    ]
    let out = aggregateRows(rows: rows, pivot: .workflow)
    #expect(Set(out.map(\.workflow)) == ["nightly-scan", "pr-review"])
    #expect(out.allSatisfy { $0.model.isEmpty })
}

@Test func aggregateRowsNullCostRowContributesZeroToCostSumTokensStillCountPricedFalse() {
    let rows = [
        row(runID: "run_a", model: "claude", totalTokens: 10, costUSD: 1, priced: true),
        row(runID: "run_b", model: "claude", totalTokens: 40, costUSD: nil, priced: false),
    ]
    let out = aggregateRows(rows: rows, pivot: .model)
    #expect(out[0].totalTokens == 50)
    #expect(out[0].costUSD == 1)
    #expect(out[0].priced == false)
}

@Test func aggregateRowsCostIsNilWhenEveryContributingRowIsUnpriced() {
    let rows = [row(runID: "run_a", model: "mystery", costUSD: nil, priced: false)]
    let out = aggregateRows(rows: rows, pivot: .model)
    #expect(out[0].costUSD == nil)
    #expect(out[0].priced == false)
}

@Test func aggregateRowsEmptyInputProducesEmptyOutput() {
    #expect(aggregateRows(rows: [], pivot: .model).isEmpty)
}

@Test func aggregateRowsCountsDistinctRunIDsNotRowCount() {
    // Two rows, same run_id (one run, two models) — `runs` counts the
    // distinct run, not the row count, per `PivotRow.runs`'s doc comment.
    let rows = [
        row(runID: "run_a", model: "claude", totalTokens: 10),
        row(runID: "run_a", model: "claude", totalTokens: 5),
    ]
    let out = aggregateRows(rows: rows, pivot: .model)
    #expect(out[0].runs == 1)
    #expect(out[0].totalTokens == 15)
}

@Test func aggregateRowsHostAndProjectPivotsPopulateOnlyTheirOwnField() {
    let rows = [
        row(runID: "run_a", workspaceID: "ws_a", hostID: "local"),
        row(runID: "run_b", workspaceID: "ws_b", hostID: "host_remote"),
    ]
    let byHost = aggregateRows(rows: rows, pivot: .host)
    #expect(Set(byHost.map(\.hostID)) == ["local", "host_remote"])
    #expect(byHost.allSatisfy { $0.model.isEmpty && $0.workspaceID.isEmpty })

    let byProject = aggregateRows(rows: rows, pivot: .project)
    #expect(Set(byProject.map(\.workspaceID)) == ["ws_a", "ws_b"])
    #expect(byProject.allSatisfy { $0.model.isEmpty && $0.hostID.isEmpty })
}

// MARK: - pivotLabel (null-key label port)

@Test func pivotLabelFallsBackToEmDashForAnEmptyKey() {
    let empty = PivotRow(inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0, costUSD: nil, priced: false, runs: 0)
    #expect(pivotLabel(empty, pivot: .model) == "—")
    #expect(pivotLabel(empty, pivot: .provider) == "—")
    #expect(pivotLabel(empty, pivot: .agent) == "—")
    #expect(pivotLabel(empty, pivot: .workflow) == "—")
    #expect(pivotLabel(empty, pivot: .host) == "—")
    #expect(pivotLabel(empty, pivot: .project) == "—")
}

@Test func pivotLabelReadsTheFieldMatchingThePivot() {
    let out = aggregateRows(rows: [row(runID: "run_a", model: "claude-sonnet")], pivot: .model)
    #expect(pivotLabel(out[0], pivot: .model) == "claude-sonnet")
}

// MARK: - buildSpendTimeline: day bucketing (port of `bucketKeyOf`)

@Test func buildSpendTimelineTwoRunsSameUTCDayLandInOneBucket() {
    let rows = [
        row(runID: "run_a", startedAt: "2026-07-01T10:00:00Z"),
        row(runID: "run_b", startedAt: "2026-07-01T18:00:00Z"),
    ]
    let buckets = buildSpendTimeline(rows: rows, since: iso("2026-07-01T00:00:00Z"), until: iso("2026-07-01T23:59:59Z"))
    #expect(buckets.count == 1)
    #expect(buckets[0].day == "2026-07-01")
    #expect(buckets[0].rows.count == 2)
}

@Test func buildSpendTimelineUTCDayBoundaryNotLocalDate() {
    // 23:30 UTC on 2026-07-01 is still 2026-07-01 in UTC, even though it
    // would be 2026-07-02 in most timezones east of UTC — the web's
    // `bucketKeyOf` reads `getUTCFullYear`/`getUTCMonth`/`getUTCDate`
    // (never local components), and this ports that exactly.
    let rows = [row(runID: "run_a", startedAt: "2026-07-01T23:30:00Z")]
    let buckets = buildSpendTimeline(rows: rows, since: iso("2026-07-01T00:00:00Z"), until: iso("2026-07-01T23:59:59Z"))
    #expect(buckets.map(\.day) == ["2026-07-01"])
}

@Test func buildSpendTimelineJustAfterUTCMidnightIsTheNextDay() {
    let rows = [row(runID: "run_a", startedAt: "2026-07-02T00:00:01Z")]
    let buckets = buildSpendTimeline(rows: rows, since: iso("2026-07-01T00:00:00Z"), until: iso("2026-07-02T23:59:59Z"))
    #expect(buckets.last?.day == "2026-07-02")
    #expect(buckets.last?.rows.count == 1)
}

// MARK: - buildSpendTimeline: gap-fill (port of Rust `build_timeline`)

@Test func buildSpendTimelineGapFillsEmptyDaysBetweenRuns() {
    let rows = [
        row(runID: "run_a", startedAt: "2026-07-01T00:00:00Z"),
        row(runID: "run_b", startedAt: "2026-07-05T00:00:00Z"),
    ]
    let buckets = buildSpendTimeline(rows: rows, since: iso("2026-07-01T00:00:00Z"), until: iso("2026-07-05T23:59:59Z"))
    #expect(buckets.map(\.day) == ["2026-07-01", "2026-07-02", "2026-07-03", "2026-07-04", "2026-07-05"])
    #expect(buckets[1].rows.isEmpty)
    #expect(buckets[2].rows.isEmpty)
    #expect(buckets[3].rows.isEmpty)
}

@Test func buildSpendTimelineFillsThroughUntilEvenPastTheLastRow() {
    // A window that extends past the most recent run still reaches `until`
    // with zero-row buckets — mirrors the Rust source's `fill_end = end`
    // (the requested window's end, not the last row's day).
    let rows = [row(runID: "run_a", startedAt: "2026-07-01T00:00:00Z")]
    let buckets = buildSpendTimeline(rows: rows, since: iso("2026-07-01T00:00:00Z"), until: iso("2026-07-03T00:00:00Z"))
    #expect(buckets.map(\.day) == ["2026-07-01", "2026-07-02", "2026-07-03"])
}

@Test func buildSpendTimelineFillStartClampsUpToTheEarliestRowNeverBeforeSince() {
    // Mirrors `timeline_fill_start`'s `window_start.max(earliest_run)`: an
    // "all time" window (`since` far in the past) fills from the first
    // actual row, not from `since` itself — otherwise an "all" window would
    // gap-fill decades of empty days back to the epoch.
    let rows = [row(runID: "run_a", startedAt: "2026-07-05T00:00:00Z")]
    let buckets = buildSpendTimeline(rows: rows, since: iso("1970-01-01T00:00:00Z"), until: iso("2026-07-05T23:59:59Z"))
    #expect(buckets.count == 1)
    #expect(buckets[0].day == "2026-07-05")
}

@Test func buildSpendTimelineEmptyRowsProducesEmptyOutput() {
    #expect(buildSpendTimeline(rows: [], since: iso("2026-07-01T00:00:00Z"), until: iso("2026-07-05T00:00:00Z")).isEmpty)
}

@Test func buildSpendTimelineDropsAnUnparseableStartedAtRatherThanGuessing() {
    let rows = [
        row(runID: "run_a", startedAt: "not-a-date"),
        row(runID: "run_b", startedAt: "2026-07-01T00:00:00Z"),
    ]
    let buckets = buildSpendTimeline(rows: rows, since: iso("2026-07-01T00:00:00Z"), until: iso("2026-07-01T23:59:59Z"))
    #expect(buckets.count == 1)
    #expect(buckets[0].rows.count == 1)
    #expect(buckets[0].rows[0].runID == "run_b")
}

@Test func buildSpendTimelineAllRowsUnparseableProducesEmptyOutput() {
    let rows = [row(runID: "run_a", startedAt: "garbage")]
    #expect(buildSpendTimeline(rows: rows, since: iso("2026-07-01T00:00:00Z"), until: iso("2026-07-05T00:00:00Z")).isEmpty)
}

// MARK: - buildSpendTimeline: unpriced rows (bucket carries raw rows; cost
// summing itself is `aggregateRows`'s job, exercised together here to prove
// the two compose correctly, per-bucket, the way Task 6's chart will use them)

@Test func buildSpendTimelineBucketRowsFeedAggregateRowsForACorrectPerDayCostTotal() {
    let rows = [
        row(runID: "run_a", startedAt: "2026-07-01T09:00:00Z", model: "claude", totalTokens: 10, costUSD: 1, priced: true),
        row(runID: "run_b", startedAt: "2026-07-01T15:00:00Z", model: "claude", totalTokens: 40, costUSD: nil, priced: false),
    ]
    let buckets = buildSpendTimeline(rows: rows, since: iso("2026-07-01T00:00:00Z"), until: iso("2026-07-01T23:59:59Z"))
    #expect(buckets.count == 1)
    let dayTotal = aggregateRows(rows: buckets[0].rows, pivot: .model)
    #expect(dayTotal.count == 1)
    #expect(dayTotal[0].totalTokens == 50)
    #expect(dayTotal[0].costUSD == 1)
    #expect(dayTotal[0].priced == false)
}
