import Foundation

/// Thin HTTP client for a `rupu cp serve` control-plane host.
public actor CPClient {
    let config: CPConfig
    let session: URLSession

    public init(config: CPConfig, session: URLSession = .shared) {
        self.config = config
        self.session = session
    }

    public func hostInfo() async throws -> HostInfo {
        try await get("api/host/info")
    }

    public func recentEvents(limit: Int = 200) async throws -> [CPEventRow] {
        try await get("api/events", query: [URLQueryItem(name: "limit", value: String(limit))])
    }

    /// `host` (default `nil`) sends `host=<value>` when set. With no `host`
    /// at all, the server fans this call out to *every* registered host
    /// (local + every Fleet node) sequentially server-side — fine for a
    /// single fast local backend, but a multi-host fleet with one slow or
    /// offline node turns a ~60ms call into several seconds (measured
    /// 2.5-4.0s against a fleet with one offline node vs 60-70ms with
    /// `host: "local"`). Callers that want progressive per-host loading
    /// (see `ActivityStore`) always pass an explicit `host`, never omit it.
    public func runs(offset: Int, limit: Int, host: String? = nil) async throws -> [APIRunListRow] {
        try await get("api/runs", query: offsetLimitQuery(offset: offset, limit: limit, host: host))
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`.
    public func workflowRuns(offset: Int, limit: Int, host: String? = nil) async throws -> [APIRunListRow] {
        try await get("api/runs/workflows", query: offsetLimitQuery(offset: offset, limit: limit, host: host))
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`.
    public func agentRuns(offset: Int, limit: Int, host: String? = nil) async throws -> [APIAgentRunRow] {
        try await get("api/runs/agents", query: offsetLimitQuery(offset: offset, limit: limit, host: host))
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`.
    public func autoflowEvents(offset: Int, limit: Int, host: String? = nil) async throws -> [APIAutoflowEventRow] {
        try await get("api/runs/autoflows/events", query: offsetLimitQuery(offset: offset, limit: limit, host: host))
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`.
    public func sessions(offset: Int, limit: Int, host: String? = nil) async throws -> [APISessionRow] {
        try await get("api/sessions", query: offsetLimitQuery(offset: offset, limit: limit, host: host))
    }

    /// `GET /api/hosts` — the registered fleet: `local` plus every attached
    /// Fleet node, each with a `status` ("online"/"offline"/...). Drives
    /// `ActivityStore`'s per-host progressive loading: only `status ==
    /// "online"` hosts other than `"local"` are worth fetching from at all.
    public func hosts() async throws -> [APIHostRow] {
        try await get("api/hosts")
    }

    /// `GET /api/projects` — every registered workspace, newest-activity
    /// first. Drives the v2 top bar's scope picker.
    public func projects() async throws -> [APIProjectRow] {
        try await get("api/projects")
    }

    /// `GET /api/dashboard` — the ops-first Overview: one fleet-wide
    /// aggregate plus per-host freshness. `range` is the wire vocabulary
    /// (`"7d"` | `"30d"` | `"all"`) always sent explicitly (there is no
    /// client-side default — the caller's segmented control drives it).
    /// `host` (default `nil`) scopes to a single host, same param as
    /// `runs(offset:limit:host:)`; omitting it fans the call out
    /// server-side across every registered host.
    public func dashboard(range: String, host: String? = nil) async throws -> APIDashboardResponse {
        var query = [URLQueryItem(name: "range", value: range)]
        query.append(contentsOf: hostQuery(host))
        return try await get("api/dashboard", query: query)
    }

    public func runDetail(id: String, host: String? = nil) async throws -> APIRunDetail {
        try await get("api/runs/\(id)", query: hostQuery(host))
    }

    public func runGraph(id: String, host: String? = nil) async throws -> APIRunGraph {
        try await get("api/runs/\(id)/graph", query: hostQuery(host))
    }

    /// `path` is the transcript file's full path (from a step result / run
    /// record / unit row / agent-run row), sent as a query parameter —
    /// `/api/transcript`, never a URL path segment.
    public func transcript(path: String, host: String? = nil) async throws -> APITranscriptPage {
        var query = [URLQueryItem(name: "path", value: path)]
        query.append(contentsOf: hostQuery(host))
        return try await get("api/transcript", query: query)
    }

    public func runNetflow(id: String) async throws -> APINetflow {
        try await get("api/runs/\(id)/netflow")
    }

    public func runFindings(id: String) async throws -> APIFindings {
        try await get("api/findings", query: [URLQueryItem(name: "run_id", value: id)])
    }

    public func sessionDetail(id: String) async throws -> APISessionRow {
        try await get("api/sessions/\(id)")
    }

    public func sessionRuns(id: String) async throws -> [APISessionRunRow] {
        try await get("api/sessions/\(id)/runs")
    }

    // MARK: - Definitions (read)

    public func agentDefinitions() async throws -> [AgentDefinition] {
        try await get("api/agents")
    }

    public func workflowDefinitions() async throws -> [WorkflowDefinition] {
        try await get("api/workflows")
    }

    public func workflowDetail(name: String) async throws -> WorkflowDetail {
        try await get("api/workflows/\(name)")
    }

    public func tools() async throws -> [ToolSpec] {
        let response: ToolsListResponse = try await get("api/tools")
        return response.tools
    }

    /// `GET /api/autoflows` — every workflow definition carrying a top-level
    /// `autoflow:` block, enabled and disabled alike (global layer merged
    /// with every distinct registered repo's `.rupu/workflows/`; project
    /// shadows global by name).
    public func autoflowDefinitions() async throws -> [AutoflowDefinition] {
        try await get("api/autoflows")
    }

    // MARK: - Fleet (read)

    /// `GET /api/workers` — every local execution identity (`rupu` CLI /
    /// `rupu cp serve` autoflow worker) enriched with run-activity
    /// attribution. Local-only: unlike `runs(offset:limit:host:)`, there is
    /// no `?host=` fan-out — a worker record only ever lives on the host
    /// that registered it.
    public func workers() async throws -> [APIWorkerRow] {
        try await get("api/workers")
    }

    // MARK: - Project detail (read)

    /// `GET /api/projects/:ws_id` — one project's rollup: identity, run/
    /// session/coverage counts, the 10 most recent runs, and a usage
    /// summary across every scoped run.
    public func projectDetail(wsID: String) async throws -> APIProjectDetail {
        try await get("api/projects/\(wsID)")
    }

    /// `GET /api/projects/:ws_id/runs` — scoped slim run list, newest-first.
    /// Local-only, same as every `/api/projects/:ws_id/...` route (a project
    /// lives on exactly one host's filesystem) — no `host` parameter here.
    public func projectRuns(wsID: String, offset: Int, limit: Int) async throws -> [APIRunListRow] {
        try await get("api/projects/\(wsID)/runs", query: offsetLimitQuery(offset: offset, limit: limit))
    }

    /// `GET /api/projects/:ws_id/sessions` — session rows scoped to the
    /// project. Same local-only rationale as `projectRuns`.
    public func projectSessions(wsID: String, offset: Int, limit: Int) async throws -> [APISessionRow] {
        try await get("api/projects/\(wsID)/sessions", query: offsetLimitQuery(offset: offset, limit: limit))
    }

    /// `GET /api/projects/:ws_id/agents` — global agents merged with the
    /// project's own `.rupu/agents/`; project shadows global by name. Same
    /// `AgentDto` shape `agentDefinitions()` decodes.
    public func projectAgents(wsID: String) async throws -> [AgentDefinition] {
        try await get("api/projects/\(wsID)/agents")
    }

    /// `GET /api/projects/:ws_id/workflows` — global workflows merged with
    /// the project's own `.rupu/workflows/`; project shadows global by name.
    /// Same `WorkflowDto` shape `workflowDefinitions()` decodes.
    public func projectWorkflows(wsID: String) async throws -> [WorkflowDefinition] {
        try await get("api/projects/\(wsID)/workflows")
    }

    /// `GET /api/projects/:ws_id/autoflows` — same merge/shadow rule as
    /// `projectWorkflows`, restricted to workflows carrying an `autoflow:`
    /// block.
    public func projectAutoflows(wsID: String) async throws -> [AutoflowDefinition] {
        try await get("api/projects/\(wsID)/autoflows")
    }

    // MARK: - Run control (write)

    /// `mode` in `body` is optional; a `nil` body is valid — the server
    /// falls back to the run's own permission mode. **Marker-only**: the
    /// response's `run.status` does NOT reflect the approval — a `cp serve`
    /// background worker resumes the run separately (see
    /// `RunControlResponse`'s doc comment).
    public func approveRun(id: String, host: String? = nil, gate: String, body: ApproveBody? = nil) async throws -> RunControlResponse {
        try await post("api/runs/\(id)/approve", query: runControlQuery(host: host, gate: gate), body: body)
    }

    /// The server requires the JSON body to be present (even empty) on
    /// reject — callers should pass `RejectBody()` rather than omitting it.
    /// **Immediate**: the response record reflects the rejection.
    public func rejectRun(id: String, host: String? = nil, gate: String, body: RejectBody = RejectBody()) async throws -> RunControlResponse {
        try await post("api/runs/\(id)/reject", query: runControlQuery(host: host, gate: gate), body: body)
    }

    /// **Immediate**: status flips in the response and a live runner pid is
    /// TERM'd. 409 `AlreadyTerminal` when the run has already finished.
    public func cancelRun(id: String, host: String? = nil, body: CancelBody? = nil) async throws -> RunControlResponse {
        try await post("api/runs/\(id)/cancel", query: writeHostQuery(host), body: body)
    }

    /// **Immediate** plus a marker for detached runners. 409 when the run
    /// isn't currently running.
    public func pauseRun(id: String, host: String? = nil) async throws -> RunControlResponse {
        try await post("api/runs/\(id)/pause", query: writeHostQuery(host), body: EmptyBody?.none)
    }

    /// **Marker-only** — a `cp serve` background worker performs the actual
    /// resume. 501 without launcher runtime configured; 409 when the run
    /// isn't currently `paused`.
    public func resumeRun(id: String, host: String? = nil) async throws -> RunControlResponse {
        try await post("api/runs/\(id)/resume", query: writeHostQuery(host), body: EmptyBody?.none)
    }

    /// Immediate filesystem move; response carries `archived: true`.
    public func archiveRun(id: String, host: String? = nil) async throws -> RunControlResponse {
        try await post("api/runs/\(id)/archive", query: writeHostQuery(host), body: EmptyBody?.none)
    }

    /// Immediate filesystem move; response carries `archived: false`.
    public func restoreRun(id: String, host: String? = nil) async throws -> RunControlResponse {
        try await post("api/runs/\(id)/restore", query: writeHostQuery(host), body: EmptyBody?.none)
    }

    // MARK: - Agent / workflow launch, session send (write)

    /// 501 without `agent_launcher` runtime configured. `host` is a BODY
    /// field on this route (see `AgentLaunchBody`), not a query param.
    public func launchAgentRun(name: String, body: AgentLaunchBody) async throws -> LaunchResponse {
        try await post("api/agents/\(name)/run", body: body)
    }

    /// Same body shape and `host` placement as `launchAgentRun`.
    public func startAgentSession(name: String, body: AgentLaunchBody) async throws -> LaunchResponse {
        try await post("api/agents/\(name)/session", body: body)
    }

    /// 501 without launcher runtime configured. `host` is a BODY field on
    /// this route (see `WorkflowLaunchBody`), not a query param.
    public func runWorkflow(name: String, body: WorkflowLaunchBody) async throws -> LaunchResponse {
        try await post("api/workflows/\(name)/run", body: body)
    }

    /// `true` on success (`{ok: true}`); a 400 (empty/whitespace-only
    /// prompt after trim) throws `CPError.http` whose `body` is a single
    /// validation-error message string.
    public func validateWorkflow(body: ValidateBody) async throws -> Bool {
        let response: OkResponse = try await post("api/workflows/validate", body: body)
        return response.ok
    }

    /// `host` is a QUERY param on this route (unlike the agent/workflow
    /// launch routes), same placement as the run-control routes.
    public func sendToSession(id: String, host: String? = nil, body: SendBody) async throws -> LaunchResponse {
        try await post("api/sessions/\(id)/send", query: writeHostQuery(host), body: body)
    }

    /// Phase 3, Task 6 addition (not in Task 2's original write surface —
    /// its brief only covered run archive/restore). `POST
    /// /api/sessions/:id/archive[?host=]`. Immediate filesystem move, same
    /// as `archiveRun`, but the **local** response shape here is `{ok:
    /// true, id}` — no `archived` field at all (verified against
    /// `mutate_session` in `crates/rupu-cp/src/api/sessions.rs`, unlike
    /// `archiveRun`/`restoreRun`'s local `RunRecord`-carrying response,
    /// which does carry one). A remote proxy adds `host_id`. Reuses
    /// `RunControlResponse` purely for its `ok`/`hostID` fields — callers
    /// must confirm off `response.ok`, never `response.archived` (always
    /// `nil` here).
    public func archiveSession(id: String, host: String? = nil) async throws -> RunControlResponse {
        try await post("api/sessions/\(id)/archive", query: writeHostQuery(host), body: EmptyBody?.none)
    }

    /// Symmetric with `archiveSession` above.
    public func restoreSession(id: String, host: String? = nil) async throws -> RunControlResponse {
        try await post("api/sessions/\(id)/restore", query: writeHostQuery(host), body: EmptyBody?.none)
    }

    // MARK: - Autoflow definitions (write)

    /// `POST /api/autoflows/:name/enable` or `.../disable`, chosen by
    /// `enabled` — flips `autoflow.enabled` in the on-disk workflow YAML.
    /// **Immediate**, not marker-only: the response reflects the file's
    /// actual new state (see `AutoflowSetEnabledResponse`'s doc comment).
    /// `scopeKind`/`scopeID` are the same optional `?scope_kind=&scope_id=`
    /// pinning pair `DELETE /api/agents/:name` / `DELETE /api/workflows/:name`
    /// accept — omit both to let the server resolve `:name` via its implicit
    /// project-first lookup; pass both to pin a specific repo when two
    /// different repos define the same autoflow name.
    public func setAutoflowEnabled(
        name: String,
        scopeKind: String? = nil,
        scopeID: String? = nil,
        enabled: Bool
    ) async throws -> AutoflowSetEnabledResponse {
        let path = enabled ? "api/autoflows/\(name)/enable" : "api/autoflows/\(name)/disable"
        return try await post(path, query: scopeQuery(scopeKind: scopeKind, scopeID: scopeID), body: EmptyBody?.none)
    }

    // MARK: - Query helpers

    private func offsetLimitQuery(offset: Int, limit: Int, host: String? = nil) -> [URLQueryItem] {
        var items = [
            URLQueryItem(name: "offset", value: String(offset)),
            URLQueryItem(name: "limit", value: String(limit)),
        ]
        items.append(contentsOf: hostQuery(host))
        return items
    }

    private func hostQuery(_ host: String?) -> [URLQueryItem] {
        guard let host else { return [] }
        return [URLQueryItem(name: "host", value: host)]
    }

    /// Same as `hostQuery`, except a literal `"local"` also omits the query
    /// param — the write routes' server-side default (no `host` = run
    /// locally) already matches what the UI means by `"local"`, so sending
    /// it explicitly would only add a needless proxy-routing branch
    /// server-side. Used by the run-control and session-send routes, whose
    /// `host` is a CPClient-level query parameter (as opposed to the agent/
    /// workflow launch routes, where `host` is a body field the caller sets
    /// directly with no such normalization — see `AgentLaunchBody`).
    private func writeHostQuery(_ host: String?) -> [URLQueryItem] {
        guard let host, host != "local" else { return [] }
        return [URLQueryItem(name: "host", value: host)]
    }

    private func runControlQuery(host: String?, gate: String) -> [URLQueryItem] {
        var items = writeHostQuery(host)
        items.append(URLQueryItem(name: "gate", value: gate))
        return items
    }

    /// `?scope_kind=&scope_id=` pinning pair shared by `setAutoflowEnabled`
    /// (see its doc comment) — either or both may be omitted.
    private func scopeQuery(scopeKind: String?, scopeID: String?) -> [URLQueryItem] {
        var items: [URLQueryItem] = []
        if let scopeKind {
            items.append(URLQueryItem(name: "scope_kind", value: scopeKind))
        }
        if let scopeID {
            items.append(URLQueryItem(name: "scope_id", value: scopeID))
        }
        return items
    }

    // MARK: - Transport

    private func buildURL(path: String, query: [URLQueryItem]) throws -> URL {
        guard var components = URLComponents(url: config.baseURL, resolvingAgainstBaseURL: false) else {
            throw CPError.transport("invalid base URL: \(config.baseURL)")
        }
        let basePath = components.path.hasSuffix("/") ? String(components.path.dropLast()) : components.path
        let relativePath = path.hasPrefix("/") ? path : "/\(path)"
        components.path = basePath + relativePath
        if !query.isEmpty {
            components.queryItems = query
        }
        guard let url = components.url else {
            throw CPError.transport("could not build URL for path: \(path)")
        }
        return url
    }

    private func authorizedRequest(url: URL) -> URLRequest {
        var request = URLRequest(url: url)
        if let token = config.token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        return request
    }

    private func perform(_ request: URLRequest) async throws -> (Data, URLResponse) {
        do {
            return try await session.data(for: request)
        } catch let error as CPError {
            throw error
        } catch is CancellationError {
            // Cancellation (e.g. a SwiftUI `.task(id:)` whose id just
            // changed) is a routine, expected way for an in-flight request
            // to end — never a transport failure. `CPError.cancelled` lets
            // every call site distinguish it from a real error and leave
            // its current state untouched rather than surfacing a "Retry"
            // failure box for something the user didn't cause and doesn't
            // need to act on.
            throw CPError.cancelled
        } catch let urlError as URLError where urlError.code == .cancelled {
            // `URLSession`'s async `data(for:)` wires a cancelled `Task`'s
            // continuation to `task.cancel()`, which surfaces here as
            // `URLError(.cancelled)` rather than Swift's own
            // `CancellationError` — same benign meaning, same mapping.
            throw CPError.cancelled
        } catch {
            throw CPError.transport(error.localizedDescription)
        }
    }

    /// Maps a non-2xx HTTP response to the appropriate `CPError`, shared by
    /// `get` and `post`. 501 (launcher runtime not configured on this host)
    /// falls through the same general non-2xx branch as every other error
    /// status — callers that care about it (stores special-casing the
    /// message for a "not supported here" banner) match on
    /// `CPError.http(status: 501, ...)` themselves.
    private func mapErrorStatus(_ response: URLResponse, data: Data, url: URL) throws {
        guard let httpResponse = response as? HTTPURLResponse else {
            throw CPError.transport("non-HTTP response for \(url)")
        }
        guard httpResponse.statusCode != 401 else {
            throw CPError.unauthorized
        }
        guard (200..<300).contains(httpResponse.statusCode) else {
            let body = String(data: data, encoding: .utf8) ?? ""
            throw CPError.http(status: httpResponse.statusCode, body: body)
        }
    }

    func get<T: Decodable>(_ path: String, query: [URLQueryItem] = []) async throws -> T {
        let url = try buildURL(path: path, query: query)
        let request = authorizedRequest(url: url)
        let (data, response) = try await perform(request)
        try mapErrorStatus(response, data: data, url: url)
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw CPError.decoding(String(describing: error))
        }
    }

    /// Content-Type `application/json` when `body` is non-`nil`; a `nil`
    /// body sends the POST with no request body at all. Same error mapping
    /// as `get` — see `mapErrorStatus`.
    func post<B: Encodable, T: Decodable>(_ path: String, query: [URLQueryItem] = [], body: B?) async throws -> T {
        let url = try buildURL(path: path, query: query)
        var request = authorizedRequest(url: url)
        request.httpMethod = "POST"
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }
        let (data, response) = try await perform(request)
        try mapErrorStatus(response, data: data, url: url)
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw CPError.decoding(String(describing: error))
        }
    }
}

/// Placeholder body type for write routes that take no request body at all
/// (pause/resume/archive/restore) — `post`'s `body: B?` parameter always
/// needs a concrete `B` to infer, and `nil` needs a type to be `nil` of.
private struct EmptyBody: Encodable {}

/// `{"ok": true}` — the success shape of `POST /api/workflows/validate`.
private struct OkResponse: Decodable {
    let ok: Bool
}
