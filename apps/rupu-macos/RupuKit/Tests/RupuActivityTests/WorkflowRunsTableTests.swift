import Testing
import Foundation
import SwiftUI
@testable import RupuActivity
@testable import RupuStore
import RupuDesign

/// `WorkflowRunsTable.sortValue`/`TriggerTone.color` are the pure,
/// `@testable`-visible seams behind the Workflows kind page's table (perf &
/// interaction arc, Plan 5 Task 4) — a SwiftUI `body` can't be meaningfully
/// unit-rendered, same "view-member pure logic gets its own testable static
/// func" idiom `ClaimTableRow`'s seams already establish.
private func row(
    id: String = "run-1", subject: String = "nightly-health", host: String = "local",
    trigger: String? = "cron", status: ActivityStatus = .completed,
    durationMS: UInt64? = 4_000, costUSD: Double? = 0.5, startedAt: Date? = Date(timeIntervalSince1970: 100),
    inputTokens: UInt64? = 10, outputTokens: UInt64? = 20, cachedTokens: UInt64? = 0, turns: UInt64? = 3
) -> ActivityRow {
    ActivityRow(
        id: id, kind: .workflow, subject: subject, project: nil, host: host,
        trigger: trigger, status: status, durationMS: durationMS, costUSD: costUSD,
        startedAt: startedAt, navigation: .run(id: id, host: nil),
        inputTokens: inputTokens, outputTokens: outputTokens, cachedTokens: cachedTokens, turns: turns
    )
}

@Suite
@MainActor
struct WorkflowRunsTableTests {
    @Test func sortValueStatusUsesDisplayLabel() {
        let r = row(status: .failed)
        expectText(WorkflowRunsTable.sortValue(r, .status), "Failed")
    }

    @Test func sortValueWorkflowUsesSubject() {
        let r = row(subject: "release-train")
        expectText(WorkflowRunsTable.sortValue(r, .workflow), "release-train")
    }

    @Test func sortValueTriggerHostTokensCostTurnsDurationStarted() {
        let r = row(host: "mini", durationMS: 9_000, costUSD: 1.25, inputTokens: 100, outputTokens: 200, cachedTokens: 5, turns: 7)
        expectText(WorkflowRunsTable.sortValue(r, .trigger), "cron")
        expectText(WorkflowRunsTable.sortValue(r, .host), "mini")
        expectNumber(WorkflowRunsTable.sortValue(r, .inTokens), 100)
        expectNumber(WorkflowRunsTable.sortValue(r, .outTokens), 200)
        expectNumber(WorkflowRunsTable.sortValue(r, .cachedTokens), 5)
        expectNumber(WorkflowRunsTable.sortValue(r, .cost), 1.25)
        expectNumber(WorkflowRunsTable.sortValue(r, .turns), 7)
        expectNumber(WorkflowRunsTable.sortValue(r, .duration), 9_000)
    }

    @Test func sortValueStartedUsesDate() {
        let date = Date(timeIntervalSince1970: 500)
        let r = row(startedAt: date)
        expectDate(WorkflowRunsTable.sortValue(r, .started), date)
    }

    @Test func sortValueNilTriggerSortsAsNilText() {
        let r = row(trigger: nil)
        expectText(WorkflowRunsTable.sortValue(r, .trigger), nil)
    }

    // MARK: - Trigger tone (web parity: TRIGGER_CHIP_CLS)

    @Test func triggerToneMapsKnownTriggersAndFallsBackToMuteForUnknown() {
        #expect(TriggerTone.color("cron") == Color.rupuBrand)
        #expect(TriggerTone.color("event") == Color.rupuInfo)
        #expect(TriggerTone.color("manual") == Color.rupuMute)
        #expect(TriggerTone.color("something-else") == Color.rupuMute)
    }
}
