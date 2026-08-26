import Foundation
import Testing
@testable import RupuAPI

/// `ISO8601Parsing` (Plan 5, Task 1 — allocation-storm fixes) replaces four independent
/// fresh-`ISO8601DateFormatter`-per-call sites (`ActivityRow.parseISO`, `UsageAggregation`'s
/// `parseUsageTimestamp`, `UsageStore.rfc3339`, `StreamCards.rfc3339ToMS`) with one shared,
/// `static let`-cached `Date.ISO8601FormatStyle` pair. These tests pin its round-trip behavior
/// against known RFC-3339 strings in both forms (with/without fractional seconds), plus the
/// `nil`/garbage-input cases every call site depends on.

@Test func parsesPlainForm() {
    let date = ISO8601Parsing.parse("2026-08-25T12:34:56Z")
    #expect(date != nil)
    #expect(date.map { Int($0.timeIntervalSince1970) } == 1_787_661_296)
}

@Test func parsesFractionalForm() {
    let date = ISO8601Parsing.parse("2026-08-25T12:34:56.789Z")
    #expect(date != nil)
    // Fractional component preserved to millisecond precision.
    let ms = date.map { ($0.timeIntervalSince1970 * 1000).rounded() }
    #expect(ms == 1_787_661_296_789)
}

@Test func plainAndFractionalFormsOfTheSameInstantAgreeToTheSecond() {
    let plain = ISO8601Parsing.parse("2026-08-25T12:34:56Z")
    let fractional = ISO8601Parsing.parse("2026-08-25T12:34:56.000Z")
    #expect(plain != nil && fractional != nil)
    #expect(plain!.timeIntervalSince1970 == fractional!.timeIntervalSince1970)
}

@Test func nilInputYieldsNil() {
    #expect(ISO8601Parsing.parse(nil) == nil)
}

@Test func garbageInputYieldsNil() {
    #expect(ISO8601Parsing.parse("not a date") == nil)
    #expect(ISO8601Parsing.parse("") == nil)
}

/// `UsageStore.rfc3339` formats via `ISO8601Parsing.fractional` (the same style object, since
/// `Date.ISO8601FormatStyle` is both a `ParseStrategy` and a `FormatStyle`) — round-tripping a
/// formatted string back through `parse` must reproduce the same instant, matching what the old
/// `ISO8601DateFormatter`-based `rfc3339`/parse pair did.
@Test func formatThenParseRoundTrips() {
    let original = Date(timeIntervalSince1970: 1_787_913_296.789)
    let formatted = ISO8601Parsing.fractional.format(original)
    let reparsed = ISO8601Parsing.parse(formatted)
    #expect(reparsed != nil)
    #expect(abs(reparsed!.timeIntervalSince1970 - original.timeIntervalSince1970) < 0.001)
}
