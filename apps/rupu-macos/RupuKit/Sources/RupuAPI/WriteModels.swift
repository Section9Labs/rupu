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

    public init(
        prompt: String? = nil,
        mode: String? = nil,
        target: String? = nil,
        workingDir: String? = nil,
        host: String? = nil
    ) {
        self.prompt = prompt
        self.mode = mode
        self.target = target
        self.workingDir = workingDir
        self.host = host
    }

    private enum CodingKeys: String, CodingKey {
        case prompt
        case mode
        case target
        case workingDir = "working_dir"
        case host
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

    public init(
        inputs: [String: String] = [:],
        mode: String? = nil,
        target: String? = nil,
        workingDir: String? = nil,
        host: String? = nil
    ) {
        self.inputs = inputs
        self.mode = mode
        self.target = target
        self.workingDir = workingDir
        self.host = host
    }

    private enum CodingKeys: String, CodingKey {
        case inputs
        case mode
        case target
        case workingDir = "working_dir"
        case host
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
