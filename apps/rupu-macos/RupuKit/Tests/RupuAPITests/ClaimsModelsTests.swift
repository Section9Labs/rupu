import Testing
import Foundation
@testable import RupuAPI

// MARK: - Decode: autoflow_claims.json fixture

@Test func decodesAutoflowClaimsFixtureAcrossAllThreeStatuses() throws {
    let rows = try JSONDecoder().decode([APIClaimRow].self, from: Fixtures.data("autoflow_claims.json"))
    #expect(rows.count == 3)

    // Row 0: await_human — carries last_error and pr_url, no claim_owner/lease.
    let awaitHuman = rows[0]
    #expect(awaitHuman.issueRef == "github:Section9Labs/rupu/issues/101")
    #expect(awaitHuman.issueDisplayRef == "101")
    #expect(awaitHuman.repoRef == "github:Section9Labs/rupu")
    #expect(awaitHuman.issueTitle == "Nightly workflow needs manual review")
    #expect(awaitHuman.issueURL == "https://github.com/Section9Labs/rupu/issues/101")
    #expect(awaitHuman.workflow == "issue-supervisor-dispatch")
    #expect(awaitHuman.status == "await_human")
    #expect(awaitHuman.lastRunID == "run_9k2f")
    #expect(awaitHuman.lastError == "panel review exceeded max_iterations")
    #expect(awaitHuman.lastSummary == "blocked pending human review")
    #expect(awaitHuman.prURL == "https://github.com/Section9Labs/rupu/pull/220")
    #expect(awaitHuman.claimOwner == nil)
    #expect(awaitHuman.leaseExpiresAt == nil)
    #expect(awaitHuman.updatedAt == "2026-08-25T12:00:00Z")

    // Row 1: running — carries claim_owner and lease_expires_at, no pr_url.
    let running = rows[1]
    #expect(running.status == "running")
    #expect(running.lastError == nil)
    #expect(running.prURL == nil)
    #expect(running.claimOwner == "host:kuki:5521")
    #expect(running.leaseExpiresAt == "2026-08-25T13:30:00Z")

    // Row 2: eligible — every optional field is None.
    let eligible = rows[2]
    #expect(eligible.issueRef == "github:Section9Labs/rupu/issues/103")
    #expect(eligible.issueDisplayRef == nil)
    #expect(eligible.issueTitle == nil)
    #expect(eligible.issueURL == nil)
    #expect(eligible.status == "eligible")
    #expect(eligible.lastRunID == nil)
    #expect(eligible.lastError == nil)
    #expect(eligible.lastSummary == nil)
    #expect(eligible.prURL == nil)
    #expect(eligible.claimOwner == nil)
    #expect(eligible.leaseExpiresAt == nil)
    #expect(eligible.updatedAt == "2026-08-25T12:00:00Z")
}

// MARK: - Encode: request bodies

@Test func releaseClaimBodyEncodesIssueRefSnakeCase() throws {
    let data = try JSONEncoder().encode(ReleaseClaimBody(issueRef: "github:Section9Labs/rupu/issues/101"))
    let obj = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    #expect(obj["issue_ref"] as? String == "github:Section9Labs/rupu/issues/101")
}

@Test func requeueClaimBodyEncodesIssueRefSnakeCase() throws {
    let data = try JSONEncoder().encode(RequeueClaimBody(issueRef: "github:Section9Labs/rupu/issues/101"))
    let obj = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    #expect(obj["issue_ref"] as? String == "github:Section9Labs/rupu/issues/101")
}

// MARK: - Decode: response bodies

@Test func decodesReleaseAndRequeueResponses() throws {
    let released = try JSONDecoder().decode(ReleaseClaimResponse.self, from: Data(#"{"released": true}"#.utf8))
    #expect(released.released)

    let requeued = try JSONDecoder().decode(RequeueClaimResponse.self, from: Data(#"{"wake_id": "wk_1"}"#.utf8))
    #expect(requeued.wakeID == "wk_1")
}
