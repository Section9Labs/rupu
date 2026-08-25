import Foundation

/// `GET /api/autoflows/claims` row (`ClaimRow` on the Rust side, `crates/
/// rupu-cp/src/api/autoflow_claims.rs`) — one tracked autoflow issue claim.
/// Plain field-name JSON, no `rename_all` on the Rust struct (its fields are
/// already `snake_case`), so `CodingKeys` here mirror the wire 1:1.
///
/// `status` stays an untyped `String` (the app's existing status-string
/// idiom, matching `APIRunRecord.status` etc.) rather than a typed enum —
/// the Rust `ClaimStatus` serializes lowercase via `serde_json::to_value`
/// round-trip (see `ClaimRow::from`'s doc comment), and the set of values is
/// small and display-driven, not branched on structurally in Swift.
public struct APIClaimRow: Decodable, Equatable, Sendable {
    public let issueRef: String
    public let issueDisplayRef: String?
    public let repoRef: String
    public let issueTitle: String?
    public let issueURL: String?
    public let workflow: String
    public let status: String
    public let lastRunID: String?
    public let lastError: String?
    public let lastSummary: String?
    public let prURL: String?
    public let claimOwner: String?
    public let leaseExpiresAt: String?
    public let updatedAt: String

    public init(
        issueRef: String,
        issueDisplayRef: String?,
        repoRef: String,
        issueTitle: String?,
        issueURL: String?,
        workflow: String,
        status: String,
        lastRunID: String?,
        lastError: String?,
        lastSummary: String?,
        prURL: String?,
        claimOwner: String?,
        leaseExpiresAt: String?,
        updatedAt: String
    ) {
        self.issueRef = issueRef
        self.issueDisplayRef = issueDisplayRef
        self.repoRef = repoRef
        self.issueTitle = issueTitle
        self.issueURL = issueURL
        self.workflow = workflow
        self.status = status
        self.lastRunID = lastRunID
        self.lastError = lastError
        self.lastSummary = lastSummary
        self.prURL = prURL
        self.claimOwner = claimOwner
        self.leaseExpiresAt = leaseExpiresAt
        self.updatedAt = updatedAt
    }

    private enum CodingKeys: String, CodingKey {
        case issueRef = "issue_ref"
        case issueDisplayRef = "issue_display_ref"
        case repoRef = "repo_ref"
        case issueTitle = "issue_title"
        case issueURL = "issue_url"
        case workflow
        case status
        case lastRunID = "last_run_id"
        case lastError = "last_error"
        case lastSummary = "last_summary"
        case prURL = "pr_url"
        case claimOwner = "claim_owner"
        case leaseExpiresAt = "lease_expires_at"
        case updatedAt = "updated_at"
    }
}

// MARK: - Request/response bodies

/// `POST /api/autoflows/claims/release` body (`ReleaseBody` on the Rust
/// side). Issue refs embed `/` and `:` so they travel in the body, not a
/// path segment (see the Rust module doc comment).
struct ReleaseClaimBody: Encodable, Equatable, Sendable {
    let issueRef: String

    private enum CodingKeys: String, CodingKey {
        case issueRef = "issue_ref"
    }
}

/// `POST /api/autoflows/claims/release` response — idempotent, `released:
/// false` for an untracked issue rather than a 404.
struct ReleaseClaimResponse: Decodable, Sendable {
    let released: Bool
}

/// `POST /api/autoflows/claims/requeue` body (`RequeueBody` on the Rust
/// side). `notBefore` is intentionally omitted here — there is no UI surface
/// for deferral, and the server defaults it to "now" when absent; see
/// `CPClient.requeueClaim(issueRef:)`'s doc comment.
struct RequeueClaimBody: Encodable, Equatable, Sendable {
    let issueRef: String

    private enum CodingKeys: String, CodingKey {
        case issueRef = "issue_ref"
    }
}

/// `POST /api/autoflows/claims/requeue` response — `{"wake_id": ...}`. No
/// caller needs the id today; `CPClient.requeueClaim` decodes and discards
/// it, same rationale as `CPClient.put`'s bare-acknowledgement routes.
struct RequeueClaimResponse: Decodable, Sendable {
    let wakeID: String

    private enum CodingKeys: String, CodingKey {
        case wakeID = "wake_id"
    }
}
