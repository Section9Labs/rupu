import Foundation

/// One `(provider × model × agent)` usage bucket, optionally grouped further
/// by workflow/host/project (`UsageBreakdownRow` on the Rust side, `crates/
/// rupu-cp/src/usage.rs`). `workflow`/`hostID`/`workspaceID` are
/// `#[serde(default)]` on the Rust side — an empty string, not an absent
/// key, when the response isn't grouped by that dimension — so they decode
/// as plain `String` here too, never `Optional`.
public struct APIUsageBreakdownRow: Decodable, Equatable, Sendable {
    public let provider: String
    public let model: String
    public let agent: String
    public let workflow: String
    public let hostID: String
    public let workspaceID: String
    public let inputTokens: UInt64
    public let outputTokens: UInt64
    public let cachedTokens: UInt64
    public let totalTokens: UInt64
    public let costUSD: Double?
    public let priced: Bool
    public let runs: UInt64

    public init(
        provider: String,
        model: String,
        agent: String,
        workflow: String,
        hostID: String,
        workspaceID: String,
        inputTokens: UInt64,
        outputTokens: UInt64,
        cachedTokens: UInt64,
        totalTokens: UInt64,
        costUSD: Double?,
        priced: Bool,
        runs: UInt64
    ) {
        self.provider = provider
        self.model = model
        self.agent = agent
        self.workflow = workflow
        self.hostID = hostID
        self.workspaceID = workspaceID
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cachedTokens = cachedTokens
        self.totalTokens = totalTokens
        self.costUSD = costUSD
        self.priced = priced
        self.runs = runs
    }

    private enum CodingKeys: String, CodingKey {
        case provider
        case model
        case agent
        case workflow
        case hostID = "host_id"
        case workspaceID = "workspace_id"
        case inputTokens = "input_tokens"
        case outputTokens = "output_tokens"
        case cachedTokens = "cached_tokens"
        case totalTokens = "total_tokens"
        case costUSD = "cost_usd"
        case priced
        case runs
    }
}

/// The models `/api/usage` could not price, named (`UnpricedGap` on the Rust
/// side). `UsageSummary.priced == false` says spend is partial but not by
/// how much or because of what — this closes that gap: `rows` is how many
/// token rows those `models` account for.
public struct APIUnpricedGap: Decodable, Equatable, Sendable {
    public let models: [String]
    public let rows: UInt64

    public init(models: [String], rows: UInt64) {
        self.models = models
        self.rows = rows
    }
}

/// `GET /api/usage[?since=&until=&group_by=&host=]` — fleet-wide token +
/// cost overview, fanned out across every registered host (`UsageResponse`
/// on the Rust side). `summary` reuses `APIUsageSummary` (`ListRows.swift`,
/// the same `{input_tokens, output_tokens, cached_tokens, total_tokens,
/// cost_usd?, priced, runs}` shape every list-row endpoint embeds); `hosts`
/// reuses Phase 4's `APIHostFreshness` (`DashboardModels.swift`) — a host
/// that cannot report contributes nothing to `summary`/`breakdown`, never
/// zeros; its reporting state lives here instead.
public struct APIUsageResponse: Decodable, Equatable, Sendable {
    public let summary: APIUsageSummary
    public let breakdown: [APIUsageBreakdownRow]
    public let unpriced: APIUnpricedGap
    public let hosts: [APIHostFreshness]

    public init(
        summary: APIUsageSummary,
        breakdown: [APIUsageBreakdownRow],
        unpriced: APIUnpricedGap,
        hosts: [APIHostFreshness]
    ) {
        self.summary = summary
        self.breakdown = breakdown
        self.unpriced = unpriced
        self.hosts = hosts
    }
}

/// One flat `(run × model)` usage row (`UsageRunRow` on the Rust side) —
/// `GET /api/usage/runs`, local-only (no host fan-out; `hostID` is always
/// `"local"`). Priced server-side with the same lookup path `summary`/
/// `breakdown` use, so the client only ever sums an already-priced number.
public struct APIUsageRunRow: Decodable, Equatable, Sendable {
    public let runID: String
    public let startedAt: String
    public let workflowName: String
    public let agent: String
    public let provider: String
    public let model: String
    public let workspaceID: String
    public let hostID: String
    public let inputTokens: UInt64
    public let outputTokens: UInt64
    public let cachedTokens: UInt64
    public let totalTokens: UInt64
    public let costUSD: Double?
    public let priced: Bool

    public init(
        runID: String,
        startedAt: String,
        workflowName: String,
        agent: String,
        provider: String,
        model: String,
        workspaceID: String,
        hostID: String,
        inputTokens: UInt64,
        outputTokens: UInt64,
        cachedTokens: UInt64,
        totalTokens: UInt64,
        costUSD: Double?,
        priced: Bool
    ) {
        self.runID = runID
        self.startedAt = startedAt
        self.workflowName = workflowName
        self.agent = agent
        self.provider = provider
        self.model = model
        self.workspaceID = workspaceID
        self.hostID = hostID
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cachedTokens = cachedTokens
        self.totalTokens = totalTokens
        self.costUSD = costUSD
        self.priced = priced
    }

    private enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case startedAt = "started_at"
        case workflowName = "workflow_name"
        case agent
        case provider
        case model
        case workspaceID = "workspace_id"
        case hostID = "host_id"
        case inputTokens = "input_tokens"
        case outputTokens = "output_tokens"
        case cachedTokens = "cached_tokens"
        case totalTokens = "total_tokens"
        case costUSD = "cost_usd"
        case priced
    }
}

/// A run costing far more than its OWN workflow's median baseline
/// (`OutlierRun` on the Rust side) — `GET /api/usage/outliers`. The
/// baseline is per-workflow, not global, so an expensive-by-design workflow
/// never gets flagged forever and a cheap one that regressed 10x always
/// does.
public struct APIOutlierRun: Decodable, Equatable, Sendable {
    public let runID: String
    public let workflowName: String
    public let costUSD: Double
    public let baselineUSD: Double
    public let ratio: Double
    public let startedAt: String

    public init(
        runID: String,
        workflowName: String,
        costUSD: Double,
        baselineUSD: Double,
        ratio: Double,
        startedAt: String
    ) {
        self.runID = runID
        self.workflowName = workflowName
        self.costUSD = costUSD
        self.baselineUSD = baselineUSD
        self.ratio = ratio
        self.startedAt = startedAt
    }

    private enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case workflowName = "workflow_name"
        case costUSD = "cost_usd"
        case baselineUSD = "baseline_usd"
        case ratio
        case startedAt = "started_at"
    }
}
