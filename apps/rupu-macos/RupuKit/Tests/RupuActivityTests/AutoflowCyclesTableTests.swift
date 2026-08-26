import Testing
import Foundation
import SwiftUI
@testable import RupuActivity
import RupuAPI
import RupuDesign

/// `AutoflowCyclesTable`'s pure static seams: `sortValue`, `durationMS`,
/// `modeTone`, `usageLabel` (perf & interaction arc, Plan 5 Task 4b — the
/// named remainder from Task 4's report). Same "view-member pure logic gets
/// its own testable static func" idiom every other kind table's tests
/// already establish.
private func cycle(
    cycleID: String = "cycle-1", mode: String = "tick", workerName: String? = "worker-a",
    startedAt: String = "2026-08-20T12:00:00Z", finishedAt: String = "2026-08-20T12:05:00Z",
    ranCycles: Int = 2, skippedCycles: Int = 0, failedCycles: Int = 0, runIDs: [String] = ["run-20"],
    usage: APIUsageSummary = APIUsageSummary(inputTokens: 100, outputTokens: 50, cachedTokens: 0, totalTokens: 150, costUSD: 0.05, priced: true, runs: 1),
    hostID: String? = "local"
) -> APIAutoflowCycleRow {
    APIAutoflowCycleRow(
        cycleID: cycleID, mode: mode, workerName: workerName, startedAt: startedAt, finishedAt: finishedAt,
        workflowCount: 1, ranCycles: ranCycles, skippedCycles: skippedCycles, failedCycles: failedCycles,
        runIDs: runIDs, usage: usage, hostID: hostID
    )
}

@Suite
@MainActor
struct AutoflowCyclesTableTests {
    // MARK: - Duration (finished - started)

    @Test func durationMSComputesWallClockGap() {
        let row = cycle(startedAt: "2026-08-20T12:00:00Z", finishedAt: "2026-08-20T12:00:05Z")
        #expect(AutoflowCyclesTable.durationMS(row) == 5_000)
    }

    @Test func durationMSIsNilWhenFinishedPrecedesStarted() {
        let row = cycle(startedAt: "2026-08-20T12:00:05Z", finishedAt: "2026-08-20T12:00:00Z")
        #expect(AutoflowCyclesTable.durationMS(row) == nil)
    }

    @Test func durationMSIsNilForUnparseableTimestamps() {
        let row = cycle(startedAt: "not-a-date", finishedAt: "also-not-a-date")
        #expect(AutoflowCyclesTable.durationMS(row) == nil)
    }

    // MARK: - Mode tone (web parity: MODE_CLS)

    @Test func modeToneMapsKnownModesAndFallsBackToMuteForUnknown() {
        #expect(AutoflowCyclesTable.modeTone("ask") == Color.rupuWarn)
        #expect(AutoflowCyclesTable.modeTone("bypass") == Color.rupuOk)
        #expect(AutoflowCyclesTable.modeTone("readonly") == Color.rupuMute)
        #expect(AutoflowCyclesTable.modeTone("tick") == Color.rupuMute)
        #expect(AutoflowCyclesTable.modeTone("serve") == Color.rupuInfo)
        #expect(AutoflowCyclesTable.modeTone("something-else") == Color.rupuMute)
    }

    // MARK: - Usage label (web parity: UsageChip)

    @Test func usageLabelFormatsTokensAndCost() {
        let usage = APIUsageSummary(inputTokens: 900, outputTokens: 300, cachedTokens: 0, totalTokens: 1_200, costUSD: 0.12, priced: true, runs: 1)
        #expect(AutoflowCyclesTable.usageLabel(usage) == "1,200 tok · $0.12")
    }

    @Test func usageLabelMarksPartialCostWithAsterisk() {
        let usage = APIUsageSummary(inputTokens: 10, outputTokens: 10, cachedTokens: 0, totalTokens: 20, costUSD: 0.01, priced: false, runs: 1)
        #expect(AutoflowCyclesTable.usageLabel(usage) == "20 tok · $0.01*")
    }

    @Test func usageLabelHasNoAsteriskWhenCostIsNil() {
        let usage = APIUsageSummary(inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0, costUSD: nil, priced: false, runs: 0)
        #expect(AutoflowCyclesTable.usageLabel(usage) == "0 tok · —")
    }

    // MARK: - Sort values

    @Test func sortValueTextColumns() {
        let row = cycle(cycleID: "cycle-9", mode: "serve", workerName: "worker-z", hostID: "mini")
        expectText(AutoflowCyclesTable.sortValue(row, .cycle), "cycle-9")
        expectText(AutoflowCyclesTable.sortValue(row, .mode), "serve")
        expectText(AutoflowCyclesTable.sortValue(row, .worker), "worker-z")
        expectText(AutoflowCyclesTable.sortValue(row, .host), "mini")
    }

    @Test func sortValueHostFallsBackToLocalWhenNil() {
        let row = cycle(hostID: nil)
        expectText(AutoflowCyclesTable.sortValue(row, .host), "local")
    }

    @Test func sortValueWorkerIsNilTextWhenAbsent() {
        let row = cycle(workerName: nil)
        expectText(AutoflowCyclesTable.sortValue(row, .worker), nil)
    }

    @Test func sortValueNumericColumns() {
        let row = cycle(ranCycles: 3, skippedCycles: 1, failedCycles: 2)
        expectNumber(AutoflowCyclesTable.sortValue(row, .ran), 3)
        expectNumber(AutoflowCyclesTable.sortValue(row, .skipped), 1)
        expectNumber(AutoflowCyclesTable.sortValue(row, .failed), 2)
    }

    @Test func sortValueStartedAndDuration() {
        let row = cycle(startedAt: "2026-08-20T12:00:00Z", finishedAt: "2026-08-20T12:00:10Z")
        expectDate(AutoflowCyclesTable.sortValue(row, .started), ISO8601Parsing.parse("2026-08-20T12:00:00Z"))
        expectNumber(AutoflowCyclesTable.sortValue(row, .duration), 10_000)
    }
}
