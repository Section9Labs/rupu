import Testing
import Foundation
import SwiftUI
@testable import RupuActivity
@testable import RupuStore
import RupuDesign

/// `AutoflowRunsTable`'s pure static seams: `isRunEvent`, `sortValue`
/// (blank-not-dash for non-run-shaped columns on a scheduling event — the
/// brief's explicit rule), and `AutoflowEventBadge`'s label/tone mapping.
private func row(
    id: String = "evt-1", subject: String = "nightly-health", status: ActivityStatus = .running,
    navigation: ActivityRow.Navigation = .run(id: "run-20", host: "local"),
    eventKind: String? = "run_launched", issueRef: String? = nil, worker: String? = nil,
    detail: String? = nil, inputTokens: UInt64? = 5, turns: UInt64? = 2, durationMS: UInt64? = 1_000,
    costUSD: Double? = 0.1
) -> ActivityRow {
    ActivityRow(
        id: id, kind: .autoflow, subject: subject, project: nil, host: "local",
        trigger: nil, status: status, durationMS: durationMS, costUSD: costUSD,
        startedAt: Date(timeIntervalSince1970: 10), navigation: navigation,
        inputTokens: inputTokens, turns: turns, issueRef: issueRef, worker: worker,
        eventKind: eventKind, detail: detail
    )
}

@Suite
@MainActor
struct AutoflowRunsTableTests {
    // MARK: - isRunEvent (web parity: Boolean(e.run_id))

    @Test func isRunEventTrueOnlyWhenNavigationIsRun() {
        #expect(AutoflowRunsTable.isRunEvent(row(navigation: .run(id: "run-20", host: "local"))) == true)
        #expect(AutoflowRunsTable.isRunEvent(row(navigation: .none)) == false)
    }

    // MARK: - Event badge (KindBadge / CycleFailedPill parity)

    @Test func badgeLabelsKnownKindsAndTitleCasesUnknownOnes() {
        #expect(AutoflowEventBadge.label("run_launched") == "launched")
        #expect(AutoflowEventBadge.label("awaiting_human") == "awaiting human")
        #expect(AutoflowEventBadge.label("awaiting_external") == "awaiting external")
        #expect(AutoflowEventBadge.label("cycle_failed") == "failed")
        #expect(AutoflowEventBadge.label("worker_started") == "worker started")
    }

    @Test func cycleFailedBorrowsTheFailedStatusTone() {
        #expect(AutoflowEventBadge.tone("cycle_failed") == Color.status(.failed))
        #expect(AutoflowEventBadge.tone("run_launched") == Color.rupuOk)
        #expect(AutoflowEventBadge.tone("unknown_kind") == Color.rupuMute)
    }

    // MARK: - Sort values: run-shaped columns blank (nil) for a non-run event

    @Test func runShapedColumnsAreNilForANonRunEvent() {
        let scheduling = row(navigation: .none, eventKind: "awaiting_human", worker: "worker-a")
        expectText(AutoflowRunsTable.sortValue(scheduling, .status), nil)
        expectText(AutoflowRunsTable.sortValue(scheduling, .worker), nil)
        expectNumber(AutoflowRunsTable.sortValue(scheduling, .inTokens), nil)
        expectNumber(AutoflowRunsTable.sortValue(scheduling, .cost), nil)
        expectNumber(AutoflowRunsTable.sortValue(scheduling, .turns), nil)
        expectNumber(AutoflowRunsTable.sortValue(scheduling, .duration), nil)
    }

    @Test func runShapedColumnsCarryRealValuesForARunEvent() {
        let launched = row(status: .completed, navigation: .run(id: "run-1", host: "local"), worker: "worker-b", inputTokens: 42, turns: 3, durationMS: 5_000, costUSD: 0.4)
        expectText(AutoflowRunsTable.sortValue(launched, .status), "Completed")
        expectText(AutoflowRunsTable.sortValue(launched, .worker), "worker-b")
        expectNumber(AutoflowRunsTable.sortValue(launched, .inTokens), 42)
        expectNumber(AutoflowRunsTable.sortValue(launched, .turns), 3)
        expectNumber(AutoflowRunsTable.sortValue(launched, .duration), 5_000)
        expectNumber(AutoflowRunsTable.sortValue(launched, .cost), 0.4)
    }

    /// Host/Issue Ref/Event describe the scheduling event itself, not a
    /// launched run — these stay populated regardless of `isRunEvent`.
    @Test func hostIssueRefAndEventAreNeverGatedByIsRunEvent() {
        let scheduling = row(navigation: .none, eventKind: "cycle_failed", issueRef: "#123")
        expectText(AutoflowRunsTable.sortValue(scheduling, .host), "local")
        expectText(AutoflowRunsTable.sortValue(scheduling, .issueRef), "#123")
        expectText(AutoflowRunsTable.sortValue(scheduling, .event), "cycle_failed")
    }
}
