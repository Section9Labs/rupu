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
    /// an omitted `host`. `since`/`until` (perf & interaction arc, Plan 5
    /// Task 5) are RFC-3339 strings, narrowing the server-side date range
    /// applied BEFORE offset/limit paging — `nil` omits the corresponding
    /// query param entirely (no filter), matching `usage(since:until:
    /// groupBy:host:)`'s existing convention for the same shape.
    public func workflowRuns(
        offset: Int, limit: Int, host: String? = nil, since: String? = nil, until: String? = nil
    ) async throws -> [APIRunListRow] {
        try await get(
            "api/runs/workflows",
            query: offsetLimitQuery(offset: offset, limit: limit, host: host, since: since, until: until)
        )
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`, and `workflowRuns(offset:limit:host:since:until:)`'s
    /// on `since`/`until`.
    public func agentRuns(
        offset: Int, limit: Int, host: String? = nil, since: String? = nil, until: String? = nil
    ) async throws -> [APIAgentRunRow] {
        try await get(
            "api/runs/agents",
            query: offsetLimitQuery(offset: offset, limit: limit, host: host, since: since, until: until)
        )
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`, and `workflowRuns(offset:limit:host:since:until:)`'s
    /// on `since`/`until`.
    public func autoflowEvents(
        offset: Int, limit: Int, host: String? = nil, since: String? = nil, until: String? = nil
    ) async throws -> [APIAutoflowEventRow] {
        try await get(
            "api/runs/autoflows/events",
            query: offsetLimitQuery(offset: offset, limit: limit, host: host, since: since, until: until)
        )
    }

    /// `GET /api/runs/autoflows` — autoflow-worker CYCLES (one row per batch
    /// tick), not events. Named `autoflowCycles`, not `autoflowRuns` — the
    /// web's own `api.ts` calls this `getAutoflowRuns` (matching the Rust
    /// route's `/api/runs/autoflows` path literally), but this app already
    /// uses "Runs" for the EVENTS sub-tab (`AutoflowsSubTab.runs`,
    /// `autoflowEvents` above) — reusing that name here for a different
    /// endpoint returning a different row shape would be a footgun for the
    /// next reader, not a fidelity requirement (the wire path/row shape is
    /// what has to match the server, not this client's internal method
    /// name). See `runs(offset:limit:host:)`'s doc comment on the fan-out
    /// cost of an omitted `host`; host-aware exactly like the other list
    /// routes above (verified against `crates/rupu-cp/src/api/
    /// run_streams.rs`'s top-of-file fan-out note).
    public func autoflowCycles(
        offset: Int, limit: Int, host: String? = nil, since: String? = nil, until: String? = nil
    ) async throws -> [APIAutoflowCycleRow] {
        try await get(
            "api/runs/autoflows",
            query: offsetLimitQuery(offset: offset, limit: limit, host: host, since: since, until: until)
        )
    }

    /// See `runs(offset:limit:host:)`'s doc comment on the fan-out cost of
    /// an omitted `host`, and `workflowRuns(offset:limit:host:since:until:)`'s
    /// on `since`/`until`.
    public func sessions(
        offset: Int, limit: Int, host: String? = nil, since: String? = nil, until: String? = nil
    ) async throws -> [APISessionRow] {
        try await get(
            "api/sessions",
            query: offsetLimitQuery(offset: offset, limit: limit, host: host, since: since, until: until)
        )
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
    ///
    /// **No `host` param — genuinely local-only, not a documentation gap to
    /// close later.** Verified against `list_projects` in `crates/rupu-cp/
    /// src/api/projects.rs`: it has no `Query` extractor at all and reads
    /// straight from `s.run_store`/this CP's own `WorkspaceStore` — a
    /// "project" is a workspace REGISTERED ON THIS CP instance, not a
    /// per-Fleet-host concept the server ever fans out across (unlike
    /// `runs`/`dashboard`/`usage`, which proxy to remote hosts).
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
    public func transcript(path: String, host: String? = nil, run: String? = nil) async throws -> APITranscriptPage {
        var query = [URLQueryItem(name: "path", value: path)]
        query.append(contentsOf: hostQuery(host))
        if let run { query.append(URLQueryItem(name: "run", value: run)) }
        return try await get("api/transcript", query: query)
    }

    public func runNetflow(id: String) async throws -> APINetflow {
        try await get("api/runs/\(id)/netflow")
    }

    /// `GET /api/runs/:id/source?path=&line=&context=[&host=]` — a windowed
    /// slice of source lines centered on `line`, for the run detail source
    /// preview panel. `line` defaults server-side to `1`; `context` defaults
    /// server-side to `20` and is clamped to `[0, 200]` — see `SourceQuery`
    /// (`crates/rupu-cp/src/api/source.rs`). This client always sends both
    /// explicitly rather than relying on the server default, so callers get
    /// deterministic behavior across server versions. An explicit `host`
    /// other than `"local"` always soft-fails (`available: false`) — this
    /// endpoint never proxies to a remote host.
    public func runSource(id: String, path: String, line: Int, context: Int, host: String? = nil) async throws -> APISourceSlice {
        var query = [
            URLQueryItem(name: "path", value: path),
            URLQueryItem(name: "line", value: String(line)),
            URLQueryItem(name: "context", value: String(context)),
        ]
        query.append(contentsOf: hostQuery(host))
        return try await get("api/runs/\(id)/source", query: query)
    }

    /// `GET /api/runs/:id/ast?path=&line=&col=[&host=]` — a bounded
    /// tree-sitter subtree around the 1-based `(line, col)` target, for the
    /// run detail CST viewer. `line`/`col` both default server-side to `1`
    /// (`AstQuery`, same file as `SourceQuery`); sent explicitly here for
    /// the same determinism reason as `runSource`. Same remote-host
    /// soft-fail policy as `runSource`.
    public func runAst(id: String, path: String, line: Int, col: Int, host: String? = nil) async throws -> APIAstResponse {
        var query = [
            URLQueryItem(name: "path", value: path),
            URLQueryItem(name: "line", value: String(line)),
            URLQueryItem(name: "col", value: String(col)),
        ]
        query.append(contentsOf: hostQuery(host))
        return try await get("api/runs/\(id)/ast", query: query)
    }

    public func runFindings(id: String) async throws -> APIFindings {
        try await get("api/findings", query: [URLQueryItem(name: "run_id", value: id)])
    }

    /// `GET /api/findings[?ws_id=<id>]` — every finding scoped to one
    /// project, across every run that ever declared one (not just
    /// `recentRuns`), OR — with `wsID` omitted (Phase 5B's global Findings
    /// view) — every finding across every registered workspace, unfiltered.
    /// Phase 5A's Projects Findings tab: the smallest honest addition over
    /// `runFindings(id:)` above — the server's `FindingsQuery` (`crates/
    /// rupu-cp/src/api/findings.rs`) already accepts `ws_id` as an
    /// independent filter from `run_id`, so this is a second thin method
    /// rather than overloading `runFindings`'s `id` parameter to mean two
    /// different query keys depending on caller.
    ///
    /// **No `host` param — genuinely local-only, not a documentation gap to
    /// close later.** Verified against `list_findings` (`findings.rs`):
    /// `FindingsQuery` has no `host` field and the handler enumerates every
    /// workspace REGISTERED ON THIS CP instance, never proxying to a remote
    /// Fleet host.
    public func findings(wsID: String? = nil) async throws -> APIFindings {
        var query: [URLQueryItem] = []
        if let wsID {
            query.append(URLQueryItem(name: "ws_id", value: wsID))
        }
        return try await get("api/findings", query: query)
    }

    public func sessionDetail(id: String) async throws -> APISessionRow {
        try await get("api/sessions/\(id)")
    }

    public func sessionRuns(id: String) async throws -> [APISessionRunRow] {
        try await get("api/sessions/\(id)/runs")
    }

    // MARK: - Definitions (read)

    /// **No `host` param — genuinely local-only, not a documentation gap to
    /// close later.** Verified against `list_agents` (`crates/rupu-cp/src/
    /// api/agents.rs`): no `Query` extractor at all, reads `.md` definitions
    /// straight off `s.global_dir`/this CP's own `WorkspaceStore`. Every
    /// other `host`-bearing route in this file's doc comments notes the
    /// same "host-unaware by design" rationale the Rust source itself
    /// states for the sibling `resolve_agent_scoped` — definitions live on
    /// THIS CP's filesystem, never proxied to a remote Fleet host.
    public func agentDefinitions() async throws -> [AgentDefinition] {
        try await get("api/agents")
    }

    /// **No `host` param — genuinely local-only, not a documentation gap to
    /// close later.** Same rationale as `agentDefinitions()` — verified
    /// against `list_workflows` (`crates/rupu-cp/src/api/workflows.rs`): no
    /// `Query` extractor, local-filesystem read only.
    public func workflowDefinitions() async throws -> [WorkflowDefinition] {
        try await get("api/workflows")
    }

    public func workflowDetail(name: String) async throws -> WorkflowDetail {
        try await get("api/workflows/\(name)")
    }

    /// `PUT /api/workflows/:name` — persists the Workflow Builder's
    /// canonical YAML for a workflow (`RupuBuilder.BuilderStore.save()`,
    /// Task 9). Synchronous server-side (200 = written to disk) — unlike
    /// the run-control routes, there is no marker/resume-worker involved,
    /// so a caller's confirm-on-response is honest here (same as
    /// `putConfigGlobal`/`putConfigProject`/`putConfigPolicy`'s precedent).
    /// `scopeKind`/`scopeID` are the same optional `?scope_kind=&scope_id=`
    /// pinning pair `DELETE /api/workflows/:name`/`setAutoflowEnabled`
    /// accept — see `scopeQuery(scopeKind:scopeID:)`'s doc comment.
    public func writeWorkflow(
        name: String,
        body: WorkflowWriteBody,
        scopeKind: String? = nil,
        scopeID: String? = nil
    ) async throws {
        try await put("api/workflows/\(name)", query: scopeQuery(scopeKind: scopeKind, scopeID: scopeID), body: body)
    }

    /// `GET /api/agents/:name` — the Library screen's Agent detail view
    /// (Phase 5A, Task 7): every `AgentDefinition` field plus `systemPrompt`/
    /// `raw` (the full `.md` source, for the mono-block source view). No
    /// scope-pinning query params — the server accepts none on this route
    /// (unlike `DELETE`/`setAutoflowEnabled`'s `?scope_kind=&scope_id=`;
    /// confirmed by reading `get_agent` in `crates/rupu-cp/src/api/
    /// agents.rs`, which takes only the path `:name`), so a name that
    /// exists in two scopes resolves via the server's own project-first
    /// `load_detail` fallback rather than a client-side pin.
    public func agentDetail(name: String) async throws -> AgentDetail {
        try await get("api/agents/\(name)")
    }

    public func tools() async throws -> [ToolSpec] {
        let response: ToolsListResponse = try await get("api/tools")
        return response.tools
    }

    /// `GET /api/autoflows` — every workflow definition carrying a top-level
    /// `autoflow:` block, enabled and disabled alike (global layer merged
    /// with every distinct registered repo's `.rupu/workflows/`; project
    /// shadows global by name).
    ///
    /// **No `host` param — genuinely local-only, not a documentation gap to
    /// close later.** Same rationale as `agentDefinitions()`/
    /// `workflowDefinitions()` — `list_autoflows` (`crates/rupu-cp/src/api/
    /// autoflows.rs`) is explicitly documented there as "Local-only, no
    /// `?host=`: unlike the run-launch endpoints, this never proxies to a
    /// remote host."
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

    // MARK: - Fleet (write)

    /// `DELETE /api/hosts/:id` — remove a registered host (Phase 5A, Task 6).
    /// 204 No Content on success (no response body — see `delete(_:query:)`'s
    /// doc comment on why this has no `T: Decodable` return), 400 when `id`
    /// is `"local"` (`crates/rupu-cp/src/api/hosts.rs::remove_host` rejects
    /// removing the built-in local host outright — `FleetStore`/
    /// `FleetScreen` never offer the control for it, but the server is the
    /// actual enforcement point).
    public func removeHost(id: String) async throws {
        try await delete("api/hosts/\(id)")
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

    // MARK: - Project code (read)

    /// `GET /api/projects/:ws_id/tree?path=` — the immediate children of one
    /// workspace-relative directory (`path` defaults server-side to `""`,
    /// the workspace root — see `TreeQuery`, `crates/rupu-cp/src/api/
    /// code.rs`). Local-only, same rationale as every other
    /// `/api/projects/:ws_id/...` route — no `host` parameter.
    public func projectTree(wsID: String, path: String) async throws -> APITreeResult {
        try await get("api/projects/\(wsID)/tree", query: [URLQueryItem(name: "path", value: path)])
    }

    /// `GET /api/projects/:ws_id/source?path=` — one workspace file, read
    /// whole (up to 2 MiB; see `FileContent`'s doc comment on the Rust
    /// side). Same `TreeQuery` shape (just `path`) as `projectTree`. Local-
    /// only, same as `projectTree`.
    public func projectFile(wsID: String, path: String) async throws -> APIFileContent {
        try await get("api/projects/\(wsID)/source", query: [URLQueryItem(name: "path", value: path)])
    }

    /// `GET /api/projects/:ws_id/files` — every workspace-relative file path
    /// (capped at 20,000, `truncated: true` past the cap), for the
    /// project-wide file search box. Local-only, same as `projectTree`.
    public func projectFiles(wsID: String) async throws -> APIFileList {
        try await get("api/projects/\(wsID)/files")
    }

    // MARK: - Coverage (read)

    /// `GET /api/coverage` — every coverage target's rollup, aggregated
    /// across every registered workspace (the firehose view, not scoped to
    /// the CP's own launch dir). Local-only: no `host` fan-out — verified
    /// against `list_coverage` (`crates/rupu-cp/src/api/coverage.rs`), which
    /// has no `Query` extractor at all. Not a documentation gap to close
    /// later: coverage targets are discovered by walking each registered
    /// workspace's own filesystem path, never proxied to a remote host.
    public func coverage() async throws -> [APICoverageSummary] {
        try await get("api/coverage")
    }

    /// `GET /api/coverage/:target[?ws_id=]` — one target's full detail
    /// (assertions/findings/per-file heatmap). `wsID` disambiguates a
    /// `target` id that collides across workspaces — omit only for
    /// hand-typed URLs, where the server best-effort scans every registered
    /// workspace for the first match.
    public func coverageDetail(target: String, wsID: String? = nil) async throws -> APICoverageDetail {
        try await get("api/coverage/\(target)", query: wsIDQuery(wsID))
    }

    /// `GET /api/coverage/:target/catalog[?ws_id=]` — the flattened concern
    /// catalog effective for `target`. A SEPARATE route (and Rust type, no
    /// relation to `CoverageSummary`/the `get_coverage` detail shape) from
    /// `coverageDetail(target:wsID:)` — same `wsID` disambiguation rule.
    public func coverageCatalog(target: String, wsID: String? = nil) async throws -> APICoverageCatalog {
        try await get("api/coverage/\(target)/catalog", query: wsIDQuery(wsID))
    }

    // MARK: - Usage (read)

    /// `GET /api/usage[?since=&until=&group_by=&host=]` — fleet-wide token +
    /// cost overview (summary + breakdown). `since`/`until` are RFC-3339
    /// timestamps; omitted, the server defaults to the trailing 30 days.
    /// `groupBy` is one of `"provider"` | `"model"` | `"agent"` |
    /// `"workflow"` | `"host"` | `"project"`; omitted, the server groups by
    /// `"model"`. `host` (default `nil`) scopes to a single host, same
    /// param/semantics as `runs(offset:limit:host:)` — see that method's doc
    /// comment on the fan-out cost of an omitted `host` (confirmed present on
    /// this route by `UsageQuery.host` in `crates/rupu-cp/src/api/usage.rs`).
    /// Callers that want progressive per-host loading (`UsageStore`) always
    /// pass an explicit `host`, never omit it.
    public func usage(since: String? = nil, until: String? = nil, groupBy: String? = nil, host: String? = nil) async throws -> APIUsageResponse {
        var query = sinceUntilQuery(since: since, until: until)
        if let groupBy {
            query.append(URLQueryItem(name: "group_by", value: groupBy))
        }
        query.append(contentsOf: hostQuery(host))
        return try await get("api/usage", query: query)
    }

    /// `GET /api/usage/runs[?since=&until=&workspace_id=]` — flat
    /// per-`(run × model)` usage rows, local-only (no host fan-out) — see
    /// `usage(since:until:groupBy:)`'s doc comment on the default window.
    public func usageRuns(since: String? = nil, until: String? = nil, wsID: String? = nil) async throws -> [APIUsageRunRow] {
        var query = sinceUntilQuery(since: since, until: until)
        if let wsID {
            query.append(URLQueryItem(name: "workspace_id", value: wsID))
        }
        return try await get("api/usage/runs", query: query)
    }

    /// `GET /api/usage/outliers[?since=&until=]` — runs costing far more
    /// than their OWN workflow's median baseline, local-only (no host
    /// fan-out) — see `usage(since:until:groupBy:)`'s doc comment on the
    /// default window.
    public func usageOutliers(since: String? = nil, until: String? = nil) async throws -> [APIOutlierRun] {
        try await get("api/usage/outliers", query: sinceUntilQuery(since: since, until: until))
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

    // MARK: - Autoflow claims (read/write)

    /// `GET /api/autoflows/claims` — every tracked autoflow issue claim.
    /// Local-only, like `workers()` (`crates/rupu-cp/src/api/run_streams.rs`
    /// notes claims stay local-only, unlike the `/api/runs*` firehose routes)
    /// — no `host` fan-out.
    public func autoflowClaims() async throws -> [APIClaimRow] {
        try await get("api/autoflows/claims")
    }

    /// `POST /api/autoflows/claims/release` — release (delete) the tracked
    /// claim for `issueRef`. **Idempotent**: releasing an untracked issue
    /// still returns `200` with `released: false`, not a 404.
    public func releaseClaim(issueRef: String) async throws -> Bool {
        let response: ReleaseClaimResponse = try await post(
            "api/autoflows/claims/release", body: ReleaseClaimBody(issueRef: issueRef)
        )
        return response.released
    }

    /// `POST /api/autoflows/claims/requeue` — enqueue a manual wake for the
    /// issue behind `issueRef`, reusing its claim's `repo_ref`. `404` when
    /// no claim is tracked for the ref. The server's `RequeueBody` also
    /// accepts an optional `not_before` to defer the wake; there is no UI
    /// surface for that deferral, so this always omits it and the server
    /// defaults to "now" — see `build_manual_wake` (`crates/rupu-cp/src/api/
    /// autoflow_claims.rs`). The response's `wake_id` isn't useful to any
    /// caller today, so this returns `Void` rather than threading it through.
    public func requeueClaim(issueRef: String) async throws {
        let _: RequeueClaimResponse = try await post(
            "api/autoflows/claims/requeue", body: RequeueClaimBody(issueRef: issueRef)
        )
    }

    // MARK: - Config (read/write)

    /// `GET /api/config[?project=<ws_id>]` — effective resolved config, per-
    /// key provenance, raw global/project TOML text, and CP runtime status.
    /// `project` (default `nil`) omits the `?project=` param entirely,
    /// mirroring the server's own `ProjectQuery { project: Option<String> }`
    /// (`crates/rupu-cp/src/api/config.rs`).
    public func fetchConfig(project: String? = nil) async throws -> APIConfigView {
        var query: [URLQueryItem] = []
        if let project {
            query.append(URLQueryItem(name: "project", value: project))
        }
        return try await get("api/config", query: query)
    }

    /// `PUT /api/config/global` — persist a raw TOML edit to the global
    /// layer. 501 (see `put(_:body:)`'s doc comment) when this `cp serve`
    /// has no launcher installed (`require_writable` in `crates/rupu-cp/src/
    /// api/config.rs`) — Task 4 maps that status to the app's read-only
    /// mode. The success body is a bare `{"ok": true, "restart_required":
    /// []}` acknowledgement no caller needs, so this returns `Void`.
    public func putConfigGlobal(raw: String) async throws {
        try await put("api/config/global", body: ConfigRawWriteBody(raw: raw))
    }

    /// `PUT /api/config/project/:id` — persist a raw TOML edit to one
    /// project's `.rupu/config.toml`. Same 501-without-launcher gate and
    /// `Void` return as `putConfigGlobal`; additionally 400s when the edit
    /// would set a key enforced by the GLOBAL `[policy].lock` list
    /// (`reject_locked_project_keys`).
    public func putConfigProject(id: String, raw: String) async throws {
        try await put("api/config/project/\(id)", body: ConfigRawWriteBody(raw: raw))
    }

    /// `PUT /api/config/policy` — set the GLOBAL `[policy].lock` enforced-key
    /// list. Same 501-without-launcher gate and `Void` return as
    /// `putConfigGlobal`.
    public func putConfigPolicy(lock: [String]) async throws {
        try await put("api/config/policy", body: ConfigPolicyWriteBody(lock: lock))
    }

    // MARK: - Query helpers

    private func offsetLimitQuery(
        offset: Int, limit: Int, host: String? = nil, since: String? = nil, until: String? = nil
    ) -> [URLQueryItem] {
        var items = [
            URLQueryItem(name: "offset", value: String(offset)),
            URLQueryItem(name: "limit", value: String(limit)),
        ]
        items.append(contentsOf: hostQuery(host))
        items.append(contentsOf: sinceUntilQuery(since: since, until: until))
        return items
    }

    private func hostQuery(_ host: String?) -> [URLQueryItem] {
        guard let host else { return [] }
        return [URLQueryItem(name: "host", value: host)]
    }

    /// `?ws_id=` when present, no query items when `nil` — shared by
    /// `coverageDetail`/`coverageCatalog`.
    private func wsIDQuery(_ wsID: String?) -> [URLQueryItem] {
        guard let wsID else { return [] }
        return [URLQueryItem(name: "ws_id", value: wsID)]
    }

    /// `?since=&until=` — either or both present when set, no query items
    /// when both `nil`. Shared by every `/api/usage*` route.
    private func sinceUntilQuery(since: String?, until: String?) -> [URLQueryItem] {
        var items: [URLQueryItem] = []
        if let since {
            items.append(URLQueryItem(name: "since", value: since))
        }
        if let until {
            items.append(URLQueryItem(name: "until", value: until))
        }
        return items
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

    /// No decode step, unlike `get`/`post` — every route this hits today
    /// (`removeHost`, `DELETE /api/hosts/:id`) responds `204 No Content`
    /// with an empty body, so there is nothing for a `T: Decodable` to
    /// decode; a generic `delete<T>` would just make every call site thread
    /// a phantom response type through for nothing. Same error mapping as
    /// `get`/`post` (`mapErrorStatus`) — a non-2xx still throws.
    func delete(_ path: String, query: [URLQueryItem] = []) async throws {
        let url = try buildURL(path: path, query: query)
        var request = authorizedRequest(url: url)
        request.httpMethod = "DELETE"
        let (data, response) = try await perform(request)
        try mapErrorStatus(response, data: data, url: url)
    }

    /// `PUT` with a required JSON body and no decode step — used by the
    /// config write routes (`putConfigGlobal`/`putConfigProject`/
    /// `putConfigPolicy`) and `writeWorkflow`, whose success body is a bare
    /// acknowledgement object no caller needs (same rationale as `delete`'s
    /// doc comment for skipping a `T: Decodable` return). `query` defaults
    /// to empty — the config routes never pass one; `writeWorkflow` is the
    /// first PUT caller that needs the `?scope_kind=&scope_id=` pinning
    /// pair, same as several `post`/`delete` routes already do. Same error
    /// mapping as `get`/`post`/`delete` (`mapErrorStatus`) — a non-2xx still
    /// throws `CPError.http`, so a 501 (this deployment has no
    /// `RunLauncher` installed — see `require_writable` in
    /// `crates/rupu-cp/src/api/config.rs`) surfaces as `CPError.http(status:
    /// 501, ...)` exactly the way every other write route's 501 already
    /// does, distinguishable by callers matching on `.status` (Task 4 maps
    /// it to read-only mode).
    func put<B: Encodable>(_ path: String, query: [URLQueryItem] = [], body: B) async throws {
        let url = try buildURL(path: path, query: query)
        var request = authorizedRequest(url: url)
        request.httpMethod = "PUT"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await perform(request)
        try mapErrorStatus(response, data: data, url: url)
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
