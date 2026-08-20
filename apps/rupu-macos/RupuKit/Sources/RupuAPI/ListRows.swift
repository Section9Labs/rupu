import Foundation

/// Shared usage-summary shape embedded in every list-row endpoint
/// (`UsageSummary` on the Rust side): `{input_tokens, output_tokens,
/// cached_tokens, total_tokens, cost_usd?, priced, runs}`.
public struct APIUsageSummary: Decodable, Equatable, Sendable {
    public let inputTokens: UInt64
    public let outputTokens: UInt64
    public let cachedTokens: UInt64
    public let totalTokens: UInt64
    public let costUSD: Double?
    public let priced: Bool
    public let runs: UInt64

    public init(
        inputTokens: UInt64,
        outputTokens: UInt64,
        cachedTokens: UInt64,
        totalTokens: UInt64,
        costUSD: Double?,
        priced: Bool,
        runs: UInt64
    ) {
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cachedTokens = cachedTokens
        self.totalTokens = totalTokens
        self.costUSD = costUSD
        self.priced = priced
        self.runs = runs
    }

    private enum CodingKeys: String, CodingKey {
        case inputTokens = "input_tokens"
        case outputTokens = "output_tokens"
        case cachedTokens = "cached_tokens"
        case totalTokens = "total_tokens"
        case costUSD = "cost_usd"
        case priced
        case runs
    }
}

/// Row from `GET /api/runs` and `GET /api/runs/workflows` (`RunListRow` on
/// the Rust side, with `host_id` injected).
public struct APIRunListRow: Decodable, Equatable, Sendable {
    public let id: String
    public let workflowName: String
    public let status: String
    public let startedAt: String
    public let finishedAt: String?
    public let trigger: String
    public let usage: APIUsageSummary
    public let turns: UInt64
    public let durationMS: UInt64?
    public let hostID: String?

    public init(
        id: String,
        workflowName: String,
        status: String,
        startedAt: String,
        finishedAt: String?,
        trigger: String,
        usage: APIUsageSummary,
        turns: UInt64,
        durationMS: UInt64?,
        hostID: String?
    ) {
        self.id = id
        self.workflowName = workflowName
        self.status = status
        self.startedAt = startedAt
        self.finishedAt = finishedAt
        self.trigger = trigger
        self.usage = usage
        self.turns = turns
        self.durationMS = durationMS
        self.hostID = hostID
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case workflowName = "workflow_name"
        case status
        case startedAt = "started_at"
        case finishedAt = "finished_at"
        case trigger
        case usage
        case turns
        case durationMS = "duration_ms"
        case hostID = "host_id"
    }
}

/// Row from `GET /api/runs/agents` (`AgentRunRow` on the Rust side).
public struct APIAgentRunRow: Decodable, Equatable, Sendable {
    public let runID: String
    public let source: String
    public let agent: String?
    public let sessionID: String?
    public let triggerSource: String?
    public let status: String?
    public let startedAt: String?
    public let transcriptPath: String?
    public let usage: APIUsageSummary
    public let turns: UInt64
    public let durationMS: UInt64?
    public let hostID: String?

    public init(
        runID: String,
        source: String,
        agent: String?,
        sessionID: String?,
        triggerSource: String?,
        status: String?,
        startedAt: String?,
        transcriptPath: String?,
        usage: APIUsageSummary,
        turns: UInt64,
        durationMS: UInt64?,
        hostID: String?
    ) {
        self.runID = runID
        self.source = source
        self.agent = agent
        self.sessionID = sessionID
        self.triggerSource = triggerSource
        self.status = status
        self.startedAt = startedAt
        self.transcriptPath = transcriptPath
        self.usage = usage
        self.turns = turns
        self.durationMS = durationMS
        self.hostID = hostID
    }

    private enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case source
        case agent
        case sessionID = "session_id"
        case triggerSource = "trigger_source"
        case status
        case startedAt = "started_at"
        case transcriptPath = "transcript_path"
        case usage
        case turns
        case durationMS = "duration_ms"
        case hostID = "host_id"
    }
}

/// Row from `GET /api/runs/autoflows/events` (`AutoflowEventRow` on the
/// Rust side).
public struct APIAutoflowEventRow: Decodable, Equatable, Sendable {
    public let eventID: String
    public let cycleID: String
    public let at: String
    public let kind: String
    public let workflow: String?
    public let issueDisplayRef: String?
    public let runID: String?
    public let status: String?
    public let workerName: String?
    public let detail: String?
    public let usage: APIUsageSummary
    public let turns: UInt64?
    public let durationMS: UInt64?
    public let hostID: String?

    public init(
        eventID: String,
        cycleID: String,
        at: String,
        kind: String,
        workflow: String?,
        issueDisplayRef: String?,
        runID: String?,
        status: String?,
        workerName: String?,
        detail: String?,
        usage: APIUsageSummary,
        turns: UInt64?,
        durationMS: UInt64?,
        hostID: String?
    ) {
        self.eventID = eventID
        self.cycleID = cycleID
        self.at = at
        self.kind = kind
        self.workflow = workflow
        self.issueDisplayRef = issueDisplayRef
        self.runID = runID
        self.status = status
        self.workerName = workerName
        self.detail = detail
        self.usage = usage
        self.turns = turns
        self.durationMS = durationMS
        self.hostID = hostID
    }

    private enum CodingKeys: String, CodingKey {
        case eventID = "event_id"
        case cycleID = "cycle_id"
        case at
        case kind
        case workflow
        case issueDisplayRef = "issue_display_ref"
        case runID = "run_id"
        case status
        case workerName = "worker_name"
        case detail
        case usage
        case turns
        case durationMS = "duration_ms"
        case hostID = "host_id"
    }
}

/// Row from `GET /api/sessions` (`SessionDto` on the Rust side, with
/// `scope`, `usage`, and `host_id` injected). `status` is deliberately not
/// decoded this phase — it is a raw JSON value of varying shape on the
/// server; `activeRunID`/`lastError` carry the UI signal instead.
public struct APISessionRow: Decodable, Equatable, Sendable {
    public let sessionID: String
    public let agentName: String
    public let model: String
    public let providerName: String
    public let totalTurns: UInt32
    public let totalTokensIn: UInt64
    public let totalTokensOut: UInt64
    public let totalTokensCached: UInt64
    public let createdAt: String
    public let updatedAt: String
    public let activeRunID: String?
    public let lastError: String?
    public let target: String?
    public let workspaceID: String
    public let scope: String?
    public let usage: APIUsageSummary?
    public let hostID: String?

    public init(
        sessionID: String,
        agentName: String,
        model: String,
        providerName: String,
        totalTurns: UInt32,
        totalTokensIn: UInt64,
        totalTokensOut: UInt64,
        totalTokensCached: UInt64,
        createdAt: String,
        updatedAt: String,
        activeRunID: String?,
        lastError: String?,
        target: String?,
        workspaceID: String,
        scope: String?,
        usage: APIUsageSummary?,
        hostID: String?
    ) {
        self.sessionID = sessionID
        self.agentName = agentName
        self.model = model
        self.providerName = providerName
        self.totalTurns = totalTurns
        self.totalTokensIn = totalTokensIn
        self.totalTokensOut = totalTokensOut
        self.totalTokensCached = totalTokensCached
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.activeRunID = activeRunID
        self.lastError = lastError
        self.target = target
        self.workspaceID = workspaceID
        self.scope = scope
        self.usage = usage
        self.hostID = hostID
    }

    private enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case agentName = "agent_name"
        case model
        case providerName = "provider_name"
        case totalTurns = "total_turns"
        case totalTokensIn = "total_tokens_in"
        case totalTokensOut = "total_tokens_out"
        case totalTokensCached = "total_tokens_cached"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
        case activeRunID = "active_run_id"
        case lastError = "last_error"
        case target
        case workspaceID = "workspace_id"
        case scope
        case usage
        case hostID = "host_id"
    }
}

/// Row from `GET /api/sessions/:id/runs` (`SessionRunRow` on the Rust
/// side) — one prior run launched from this session.
public struct APISessionRunRow: Decodable, Equatable, Sendable {
    public let runID: String
    public let prompt: String
    public let transcriptPath: String
    public let status: String?
    public let startedAt: String?
    public let completedAt: String?
    public let tokensIn: UInt64
    public let tokensOut: UInt64
    public let durationMS: UInt64
    public let error: String?

    public init(
        runID: String,
        prompt: String,
        transcriptPath: String,
        status: String?,
        startedAt: String?,
        completedAt: String?,
        tokensIn: UInt64,
        tokensOut: UInt64,
        durationMS: UInt64,
        error: String?
    ) {
        self.runID = runID
        self.prompt = prompt
        self.transcriptPath = transcriptPath
        self.status = status
        self.startedAt = startedAt
        self.completedAt = completedAt
        self.tokensIn = tokensIn
        self.tokensOut = tokensOut
        self.durationMS = durationMS
        self.error = error
    }

    private enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case prompt
        case transcriptPath = "transcript_path"
        case status
        case startedAt = "started_at"
        case completedAt = "completed_at"
        case tokensIn = "tokens_in"
        case tokensOut = "tokens_out"
        case durationMS = "duration_ms"
        case error
    }
}
