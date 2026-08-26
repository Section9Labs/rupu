import Testing
import Foundation
import SwiftUI
@testable import RupuActivity
@testable import RupuStore
import RupuDesign

/// `AgentRunsTable`'s pure static seams — `subtitle`/`sourceTone`/
/// `sortValue` — same "view-member pure logic gets its own testable static
/// func" idiom `ClaimTableRow` already establishes (a SwiftUI `body` can't
/// be meaningfully unit-rendered). See `WorkflowRunsTableTests.swift` for
/// the `expectText`/`expectNumber`/`expectDate` pattern-matching helpers
/// shared by every kind table's sort-value tests (no retroactive
/// `Equatable` on `ListSortValue`).
private func row(
    id: String = "run-1", subject: String = "codegen-agent", trigger: String? = "session_turn",
    status: ActivityStatus = .running, source: String? = "session",
    navigation: ActivityRow.Navigation = .session(id: "sess-abc12345")
) -> ActivityRow {
    ActivityRow(
        id: id, kind: .agent, subject: subject, project: nil, host: "local",
        trigger: trigger, status: status, durationMS: nil, costUSD: nil,
        startedAt: nil, navigation: navigation,
        source: source
    )
}

@Suite
@MainActor
struct AgentRunsTableTests {
    // MARK: - Subject subtitle ("via {trigger} · session {shortId}")

    @Test func subtitleShowsBothTriggerAndSessionWhenBothPresent() {
        let r = row(trigger: "session_turn", navigation: .session(id: "sess-abc12345"))
        #expect(AgentRunsTable.subtitle(r) == "via session_turn · session sess-abc")
    }

    @Test func subtitleShowsOnlyTriggerWhenNoSessionNavigation() {
        let r = row(trigger: "cron", navigation: .agentRun(id: "run-1", transcriptPath: nil, host: nil))
        #expect(AgentRunsTable.subtitle(r) == "via cron")
    }

    @Test func subtitleIsNilWhenNeitherTriggerNorSessionPresent() {
        let r = row(trigger: nil, navigation: .agentRun(id: "run-1", transcriptPath: nil, host: nil))
        #expect(AgentRunsTable.subtitle(r) == nil)
    }

    // MARK: - Source badge tone (web parity: source == "session" -> info, else neutral)

    @Test func sourceToneIsInfoForSessionSourceAndMuteOtherwise() {
        #expect(AgentRunsTable.sourceTone(row(source: "session")) == Color.rupuInfo)
        #expect(AgentRunsTable.sourceTone(row(source: "cli")) == Color.rupuMute)
        #expect(AgentRunsTable.sourceTone(row(source: nil)) == Color.rupuMute)
    }

    // MARK: - Sort values

    @Test func sortValueAgentUsesSubjectAndSourceUsesRawSource() {
        let r = row(subject: "nightly-audit", source: "cron")
        expectText(AgentRunsTable.sortValue(r, .agent), "nightly-audit")
        expectText(AgentRunsTable.sortValue(r, .source), "cron")
    }

    @Test func sortValueStatusUsesDisplayLabel() {
        let r = row(status: .awaiting)
        expectText(AgentRunsTable.sortValue(r, .status), "Awaiting approval")
    }
}
