import Foundation

/// `GET /api/dashboard` response — the ops-first dashboard, fanned out
/// across every registered host. Mirrors `DashboardResponse` on the Rust
/// side (`crates/rupu-cp/src/api/dashboard.rs`), whose `summary` field is
/// `#[serde(flatten)]`ed — so `active`/`activeLongest`/`terminalBuckets`/
/// `throughputBuckets`/`cycles`/`findingsOpen`/`fleet`/`capturedAt` decode
/// from the SAME top-level object as `hosts`/`findingsPartial`/
/// `cyclesPartial`/`fleetPartial`, not a nested key.
///
/// A "*Partial" flag means: at least one host that reported successfully
/// did not include that data (SSH, today, has no findings/cycle-breakdown
/// surface). When true, the paired field is a partial sum across only the
/// hosts that DID report it — never the fleet total. See the Rust doc
/// comments on `DashboardResponse` for the full rule ("not reported ≠ 0").
public struct APIDashboardResponse: Decodable, Equatable, Sendable {
    public let hosts: [APIHostFreshness]
    public let findingsPartial: Bool
    public let cyclesPartial: Bool
    public let fleetPartial: Bool
    public let active: APIActiveCounts
    public let activeLongest: APIActiveLongest?
    public let terminalBuckets: [APITerminalBucket]
    public let throughputBuckets: [APIThroughputBucket]
    public let cycles: APICycleCounts
    public let findingsOpen: Int?
    public let fleet: APIFleetCounts
    public let capturedAt: String?

    public init(
        hosts: [APIHostFreshness],
        findingsPartial: Bool,
        cyclesPartial: Bool,
        fleetPartial: Bool,
        active: APIActiveCounts,
        activeLongest: APIActiveLongest?,
        terminalBuckets: [APITerminalBucket],
        throughputBuckets: [APIThroughputBucket],
        cycles: APICycleCounts,
        findingsOpen: Int?,
        fleet: APIFleetCounts,
        capturedAt: String?
    ) {
        self.hosts = hosts
        self.findingsPartial = findingsPartial
        self.cyclesPartial = cyclesPartial
        self.fleetPartial = fleetPartial
        self.active = active
        self.activeLongest = activeLongest
        self.terminalBuckets = terminalBuckets
        self.throughputBuckets = throughputBuckets
        self.cycles = cycles
        self.findingsOpen = findingsOpen
        self.fleet = fleet
        self.capturedAt = capturedAt
    }

    private enum CodingKeys: String, CodingKey {
        case hosts
        case findingsPartial = "findings_partial"
        case cyclesPartial = "cycles_partial"
        case fleetPartial = "fleet_partial"
        case active
        case activeLongest = "active_longest"
        case terminalBuckets = "terminal_buckets"
        case throughputBuckets = "throughput_buckets"
        case cycles
        case findingsOpen = "findings_open"
        case fleet
        case capturedAt = "captured_at"
    }
}

/// One host's reporting state in the freshness strip (`HostFreshness` on the
/// Rust side). `state` is `"ok"` | `"offline"` | `"unavailable"` — a host
/// that cannot report is NOT a host with no runs, so `state != "ok"` must
/// never be read as zeroed counts. `capturedAt` is present only when `state
/// == "ok"`; `reason` only when it isn't.
public struct APIHostFreshness: Decodable, Equatable, Sendable {
    public let hostID: String
    public let name: String
    public let transportKind: String
    public let state: String
    public let capturedAt: String?
    public let reason: String?

    public init(
        hostID: String,
        name: String,
        transportKind: String,
        state: String,
        capturedAt: String?,
        reason: String?
    ) {
        self.hostID = hostID
        self.name = name
        self.transportKind = transportKind
        self.state = state
        self.capturedAt = capturedAt
        self.reason = reason
    }

    private enum CodingKeys: String, CodingKey {
        case hostID = "host_id"
        case name
        case transportKind = "transport_kind"
        case state
        case capturedAt = "captured_at"
        case reason
    }
}

/// Live, non-terminal run counts — the states that answer "is anything
/// stuck right now" (`ActiveCounts` on the Rust side).
public struct APIActiveCounts: Decodable, Equatable, Sendable {
    public let running: Int
    public let awaitingApproval: Int
    public let paused: Int
    public let pending: Int

    public init(running: Int, awaitingApproval: Int, paused: Int, pending: Int) {
        self.running = running
        self.awaitingApproval = awaitingApproval
        self.paused = paused
        self.pending = pending
    }

    private enum CodingKeys: String, CodingKey {
        case running
        case awaitingApproval = "awaiting_approval"
        case paused
        case pending
    }
}

/// The single longest-running run fleet-wide (`ActiveLongest` on the Rust
/// side) — present only when at least one host has a run active.
public struct APIActiveLongest: Decodable, Equatable, Sendable {
    public let runID: String
    public let workflowName: String
    public let ageMs: Int

    public init(runID: String, workflowName: String, ageMs: Int) {
        self.runID = runID
        self.workflowName = workflowName
        self.ageMs = ageMs
    }

    private enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case workflowName = "workflow_name"
        case ageMs = "age_ms"
    }
}

/// One time bucket of terminal outcomes, for the trend area
/// (`TerminalBucket` on the Rust side). `ts` is zero-filled and
/// midnight-UTC-aligned by the server-side merge.
public struct APITerminalBucket: Decodable, Equatable, Sendable {
    public let ts: String
    public let completed: Int
    public let failed: Int
    public let rejected: Int
    public let cancelled: Int

    public init(ts: String, completed: Int, failed: Int, rejected: Int, cancelled: Int) {
        self.ts = ts
        self.completed = completed
        self.failed = failed
        self.rejected = rejected
        self.cancelled = cancelled
    }
}

/// Runs STARTED in a bucket, split by trigger (`ThroughputBucket` on the
/// Rust side). Same day-key alignment as `APITerminalBucket`.
public struct APIThroughputBucket: Decodable, Equatable, Sendable {
    public let ts: String
    public let manual: Int
    public let cron: Int
    public let event: Int

    public init(ts: String, manual: Int, cron: Int, event: Int) {
        self.ts = ts
        self.manual = manual
        self.cron = cron
        self.event = event
    }
}

/// Scalar cycle summary (`CycleCounts` on the Rust side) — one line of
/// numbers, not a row array. `clean`/`withFailures` are `nil` when the merge
/// is `cyclesPartial` (see `APIDashboardResponse`) — never fabricated as 0.
public struct APICycleCounts: Decodable, Equatable, Sendable {
    public let total: Int
    public let clean: Int?
    public let withFailures: Int?

    public init(total: Int, clean: Int?, withFailures: Int?) {
        self.total = total
        self.clean = clean
        self.withFailures = withFailures
    }

    private enum CodingKeys: String, CodingKey {
        case total
        case clean
        case withFailures = "with_failures"
    }
}

/// The fleet inventory strip (`FleetCounts` on the Rust side). Every count
/// is optional for the same "not reported ≠ 0" reason `findingsOpen` is on
/// `APIDashboardResponse` — `nil` renders as an em-dash, never a zero.
public struct APIFleetCounts: Decodable, Equatable, Sendable {
    public let repos: Int?
    public let providersConfigured: Int?
    public let providersUnhealthy: Int?
    public let autoflowsEnabled: Int?
    public let autoflowsDisabled: Int?
    public let workers: Int?
    public let claimsActive: Int?
    public let issuesPending: Int?
    public let issuesOpen: Int?
    public let issuesCapped: Bool
    public let inventoryCapturedAt: String?

    public init(
        repos: Int?,
        providersConfigured: Int?,
        providersUnhealthy: Int?,
        autoflowsEnabled: Int?,
        autoflowsDisabled: Int?,
        workers: Int?,
        claimsActive: Int?,
        issuesPending: Int?,
        issuesOpen: Int?,
        issuesCapped: Bool,
        inventoryCapturedAt: String?
    ) {
        self.repos = repos
        self.providersConfigured = providersConfigured
        self.providersUnhealthy = providersUnhealthy
        self.autoflowsEnabled = autoflowsEnabled
        self.autoflowsDisabled = autoflowsDisabled
        self.workers = workers
        self.claimsActive = claimsActive
        self.issuesPending = issuesPending
        self.issuesOpen = issuesOpen
        self.issuesCapped = issuesCapped
        self.inventoryCapturedAt = inventoryCapturedAt
    }

    private enum CodingKeys: String, CodingKey {
        case repos
        case providersConfigured = "providers_configured"
        case providersUnhealthy = "providers_unhealthy"
        case autoflowsEnabled = "autoflows_enabled"
        case autoflowsDisabled = "autoflows_disabled"
        case workers
        case claimsActive = "claims_active"
        case issuesPending = "issues_pending"
        case issuesOpen = "issues_open"
        case issuesCapped = "issues_capped"
        case inventoryCapturedAt = "inventory_captured_at"
    }
}
