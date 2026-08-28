import Foundation

/// Response shared by every launch-style write route (`agents/:name/run`,
/// `agents/:name/session`, `workflows/:name/run`, `sessions/:id/send`).
/// The server returns one of three shapes depending on route and whether the
/// call was proxied to a remote host: `{run_id, host_id}`,
/// `{session_id, host_id}`, or (remote proxy) `{ok, host_id}` — all three
/// decode into this one struct, unpopulated fields simply nil.
public struct LaunchResponse: Decodable, Equatable, Sendable {
    public let runID: String?
    public let sessionID: String?
    public let ok: Bool?
    public let hostID: String

    public init(runID: String?, sessionID: String?, ok: Bool?, hostID: String) {
        self.runID = runID
        self.sessionID = sessionID
        self.ok = ok
        self.hostID = hostID
    }

    private enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case sessionID = "session_id"
        case ok
        case hostID = "host_id"
    }
}

/// Response shared by every run-control route (approve/reject/cancel/pause/
/// resume/archive/restore). The local shape carries the full updated
/// `RunRecord` under `run` (plus `steps`/`usage`, ignored here); the remote
/// proxy shape is `{ok, host_id}`, and archive/restore add `{id, archived}`.
/// One struct decodes all of them, every field optional.
///
/// Approve is **marker-only** — the record's `status` in this response does
/// NOT reflect the approval; a `cp serve` background worker resumes the run
/// separately. `confirmedStatus` surfaces `run?.status` as-is for callers
/// that want to observe it anyway (e.g. immediate routes like reject/cancel,
/// where the status genuinely does flip in the response).
public struct RunControlResponse: Decodable, Sendable {
    public let run: APIRunRecord?
    public let ok: Bool?
    public let hostID: String?
    public let archived: Bool?

    public var confirmedStatus: String? { run?.status }

    public init(run: APIRunRecord?, ok: Bool?, hostID: String?, archived: Bool?) {
        self.run = run
        self.ok = ok
        self.hostID = hostID
        self.archived = archived
    }

    private enum CodingKeys: String, CodingKey {
        case run
        case ok
        case hostID = "host_id"
        case archived
    }
}

/// Response of `POST /api/autoflows/:name/enable` or `.../disable`
/// (`SetEnabledResponse` on the Rust side) — `{name, enabled}`. Same
/// **immediate** contract as reject/cancel (see `RunControlResponse`'s doc
/// comment on marker-only vs. immediate): this rewrites the on-disk YAML
/// directly, no background worker involved, so `enabled` in the response is
/// the definition's actual new state.
public struct AutoflowSetEnabledResponse: Decodable, Equatable, Sendable {
    public let name: String
    public let enabled: Bool

    public init(name: String, enabled: Bool) {
        self.name = name
        self.enabled = enabled
    }

    private enum CodingKeys: String, CodingKey {
        case name
        case enabled
    }
}

// MARK: - Request bodies

/// `POST /api/runs/:id/approve` body. Bodyless requests are valid (server
/// defaults `mode` from the run's own permission mode) — `CPClient.post`'s
/// `body: B?` accepts `nil` for that case.
public struct ApproveBody: Encodable, Equatable, Sendable {
    public let mode: String?

    public init(mode: String? = nil) {
        self.mode = mode
    }

    private enum CodingKeys: String, CodingKey {
        case mode
    }
}

/// `POST /api/runs/:id/reject` body. Unlike approve, the server requires the
/// JSON object itself to be present even with no reason — callers should
/// always pass a (possibly empty) `RejectBody`, never a `nil` body.
public struct RejectBody: Encodable, Equatable, Sendable {
    public let reason: String?

    public init(reason: String? = nil) {
        self.reason = reason
    }

    private enum CodingKeys: String, CodingKey {
        case reason
    }
}

/// `POST /api/runs/:id/cancel` body — optional; `nil` sends no body.
public struct CancelBody: Encodable, Equatable, Sendable {
    public let reason: String?

    public init(reason: String? = nil) {
        self.reason = reason
    }

    private enum CodingKeys: String, CodingKey {
        case reason
    }
}

/// `POST /api/agents/:name/run` and `POST /api/agents/:name/session` body —
/// identical shape for both routes, every field optional. `host` is a BODY
/// field here (unlike the run-control routes, where it's a query param).
public struct AgentLaunchBody: Encodable, Equatable, Sendable {
    public let prompt: String?
    public let mode: String?
    public let target: String?
    public let workingDir: String?
    public let host: String?
    /// Scope-aware launch (Phase 5A): pins the launch to the definition the
    /// picker showed. Server contract: LOCAL launches only (the server
    /// ignores scope on remote-targeted launches — never send it with a
    /// non-local `host`), and mutually exclusive with `workingDir` (400).
    /// Threading from the picker's selection is LauncherStore's job
    /// (Phase 5A Task 4); the fields exist here so the request fixture
    /// round-trips.
    public let scopeKind: String?
    public let scopeID: String?

    public init(
        prompt: String? = nil,
        mode: String? = nil,
        target: String? = nil,
        workingDir: String? = nil,
        host: String? = nil,
        scopeKind: String? = nil,
        scopeID: String? = nil
    ) {
        self.prompt = prompt
        self.mode = mode
        self.target = target
        self.workingDir = workingDir
        self.host = host
        self.scopeKind = scopeKind
        self.scopeID = scopeID
    }

    private enum CodingKeys: String, CodingKey {
        case prompt
        case mode
        case target
        case workingDir = "working_dir"
        case host
        case scopeKind = "scope_kind"
        case scopeID = "scope_id"
    }
}

/// `POST /api/workflows/:name/run` body. `inputs` defaults to `{}` and is
/// always encoded (not optional); `host` is a BODY field here, same as
/// `AgentLaunchBody`.
public struct WorkflowLaunchBody: Encodable, Equatable, Sendable {
    public let inputs: [String: String]
    public let mode: String?
    public let target: String?
    public let workingDir: String?
    public let host: String?
    /// Scope-aware launch (Phase 5A): same contract as
    /// `AgentLaunchBody.scopeKind`/`scopeID` — pins the launch to the
    /// definition the picker showed. LOCAL launches only (never send with a
    /// non-local `host`), mutually exclusive with `workingDir` (400).
    /// Threading from the picker's selection is `LauncherStore`'s job
    /// (Phase 5A Task 4); the fields exist here so the request fixture
    /// round-trips.
    public let scopeKind: String?
    public let scopeID: String?

    public init(
        inputs: [String: String] = [:],
        mode: String? = nil,
        target: String? = nil,
        workingDir: String? = nil,
        host: String? = nil,
        scopeKind: String? = nil,
        scopeID: String? = nil
    ) {
        self.inputs = inputs
        self.mode = mode
        self.target = target
        self.workingDir = workingDir
        self.host = host
        self.scopeKind = scopeKind
        self.scopeID = scopeID
    }

    private enum CodingKeys: String, CodingKey {
        case inputs
        case mode
        case target
        case workingDir = "working_dir"
        case host
        case scopeKind = "scope_kind"
        case scopeID = "scope_id"
    }
}

/// `POST /api/workflows/validate` body.
public struct ValidateBody: Encodable, Equatable, Sendable {
    public let raw: String

    public init(raw: String) {
        self.raw = raw
    }

    private enum CodingKeys: String, CodingKey {
        case raw
    }
}

/// `PUT /api/workflows/:name` body — the Workflow Builder's canonical YAML
/// (`RupuBuilder.BuilderStore.save()`, Task 9). Same one-field shape as
/// `ValidateBody` above (both routes take the whole document as raw text,
/// never a structured object), kept as its own type rather than reused
/// because the two routes are semantically distinct (validate-only vs.
/// persist) and Task 9's brief specifies this exact name.
public struct WorkflowWriteBody: Encodable, Equatable, Sendable {
    public let raw: String

    public init(raw: String) {
        self.raw = raw
    }

    private enum CodingKeys: String, CodingKey {
        case raw
    }
}

/// `POST /api/sessions/:id/send` body. `host` is a QUERY param on this
/// route (see `CPClient.sendToSession(id:host:body:)`), not part of this
/// body.
public struct SendBody: Encodable, Equatable, Sendable {
    public let prompt: String

    public init(prompt: String) {
        self.prompt = prompt
    }

    private enum CodingKeys: String, CodingKey {
        case prompt
    }
}
