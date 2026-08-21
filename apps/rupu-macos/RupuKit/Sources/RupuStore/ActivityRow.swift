import Foundation
import RupuAPI
import RupuDesign

/// The four list-row "kinds" the Activity screen federates into one feed
/// (`GET /api/runs` → workflow, `GET /api/runs/agents` → agent,
/// `GET /api/runs/autoflows/events` → autoflow, `GET /api/sessions` →
/// session — see api-facts.md). Drives the kind filter.
public enum ActivityKindTag: String, Sendable {
    case agent, workflow, autoflow, session
}

/// Normalized run/agent/session lifecycle state. `ActivityRow.status` is
/// always one of these — never a raw server string — so every screen that
/// renders status (filters, glyphs, `RunTone` color) shares one vocabulary
/// regardless of which upstream endpoint the row came from.
public enum ActivityStatus: Hashable, Sendable {
    case pending, running, completed, failed, awaiting, rejected, cancelled, paused
    case unknown(String)

    public var tone: RunTone {
        switch self {
        case .running: return .run
        case .completed: return .done
        case .failed, .rejected: return .fail
        case .awaiting: return .waiting
        case .paused, .cancelled: return .pause
        case .pending, .unknown: return .pause
        }
    }

    /// Maps a raw server status string to `ActivityStatus`. Run statuses
    /// (`RunStatus` on the Rust side: `pending|running|completed|failed|
    /// awaiting_approval|rejected|cancelled|paused`) are already
    /// snake_case and map 1:1; agent/autoflow rows instead carry a looser
    /// string vocabulary (`"ok"`/`"error"`/`"aborted"`/`"running"`),
    /// mapped onto the same cases. `nil` (an agent row before its first
    /// status update, an autoflow event with no run yet) becomes
    /// `.unknown("—")` — an explicit placeholder rather than silently
    /// picking a real status case.
    public static func normalize(_ raw: String?) -> ActivityStatus {
        guard let raw else { return .unknown("—") }
        switch raw {
        case "pending": return .pending
        case "running": return .running
        case "completed", "ok": return .completed
        case "failed", "error": return .failed
        case "awaiting_approval": return .awaiting
        case "rejected": return .rejected
        case "cancelled", "aborted": return .cancelled
        case "paused": return .paused
        default: return .unknown(raw)
        }
    }
}

/// One row in the federated Activity feed — the normalized shape every
/// list-row API type (`APIRunListRow` / `APIAgentRunRow` /
/// `APIAutoflowEventRow` / `APISessionRow`) maps onto, so the Activity
/// table renders one row type regardless of source endpoint.
public struct ActivityRow: Identifiable, Equatable, Sendable {
    public let id: String
    public let kind: ActivityKindTag
    public let subject: String
    public let project: String?
    public let host: String
    public let trigger: String?
    public let status: ActivityStatus
    public let durationMS: UInt64?
    public let costUSD: Double?
    public let startedAt: Date?
    public let navigation: Navigation

    public enum Navigation: Equatable, Sendable {
        case run(id: String, host: String?)
        case session(id: String)
        /// A standalone agent run (`GET /api/runs/agents` row with
        /// `source != "session"`, or `source == "session"` but no
        /// `session_id`) — **not** an orchestrator run: `GET /api/runs/:id`
        /// can never serve it (verified 404 live against a real `rupu cp
        /// serve` for exactly this row shape). `.run(id:host:)` must never
        /// be used for these — see `ActivityRow.init(_:APIAgentRunRow)`.
        /// `transcriptPath` is carried straight through (already `nil` when
        /// the run never recorded one) so `AgentRunDetailStore` never has
        /// to re-derive it.
        case agentRun(id: String, transcriptPath: String?, host: String?)
        case none
    }

    /// Fallback host label for a row whose upstream `host_id` was never
    /// injected. Server convention (see api-facts.md and
    /// `crates/rupu-cp/src/api/run_streams.rs`'s repeated
    /// `row.host_id = Some("local".to_string())` sites): an absent
    /// `host_id` means the local, non-node backend, so every row the
    /// server tags itself uses this same literal.
    private static let localHost = "local"

    public init(_ r: APIRunListRow) {
        id = r.id
        kind = .workflow
        subject = r.workflowName
        project = nil
        host = r.hostID ?? Self.localHost
        trigger = r.trigger
        status = .normalize(r.status)
        durationMS = r.durationMS
        costUSD = r.usage.costUSD
        startedAt = Self.parseISO(r.startedAt)
        navigation = .run(id: r.id, host: r.hostID)
    }

    /// **Navigation (hotfix root cause C)**: an agent-run row is never an
    /// orchestrator run, so it must never navigate via `.run(id:host:)` —
    /// `GET /api/runs/:id` 404s for it every time, live-verified against a
    /// real `rupu cp serve` for a `source:"session"` row with a
    /// `transcript_path`. A `source == "session"` row *with* a `session_id`
    /// really is one turn of a session (the same session detail screen
    /// already renders for the session-list source), so it navigates there
    /// instead — the honest destination this row's data actually supports.
    /// Every other agent row (`source != "session"`, or `"session"` with no
    /// `session_id`) is a standalone agent run with no richer destination
    /// than its own transcript: `.agentRun(id:transcriptPath:host:)`.
    public init(_ r: APIAgentRunRow) {
        id = r.runID
        kind = .agent
        subject = r.agent ?? "agent run"
        project = nil
        host = r.hostID ?? Self.localHost
        trigger = r.triggerSource
        status = .normalize(r.status)
        durationMS = r.durationMS
        costUSD = r.usage.costUSD
        startedAt = Self.parseISO(r.startedAt)
        if r.source == "session", let sessionID = r.sessionID {
            navigation = .session(id: sessionID)
        } else {
            navigation = .agentRun(id: r.runID, transcriptPath: r.transcriptPath, host: r.hostID)
        }
    }

    public init(_ r: APIAutoflowEventRow) {
        id = r.eventID
        kind = .autoflow
        subject = r.workflow ?? r.kind
        project = nil
        host = r.hostID ?? Self.localHost
        // No trigger-equivalent field on this row: `kind` already backs
        // `subject`'s fallback, and `worker_name` isn't display-ready
        // trigger vocabulary (cron/manual/session_turn) — left nil rather
        // than mislabeling worker identity as a trigger.
        trigger = nil
        status = .normalize(r.status)
        durationMS = r.durationMS
        costUSD = r.usage.costUSD
        startedAt = Self.parseISO(r.at)
        navigation = r.runID.map { .run(id: $0, host: r.hostID) } ?? .none
    }

    public init(_ r: APISessionRow) {
        id = r.sessionID
        kind = .session
        subject = r.agentName
        project = r.workspaceID
        host = r.hostID ?? Self.localHost
        trigger = nil
        status = r.activeRunID != nil ? .running : (r.lastError != nil ? .failed : .completed)
        durationMS = nil
        costUSD = r.usage?.costUSD
        startedAt = Self.parseISO(r.createdAt)
        navigation = .session(id: r.sessionID)
    }

    /// Full-field memberwise initializer used only by
    /// `patchingStatus(_:durationMS:)` below to rebuild a copy with an
    /// updated `status`/`durationMS` after a live `ActivityDelta`. Deliberately
    /// `internal` (not part of the public surface the Task 4 report
    /// documents) — `ActivityStore`, the only caller, lives in this same
    /// module.
    init(
        id: String, kind: ActivityKindTag, subject: String, project: String?, host: String,
        trigger: String?, status: ActivityStatus, durationMS: UInt64?, costUSD: Double?,
        startedAt: Date?, navigation: Navigation
    ) {
        self.id = id
        self.kind = kind
        self.subject = subject
        self.project = project
        self.host = host
        self.trigger = trigger
        self.status = status
        self.durationMS = durationMS
        self.costUSD = costUSD
        self.startedAt = startedAt
        self.navigation = navigation
    }

    /// Returns a copy with `status` (and, when provided, `durationMS`)
    /// replaced — the write side of an `ActivityDelta.statusPatch` applied
    /// directly to an already-merged row, without refetching it.
    /// `newDurationMS` only overrides when non-nil: none of
    /// `ActivityDelta.reduce`'s produced status patches currently carry a
    /// duration (see `LiveReducer.swift`'s doc comment), so a `nil` here
    /// keeps whatever REST-sourced duration the row already had rather than
    /// clobbering it.
    func patchingStatus(_ newStatus: ActivityStatus, durationMS newDurationMS: UInt64?) -> ActivityRow {
        ActivityRow(
            id: id, kind: kind, subject: subject, project: project, host: host,
            trigger: trigger, status: newStatus, durationMS: newDurationMS ?? durationMS,
            costUSD: costUSD, startedAt: startedAt, navigation: navigation
        )
    }

    /// Parses an RFC3339 timestamp with or without fractional seconds
    /// (server payloads use both — e.g. plain-second `started_at` fields
    /// vs. millisecond-precision transcript/event timestamps): tries the
    /// fractional-seconds formatter first, falling back to the plain one.
    ///
    /// Builds a fresh `ISO8601DateFormatter` per call rather than caching
    /// one in a `static let`: `ISO8601DateFormatter` isn't `Sendable`, so a
    /// shared static instance trips Swift 6's mutable-global-state check
    /// on this `Sendable` struct; a formatter is cheap enough to construct
    /// that this isn't a meaningful hot path either way.
    public static func parseISO(_ s: String?) -> Date? {
        guard let s else { return nil }
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = fractional.date(from: s) { return date }

        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        return plain.date(from: s)
    }
}
