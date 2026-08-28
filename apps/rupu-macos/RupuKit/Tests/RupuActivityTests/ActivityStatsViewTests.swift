import Testing
import Foundation
@testable import RupuActivity
@testable import RupuStore

/// `deriveActivityStats` — the pure derivation behind the Activity parent's
/// (`.all` kind) stats surface (perf & interaction arc, Plan 5 Task 4's
/// restructure: no table, no kind picker, KPI cards instead). Free-standing
/// and testable without `@MainActor`, same rule `deriveNeedsYou` follows.
private func row(
    id: String = "r1", kind: ActivityKindTag = .workflow, status: ActivityStatus = .completed,
    startedAt: Date? = nil
) -> ActivityRow {
    ActivityRow(
        id: id, kind: kind, subject: "s", project: nil, host: "local",
        trigger: nil, status: status, durationMS: nil, costUSD: nil,
        startedAt: startedAt, navigation: .run(id: "run-\(id)", host: nil)
    )
}

private let now = Date(timeIntervalSince1970: 1_700_000_000)
private let calendar = Calendar(identifier: .gregorian)

@Suite
struct ActivityStatsViewTests {
    @Test func countsRowsByKind() {
        let rows = [
            row(id: "1", kind: .agent), row(id: "2", kind: .agent),
            row(id: "3", kind: .workflow), row(id: "4", kind: .autoflow), row(id: "5", kind: .session),
        ]
        let stats = deriveActivityStats(rows: rows, now: now, calendar: calendar)
        #expect(stats.totalLoaded == 5)
        #expect(stats.agentCount == 2)
        #expect(stats.workflowCount == 1)
        #expect(stats.autoflowCount == 1)
        #expect(stats.sessionCount == 1)
    }

    @Test func runningAwaitingFailedTodayOnlyCountRowsStartedToday() {
        let today = now
        let yesterday = calendar.date(byAdding: .day, value: -1, to: now)!
        let rows = [
            row(id: "running-today", status: .running, startedAt: today),
            row(id: "running-yesterday", status: .running, startedAt: yesterday),
            row(id: "awaiting-today", status: .awaiting, startedAt: today),
            row(id: "failed-today", status: .failed, startedAt: today),
            row(id: "failed-yesterday", status: .failed, startedAt: yesterday),
            row(id: "failed-unknown-start", status: .failed, startedAt: nil),
        ]
        let stats = deriveActivityStats(rows: rows, now: now, calendar: calendar)
        #expect(stats.runningToday == 1)
        #expect(stats.awaitingToday == 1)
        #expect(stats.failedToday == 1)
    }

    @Test func oldestAwaitingAgeIsTheLargestGapAmongAwaitingRows() {
        let rows = [
            row(id: "newer", status: .awaiting, startedAt: now.addingTimeInterval(-60)),
            row(id: "older", status: .awaiting, startedAt: now.addingTimeInterval(-3_600)),
            row(id: "not-awaiting", status: .completed, startedAt: now.addingTimeInterval(-100_000)),
        ]
        let stats = deriveActivityStats(rows: rows, now: now, calendar: calendar)
        #expect(stats.oldestAwaitingAge == 3_600)
    }

    @Test func oldestAwaitingAgeIsNilWhenNoAwaitingRowHasAKnownStart() {
        let rows = [row(id: "unknown", status: .awaiting, startedAt: nil)]
        let stats = deriveActivityStats(rows: rows, now: now, calendar: calendar)
        #expect(stats.oldestAwaitingAge == nil)
    }

    @Test func emptyRowsProduceAllZeroStats() {
        let stats = deriveActivityStats(rows: [], now: now, calendar: calendar)
        #expect(stats.totalLoaded == 0)
        #expect(stats.runningToday == 0)
        #expect(stats.awaitingToday == 0)
        #expect(stats.failedToday == 0)
        #expect(stats.oldestAwaitingAge == nil)
    }
}
