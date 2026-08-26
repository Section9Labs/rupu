import Testing
import Foundation
@testable import RupuActivity
@testable import RupuStore

/// `SessionsTable`'s pure static seams: `durationMS` (created→updated) and
/// `updatedAtDescending` (the "NO initial client sort" default — see that
/// type's doc comment for why this table can't just lean on `ActivityStore`'s
/// own presort). The session `ok|error|aborted` → `completed|failed|
/// cancelled` status normalization this table's Status column relies on is
/// already covered by `RupuStoreTests/ActivityRowTests.swift`'s
/// `normalizeMapsAllKnownRawStatuses` — `ActivityRow`'s existing
/// `ActivityStatus.normalize`, unchanged by this task, is what actually does
/// that mapping; this table just renders whatever `row.status` already is.
private func row(
    id: String = "sess-1", subject: String = "rupuso", status: ActivityStatus = .running,
    host: String = "local", model: String? = "claude-sonnet-4-6",
    startedAt: Date? = Date(timeIntervalSince1970: 1_000), updatedAt: Date? = Date(timeIntervalSince1970: 1_090)
) -> ActivityRow {
    ActivityRow(
        id: id, kind: .session, subject: subject, project: nil, host: host,
        trigger: nil, status: status, durationMS: nil, costUSD: nil,
        startedAt: startedAt, navigation: .session(id: id),
        model: model, updatedAt: updatedAt
    )
}

@Suite
@MainActor
struct SessionsTableTests {
    // MARK: - Duration (created -> updated)

    @Test func durationMSComputesFromCreatedToUpdated() {
        let r = row(startedAt: Date(timeIntervalSince1970: 1_000), updatedAt: Date(timeIntervalSince1970: 1_090))
        #expect(SessionsTable.durationMS(r) == 90_000)
    }

    @Test func durationMSIsNilWhenEitherTimestampIsMissing() {
        #expect(SessionsTable.durationMS(row(startedAt: nil)) == nil)
        #expect(SessionsTable.durationMS(row(updatedAt: nil)) == nil)
    }

    @Test func durationMSIsNilWhenUpdatedPrecedesStarted() {
        let r = row(startedAt: Date(timeIntervalSince1970: 1_090), updatedAt: Date(timeIntervalSince1970: 1_000))
        #expect(SessionsTable.durationMS(r) == nil)
    }

    // MARK: - Default order: updated_at descending, nulls last, stable

    @Test func updatedAtDescendingOrdersNewestFirst() {
        let older = row(id: "a", updatedAt: Date(timeIntervalSince1970: 100))
        let newer = row(id: "b", updatedAt: Date(timeIntervalSince1970: 200))
        let sorted = SessionsTable.updatedAtDescending([older, newer])
        #expect(sorted.map(\.id) == ["b", "a"])
    }

    @Test func updatedAtDescendingSortsNilLastRegardless() {
        let unknown = row(id: "unknown", updatedAt: nil)
        let known = row(id: "known", updatedAt: Date(timeIntervalSince1970: 100))
        let sorted = SessionsTable.updatedAtDescending([unknown, known])
        #expect(sorted.map(\.id) == ["known", "unknown"])
    }

    @Test func updatedAtDescendingIsStableForTies() {
        let a = row(id: "a", updatedAt: Date(timeIntervalSince1970: 100))
        let b = row(id: "b", updatedAt: Date(timeIntervalSince1970: 100))
        let sorted = SessionsTable.updatedAtDescending([a, b])
        #expect(sorted.map(\.id) == ["a", "b"])
    }

    // MARK: - Sort values

    @Test func sortValueModelAndAgentUseTheirOwnFields() {
        let r = row(subject: "rupuso", model: "gpt-5")
        expectText(SessionsTable.sortValue(r, .agent), "rupuso")
        expectText(SessionsTable.sortValue(r, .model), "gpt-5")
    }

    @Test func sortValueDurationUsesCreatedToUpdated() {
        let r = row(startedAt: Date(timeIntervalSince1970: 0), updatedAt: Date(timeIntervalSince1970: 5))
        expectNumber(SessionsTable.sortValue(r, .duration), 5_000)
    }
}
