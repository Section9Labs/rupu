import Testing
import Foundation
@testable import RupuOverview
@testable import RupuStore

/// `deriveNeedsYou(rows:range:now:)` is the pure, testable seam behind
/// `NeedsYouCard` — a free function, not a `View`-type member, so none of
/// these need `@MainActor` (CI rule: only tests touching a `View`-type
/// member do).
///
/// Row fixtures are built directly via `ActivityRow`'s module-internal
/// memberwise init (visible here through `@testable import RupuStore`) —
/// same fixture pattern `RupuStoreTests/ActivitySortTests.swift` already
/// uses for the same type.
private func row(
    id: String, subject: String = "s", project: String? = nil, host: String = "local",
    status: ActivityStatus = .completed, startedAt: Date? = nil,
    navigation: ActivityRow.Navigation? = nil
) -> ActivityRow {
    ActivityRow(
        id: id, kind: .workflow, subject: subject, project: project, host: host,
        trigger: nil, status: status, durationMS: nil, costUSD: nil,
        startedAt: startedAt, navigation: navigation ?? .run(id: "run-\(id)", host: nil)
    )
}

private let now = Date(timeIntervalSince1970: 1_700_000_000)
private let oneDay: TimeInterval = 86_400

// MARK: - Ordering across kinds

@Test func gatesSortOldestFirstThenFailedRunsSortNewestFirst() {
    let rows = [
        row(id: "gate-newer", status: .awaiting, startedAt: now.addingTimeInterval(-60)),
        row(id: "gate-older", status: .awaiting, startedAt: now.addingTimeInterval(-3_600)),
        row(id: "failed-older", status: .failed, startedAt: now.addingTimeInterval(-2 * oneDay)),
        row(id: "failed-newer", status: .failed, startedAt: now.addingTimeInterval(-oneDay)),
    ]
    let result = deriveNeedsYou(rows: rows, range: .d7, now: now)
    #expect(result.items.map(\.row.id) == ["gate-older", "gate-newer", "failed-newer", "failed-older"])
    #expect(result.items.map(\.kind) == [.gate, .gate, .failedRun, .failedRun])
    #expect(result.overflow == 0)
}

@Test func gateRowsWithUnknownStartedAtSortLastAmongGates() {
    let rows = [
        row(id: "gate-unknown", status: .awaiting, startedAt: nil),
        row(id: "gate-known", status: .awaiting, startedAt: now.addingTimeInterval(-60)),
    ]
    let result = deriveNeedsYou(rows: rows, range: .d7, now: now)
    #expect(result.items.map(\.row.id) == ["gate-known", "gate-unknown"])
}

@Test func nonAwaitingNonFailedRowsAreExcludedEntirely() {
    let rows = [
        row(id: "running", status: .running, startedAt: now),
        row(id: "completed", status: .completed, startedAt: now),
        row(id: "paused", status: .paused, startedAt: now),
        row(id: "gate", status: .awaiting, startedAt: now),
    ]
    let result = deriveNeedsYou(rows: rows, range: .d7, now: now)
    #expect(result.items.map(\.row.id) == ["gate"])
}

// MARK: - Range window

@Test func failedRowOutsideD7WindowIsExcluded() {
    let rows = [
        row(id: "in-window", status: .failed, startedAt: now.addingTimeInterval(-6 * oneDay)),
        row(id: "out-of-window", status: .failed, startedAt: now.addingTimeInterval(-8 * oneDay)),
    ]
    let result = deriveNeedsYou(rows: rows, range: .d7, now: now)
    #expect(result.items.map(\.row.id) == ["in-window"])
}

@Test func failedRowOutsideD30WindowIsExcluded() {
    let rows = [
        row(id: "in-window", status: .failed, startedAt: now.addingTimeInterval(-29 * oneDay)),
        row(id: "out-of-window", status: .failed, startedAt: now.addingTimeInterval(-31 * oneDay)),
    ]
    let result = deriveNeedsYou(rows: rows, range: .d30, now: now)
    #expect(result.items.map(\.row.id) == ["in-window"])
}

@Test func allRangeAppliesNoWindowAtAllIncludingVeryOldFailures() {
    let rows = [
        row(id: "ancient", status: .failed, startedAt: now.addingTimeInterval(-365 * oneDay)),
    ]
    let result = deriveNeedsYou(rows: rows, range: .all, now: now)
    #expect(result.items.map(\.row.id) == ["ancient"])
}

@Test func failedRowWithNilStartedAtFailsTheWindowForD7AndD30() {
    let rows = [row(id: "no-date", status: .failed, startedAt: nil)]
    #expect(deriveNeedsYou(rows: rows, range: .d7, now: now).items.isEmpty)
    #expect(deriveNeedsYou(rows: rows, range: .d30, now: now).items.isEmpty)
}

@Test func failedRowWithNilStartedAtPassesUnderAllRange() {
    let rows = [row(id: "no-date", status: .failed, startedAt: nil)]
    let result = deriveNeedsYou(rows: rows, range: .all, now: now)
    #expect(result.items.map(\.row.id) == ["no-date"])
}

@Test func failedRowInTheFutureIsExcludedFromAD7Window() {
    // Clock-skew safety: a `startedAt` after `now` isn't honestly "within
    // the last N days" — it must not pass the window just because the raw
    // interval magnitude happens to be small.
    let rows = [row(id: "future", status: .failed, startedAt: now.addingTimeInterval(3_600))]
    #expect(deriveNeedsYou(rows: rows, range: .d7, now: now).items.isEmpty)
}

@Test func failedRowExactlyAtTheD7WindowBoundaryIsIncluded() {
    // Locks the documented inclusive `<=`: `now - 7d` is exactly on the
    // boundary `fallsInsideRange` compares against, not one tick inside or
    // outside it.
    let rows = [row(id: "on-boundary", status: .failed, startedAt: now.addingTimeInterval(-7 * oneDay))]
    let result = deriveNeedsYou(rows: rows, range: .d7, now: now)
    #expect(result.items.map(\.row.id) == ["on-boundary"])
}

// MARK: - Cap + overflow

@Test func capsAtSixItemsAndReportsOverflow() {
    let gates = (0..<4).map { i in
        row(id: "gate-\(i)", status: .awaiting, startedAt: now.addingTimeInterval(-Double(i) * 60))
    }
    let failed = (0..<4).map { i in
        row(id: "failed-\(i)", status: .failed, startedAt: now.addingTimeInterval(-Double(i) * oneDay))
    }
    let result = deriveNeedsYou(rows: gates + failed, range: .d7, now: now)
    #expect(result.items.count == 6)
    #expect(result.overflow == 2)
    // Gates fill first (all 4, oldest-first — `gate-3`'s `-3*60s` offset is
    // the earliest), then the two newest failed rows (`failed-0`'s `-0*1d`
    // offset is the most recent).
    #expect(result.items.map(\.row.id) == ["gate-3", "gate-2", "gate-1", "gate-0", "failed-0", "failed-1"])
}

@Test func sevenGatesAloneFillTheCapAndExcludeEveryFailedRow() {
    // Gates alone can exceed the cap: the failed side must be shut out
    // entirely (not partially squeezed in), and `overflow` counts the
    // excluded gate plus the entirely-excluded failed row.
    let gates = (0..<7).map { i in
        row(id: "gate-\(i)", status: .awaiting, startedAt: now.addingTimeInterval(-Double(i) * 60))
    }
    let failed = [row(id: "failed-0", status: .failed, startedAt: now)]
    let result = deriveNeedsYou(rows: gates + failed, range: .d7, now: now)
    #expect(result.items.count == 6)
    #expect(result.items.allSatisfy { $0.kind == .gate })
    #expect(!result.items.map(\.row.id).contains("failed-0"))
    #expect(result.overflow == 2)
}

@Test func overflowIsZeroFlooredWhenTotalIsUnderTheCap() {
    let rows = [row(id: "gate-1", status: .awaiting, startedAt: now)]
    let result = deriveNeedsYou(rows: rows, range: .d7, now: now)
    #expect(result.overflow == 0)
}

@Test func overflowIsZeroExactlyAtTheCap() {
    let rows = (0..<6).map { i in row(id: "gate-\(i)", status: .awaiting, startedAt: now.addingTimeInterval(-Double(i))) }
    let result = deriveNeedsYou(rows: rows, range: .d7, now: now)
    #expect(result.items.count == 6)
    #expect(result.overflow == 0)
}

// MARK: - Empty

@Test func emptyRowsProduceEmptyItemsAndZeroOverflow() {
    let result = deriveNeedsYou(rows: [], range: .d7, now: now)
    #expect(result.items.isEmpty)
    #expect(result.overflow == 0)
}

@Test func rowsWithNoAwaitingOrFailedProduceEmptyItems() {
    let rows = [row(id: "a", status: .running), row(id: "b", status: .completed)]
    let result = deriveNeedsYou(rows: rows, range: .d7, now: now)
    #expect(result.items.isEmpty)
    #expect(result.overflow == 0)
}

// MARK: - NeedsYouItem identity

@Test func itemIDsAreUniqueEvenIfARowIDCollidesAcrossFeeds() {
    // Distinguishing by kind protects `Identifiable`/`ForEach` even though
    // a row can never actually be both `.awaiting` and `.failed` at once —
    // belt-and-suspenders, not a scenario this store produces today.
    let gate = NeedsYouItem(kind: .gate, row: row(id: "x", status: .awaiting))
    let failed = NeedsYouItem(kind: .failedRun, row: row(id: "x", status: .failed))
    #expect(gate.id != failed.id)
}
