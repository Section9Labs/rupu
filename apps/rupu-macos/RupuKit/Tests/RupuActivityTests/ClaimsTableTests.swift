import Testing
import Foundation
@testable import RupuActivity
import RupuAPI
import RupuDesign

/// Exercises `ClaimTableRow`'s/`ClaimsTable`'s pure static seams directly —
/// a SwiftUI `body` can't be meaningfully unit-rendered, so (same idiom
/// `RunDetailScreenStatusTests` establishes for `RunDetailScreen.
/// unrecognizedStatusRaw`) the row/table's view-member logic that decides
/// WHAT text/state to show is pulled out as `static func`s these tests call
/// directly, with `@testable import RupuActivity` reaching past the
/// `internal` (not `public`) access those seams use.
@Suite
@MainActor
struct ClaimsTableTests {
    /// Every field explicit so a caller only needs to name what it cares
    /// about — mirrors `APIClaimRow`'s own full memberwise `init`.
    private static func claim(
        issueRef: String = "github:Section9Labs/rupu/issues/101",
        issueDisplayRef: String? = "101",
        repoRef: String = "github:Section9Labs/rupu",
        issueTitle: String? = "Some issue",
        issueURL: String? = "https://github.com/Section9Labs/rupu/issues/101",
        workflow: String = "issue-supervisor-dispatch",
        status: String = "await_human",
        lastRunID: String? = "run_1",
        lastError: String? = nil,
        lastSummary: String? = nil,
        prURL: String? = nil,
        claimOwner: String? = nil,
        leaseExpiresAt: String? = nil,
        updatedAt: String = "2026-08-25T12:00:00Z"
    ) -> APIClaimRow {
        APIClaimRow(
            issueRef: issueRef, issueDisplayRef: issueDisplayRef, repoRef: repoRef,
            issueTitle: issueTitle, issueURL: issueURL, workflow: workflow, status: status,
            lastRunID: lastRunID, lastError: lastError, lastSummary: lastSummary, prURL: prURL,
            claimOwner: claimOwner, leaseExpiresAt: leaseExpiresAt, updatedAt: updatedAt
        )
    }

    // MARK: - Nil-heavy row renders `—`s (fixture's third, "eligible" row shape)

    @Test func nilHeavyRowRendersEveryOptionalFieldAsAnEmDash() {
        let row = Self.claim(
            issueDisplayRef: nil, issueURL: nil, lastRunID: nil, lastError: nil,
            lastSummary: nil, prURL: nil, claimOwner: nil, leaseExpiresAt: nil
        )

        #expect(ClaimTableRow.issueLabel(row) == row.issueRef, "no issue_display_ref — falls back to the bare issue_ref")
        #expect(ClaimTableRow.ownerLabel(row) == "—")
        #expect(ClaimTableRow.leaseLabel(row) == "—")
        #expect(ClaimTableRow.noteText(row) == nil, "no last_error AND no last_summary — nothing to show")
    }

    @Test func fullyPopulatedRowRendersEveryField() {
        let row = Self.claim(
            issueDisplayRef: "101", lastError: "boom", lastSummary: "should be shadowed by lastError",
            claimOwner: "host:kuki:5521", leaseExpiresAt: "2026-08-25T13:30:00Z"
        )

        #expect(ClaimTableRow.issueLabel(row) == "101")
        #expect(ClaimTableRow.ownerLabel(row) == "host:kuki:5521")
        #expect(ClaimTableRow.leaseLabel(row) != "—")
        guard let note = ClaimTableRow.noteText(row) else {
            Issue.record("expected a note")
            return
        }
        #expect(note.text == "boom", "last_error takes priority over last_summary")
        #expect(note.isError == true)
    }

    @Test func lastSummaryShowsOnlyWhenThereIsNoLastError() {
        let row = Self.claim(lastError: nil, lastSummary: "in progress")
        guard let note = ClaimTableRow.noteText(row) else {
            Issue.record("expected a note")
            return
        }
        #expect(note.text == "in progress")
        #expect(note.isError == false)
    }

    @Test func updatedLabelFallsBackToEmDashForAnUnparseableTimestamp() {
        let row = Self.claim(updatedAt: "not-a-date")
        #expect(ClaimTableRow.updatedLabel(row) == "—")
    }

    @Test func updatedLabelIsRelativeAndNonEmDashForAValidTimestamp() {
        let row = Self.claim(updatedAt: "2026-08-25T11:00:00Z")
        let now = ISO8601DateFormatter().date(from: "2026-08-25T12:00:00Z")!
        #expect(ClaimTableRow.updatedLabel(row, now: now) != "—")
    }

    @Test func leaseLabelFallsBackToEmDashForAnUnparseableLease() {
        let row = Self.claim(leaseExpiresAt: "garbage")
        #expect(ClaimTableRow.leaseLabel(row) == "—")
    }

    // MARK: - Status label / tone mapping

    @Test func claimStatusLabelTitleCasesTheSnakeCaseVocabulary() {
        #expect(ClaimTableRow.claimStatusLabel("eligible") == "Eligible")
        #expect(ClaimTableRow.claimStatusLabel("await_human") == "Await Human")
        #expect(ClaimTableRow.claimStatusLabel("retry_backoff") == "Retry Backoff")
        #expect(ClaimTableRow.claimStatusLabel("released") == "Released")
    }

    @Test func claimToneMapsTheKnownVocabularyAndFallsBackToPendingForAnUnknownStatus() {
        #expect(ClaimTableRow.claimTone("eligible") == .pending)
        #expect(ClaimTableRow.claimTone("claimed") == .running)
        #expect(ClaimTableRow.claimTone("running") == .running)
        #expect(ClaimTableRow.claimTone("await_human") == .awaiting)
        #expect(ClaimTableRow.claimTone("await_external") == .awaiting)
        #expect(ClaimTableRow.claimTone("retry_backoff") == .paused)
        #expect(ClaimTableRow.claimTone("blocked") == .failed)
        #expect(ClaimTableRow.claimTone("complete") == .done)
        #expect(ClaimTableRow.claimTone("released") == .cancelled)
        #expect(ClaimTableRow.claimTone("some_future_status") == .pending, "an unrecognized status must never guess something alarming")
    }

    // MARK: - Confirm-dialog presence flag

    @Test func releaseDialogIsNotPresentedWhenNoRowIsQueued() {
        #expect(ClaimsTable.isReleaseDialogPresented(nil) == false)
    }

    @Test func releaseDialogIsPresentedOnceARowIsQueued() {
        #expect(ClaimsTable.isReleaseDialogPresented(Self.claim()) == true)
    }
}
