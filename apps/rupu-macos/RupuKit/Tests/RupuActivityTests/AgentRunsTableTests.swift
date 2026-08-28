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

    // MARK: - Find (perf & interaction arc, Plan 5 Task 5) — web-parity fields:
    // [r.agent, r.run_id, r.session_id, r.host_id]

    @Test func matchesAgentNameRunIDAndHost() {
        let r = row(id: "run-xyz", subject: "codegen-agent")
        #expect(AgentRunsTable.matches(r, query: "codegen"))
        #expect(AgentRunsTable.matches(r, query: "run-xyz"))
        #expect(AgentRunsTable.matches(r, query: "local")) // host, always "local" from this row() helper
        #expect(!AgentRunsTable.matches(r, query: "nope"))
    }

    @Test func matchesSessionIDOnlyWhenNavigationIsSession() {
        let sessionRow = row(navigation: .session(id: "sess-abc12345"))
        #expect(AgentRunsTable.matches(sessionRow, query: "abc12345"))

        let standaloneRow = row(navigation: .agentRun(id: "run-1", transcriptPath: nil, host: nil))
        #expect(!AgentRunsTable.matches(standaloneRow, query: "abc12345"))
    }
}
