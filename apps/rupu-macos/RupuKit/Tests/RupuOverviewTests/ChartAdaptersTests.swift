import Testing
import Foundation
import RupuAPI
@testable import RupuOverview

/// `chartRows`/`throughputRows` are the pure, testable seam behind
/// `OutcomesChart`/`ThroughputChart`/`FailedSparkline` — free functions, not
/// members of a `View` type, so none of these need `@MainActor`.

@Test func chartRowsParsesWholeSecondRFC3339Timestamps() throws {
    let buckets = [
        APITerminalBucket(ts: "2026-08-20T00:00:00Z", completed: 3, failed: 1, rejected: 0, cancelled: 2),
    ]
    let rows = chartRows(from: buckets)
    let expected = try #require(ISO8601DateFormatter().date(from: "2026-08-20T00:00:00Z"))
    #expect(rows.allSatisfy { $0.ts == expected })
}

@Test func chartRowsParsesFractionalSecondRFC3339Timestamps() throws {
    // Both shapes are real: server-side chrono emits fractional seconds only
    // when nanos are non-zero (mirrors the DashboardStore.parseTimestamp
    // rationale in RupuStore).
    let buckets = [
        APITerminalBucket(ts: "2026-08-21T00:00:00.500Z", completed: 1, failed: 0, rejected: 1, cancelled: 0),
    ]
    let fractional = ISO8601DateFormatter()
    fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    let expected = try #require(fractional.date(from: "2026-08-21T00:00:00.500Z"))

    let rows = chartRows(from: buckets)
    #expect(rows.count == 4)
    #expect(rows.allSatisfy { $0.ts == expected })
}

@Test func chartRowsStackOrderIsCompletedBottomToCancelledTop() {
    let bucket = APITerminalBucket(ts: "2026-08-20T00:00:00Z", completed: 3, failed: 1, rejected: 5, cancelled: 2)
    let rows = chartRows(from: [bucket])
    #expect(rows.map { $0.series } == ["completed", "failed", "rejected", "cancelled"])
    #expect(rows.map { $0.value } == [3, 1, 5, 2])
}

@Test func chartRowsPreservesBucketOrderAcrossMultipleBuckets() {
    let buckets = [
        APITerminalBucket(ts: "2026-08-20T00:00:00Z", completed: 1, failed: 0, rejected: 0, cancelled: 0),
        APITerminalBucket(ts: "2026-08-21T00:00:00Z", completed: 2, failed: 0, rejected: 0, cancelled: 0),
    ]
    let rows = chartRows(from: buckets)
    #expect(rows.count == 8)
    // First bucket's four series rows precede the second bucket's.
    #expect(rows[0].series == "completed" && rows[0].value == 1)
    #expect(rows[4].series == "completed" && rows[4].value == 2)
}

@Test func chartRowsZeroBucketsPassThroughEmptyNoClientFill() {
    // The server zero-fills buckets; an empty input means an empty range was
    // requested/returned, not a gap the adapter should invent rows for.
    #expect(chartRows(from: []).isEmpty)
}

@Test func chartRowsDropsBucketWithUnparseableTimestampRatherThanGuessing() {
    let bucket = APITerminalBucket(ts: "not-a-timestamp", completed: 1, failed: 2, rejected: 3, cancelled: 4)
    #expect(chartRows(from: [bucket]).isEmpty)
}

@Test func throughputRowsStackOrderIsManualToEvent() {
    let bucket = APIThroughputBucket(ts: "2026-08-20T00:00:00Z", manual: 4, cron: 9, event: 1)
    let rows = throughputRows(from: [bucket])
    #expect(rows.map { $0.series } == ["manual", "cron", "event"])
    #expect(rows.map { $0.value } == [4, 9, 1])
}

@Test func throughputRowsZeroBucketsPassThroughEmpty() {
    #expect(throughputRows(from: []).isEmpty)
}

@Test func throughputRowsDropsBucketWithUnparseableTimestamp() {
    let bucket = APIThroughputBucket(ts: "garbage", manual: 1, cron: 1, event: 1)
    #expect(throughputRows(from: [bucket]).isEmpty)
}
