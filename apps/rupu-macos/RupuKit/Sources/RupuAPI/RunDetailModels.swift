import Foundation

/// `GET /api/runs/:id` response envelope.
public struct APIRunDetail: Decodable, Sendable {
    public let run: APIRunRecord
    public let steps: [APIStepResult]
    public let usage: APIUsageSummary

    public init(run: APIRunRecord, steps: [APIStepResult], usage: APIUsageSummary) {
        self.run = run
        self.steps = steps
        self.usage = usage
    }
}

/// `RunRecord` on the Rust side. The server emits ~36 fields total; this
/// decodes only the ones the UI needs (`decodeIfPresent` throughout) —
/// extra fields are ignored by design.
public struct APIRunRecord: Decodable, Sendable {
    public let id: String
    public let workflowName: String
    public let status: String
    public let workspaceID: String
    public let startedAt: String
    public let finishedAt: String?
    public let errorMessage: String?
    public let awaiting: [APIAwaitingGate]
    public let activeStepID: String?
    public let activeStepTranscriptPath: String?
    public let parentRunID: String?
    public let permissionMode: String?
    public let finalOutput: String?

    public init(
        id: String,
        workflowName: String,
        status: String,
        workspaceID: String,
        startedAt: String,
        finishedAt: String?,
        errorMessage: String?,
        awaiting: [APIAwaitingGate],
        activeStepID: String?,
        activeStepTranscriptPath: String?,
        parentRunID: String?,
        permissionMode: String?,
        finalOutput: String?
    ) {
        self.id = id
        self.workflowName = workflowName
        self.status = status
        self.workspaceID = workspaceID
        self.startedAt = startedAt
        self.finishedAt = finishedAt
        self.errorMessage = errorMessage
        self.awaiting = awaiting
        self.activeStepID = activeStepID
        self.activeStepTranscriptPath = activeStepTranscriptPath
        self.parentRunID = parentRunID
        self.permissionMode = permissionMode
        self.finalOutput = finalOutput
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case workflowName = "workflow_name"
        case status
        case workspaceID = "workspace_id"
        case startedAt = "started_at"
        case finishedAt = "finished_at"
        case errorMessage = "error_message"
        case awaiting
        case activeStepID = "active_step_id"
        case activeStepTranscriptPath = "active_step_transcript_path"
        case parentRunID = "parent_run_id"
        case permissionMode = "permission_mode"
        case finalOutput = "final_output"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        workflowName = try container.decode(String.self, forKey: .workflowName)
        status = try container.decode(String.self, forKey: .status)
        workspaceID = try container.decode(String.self, forKey: .workspaceID)
        startedAt = try container.decode(String.self, forKey: .startedAt)
        finishedAt = try container.decodeIfPresent(String.self, forKey: .finishedAt)
        errorMessage = try container.decodeIfPresent(String.self, forKey: .errorMessage)
        // Absent for a run that has no gate awaiting an answer.
        awaiting = try container.decodeIfPresent([APIAwaitingGate].self, forKey: .awaiting) ?? []
        activeStepID = try container.decodeIfPresent(String.self, forKey: .activeStepID)
        activeStepTranscriptPath = try container.decodeIfPresent(String.self, forKey: .activeStepTranscriptPath)
        parentRunID = try container.decodeIfPresent(String.self, forKey: .parentRunID)
        permissionMode = try container.decodeIfPresent(String.self, forKey: .permissionMode)
        finalOutput = try container.decodeIfPresent(String.self, forKey: .finalOutput)
    }
}

/// One entry of `RunRecord.awaiting` — a step parked on an approval gate.
public struct APIAwaitingGate: Decodable, Sendable {
    public let stepID: String
    public let prompt: String?
    public let since: String

    public init(stepID: String, prompt: String?, since: String) {
        self.stepID = stepID
        self.prompt = prompt
        self.since = since
    }

    private enum CodingKeys: String, CodingKey {
        case stepID = "step_id"
        case prompt
        case since
    }
}

/// `StepResultRecord` on the Rust side, decoding only the UI-relevant
/// fields. `kind` and `iterations` default (`"linear"` / `0`) because
/// older/simpler step records omit them.
public struct APIStepResult: Decodable, Sendable {
    public let stepID: String
    public let runID: String
    public let transcriptPath: String
    public let output: String
    public let success: Bool
    public let skipped: Bool
    public let kind: String
    public let iterations: UInt32

    public init(
        stepID: String,
        runID: String,
        transcriptPath: String,
        output: String,
        success: Bool,
        skipped: Bool,
        kind: String,
        iterations: UInt32
    ) {
        self.stepID = stepID
        self.runID = runID
        self.transcriptPath = transcriptPath
        self.output = output
        self.success = success
        self.skipped = skipped
        self.kind = kind
        self.iterations = iterations
    }

    private enum CodingKeys: String, CodingKey {
        case stepID = "step_id"
        case runID = "run_id"
        case transcriptPath = "transcript_path"
        case output
        case success
        case skipped
        case kind
        case iterations
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        stepID = try container.decode(String.self, forKey: .stepID)
        runID = try container.decode(String.self, forKey: .runID)
        transcriptPath = try container.decode(String.self, forKey: .transcriptPath)
        output = try container.decode(String.self, forKey: .output)
        success = try container.decode(Bool.self, forKey: .success)
        skipped = try container.decode(Bool.self, forKey: .skipped)
        kind = try container.decodeIfPresent(String.self, forKey: .kind) ?? "linear"
        iterations = try container.decodeIfPresent(UInt32.self, forKey: .iterations) ?? 0
    }
}

/// `GET /api/runs/:id/graph` response envelope.
public struct APIRunGraph: Decodable, Sendable {
    public let run: APIRunRecord
    public let workflow: APIStepDag
    public let stepResults: [APIStepResult]
    public let units: [APIUnitRow]
    public let usage: APIUsageSummary

    public init(
        run: APIRunRecord,
        workflow: APIStepDag,
        stepResults: [APIStepResult],
        units: [APIUnitRow],
        usage: APIUsageSummary
    ) {
        self.run = run
        self.workflow = workflow
        self.stepResults = stepResults
        self.units = units
        self.usage = usage
    }

    private enum CodingKeys: String, CodingKey {
        case run
        case workflow
        case stepResults = "step_results"
        case units
        case usage
    }
}

/// The static workflow DAG shape (`workflow.steps` on the graph response).
public struct APIStepDag: Decodable, Sendable {
    public let steps: [APIStepNode]

    public init(steps: [APIStepNode]) {
        self.steps = steps
    }
}

/// `StepNodeDto` on the Rust side — one node in the workflow DAG. `kind`
/// discriminates which of the optional payload fields are populated
/// (`"step"`/`"for_each"` use `agent`/`forEach`; `"parallel"` uses
/// `parallel`; `"panel"` uses `panelists`/`gate`; `"gate"` uses
/// `approvalGate`; `"action"`/`"run"` use `action`).
public struct APIStepNode: Decodable, Sendable {
    public let id: String
    public let kind: String
    public let agent: String?
    public let forEach: String?
    public let parallel: [APISubStep]?
    public let panelists: [String]?
    public let gate: APIPanelGate?
    public let action: String?
    public let approvalGate: APIApprovalGate?

    public init(
        id: String,
        kind: String,
        agent: String?,
        forEach: String?,
        parallel: [APISubStep]?,
        panelists: [String]?,
        gate: APIPanelGate?,
        action: String?,
        approvalGate: APIApprovalGate?
    ) {
        self.id = id
        self.kind = kind
        self.agent = agent
        self.forEach = forEach
        self.parallel = parallel
        self.panelists = panelists
        self.gate = gate
        self.action = action
        self.approvalGate = approvalGate
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case kind
        case agent
        case forEach = "for_each"
        case parallel
        case panelists
        case gate
        case action
        case approvalGate = "approval_gate"
    }
}

/// One branch of a `"parallel"` step node.
public struct APISubStep: Decodable, Sendable {
    public let id: String
    public let agent: String

    public init(id: String, agent: String) {
        self.id = id
        self.agent = agent
    }
}

/// The panel-review gate config on a `"panel"` step node. `Equatable` so
/// `GraphNodeVM` (which threads this through as `panelGate`) can derive its
/// own `Equatable` conformance.
public struct APIPanelGate: Decodable, Equatable, Sendable {
    public let maxIterations: UInt32
    public let untilSeverity: String
    public let fixWith: String

    public init(maxIterations: UInt32, untilSeverity: String, fixWith: String) {
        self.maxIterations = maxIterations
        self.untilSeverity = untilSeverity
        self.fixWith = fixWith
    }

    private enum CodingKeys: String, CodingKey {
        case maxIterations = "max_iterations"
        case untilSeverity = "until_severity"
        case fixWith = "fix_with"
    }
}

/// The approval-gate config on a `"gate"` step node.
public struct APIApprovalGate: Decodable, Sendable {
    public let autoApprove: Bool
    public let hasOnReject: Bool
    public let timeoutSeconds: UInt64?

    public init(autoApprove: Bool, hasOnReject: Bool, timeoutSeconds: UInt64?) {
        self.autoApprove = autoApprove
        self.hasOnReject = hasOnReject
        self.timeoutSeconds = timeoutSeconds
    }

    private enum CodingKeys: String, CodingKey {
        case autoApprove = "auto_approve"
        case hasOnReject = "has_on_reject"
        case timeoutSeconds = "timeout_seconds"
    }
}

/// One row of `graph.units` — a fan-out/for-each unit's result. Units
/// synthesized from in-flight events (rather than a finished
/// `StepResultRecord`) carry `success: null`, so this decodes it as
/// `Bool?` rather than `Bool`.
public struct APIUnitRow: Decodable, Sendable {
    public let stepID: String
    public let index: Int
    public let runID: String?
    public let transcriptPath: String
    public let success: Bool?
    public let host: String?

    public init(
        stepID: String,
        index: Int,
        runID: String?,
        transcriptPath: String,
        success: Bool?,
        host: String?
    ) {
        self.stepID = stepID
        self.index = index
        self.runID = runID
        self.transcriptPath = transcriptPath
        self.success = success
        self.host = host
    }

    private enum CodingKeys: String, CodingKey {
        case stepID = "step_id"
        case index
        case runID = "run_id"
        case transcriptPath = "transcript_path"
        case success
        case host
    }
}
