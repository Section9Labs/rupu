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
/// renders status (filters, glyphs, `StatusTone` color) shares one vocabulary
/// regardless of which upstream endpoint the row came from.
public enum ActivityStatus: Hashable, Sendable {
    case pending, running, completed, failed, awaiting, rejected, cancelled, paused
    case unknown(String)

    public var tone: StatusTone {
        switch self {
        case .pending: return .pending
        case .running: return .running
        case .completed: return .done
        case .failed: return .failed
        case .awaiting: return .awaiting
        case .rejected: return .rejected
        case .cancelled: return .cancelled
        case .paused: return .paused
        case .unknown: return .pending
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

    /// Display text for the Activity table's status cell, `FilterBar`'s
    /// chips, and the command palette's run subtitles (flows-composition
    /// Task 3 — moved here from `RupuActivity/ActivityTable.swift` so
    /// `PaletteStore`, which lives in this module, can share it rather than
    /// duplicating the mapping). `.unknown` renders its carried raw string
    /// directly — `ActivityStatus.normalize` already puts an explicit `"—"`
    /// there for a genuinely absent status, so there's nothing further to
    /// null-guard here.
    ///
    /// **`.awaiting` is the full canonical "Awaiting approval"**
    /// (redesign-pass fix — audit A4): this is the app-wide status string
    /// (`StatusPill`'s `.awaiting` descriptor, the web's `status.ts:95`),
    /// and the audit found room for the full string in the Activity table's
    /// status column, which reads this property directly
    /// (`ActivityTable.swift:315`). `FilterBar`'s status chip is narrower
    /// and deliberately overrides back to the short "Awaiting" for THAT one
    /// case rather than reading this property verbatim — see
    /// `FilterBar.statusChip`'s own doc comment for why.
    public var displayLabel: String {
        switch self {
        case .pending: "Pending"
        case .running: "Running"
        case .completed: "Completed"
        case .failed: "Failed"
        case .awaiting: "Awaiting approval"
        case .rejected: "Rejected"
        case .cancelled: "Cancelled"
        case .paused: "Paused"
        case .unknown(let raw): raw
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

    // MARK: - Per-kind detail (perf & interaction arc, Plan 5 Task 4)
    //
    // The merged Activity feed (`ActivityTable`, `NeedsYou`, the command
    // palette) only ever needed the common fields above — every list-row API
    // type carries more than that, but nothing downstream of the merged view
    // read it, so it was never mapped in. The Task 4 per-kind tables (per-kind
    // columns ported verbatim from the web: token counts, model, issue ref,
    // worker, autoflow failure detail, agent run source) DO need it — rather
    // than inventing a second per-kind row type (and a second mapping site per
    // API type), these are added here as plain optional fields, populated only
    // by the one or two kind-specific inits below that have the data; every
    // other kind's rows (and the merged view, which never reads these) simply
    // carry `nil`. `Equatable`/`Sendable` conformance is unaffected — every
    // added type already conforms.
    /// Input tokens (`usage.input_tokens`) — agent/workflow/autoflow/session.
    public let inputTokens: UInt64?
    /// Output tokens (`usage.output_tokens`) — agent/workflow/autoflow/session.
    public let outputTokens: UInt64?
    /// Cached tokens (`usage.cached_tokens`) — agent/workflow/autoflow/session.
    public let cachedTokens: UInt64?
    /// Turn count — agent/workflow rows always carry a real `UInt64`; autoflow
    /// events carry an optional one (some event kinds have no turn count at
    /// all); sessions carry `total_turns`.
    public let turns: UInt64?
    /// Session model id (`SessionDto.model`) — sessions only.
    public let model: String?
    /// Raw agent-run `source` string (`"session"`/`"cli"`/`"cron"`/etc.) —
    /// agent rows only. Backs the AgentRuns table's Source badge (web parity:
    /// `source == "session"` gets the info tone, everything else neutral).
    public let source: String?
    /// Autoflow event's `issue_display_ref` — autoflow rows only.
    public let issueRef: String?
    /// Autoflow event's `worker_name` — autoflow rows only.
    public let worker: String?
    /// Autoflow event's raw `kind` (`"cycle_failed"`/`"run_started"`/etc.) —
    /// autoflow rows only. Kept distinct from `subject` (which already falls
    /// back to this same string when `workflow` is nil) because the
    /// AutoflowRuns table's Event column needs the raw kind independently of
    /// whichever workflow name `subject` resolved to.
    public let eventKind: String?
    /// Autoflow event's failure `detail` text — autoflow rows only, and only
    /// non-nil for events that actually carry one (`cycle_failed` et al.).
    /// Backs the AutoflowRuns table's expandable failure-detail row.
    public let detail: String?
    /// Session `updated_at` — sessions only. Backs the Sessions table's
    /// Duration column (`created→updated`), computed from this and
    /// `startedAt` (which carries `created_at` for session rows).
    public let updatedAt: Date?

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

        /// The `Route` this navigation is a request to push, or `nil` for
        /// `.none` (nothing to navigate to). Lifted out of
        /// `ActivityScreen.handleSelect` (flows-composition Task 3) so the
        /// command palette's run search reuses the exact same
        /// kind-discrimination rules a table-row click already uses, rather
        /// than a second copy of this switch living in the view layer.
        public var route: Route? {
            switch self {
            case .run(let id, let host):
                return .runDetail(id: id, host: host)
            case .session(let id):
                return .sessionDetail(id: id)
            case .agentRun(let id, let transcriptPath, let host):
                return .agentRunDetail(id: id, transcriptPath: transcriptPath, host: host)
            case .none:
                return nil
            }
        }
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
        inputTokens = r.usage.inputTokens
        outputTokens = r.usage.outputTokens
        cachedTokens = r.usage.cachedTokens
        turns = r.turns
        model = nil
        source = nil
        issueRef = nil
        worker = nil
        eventKind = nil
        detail = nil
        updatedAt = nil
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
        inputTokens = r.usage.inputTokens
        outputTokens = r.usage.outputTokens
        cachedTokens = r.usage.cachedTokens
        turns = r.turns
        model = nil
        source = r.source
        issueRef = nil
        worker = nil
        eventKind = nil
        detail = nil
        updatedAt = nil
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
        inputTokens = r.usage.inputTokens
        outputTokens = r.usage.outputTokens
        cachedTokens = r.usage.cachedTokens
        turns = r.turns
        model = nil
        source = nil
        issueRef = r.issueDisplayRef
        worker = r.workerName
        eventKind = r.kind
        detail = r.detail
        updatedAt = nil
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
        inputTokens = r.totalTokensIn
        outputTokens = r.totalTokensOut
        cachedTokens = r.totalTokensCached
        turns = UInt64(r.totalTurns)
        model = r.model
        source = nil
        issueRef = nil
        worker = nil
        eventKind = nil
        detail = nil
        updatedAt = Self.parseISO(r.updatedAt)
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
        startedAt: Date?, navigation: Navigation,
        inputTokens: UInt64? = nil, outputTokens: UInt64? = nil, cachedTokens: UInt64? = nil,
        turns: UInt64? = nil, model: String? = nil, source: String? = nil, issueRef: String? = nil,
        worker: String? = nil, eventKind: String? = nil, detail: String? = nil, updatedAt: Date? = nil
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
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cachedTokens = cachedTokens
        self.turns = turns
        self.model = model
        self.source = source
        self.issueRef = issueRef
        self.worker = worker
        self.eventKind = eventKind
        self.detail = detail
        self.updatedAt = updatedAt
    }

    /// Returns a copy with `status` (and, when provided, `durationMS`)
    /// replaced — the write side of an `ActivityDelta.statusPatch` applied
    /// directly to an already-merged row, without refetching it.
    /// `newDurationMS` only overrides when non-nil: none of
    /// `ActivityDelta.reduce`'s produced status patches currently carry a
    /// duration (see `LiveReducer.swift`'s doc comment), so a `nil` here
    /// keeps whatever REST-sourced duration the row already had rather than
    /// clobbering it. Every per-kind detail field is carried through
    /// unchanged — a live status patch never touches them.
    func patchingStatus(_ newStatus: ActivityStatus, durationMS newDurationMS: UInt64?) -> ActivityRow {
        ActivityRow(
            id: id, kind: kind, subject: subject, project: project, host: host,
            trigger: trigger, status: newStatus, durationMS: newDurationMS ?? durationMS,
            costUSD: costUSD, startedAt: startedAt, navigation: navigation,
            inputTokens: inputTokens, outputTokens: outputTokens, cachedTokens: cachedTokens,
            turns: turns, model: model, source: source, issueRef: issueRef,
            worker: worker, eventKind: eventKind, detail: detail, updatedAt: updatedAt
        )
    }

    /// Parses an RFC3339 timestamp with or without fractional seconds
    /// (server payloads use both — e.g. plain-second `started_at` fields
    /// vs. millisecond-precision transcript/event timestamps): tries the
    /// fractional-seconds form first, falling back to the plain one.
    ///
    /// Delegates to `RupuAPI.ISO8601Parsing` — the shared, allocation-free (`static let`
    /// `Date.ISO8601FormatStyle`) home for this exact fractional-then-plain fallback, now reused
    /// by every site in the app that used to build its own fresh, per-call
    /// `ISO8601DateFormatter` pair (see that type's doc comment for the full list and rationale).
    public static func parseISO(_ s: String?) -> Date? {
        ISO8601Parsing.parse(s)
    }
}
