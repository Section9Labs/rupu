import Foundation

/// `GET /api/host/info` response.
public struct HostInfo: Decodable, Equatable, Sendable {
    public let version: String
    public let capabilities: HostCapabilities

    public init(version: String, capabilities: HostCapabilities) {
        self.version = version
        self.capabilities = capabilities
    }
}

public struct HostCapabilities: Decodable, Equatable, Sendable {
    public let backends: [String]
    public let scmHosts: [String]
    public let permissionModes: [String]

    public init(backends: [String], scmHosts: [String], permissionModes: [String]) {
        self.backends = backends
        self.scmHosts = scmHosts
        self.permissionModes = permissionModes
    }

    private enum CodingKeys: String, CodingKey {
        case backends
        case scmHosts = "scm_hosts"
        case permissionModes = "permission_modes"
    }
}

/// One row from `GET /api/events`: the endpoint injects a stream position
/// (`pos`) and timestamp (`ts`) into the same JSON object as the event
/// payload, so this decodes both from that shared object. The server
/// injects both fields into every row unconditionally, so a row missing
/// either one fails to decode rather than silently defaulting.
public struct CPEventRow: Decodable, Equatable, Sendable {
    public let event: CPEvent
    public let ts: Int64
    public let pos: Int

    private enum CodingKeys: String, CodingKey {
        case ts
        case pos
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.ts = try container.decode(Int64.self, forKey: .ts)
        self.pos = try container.decode(Int.self, forKey: .pos)
        self.event = try CPEvent(from: decoder)
    }
}

/// Configuration for talking to a `rupu cp serve` host.
public struct CPConfig: Sendable {
    public var baseURL: URL
    public var token: String?

    public init(baseURL: URL, token: String? = nil) {
        self.baseURL = baseURL
        self.token = token
    }
}

public enum CPError: Error, Equatable {
    case http(status: Int, body: String)
    case transport(String)
    case decoding(String)
    case unauthorized
    /// The underlying request was cancelled (a `CancellationError` or
    /// `URLError(.cancelled)` from `URLSession`) — never a real transport
    /// failure. Every store's load path checks for this specifically and
    /// leaves its current state untouched rather than surfacing `.failed`.
    case cancelled
}

/// One row from `GET /api/hosts`: the registered fleet (`local` plus every
/// attached Fleet node). Only `id`/`name`/`transportKind`/`status` are
/// decoded — the endpoint returns more fields, ignored here (`Decodable`'s
/// default behavior already skips unknown keys, so no custom `init` is
/// needed for that).
public struct APIHostRow: Decodable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let transportKind: String
    public let status: String
    public let version: String?
    public let activeRunCount: Int?
    public let lastSeenAt: String?

    public init(
        id: String,
        name: String,
        transportKind: String,
        status: String,
        version: String? = nil,
        activeRunCount: Int? = nil,
        lastSeenAt: String? = nil
    ) {
        self.id = id
        self.name = name
        self.transportKind = transportKind
        self.status = status
        self.version = version
        self.activeRunCount = activeRunCount
        self.lastSeenAt = lastSeenAt
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case name
        case transportKind = "transport_kind"
        case status
        case version
        case activeRunCount = "active_run_count"
        case lastSeenAt = "last_seen_at"
    }
}

/// One row from `GET /api/projects` (`ProjectRow` on the Rust side). Only
/// `ws_id`/`name`/`run_count`/`last_run_at`/`usage` are decoded — the
/// endpoint returns more fields (path, repo_remote, branch, repo_home_url,
/// created_at, last_active), ignored here (`Decodable`'s default behavior
/// already skips unknown keys) since the v2 top bar's scope picker only
/// needs an id, a label, and enough to show recent activity. `usage` was
/// added for Phase 5A's Projects list screen (`RupuProjects`), whose
/// sortable "Spend" column needs `usage.cost_usd` — `ProjectRow` on the wire
/// always carries it (non-optional `UsageSummary`, see `crates/rupu-cp/src/
/// api/projects.rs`), so this is a required field, not optional.
public struct APIProjectRow: Decodable, Equatable, Sendable {
    public let wsID: String
    public let name: String
    public let runCount: Int?
    public let lastRunAt: String?
    public let usage: APIUsageSummary

    public init(wsID: String, name: String, runCount: Int?, lastRunAt: String?, usage: APIUsageSummary) {
        self.wsID = wsID
        self.name = name
        self.runCount = runCount
        self.lastRunAt = lastRunAt
        self.usage = usage
    }

    private enum CodingKeys: String, CodingKey {
        case wsID = "ws_id"
        case name
        case runCount = "run_count"
        case lastRunAt = "last_run_at"
        case usage
    }
}

/// `WorkerCapabilities` on the Rust side, nested inside `GET /api/workers`'s
/// rows. Every list is individually omitted from the wire when empty
/// (`skip_serializing_if = "Vec::is_empty"`) — including all three at once,
/// which serializes `capabilities` as `{}` — so this decodes each with a
/// custom `init` rather than a synthesized one, defaulting a missing key to
/// `[]` instead of failing to decode.
public struct APIWorkerCapabilities: Decodable, Equatable, Sendable {
    public let backends: [String]
    public let scmHosts: [String]
    public let permissionModes: [String]

    public init(backends: [String], scmHosts: [String], permissionModes: [String]) {
        self.backends = backends
        self.scmHosts = scmHosts
        self.permissionModes = permissionModes
    }

    private enum CodingKeys: String, CodingKey {
        case backends
        case scmHosts = "scm_hosts"
        case permissionModes = "permission_modes"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        backends = try container.decodeIfPresent([String].self, forKey: .backends) ?? []
        scmHosts = try container.decodeIfPresent([String].self, forKey: .scmHosts) ?? []
        permissionModes = try container.decodeIfPresent([String].self, forKey: .permissionModes) ?? []
    }
}

/// Row from `GET /api/workers` (`WorkerView` on the Rust side): a
/// `WorkerRecord` (`#[serde(flatten)]`ed on the wire) plus run-activity
/// attribution. A worker is a local execution identity, not a per-run
/// object — `activeRunCount`/`totalRunCount`/`lastRunAt` summarize the runs
/// whose `worker_id` matches this worker (`lastRunAt` is `nil` when it has
/// none).
public struct APIWorkerRow: Decodable, Equatable, Sendable {
    public let version: Int
    public let workerID: String
    public let kind: String
    public let name: String
    public let host: String
    public let capabilities: APIWorkerCapabilities
    public let registeredAt: String
    public let lastSeenAt: String
    public let activeRunCount: UInt64
    public let totalRunCount: UInt64
    public let lastRunAt: String?

    public init(
        version: Int,
        workerID: String,
        kind: String,
        name: String,
        host: String,
        capabilities: APIWorkerCapabilities,
        registeredAt: String,
        lastSeenAt: String,
        activeRunCount: UInt64,
        totalRunCount: UInt64,
        lastRunAt: String?
    ) {
        self.version = version
        self.workerID = workerID
        self.kind = kind
        self.name = name
        self.host = host
        self.capabilities = capabilities
        self.registeredAt = registeredAt
        self.lastSeenAt = lastSeenAt
        self.activeRunCount = activeRunCount
        self.totalRunCount = totalRunCount
        self.lastRunAt = lastRunAt
    }

    private enum CodingKeys: String, CodingKey {
        case version
        case workerID = "worker_id"
        case kind
        case name
        case host
        case capabilities
        case registeredAt = "registered_at"
        case lastSeenAt = "last_seen_at"
        case activeRunCount = "active_run_count"
        case totalRunCount = "total_run_count"
        case lastRunAt = "last_run_at"
    }
}

/// The `runs` block of `GET /api/projects/:ws_id` (ad-hoc `json!` on the Rust
/// side, not a named type — see `get_project` in `api/projects.rs`).
/// `byStatus` keys are whatever `RunStatus::as_str()` produces
/// (`"completed"`, `"running"`, …), so a dictionary rather than a fixed set
/// of fields.
public struct APIProjectRunsSummary: Decodable, Equatable, Sendable {
    public let total: Int
    public let running: Int
    public let byStatus: [String: Int]
    public let bySurface: APIProjectRunsBySurface

    public init(total: Int, running: Int, byStatus: [String: Int], bySurface: APIProjectRunsBySurface) {
        self.total = total
        self.running = running
        self.byStatus = byStatus
        self.bySurface = bySurface
    }

    private enum CodingKeys: String, CodingKey {
        case total
        case running
        case byStatus = "by_status"
        case bySurface = "by_surface"
    }
}

/// `runs.by_surface` — a fixed two-key breakdown (`"manual"` triggers count
/// as `workflow`, `"cron"`/`"event"` as `autoflow` — see `get_project`).
public struct APIProjectRunsBySurface: Decodable, Equatable, Sendable {
    public let workflow: Int
    public let autoflow: Int

    public init(workflow: Int, autoflow: Int) {
        self.workflow = workflow
        self.autoflow = autoflow
    }
}

/// The `sessions` block of `GET /api/projects/:ws_id`.
public struct APIProjectSessionsSummary: Decodable, Equatable, Sendable {
    public let total: Int
    public let active: Int

    public init(total: Int, active: Int) {
        self.total = total
        self.active = active
    }
}

/// The `coverage` block of `GET /api/projects/:ws_id` — cheap signals only
/// (target count + findings count); the expensive `assessed_pct` is a
/// separate `GET /api/projects/:ws_id/coverage/assessed` call this phase
/// doesn't cover.
public struct APIProjectCoverageSummary: Decodable, Equatable, Sendable {
    public let targets: Int
    public let findings: Int

    public init(targets: Int, findings: Int) {
        self.targets = targets
        self.findings = findings
    }
}

/// `GET /api/projects/:ws_id` response (`ProjectDetail` on the Rust side).
/// `project` reuses `APIProjectRow` — its partial decode (only `wsID`/
/// `name`/`runCount`/`lastRunAt`) still succeeds here since `ProjectDetail`'s
/// `project` field is the FULL `ProjectRow` (a superset of what
/// `GET /api/projects` returns), and `Decodable` ignores unknown keys.
/// `recentRuns` reuses `APIRunListRow` (`Vec<RunListRow>` on the Rust side,
/// same type `GET /api/runs` decodes — no `host_id` here since this endpoint
/// never proxies to a remote host).
public struct APIProjectDetail: Decodable, Equatable, Sendable {
    public let project: APIProjectRow
    public let runs: APIProjectRunsSummary
    public let sessions: APIProjectSessionsSummary
    public let coverage: APIProjectCoverageSummary
    public let recentRuns: [APIRunListRow]
    public let usage: APIUsageSummary

    public init(
        project: APIProjectRow,
        runs: APIProjectRunsSummary,
        sessions: APIProjectSessionsSummary,
        coverage: APIProjectCoverageSummary,
        recentRuns: [APIRunListRow],
        usage: APIUsageSummary
    ) {
        self.project = project
        self.runs = runs
        self.sessions = sessions
        self.coverage = coverage
        self.recentRuns = recentRuns
        self.usage = usage
    }

    private enum CodingKeys: String, CodingKey {
        case project
        case runs
        case sessions
        case coverage
        case recentRuns = "recent_runs"
        case usage
    }
}
