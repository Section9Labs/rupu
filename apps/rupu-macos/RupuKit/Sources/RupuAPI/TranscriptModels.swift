import Foundation

/// Minimal recursive JSON value for the opaque payloads on a few transcript
/// event variants (`tool_call.input`, `tool_result.structured`) whose shape
/// isn't modeled field-by-field this phase.
public enum JSONValue: Decodable, Equatable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([String: JSONValue].self) {
            self = .object(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Unsupported JSON value")
        }
    }
}

/// One event from a transcript JSONL file / `/api/transcript/stream`, as
/// served by `GET /api/transcript`. Unlike `CPEvent` (internally tagged on
/// `type` with fields flattened into the same object), `TranscriptEvent` is
/// **adjacently tagged**: `{"type": "<snake_case>", "data": {...}}`.
///
/// `action_emitted`, `tool_audit`, and `net_flow` decode successfully but
/// carry no associated payload this phase — nothing renders them yet, so
/// their `data` bodies are intentionally left unparsed.
///
/// Unknown `type` tags decode as `.unknown(type:)` rather than throwing, so
/// the client stays forward-compatible with variants added on the Rust side
/// after this client ships.
public enum TranscriptEvent: Decodable, Equatable, Sendable {
    case assistantMessage(content: String, thinking: String?)
    case assistantDelta(content: String)
    case toolCall(callID: String, tool: String, input: JSONValue)
    case toolResult(callID: String, output: String, error: String?, durationMS: UInt64, structured: JSONValue?)
    case gateRequested(gateID: String, prompt: String, decision: String?, decidedBy: String?)
    case runStart(runID: String, workspaceID: String, agent: String, provider: String, model: String, startedAt: String, mode: String)
    case turnStart(turnIdx: UInt32)
    case turnEnd(turnIdx: UInt32, tokensIn: UInt64?, tokensOut: UInt64?)
    case fileEdit(path: String, kind: String, diff: String)
    case commandRun(argv: [String], cwd: String, exitCode: Int32, stdoutBytes: UInt64, stderrBytes: UInt64)
    case usage(provider: String, model: String, servedModel: String?, inputTokens: UInt64, outputTokens: UInt64, cachedTokens: UInt64)
    case runComplete(runID: String, status: String, totalTokens: UInt64, durationMS: UInt64, error: String?)
    case actionEmitted
    case toolAudit
    case netFlow
    case unknown(type: String)

    private enum RootKeys: String, CodingKey {
        case type
        case data
    }

    private enum DataKeys: String, CodingKey {
        case content, thinking
        case callID = "call_id"
        case tool, input, output, error, structured
        case durationMS = "duration_ms"
        case gateID = "gate_id"
        case prompt, decision
        case decidedBy = "decided_by"
        case runID = "run_id"
        case workspaceID = "workspace_id"
        case agent, provider, model
        case startedAt = "started_at"
        case mode
        case turnIdx = "turn_idx"
        case tokensIn = "tokens_in"
        case tokensOut = "tokens_out"
        case path, kind, diff, argv, cwd
        case exitCode = "exit_code"
        case stdoutBytes = "stdout_bytes"
        case stderrBytes = "stderr_bytes"
        case servedModel = "served_model"
        case inputTokens = "input_tokens"
        case outputTokens = "output_tokens"
        case cachedTokens = "cached_tokens"
        case status
        case totalTokens = "total_tokens"
    }

    public init(from decoder: Decoder) throws {
        let root = try decoder.container(keyedBy: RootKeys.self)
        let type = try root.decode(String.self, forKey: .type)

        switch type {
        case "assistant_message":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .assistantMessage(
                content: try data.decode(String.self, forKey: .content),
                thinking: try data.decodeIfPresent(String.self, forKey: .thinking)
            )
        case "assistant_delta":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .assistantDelta(content: try data.decode(String.self, forKey: .content))
        case "tool_call":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .toolCall(
                callID: try data.decode(String.self, forKey: .callID),
                tool: try data.decode(String.self, forKey: .tool),
                input: try data.decode(JSONValue.self, forKey: .input)
            )
        case "tool_result":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .toolResult(
                callID: try data.decode(String.self, forKey: .callID),
                output: try data.decode(String.self, forKey: .output),
                error: try data.decodeIfPresent(String.self, forKey: .error),
                durationMS: try data.decode(UInt64.self, forKey: .durationMS),
                structured: try data.decodeIfPresent(JSONValue.self, forKey: .structured)
            )
        case "gate_requested":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .gateRequested(
                gateID: try data.decode(String.self, forKey: .gateID),
                prompt: try data.decode(String.self, forKey: .prompt),
                decision: try data.decodeIfPresent(String.self, forKey: .decision),
                decidedBy: try data.decodeIfPresent(String.self, forKey: .decidedBy)
            )
        case "run_start":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .runStart(
                runID: try data.decode(String.self, forKey: .runID),
                workspaceID: try data.decode(String.self, forKey: .workspaceID),
                agent: try data.decode(String.self, forKey: .agent),
                provider: try data.decode(String.self, forKey: .provider),
                model: try data.decode(String.self, forKey: .model),
                startedAt: try data.decode(String.self, forKey: .startedAt),
                mode: try data.decode(String.self, forKey: .mode)
            )
        case "turn_start":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .turnStart(turnIdx: try data.decode(UInt32.self, forKey: .turnIdx))
        case "turn_end":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .turnEnd(
                turnIdx: try data.decode(UInt32.self, forKey: .turnIdx),
                tokensIn: try data.decodeIfPresent(UInt64.self, forKey: .tokensIn),
                tokensOut: try data.decodeIfPresent(UInt64.self, forKey: .tokensOut)
            )
        case "file_edit":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .fileEdit(
                path: try data.decode(String.self, forKey: .path),
                kind: try data.decode(String.self, forKey: .kind),
                diff: try data.decode(String.self, forKey: .diff)
            )
        case "command_run":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .commandRun(
                argv: try data.decode([String].self, forKey: .argv),
                cwd: try data.decode(String.self, forKey: .cwd),
                exitCode: try data.decode(Int32.self, forKey: .exitCode),
                stdoutBytes: try data.decode(UInt64.self, forKey: .stdoutBytes),
                stderrBytes: try data.decode(UInt64.self, forKey: .stderrBytes)
            )
        case "usage":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .usage(
                provider: try data.decode(String.self, forKey: .provider),
                model: try data.decode(String.self, forKey: .model),
                servedModel: try data.decodeIfPresent(String.self, forKey: .servedModel),
                inputTokens: try data.decode(UInt64.self, forKey: .inputTokens),
                outputTokens: try data.decode(UInt64.self, forKey: .outputTokens),
                cachedTokens: try data.decode(UInt64.self, forKey: .cachedTokens)
            )
        case "run_complete":
            let data = try root.nestedContainer(keyedBy: DataKeys.self, forKey: .data)
            self = .runComplete(
                runID: try data.decode(String.self, forKey: .runID),
                status: try data.decode(String.self, forKey: .status),
                totalTokens: try data.decode(UInt64.self, forKey: .totalTokens),
                durationMS: try data.decode(UInt64.self, forKey: .durationMS),
                error: try data.decodeIfPresent(String.self, forKey: .error)
            )
        case "action_emitted":
            self = .actionEmitted
        case "tool_audit":
            self = .toolAudit
        case "net_flow":
            self = .netFlow
        default:
            self = .unknown(type: type)
        }
    }
}

/// `RunSummary` on the Rust side — the trailing summary object returned
/// alongside `events` from `GET /api/transcript` (`null` when the
/// transcript file is missing).
public struct APIRunSummary: Decodable, Sendable {
    public let runID: String
    public let agent: String
    public let provider: String
    public let model: String
    public let status: String
    public let totalTokens: UInt64
    public let durationMS: UInt64
    public let error: String?
    public let firstAssistantText: String?

    public init(
        runID: String,
        agent: String,
        provider: String,
        model: String,
        status: String,
        totalTokens: UInt64,
        durationMS: UInt64,
        error: String?,
        firstAssistantText: String?
    ) {
        self.runID = runID
        self.agent = agent
        self.provider = provider
        self.model = model
        self.status = status
        self.totalTokens = totalTokens
        self.durationMS = durationMS
        self.error = error
        self.firstAssistantText = firstAssistantText
    }

    private enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case agent
        case provider
        case model
        case status
        case totalTokens = "total_tokens"
        case durationMS = "duration_ms"
        case error
        case firstAssistantText = "first_assistant_text"
    }
}

/// `GET /api/transcript` response envelope.
public struct APITranscriptPage: Decodable, Sendable {
    public let events: [TranscriptEvent]
    public let summary: APIRunSummary?

    public init(events: [TranscriptEvent], summary: APIRunSummary?) {
        self.events = events
        self.summary = summary
    }
}
