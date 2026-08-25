import Foundation

/// Mirrors the Rust `Event` enum emitted by `rupu-orchestrator`'s executor
/// (see `crates/rupu-orchestrator/src/executor/event.rs`), tagged on `"type"`
/// with snake_case field names. Timestamps and `kind`/`status` decode as
/// plain `String` in this phase (typed enums arrive with the screens that
/// render them).
///
/// Unknown `type` tags decode as `.unknown(type:runID:)` rather than
/// throwing, so the client stays forward-compatible with events added on
/// the Rust side after this client ships.
///
/// `Hashable` (Phase 6B, Task 7 review fix round 1, ruling 2): every case's
/// associated values are themselves `Hashable` primitives, so this
/// synthesizes for free. Added so `SituationStore` (`RupuStore`) can
/// maintain an O(1) `Set<CPEvent>` of "already seen" content identities for
/// its reconnect-replay dedup, instead of an O(n) linear `contains(where:)`
/// scan over up to 5,000 rows on every live event.
public enum CPEvent: Equatable, Hashable, Sendable {
    case runStarted(runID: String, workflowPath: String, startedAt: String)
    case stepStarted(runID: String, stepID: String, kind: String, agent: String?, host: String?)
    case stepWorking(runID: String, stepID: String, note: String?, transcriptPath: String?)
    case stepAwaitingApproval(runID: String, stepID: String, reason: String)
    case stepCompleted(runID: String, stepID: String, success: Bool, durationMS: UInt64, host: String?)
    case stepFailed(runID: String, stepID: String, error: String)
    case stepSkipped(runID: String, stepID: String, reason: String)
    case unitStarted(runID: String, stepID: String, index: Int, unitKey: String, agent: String?, transcriptPath: String, host: String?)
    case unitCompleted(runID: String, stepID: String, index: Int, unitKey: String, success: Bool, tokensIn: UInt64, tokensOut: UInt64, host: String?)
    case panelRound(runID: String, stepID: String, round: UInt32, maxIterations: UInt32, maxSeverityRemaining: String?)
    case runCompleted(runID: String, status: String, finishedAt: String)
    case runFailed(runID: String, error: String, finishedAt: String)
    case runPaused(runID: String)
    case runResumed(runID: String)
    case stepPaused(runID: String, stepID: String)
    case stepResumed(runID: String, stepID: String)
    case dispatchStarted(runID: String, subRunID: String, agent: String?, transcriptPath: String)
    case dispatchCompleted(runID: String, subRunID: String, success: Bool, tokensIn: UInt64, tokensOut: UInt64)
    case unknown(type: String, runID: String?)

    public var runID: String? {
        switch self {
        case .runStarted(let runID, _, _): return runID
        case .stepStarted(let runID, _, _, _, _): return runID
        case .stepWorking(let runID, _, _, _): return runID
        case .stepAwaitingApproval(let runID, _, _): return runID
        case .stepCompleted(let runID, _, _, _, _): return runID
        case .stepFailed(let runID, _, _): return runID
        case .stepSkipped(let runID, _, _): return runID
        case .unitStarted(let runID, _, _, _, _, _, _): return runID
        case .unitCompleted(let runID, _, _, _, _, _, _, _): return runID
        case .panelRound(let runID, _, _, _, _): return runID
        case .runCompleted(let runID, _, _): return runID
        case .runFailed(let runID, _, _): return runID
        case .runPaused(let runID): return runID
        case .runResumed(let runID): return runID
        case .stepPaused(let runID, _): return runID
        case .stepResumed(let runID, _): return runID
        case .dispatchStarted(let runID, _, _, _): return runID
        case .dispatchCompleted(let runID, _, _, _, _): return runID
        case .unknown(_, let runID): return runID
        }
    }
}

extension CPEvent: Decodable {
    private enum CodingKeys: String, CodingKey {
        case type
        case eventVersion = "event_version"
        case runID = "run_id"
        case stepID = "step_id"
        case subRunID = "sub_run_id"
        case workflowPath = "workflow_path"
        case startedAt = "started_at"
        case finishedAt = "finished_at"
        case transcriptPath = "transcript_path"
        case unitKey = "unit_key"
        case tokensIn = "tokens_in"
        case tokensOut = "tokens_out"
        case maxIterations = "max_iterations"
        case maxSeverityRemaining = "max_severity_remaining"
        case durationMS = "duration_ms"
        case kind
        case agent
        case host
        case note
        case reason
        case success
        case error
        case index
        case round
        case status
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let typeTag = try container.decode(String.self, forKey: .type)
        let runIDIfPresent = try container.decodeIfPresent(String.self, forKey: .runID)

        // Unknown type tags must never throw (forward compatibility): fall
        // through to `.unknown` before requiring any field a known variant
        // depends on.
        guard Self.knownTypeTags.contains(typeTag) else {
            self = .unknown(type: typeTag, runID: runIDIfPresent)
            return
        }
        let runID = try container.decode(String.self, forKey: .runID)

        switch typeTag {
        case "run_started":
            self = .runStarted(
                runID: runID,
                workflowPath: try container.decode(String.self, forKey: .workflowPath),
                startedAt: try container.decode(String.self, forKey: .startedAt)
            )
        case "step_started":
            self = .stepStarted(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID),
                kind: try container.decode(String.self, forKey: .kind),
                agent: try container.decodeIfPresent(String.self, forKey: .agent),
                host: try container.decodeIfPresent(String.self, forKey: .host)
            )
        case "step_working":
            self = .stepWorking(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID),
                note: try container.decodeIfPresent(String.self, forKey: .note),
                transcriptPath: try container.decodeIfPresent(String.self, forKey: .transcriptPath)
            )
        case "step_awaiting_approval":
            self = .stepAwaitingApproval(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID),
                reason: try container.decode(String.self, forKey: .reason)
            )
        case "step_completed":
            self = .stepCompleted(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID),
                success: try container.decode(Bool.self, forKey: .success),
                durationMS: try container.decode(UInt64.self, forKey: .durationMS),
                host: try container.decodeIfPresent(String.self, forKey: .host)
            )
        case "step_failed":
            self = .stepFailed(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID),
                error: try container.decode(String.self, forKey: .error)
            )
        case "step_skipped":
            self = .stepSkipped(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID),
                reason: try container.decode(String.self, forKey: .reason)
            )
        case "unit_started":
            self = .unitStarted(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID),
                index: try container.decode(Int.self, forKey: .index),
                unitKey: try container.decode(String.self, forKey: .unitKey),
                agent: try container.decodeIfPresent(String.self, forKey: .agent),
                transcriptPath: try container.decode(String.self, forKey: .transcriptPath),
                host: try container.decodeIfPresent(String.self, forKey: .host)
            )
        case "unit_completed":
            self = .unitCompleted(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID),
                index: try container.decode(Int.self, forKey: .index),
                unitKey: try container.decode(String.self, forKey: .unitKey),
                success: try container.decode(Bool.self, forKey: .success),
                tokensIn: try container.decode(UInt64.self, forKey: .tokensIn),
                tokensOut: try container.decode(UInt64.self, forKey: .tokensOut),
                host: try container.decodeIfPresent(String.self, forKey: .host)
            )
        case "panel_round":
            self = .panelRound(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID),
                round: try container.decode(UInt32.self, forKey: .round),
                maxIterations: try container.decode(UInt32.self, forKey: .maxIterations),
                maxSeverityRemaining: try container.decodeIfPresent(String.self, forKey: .maxSeverityRemaining)
            )
        case "run_completed":
            self = .runCompleted(
                runID: runID,
                status: try container.decode(String.self, forKey: .status),
                finishedAt: try container.decode(String.self, forKey: .finishedAt)
            )
        case "run_failed":
            self = .runFailed(
                runID: runID,
                error: try container.decode(String.self, forKey: .error),
                finishedAt: try container.decode(String.self, forKey: .finishedAt)
            )
        case "run_paused":
            self = .runPaused(runID: runID)
        case "run_resumed":
            self = .runResumed(runID: runID)
        case "step_paused":
            self = .stepPaused(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID)
            )
        case "step_resumed":
            self = .stepResumed(
                runID: runID,
                stepID: try container.decode(String.self, forKey: .stepID)
            )
        case "dispatch_started":
            self = .dispatchStarted(
                runID: runID,
                subRunID: try container.decode(String.self, forKey: .subRunID),
                agent: try container.decodeIfPresent(String.self, forKey: .agent),
                transcriptPath: try container.decode(String.self, forKey: .transcriptPath)
            )
        case "dispatch_completed":
            self = .dispatchCompleted(
                runID: runID,
                subRunID: try container.decode(String.self, forKey: .subRunID),
                success: try container.decode(Bool.self, forKey: .success),
                tokensIn: try container.decode(UInt64.self, forKey: .tokensIn),
                tokensOut: try container.decode(UInt64.self, forKey: .tokensOut)
            )
        default:
            // Unreachable: `typeTag` was already checked against
            // `knownTypeTags` above.
            self = .unknown(type: typeTag, runID: runIDIfPresent)
        }
    }

    private static let knownTypeTags: Set<String> = [
        "run_started", "step_started", "step_working", "step_awaiting_approval",
        "step_completed", "step_failed", "step_skipped", "unit_started",
        "unit_completed", "panel_round", "run_completed", "run_failed",
        "run_paused", "run_resumed", "step_paused", "step_resumed",
        "dispatch_started", "dispatch_completed",
    ]
}
