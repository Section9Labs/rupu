# Operator MCP, Plan 1: Foundation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rupu cp serve` serves an MCP endpoint at `/mcp` (HTTP only). It has an operator tool layer that calls the CP's own endpoint code in process, a first set of operator tools that proves the path end to end, a `wait` tool for runs and messages, and the catalog tools (files, shell, SCM, findings) beside them. `rupu mcp serve` and stdio are retired.

**Architecture:**
- `rupu-mcp` becomes the protocol core. It owns JSON-RPC dispatch over a set of `ToolSource`s, the `--tools` filter and the catalog source. It gains no `rupu-cp` dependency.
- `rupu-cp` depends on `rupu-mcp` and adds:
  - the operator tool table and its source, which sends `http::Request`s through the CP's unauthenticated inner API router with `tower::ServiceExt::oneshot`
  - the `wait` source
  - the `/mcp` axum endpoint
- `rupu-cli` builds the catalog source's tool contexts (it holds the SCM registry and the assembler) and passes it in through `ServeOpts`.

**Tech Stack:**
- Rust (workspace MSRV), axum, tower 0.5 (`util`), tokio
- `schemars` 0.8 (already a workspace dep) for operator tool input schemas
- MCP streamable-HTTP transport

**Spec:** `docs/superpowers/specs/2026-10-19-rupu-operator-mcp-design.md`. This plan implements M1, M2, M4 (machinery + 13 tools), M5, M6, M7 (local runs and messages), and decision 7 for operator messages. It does not implement M3, because one process needs nothing new.

## Global Constraints

- **Full access, no guardrail layer.** No permission mode, admin tier or confirmation step for MCP calls. Catalog tools run under `PermissionPolicy::unattended(PermissionMode::Bypass)`.
- **HTTP only.** No stdio transport ships. `InProcessTransport` stays for tests.
- **Serve modes:** `rupu cp serve` (full), `--no-mcp` (web only), `--no-web` (MCP only; `/api/*` still served). `--no-web --no-mcp` is refused at startup. `--no-open` is implied by `--no-web`.
- **`/mcp` sits behind the same bearer-token layer as `/api/*`.** No `--token` means no token check.
- **`/mcp` always validates `Origin`.** A request whose `Origin` header is present and is not the CP's own origin is refused.
- **Attribution:** a new optional `client: "mcp"` field next to `via: cp`. The existing `via` value never changes on the wire.
- **Tool hints** are informational only: `readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`.
- **`wait` timing:** default timeout 30 s, capped at 55 s.
- **Workspace deps only** (versions in the root `Cargo.toml`), `#![deny(clippy::all)]`, no `unsafe`.
- **Integration tests:** one binary per crate, modules under `tests/it/`. Run one module with `cargo test -p <crate> --test it <module>::`.
- **Keep files formatted** with per-file `rustfmt`. Never package-wide `cargo fmt`: main is fmt-dirty under the pinned toolchain.

## Review Focus

These are likely failures no task's happy-path tests cover. Each is pinned by a test in the task named.

1. **Path traversal through a tool argument.** For example, `runs.get {"id": "../../config"}`. The id must be percent-encoded into its segment, and the CP's `path_guard` must still apply in process. The call must never reach another route. Pinned in Task 5.
2. **Two MCP clients reusing request id `1`.** A `notifications/cancelled` from client A must not abort client B's call: in-flight calls are keyed by session. Pinned in Task 1 and Task 4.
3. **`wait` on a run that is already settled or parked must return at once, never time out.** `wait` on an unknown run must return `isError`, not hang. Pinned in Task 8.
4. **Origin checks.**
   - A cross-origin `Origin` is refused, while same-origin and absent are served.
   - With `--token`, `/mcp` without the bearer is a 401.

   Pinned in Task 4.
5. **A tool call missing a required argument** must be an `isError` result naming the argument. It must not be a request built with an empty path segment, and not a 500. Pinned in Task 5.

---

## File Structure

**`crates/rupu-mcp/src/`**

| File | Responsibility |
|---|---|
| `lib.rs` | Re-exports |
| `source.rs` (new) | `ToolSource` trait, `ToolDef`, `ToolHints`, `ToolResult`, `CallCx` |
| `server.rs` (rewritten) | `McpServer`: JSON-RPC dispatch over sources, in-flight calls keyed by session and id, `handle`, the `run` loop over a `Transport` |
| `filter.rs` (new) | `ToolFilter`: the `--tools` grammar over the combined tool list |
| `catalog.rs` (new) | `CatalogSource` + `ContextFactory`: the `rupu-tools` catalog as a source |
| `transport.rs` | `Transport` + `InProcessTransport` only; `StdioTransport` removed |
| `error.rs` | `McpError`, `ServeError` (gains `Filter`) |

**`crates/rupu-cp/src/mcp/`** (new module)

| File | Responsibility |
|---|---|
| `mod.rs` | `McpOpts`, `build_server(state, api_inner, opts) -> Result<Arc<McpServer>, ServeError>` |
| `ops.rs` | `OpSpec` table (13 entries) + `ops()` |
| `operator_source.rs` | `OperatorSource`: an `OpSpec` call sent through the inner router |
| `wait.rs` | `WaitSource`: the `wait` tool |
| `endpoint.rs` | `/mcp` axum handler: Origin check, sessions, JSON-RPC over POST |

**Other files**

| File | Change |
|---|---|
| `crates/rupu-cp/src/server.rs` | `api_routes()`, `RouterOpts`, `router_with()`; `router()` stays as is |
| `crates/rupu-cp/src/lib.rs` | `build_state()`, `spawn_prewarm()`, `ServeOpts { web, mcp }`, MCP startup lines |
| `crates/rupu-cp/tests/it/mcp.rs` (new) | Endpoint, operator tools, attribution |
| `crates/rupu-cp/tests/it/mcp_wait.rs` (new) | `wait` |
| `crates/rupu-cp/tests/it/mcp_routes.rs` (new) | Route-coverage lockstep |
| `crates/rupu-fleet/src/{bus.rs,log.rs}` | `client` on `OperatorMessage` / `LogMessage` |
| `crates/rupu-cli/src/cmd/cp.rs` | Flags, `CliContextFactory`, `McpOpts` |
| `crates/rupu-cli/src/cmd/mcp.rs` | Deprecation alias |
| `crates/rupu-cli/tests/it/` | `mcp_serve_stdio_smoke.rs` removed, `cp_serve_mcp_smoke.rs` added |

---

### Task 1: `rupu-mcp` protocol core over tool sources

**Files:**
- Create: `crates/rupu-mcp/src/source.rs`
- Rewrite: `crates/rupu-mcp/src/server.rs`
- Modify: `crates/rupu-mcp/src/transport.rs` (delete `StdioTransport`), `crates/rupu-mcp/src/lib.rs`, `crates/rupu-mcp/src/error.rs`
- Test: `crates/rupu-mcp/tests/it/serve.rs` (port onto a stub source), `crates/rupu-mcp/tests/it/sessions.rs` (new)

**Interfaces:**

```rust
// source.rs
pub struct ToolHints { pub read_only: bool, pub destructive: bool, pub idempotent: bool, pub open_world: bool }
impl ToolHints { pub fn to_annotations(&self) -> serde_json::Value } // {"readOnlyHint":…, "destructiveHint":…, "idempotentHint":…, "openWorldHint":…}

pub struct ToolDef { pub name: String, pub description: String, pub input_schema: Value, pub hints: ToolHints }

pub enum ToolResult {
    /// `structuredContent` = value; `content` = one text block with the same JSON.
    Json(Value),
    /// Plain text content (catalog tools return text today).
    Text(String),
    /// `isError: true` with this text.
    Error(String),
}

pub struct CallCx { pub session: Option<String> }

#[async_trait]
pub trait ToolSource: Send + Sync {
    fn tools(&self) -> Vec<ToolDef>;
    /// `None` when `name` is not this source's tool.
    async fn call(&self, name: &str, args: Value, cx: &CallCx) -> Option<ToolResult>;
}
```

```rust
// server.rs
pub struct McpServer { /* sources, filtered tool index, in-flight map */ }
impl McpServer {
    /// Lists every tool of `sources` that `filter` keeps. A name served by two sources is
    /// `ServeError::Duplicate(name)` (sources resolve collisions before this).
    pub fn new(sources: Vec<Arc<dyn ToolSource>>, filter: &ToolFilter) -> Result<Self, ServeError>;
    pub fn tool_names(&self) -> Vec<String>;
    /// One JSON-RPC message or batch → the reply (`None` for notifications / responses only).
    /// `session` keys in-flight calls, so a cancel only reaches its own session's call.
    pub async fn handle(&self, msg: Value, session: Option<&str>) -> Option<Value>;
    /// Serve a transport until it closes (tests; `InProcessTransport`).
    pub async fn run<T: Transport + 'static>(self: Arc<Self>, transport: T) -> Result<(), McpError>;
}
```

Keep #879's behavior and its tests:
- `ping`
- the parse error on a bad message
- batch replies in order
- version negotiation (`PROTOCOL_VERSIONS`, `negotiate_protocol_version`)
- concurrent `tools/call` with cancel

`initialize` advertises `capabilities: { tools: {} }`. `tools/list` returns each `ToolDef` as `{name, description, inputSchema, annotations}`. A `tools/call` for an unlisted name answers `isError` "unknown tool: <name>".

- [ ] **Step 1: Write the failing tests.**
  - Port `serve.rs`'s protocol tests (`protocol_errors_and_notifications`, `ping_answers_an_empty_result`, `a_malformed_line_is_answered_and_the_server_keeps_serving`, `a_batch_is_answered_with_one_array_in_order`, `initialize_negotiates_the_protocol_version`, `calls_run_concurrently_and_a_cancel_aborts_one`) onto a test-only `StubSource`. It has a tool `stub.echo` that returns `ToolResult::Json(args)`, and `stub.sleep` that sleeps `args.ms`.
  - Add to `sessions.rs`:
    - `a_cancel_only_reaches_its_own_session`: start `stub.sleep {ms: 2000}` with id 1 under session `"a"` and under session `"b"`. Send `notifications/cancelled {requestId: 1}` under `"a"`. Assert `"b"`'s call answers and `"a"`'s gets no reply.
    - `tools_list_carries_annotations`: assert `annotations.readOnlyHint == true` for a stub tool defined `read_only`.
    - `json_results_carry_structured_content`: `stub.echo {x: 1}` → `result.structuredContent == {"x":1}` and `result.content[0].text` parses to the same value.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-mcp --test it`
  Expected: compile errors (`McpServer`, `ToolSource` not found).
- [ ] **Step 3: Implement.** Write `source.rs` and `server.rs` as above, moving #879's dispatch, batch and in-flight code over. The in-flight key becomes `(session.unwrap_or(""), id.to_string())`. Delete `StdioTransport` and `CatalogServer` (Task 3 replaces the catalog part). Update `lib.rs` re-exports: `McpServer, ToolSource, ToolDef, ToolHints, ToolResult, CallCx, ToolFilter, InProcessTransport, Transport, McpError, ServeError, PROTOCOL_VERSIONS, negotiate_protocol_version`. Remove `io-std` from the tokio features.
- [ ] **Step 4: Run the tests and confirm they pass.**
  Run: `cargo test -p rupu-mcp --test it serve:: sessions::`
  Expected: all pass. (`schema_snapshot` is reworked in Task 3; mark it `#[ignore = "reworked in Task 3"]` until then.)
- [ ] **Step 5: Commit.**
  `git commit -m "rupu-mcp: protocol core over tool sources, session-keyed cancel, no stdio"`

### Task 2: `ToolFilter` for `--tools`

**Files:**
- Create: `crates/rupu-mcp/src/filter.rs`
- Test: `crates/rupu-mcp/tests/it/filter.rs`

**Interfaces:**
- **Produces:**
  ```rust
  pub struct ToolFilter { /* entries */ }
  impl ToolFilter {
      pub fn all() -> Self;
      /// Comma-split entries already split by clap.
      pub fn parse(entries: &[String]) -> Self;
      /// The subset of `names` kept, or `ServeError::Filter(entry)` naming the first entry that matches nothing.
      pub fn apply(&self, names: &[String]) -> Result<BTreeSet<String>, ServeError>;
  }
  ```
- **Grammar:**
  - `*`: all
  - `core.*`: the catalog core tools (`ToolCatalog::all()` where `is_core()`)
  - `<ns>.*`: every name whose first dotted segment is `<ns>`
  - an exact name
  - a catalog legacy alias, resolved with `ToolCatalog::builtin().resolve_name(_, AliasScope::Everywhere)`

  An empty `entries` = `all()`.
- **Consumed by:** `McpServer::new` (Task 1).

The `ServeError::Filter` message: ``"--tools: `{entry}` matches no tool; namespaces here: {sorted, comma-separated first segments}"``.

- [ ] **Step 1: Write the failing tests** in `filter.rs`:
  - `empty_keeps_everything`
  - `namespace_glob_keeps_only_that_namespace`: `runs.*` over `[runs.get, runs.list, scm.prs.get]` → `{runs.get, runs.list}`
  - `core_glob_keeps_catalog_core_tools`: `core.*` over `[read_file, bash, runs.get]` → `{read_file, bash}`
  - `alias_resolves_to_canonical`: `report_finding` over `[findings.report]` → `{findings.report}`
  - `unknown_entry_fails_naming_it`: the error contains `` `nope.*` `` and `runs`
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-mcp --test it filter::`
  Expected: compile error.
- [ ] **Step 3: Implement `ToolFilter`** in `filter.rs`. Add `ServeError::Filter(String)`. Have `McpServer::new` apply the filter to the union of source tool names.
- [ ] **Step 4: Run them and confirm they pass.**
  Run: `cargo test -p rupu-mcp --test it filter::`
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "rupu-mcp: --tools filter over the combined tool list"`

### Task 3: The catalog as a tool source

**Files:**
- Create: `crates/rupu-mcp/src/catalog.rs`
- Test: `crates/rupu-mcp/tests/it/catalog.rs`, `crates/rupu-mcp/tests/it/schema_snapshot.rs` (reworked), `crates/rupu-orchestrator/tests/it/findings_single_impl.rs` (port)

**Interfaces:**

```rust
pub trait ContextFactory: Send + Sync {
    /// A tool context for one call. `workspace` is a project id or the absolute path of a
    /// registered project; `None` for tools that act on no workspace (connectors).
    /// `Err` text is returned to the caller as the tool's error.
    fn context(&self, workspace: Option<&str>) -> Result<ToolContext, String>;
}

pub struct CatalogSource { /* factory, served descriptors */ }
impl CatalogSource {
    /// Every catalog tool the factory's base context can serve (`ctx.services.provided(false)`,
    /// grant `*`), minus `HIDDEN_BY_OPERATOR_TOOLS`.
    pub fn new(factory: Arc<dyn ContextFactory>) -> Result<Self, ServeError>;
}
pub const HIDDEN_BY_OPERATOR_TOOLS: &[&str] =
    &["agents.list", "agents.get", "workflows.list", "workflows.get", "findings.query", "findings.tag"];
```

- **Workspace tools.** A catalog tool **needs a workspace** when `d.is_core()` or `d.needs` contains `Service::Findings`. For those, `tools()` adds a required string property `workspace` to the tool's `input_schema()`, with the description "Project id, or the absolute path of a registered project, the tool acts on." On `call`, it removes `workspace` from the args before calling the tool, builds the context with `factory.context(Some(ws))`, builds the body with `rupu_tools::bodies::body(name, &ctx)`, and runs it through `rupu_tools::call::call_shared` under `PermissionPolicy::unattended(PermissionMode::Bypass)`.
- **Results.** `CallOutcome::Done(text)` → `ToolResult::Text`. `Denied` / `Failed` → `ToolResult::Error`.
- **Hints.** From the descriptor's `Effect`: `Read` → read_only. `Write` / `External` → destructive=false, open_world = `External`. `Spawn` → open_world.

- [ ] **Step 1: Write the failing tests** in `catalog.rs`, with a `TestFactory` over `ToolContext::in_workspace(tempdir)` that maps any `workspace` string to that tempdir and errors on `"missing"`:
  - `operator_named_tools_are_not_listed`: none of `HIDDEN_BY_OPERATOR_TOOLS` appear in `tools()`.
  - `fs_tools_require_a_workspace_argument`: the `read_file` schema's `required` contains `workspace`. `scm.repos.list`'s does not.
  - `read_file_reads_inside_the_named_workspace`: write `hello.txt` in the tempdir, call `read_file {workspace: "p1", path: "hello.txt"}`, and assert the text contains the file's content.
  - `an_unknown_workspace_is_a_tool_error`: `read_file {workspace: "missing", path: "x"}` → `ToolResult::Error` containing the factory's text.
  - `findings_record_alias_records_through_the_one_findings_tool`: port the existing test.
- [ ] **Step 2: Rework `schema_snapshot.rs`.** Snapshot the `tools()` list (name, inputSchema) of a `CatalogSource` over a context with SCM + findings services, and re-bless. Port `findings_single_impl.rs` to call through `McpServer::new(vec![catalog_source], &ToolFilter::all())` + `InProcessTransport`, passing `workspace`.
- [ ] **Step 3: Run them and confirm they fail.**
  Run: `cargo test -p rupu-mcp --test it catalog::`
  Expected: compile error.
- [ ] **Step 4: Implement `CatalogSource`.**
- [ ] **Step 5: Run the tests and confirm they pass.**
  Run: `cargo test -p rupu-mcp --test it` and `cargo test -p rupu-orchestrator --test it findings_single_impl::`
  Expected: PASS (snapshot blessed in step 2).
- [ ] **Step 6: Commit.**
  `git commit -m "rupu-mcp: the tool catalog as a source, per-call workspace contexts"`

### Task 4: `/mcp` in `cp serve`: router split, state builder, endpoint

**Files:**
- Create: `crates/rupu-cp/src/mcp/{mod.rs,endpoint.rs}`
- Modify: `crates/rupu-cp/src/server.rs`, `crates/rupu-cp/src/lib.rs`, `crates/rupu-cp/Cargo.toml` (add `rupu-mcp`, `tower`, `http`, `schemars` workspace deps)
- Test: `crates/rupu-cp/tests/it/mcp.rs` (new; add `mod mcp;` to `main.rs`)

**Interfaces:**

```rust
// server.rs
/// Every `/api/*` route, no auth layer: the in-process MCP caller's router.
pub fn api_routes() -> Router<AppState>;
pub struct RouterOpts { pub web: bool, pub mcp: Option<Arc<rupu_mcp::McpServer>> }
/// `router()` = `router_with(state, token, RouterOpts { web: true, mcp: None })`.
pub fn router_with(state: AppState, token: Option<String>, opts: RouterOpts) -> Router;

// lib.rs
pub fn build_state(opts: &ServeOpts) -> AppState;   // everything serve_on wires before the prewarm
pub fn spawn_prewarm(state: &AppState);              // the usage + netflow prewarm tasks
pub struct ServeOpts { /* existing fields */ pub web: bool, pub mcp: Option<mcp::McpOpts> }

// mcp/mod.rs
pub struct McpOpts {
    pub catalog: Option<Arc<dyn rupu_mcp::ToolSource>>,
    pub tools: Vec<String>,
}
/// Operator source (Task 5) + wait source (Task 8) + `opts.catalog`, filtered by `opts.tools`.
pub fn build_server(state: &AppState, opts: &McpOpts) -> Result<Arc<rupu_mcp::McpServer>, rupu_mcp::ServeError>;
/// `api_routes().with_state(state)` with `crate::path_guard::reject_unsafe_segments` applied
/// (no token layer): what in-process calls go through.
pub(crate) fn inner_api(state: &AppState) -> axum::Router;
```

**`/mcp` endpoint** (`endpoint.rs`), mounted inside the token layer next to `/api`:
- **POST:**
  - The body is a JSON-RPC object or array. If the body doesn't parse, the reply is 400 with a JSON-RPC `-32700` error.
  - The session is the `Mcp-Session-Id` request header. An `initialize` request gets a new session id: `mcp_` + ULID, sent back in the `Mcp-Session-Id` response header.
  - The response is 200 `application/json` with `server.handle(msg, session)`'s reply, or 202 with no body when the reply is `None`.
- **GET and DELETE:** 405.
- **Origin check:** the request is refused with 403 when an `Origin` header is present and its authority (`host[:port]`) differs from the request's `Host` header.

**Serve modes:**
- `--no-web` (`web: false`) replaces the SPA fallback with a plain 404.
- `ServeOpts { web: false, mcp: None }` is refused by `serve_on` with "nothing to serve: --no-web and --no-mcp together".
- When `mcp` is `Some`, `serve_on` prints after the UI line, with the `--header` part only when a token is set:
  ```
  ➜  rupu MCP  →  {url}/mcp
     claude mcp add --transport http rupu {url}/mcp --header "Authorization: Bearer {token}"
  ```

- [ ] **Step 1: Write the failing tests** in `tests/it/mcp.rs`, serving `router_with` on `127.0.0.1:0` with `build_state` over a temp global dir and an `McpOpts { catalog: None, tools: vec![] }`:
  - `initialize_returns_a_session_header_and_tools_capability`
  - `get_on_mcp_is_405`
  - `notification_only_post_is_202`
  - `cross_origin_is_refused`: `Origin: http://evil.example` → 403. `Origin: http://127.0.0.1:<port>` → 200. No `Origin` → 200.
  - `bearer_required_when_token_set`: router with `Some("t")`; no header → 401, `Authorization: Bearer t` → 200.
  - `cancel_is_scoped_to_the_session`: two sessions, each running a long `wait` with id 1 (Task 8; until then a `#[ignore]`d placeholder naming Task 8).
  - `no_web_serves_api_and_mcp_but_not_the_spa`: `GET /` → 404, `GET /healthz` → 200, `POST /mcp initialize` → 200.
  - `web_only_has_no_mcp`: `mcp: None`; `POST /mcp` → 404 with JSON `{"error": "MCP is off (--no-mcp)"}`. When `mcp` is `None`, `router_with` mounts `/mcp` to that 404 explicitly, so the SPA fallback never answers it.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-cp --test it mcp::`
  Expected: compile error.
- [ ] **Step 3: Implement.** Extract `build_state` / `spawn_prewarm` from `serve_on` (behavior unchanged; `serve_on` calls both). Add `api_routes()`. Make `router()` delegate to `router_with`. Write `endpoint.rs` and `build_server`, with the operator source as an empty table until Task 5 and `wait` absent until Task 8.
- [ ] **Step 4: Run the tests and confirm they pass.**
  Run: `cargo test -p rupu-cp --test it mcp:: server:: auth:: embed::`
  Expected: PASS (existing server/auth/embed tests unchanged).
- [ ] **Step 5: Commit.**
  `git commit -m "cp: /mcp endpoint (HTTP), serve modes, one state builder"`

### Task 5: Operator tool table and in-process calls (13 tools)

**Files:**
- Create: `crates/rupu-cp/src/mcp/{ops.rs,operator_source.rs}`
- Modify: the handler query/body structs named below: add `#[derive(schemars::JsonSchema)]`, with `#[schemars(with = "String")]` on fields whose types lack it.
- Test: `crates/rupu-cp/tests/it/mcp.rs`

**Interfaces:**

```rust
pub struct OpSpec {
    pub name: &'static str,
    pub method: http::Method,
    /// CP path with `:params`; `{root}` = the message route segment chosen from the `id`
    /// argument's prefix (`MessageRoute::of_id`).
    pub path: &'static str,
    pub description: &'static str,
    /// The handler's query struct schema; its properties are taken from the arguments into the URI.
    pub query: fn() -> Option<schemars::schema::RootSchema>,
    /// The handler's body struct schema; the remaining arguments form the JSON body.
    pub body: fn() -> Option<schemars::schema::RootSchema>,
    pub hints: rupu_mcp::ToolHints,
}
pub fn ops() -> &'static [OpSpec];
pub struct OperatorSource { /* inner router, ops */ }
impl OperatorSource { pub fn new(inner: axum::Router) -> Self }
```

**Input schema.** Each tool's input schema is one object: a required string property per path param, plus the query schema's properties, plus the body schema's properties. Required = path params + the body's required fields. The query and body properties are lifted out of schemars' `RootSchema`, with `definitions` inlined.

**A call:**
1. Validate the required properties. A missing one returns `ToolResult::Error("missing required argument `<name>`")`.
2. Percent-encode each path param into its segment (`url::form_urlencoded`-style, `/` encoded).
3. Put the query args in the URI.
4. Put the remaining args in a JSON body (only when `body` is `Some`).
5. Set `content-type: application/json` and `x-rupu-client: mcp`.
6. `inner.clone().oneshot(req).await`.

**Response mapping:**
- 2xx with a JSON body → `ToolResult::Json`.
- 2xx with an empty body → `ToolResult::Json(json!({"status": <code>}))`.
- Anything else → `ToolResult::Error(format!("HTTP {code}: {body}"))`, with the body text kept intact.

**The 13 entries:**

| Name | Method + path | Query type | Body type | Hints |
|---|---|---|---|---|
| `runs.list` | GET `/api/runs/workflows` | `WorkflowRunsQuery` | — | read_only |
| `runs.get` | GET `/api/runs/:id` | `RunDetailQuery` | — | read_only |
| `runs.approve` | POST `/api/runs/:id/approve` | `RunControlQuery` | `ApproveBody` | — |
| `runs.reject` | POST `/api/runs/:id/reject` | `RunControlQuery` | `RejectBody` | destructive |
| `runs.cancel` | POST `/api/runs/:id/cancel` | `RunControlQuery` | `CancelBody` | destructive |
| `workflows.list` | GET `/api/workflows` | — | — | read_only |
| `workflows.run` | POST `/api/workflows/:name/run` | — | `LaunchBody` | open_world |
| `agents.list` | GET `/api/agents` | — | — | read_only |
| `agents.run` | POST `/api/agents/:name/run` | — | `AgentRunBody` | open_world |
| `sessions.start` | POST `/api/agents/:name/session` | — | `SessionStartBody` | open_world |
| `sessions.send` | POST `/api/sessions/:id/send` | `SendQuery` | `SendBody` | open_world |
| `messages.list` | GET `/api/{root}/:id/messages` | `ListQuery` (messages) | — | read_only |
| `messages.send` | POST `/api/{root}/:id/messages` | `HostQuery` (messages) | `PostBody` | — |

Each `description`'s first line says what the tool does on its own, e.g. `runs.approve`: "Approve a workflow run's parked gate so the run continues." The second paragraph names the arguments that matter and what it returns.

- [ ] **Step 1: Write the failing tests** in `tests/it/mcp.rs`, using the `AppState` helpers `tests/it/runs.rs` and `messages.rs` already use for a run record and a message space:
  - `tools_list_has_the_13_operator_tools_with_schemas`: names equal the table. `runs.approve`'s `inputSchema.required` contains `id`, and its properties include `gate` and `mode`.
  - `runs_get_returns_the_run_detail_as_structured_content`: create run `RUN`, call `runs.get {id: RUN}`, assert `structuredContent.id == RUN`.
  - `a_missing_required_argument_names_it`: `runs.get {}` → `isError`, text contains `` `id` ``.
  - `a_traversal_id_never_reaches_another_route`: `runs.get {id: "../../config"}` → `isError`, text contains `HTTP 4`, and does not contain `"providers"` (the config body).
  - `an_http_error_keeps_its_status_and_body`: `runs.get {id: "run_nope"}` → `isError`, text starts with `HTTP 404`.
  - `messages_send_picks_the_route_from_the_id_prefix`: on a live session space (the `messages.rs` fixture), `messages.send {id: SESSION, body: "hi"}` → `structuredContent.id` starts with `msg_`.
  - `reject_and_cancel_are_marked_destructive`.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-cp --test it mcp::`
  Expected: FAIL (empty table).
- [ ] **Step 3: Implement `ops.rs` and `operator_source.rs`.** Add the derives. Wire `OperatorSource::new(inner_api(state))` into `build_server`.
- [ ] **Step 4: Run them and confirm they pass.**
  Run: `cargo test -p rupu-cp --test it mcp::`
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "cp: operator tool table over the in-process API (13 tools)"`

### Task 6: Route-coverage lockstep

**Files:**
- Create: `crates/rupu-cp/tests/it/mcp_routes.rs` (add to `main.rs`)
- Modify: `crates/rupu-cp/src/mcp/ops.rs` (add the lists)

**Interfaces:**
- **Produces:**
  ```rust
  pub const EXCLUDED: &[(&str /*METHOD*/, &str /*path*/, &str /*reason*/)];
  pub const PENDING_PLAN_2: &[(&str, &str)];
  ```
- **`EXCLUDED`** holds exactly the spec's M4 exclusions. Each needs a reason:
  - the four SSE streams: `GET /api/runs/:id/log`, `GET /api/events/stream`, `GET /api/transcript/stream`, and `GET /api/{runs,sessions,agentiflows}/:id/messages/stream`
  - the host-transfer endpoints: `GET /api/runs/:id/coverage`, `GET /api/runs/:id/netflow/stream`, `GET /api/findings/artifacts/:sha256`, `POST /api/workspace/stage`, `GET /api/workspace/delta`, `DELETE /api/workspace/discard`, `POST /api/skills/bundle/missing`, `PUT /api/skills/bundle/:hash`, `GET /api/node/connect`
  - `GET /healthz`
  - `GET /api/fs/browse`
- **`PENDING_PLAN_2`** holds every other route that is not in `ops()`. Generate it once from the scanner's output; Plan 2 empties it.

- [ ] **Step 1: Write the test.** `every_route_is_a_tool_or_listed`:
  - Scan `crates/rupu-cp/src/**/*.rs` (outside `#[cfg(test)]` modules) for `.route(` calls with a string-literal path.
  - For each, take the method-router expression up to the matching `)` (paren-depth scan) and collect each `get(` / `post(` / `put(` / `patch(` / `delete(` (also `axum::routing::delete(`) at depth 0 of that expression.
  - Add the 9 generated message routes from `MessageRoute::ALL`: GET + POST `…/messages`, and GET `…/messages/stream` for each segment.
  - Assert at least 150 `(method, path)` pairs, so a broken scanner can't pass by finding nothing.
  - Assert every pair is in `ops()` (with `{root}` expanded over `MessageRoute::ALL`) ∪ `EXCLUDED` ∪ `PENDING_PLAN_2`.
  - Assert every listed pair exists (no stale entries).
- [ ] **Step 2: Run it** to print the unlisted pairs.
  Run: `cargo test -p rupu-cp --test it mcp_routes:: -- --nocapture`
  Expected: FAIL listing the missing pairs.
- [ ] **Step 3: Fill `EXCLUDED` and `PENDING_PLAN_2`** from that output.
- [ ] **Step 4: Run it and confirm it passes.**
  Run: `cargo test -p rupu-cp --test it mcp_routes::`
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "cp: lockstep — every route is an MCP tool or listed"`

### Task 7: `client: mcp` on operator messages

**Files:**
- Modify:
  - `crates/rupu-fleet/src/bus.rs`: `OperatorMessage.client: Option<String>`; `operator_send` copies it to the log line.
  - `crates/rupu-fleet/src/log.rs`: `LogMessage.client: Option<String>`, `#[serde(default, skip_serializing_if = "Option::is_none")]`.
  - `crates/rupu-cp/src/host/connector.rs`: `OperatorMessageRequest.client: Option<String>`.
  - `crates/rupu-cp/src/host/messages.rs`: `send` passes it.
  - `crates/rupu-cp/src/api/messages.rs`: `post` takes `HeaderMap` and sets `client: Some("mcp")` only when `x-rupu-client` is exactly `mcp` and the write is local.
  - Every other `OperatorMessage {` / `OperatorMessageRequest {` literal gets `client: None` (`rupu-cli` `cmd/{session,message,agentiflow}.rs`, `rupu-agentiflow` `envelope.rs` / `lead.rs`, `rupu-runtime` `services/operator.rs`, tests).
- Test: `crates/rupu-fleet/src/tests.rs`, `crates/rupu-cp/tests/it/mcp.rs`

- [ ] **Step 1: Write the failing tests.**
  - `rupu-fleet`, `a_line_without_client_still_parses_and_round_trips_without_it`: an older line with no `client` key parses, and re-serializes with no `client` key.
  - `rupu-cp`, `messages_sent_through_mcp_record_client_mcp_and_keep_via_cp`: after `messages.send` through MCP, the `log.jsonl` line has `"via":"cp"` and `"client":"mcp"`. A plain HTTP POST with no header has no `client` key.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-fleet` and `cargo test -p rupu-cp --test it mcp::messages_sent_through_mcp`
  Expected: FAIL.
- [ ] **Step 3: Implement the field and its threading.**
- [ ] **Step 4: Run them and confirm they pass**, plus `cargo test -p rupu-cp --test it messages::`.
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "messages: record client=mcp next to via=cp for MCP operator sends"`

### Task 8: `wait` (local runs and messages)

**Files:**
- Create: `crates/rupu-cp/src/mcp/wait.rs`
- Modify: `crates/rupu-cp/src/sse.rs` (`run_is_settled` → `pub(crate)`), `crates/rupu-cp/src/mcp/mod.rs` (add `WaitSource` to `build_server`)
- Test: `crates/rupu-cp/tests/it/mcp_wait.rs` (add to `main.rs`)

**Interfaces:**
- **Produces:** `WaitSource::new(state: AppState)`, serving the tool `wait`.
- **Input:**
  ```
  { watch: [Condition], cursor?: object, timeout_ms?: integer }
  Condition = { kind: "run_settled"|"run_gate", run: string, host?: string }
            | { kind: "run_events", run: string, types?: [string], host?: string }
            | { kind: "messages", root: string, kinds?: [string], host?: string }
  ```
- **Output** (`ToolResult::Json`):
  ```
  { fired: [{ index, kind, detail }], events: [{ source, id, event }],
    cursor: { "<source>": <line id> }, timed_out: bool }
  ```
  - `source` is `"run:<id>"` or `"messages:<root>"`.
  - `id` is the line number the SSE streams use.
  - A `run_gate`'s `detail` is `{gates: [{step_id, prompt, expires_at}]}` from `RunRecord.awaiting`.
- **Behavior:**
  1. Check current state first: `run_settled` via `run_is_settled`, and `run_gate` via `RunStore::load(run).awaiting` being non-empty. If anything fires, return at once with no wait.
  2. Otherwise open a `NumberedTail<rupu_orchestrator::executor::Event>` on `RunStore::events_path(run)` for each run-sourced condition, and a `NumberedTail<rupu_fleet::LogLine>` on `MessageRoot::resolve(global, root)?.log_path()` for each `messages` condition. Each tail starts `after` = `cursor[source]` or 0.
  3. Collect lines with `send == true` that match the filters: `types` on the event's `type` tag, `kinds` on a `LogLine::Msg`'s kind; `Seen` lines are skipped.
  4. Re-check the state conditions after each line and every 1 s.
  5. Return when a condition fires (after draining the lines already buffered, at most 250 ms more), when `run_events` / `messages` collected at least one line, or at the timeout.
- **Timeout:** default 30 000 ms, clamped to 55 000. A timeout is `timed_out: true`, not an error.
- **Errors:**
  - An unknown run or root → `ToolResult::Error` naming it.
  - A `host` other than `local` → `ToolResult::Error("wait on a remote host is not built yet (Plan 2)")`.

- [ ] **Step 1: Write the failing tests:**
  - `a_settled_run_fires_at_once`: run record terminal, no runner pid; `wait {run_settled}` returns in under 1 s with `fired[0].kind == "run_settled"`.
  - `a_parked_gate_fires_at_once_with_its_gates`: a record with `awaiting: [{step_id: "review", …}]` → `detail.gates[0].step_id == "review"`.
  - `new_events_are_returned_with_their_line_ids_and_the_cursor_resumes`: append 2 events, wait → ids `[1, 2]`, cursor `{"run:<id>": 2}`. Append 1 more, wait with that cursor → ids `[3]`.
  - `messages_wait_returns_new_operator_lines_and_skips_seen`.
  - `times_out_cleanly`: `timeout_ms: 300` on a quiet running run → `timed_out: true`, no `isError`, back in under 1 s.
  - `an_unknown_run_is_an_error_not_a_hang`.
  - `timeout_is_capped_at_55s`: unit test of the clamp function.
  - Un-ignore Task 4's `cancel_is_scoped_to_the_session` and make it use `wait`.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-cp --test it mcp_wait::`
  Expected: compile error.
- [ ] **Step 3: Implement `WaitSource`.**
- [ ] **Step 4: Run them and confirm they pass**, plus the session-cancel test.
  Run: `cargo test -p rupu-cp --test it mcp_wait:: mcp::`
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "cp: wait tool — runs settle, gates park, events and messages with a cursor"`

### Task 9: CLI — serve flags, catalog contexts, `rupu mcp serve` deprecation

**Files:**
- Modify: `crates/rupu-cli/src/cmd/cp.rs`, `crates/rupu-cli/src/cmd/mcp.rs`, `crates/rupu-cli/src/lib.rs` (the `Mcp` help text)
- Delete: `crates/rupu-cli/tests/it/mcp_serve_stdio_smoke.rs`
- Create: `crates/rupu-cli/tests/it/cp_serve_mcp_smoke.rs` (update `main.rs`)

**Interfaces:**
- **Consumes:** `rupu_cp::ServeOpts { web, mcp }`, `rupu_cp::mcp::McpOpts`, `rupu_mcp::{CatalogSource, ContextFactory}`.
- **Produces:**
  - `cp serve` flags: `--no-web` (also implies `--no-open`), `--no-mcp`, `--tools <grant>` (`value_delimiter = ','`). `--tools` together with `--no-mcp` is refused: "--tools needs the MCP; drop --no-mcp".
  - `struct CliContextFactory` in `cmd/cp.rs`, implementing `ContextFactory`:
    - Its `RunAssembler` is built as `cmd/mcp.rs` builds it today: layered config, `resolver_for`, the SCM `Registry` (shared with `cp_repos`), `findings_opts::base_options`.
    - One `RunIdentity { run_id: "mcp_<ULID>" per process, surface: Agent, scope_name: Some("mcp") }`.
    - `Some(ws)` resolves as a workspace id via `WorkspaceStore::load`, else as a path via `find_by_path`, else the error ``"no registered project `<ws>`"``.
    - `None` uses the global dir as the workspace path.
    - It calls `assembler.call_site_context(identity, &WorkspaceBinding { id, path }, Arc::new(NullSink))`.

**`rupu mcp serve`** keeps its args (`--transport`, `--tools`, `--mode`).
- It prints this on stderr, with exact copy:
  ```
  rupu mcp serve is deprecated and will be removed: it now runs `rupu cp serve --no-web` (MCP over HTTP at http://127.0.0.1:7878/mcp).
  Connect with: claude mcp add --transport http rupu http://127.0.0.1:7878/mcp
  ```
- It adds `rupu mcp serve: ignoring --mode; every tool is served and your MCP client's permissions decide what runs` when `--mode` is given.
- It adds `rupu mcp serve: ignoring --transport; stdio is no longer supported, the MCP is HTTP only` when `--transport` is given.
- Then it runs the same code path as `cp serve --no-web --tools <its --tools>`, with the default bind.

- [ ] **Step 1: Write the failing smoke test** `cp_serve_mcp_smoke.rs::cp_serve_no_web_serves_mcp_tools`:
  - Spawn the `rupu` binary with `cp serve --no-web --bind 127.0.0.1:<free port>` and a temp `RUPU_HOME`.
  - Poll `/healthz` until up, at most 20 s.
  - POST `initialize`, then `tools/list` to `/mcp`.
  - Assert the names include `runs.approve`, `wait`, `scm.repos.list` and `read_file`, and exclude `findings.query`. `GET /` is 404.
  - Kill the child.

  Also add `mcp_serve_alias_prints_the_deprecation_and_serves_http`: spawn `rupu mcp serve --transport stdio`. Stderr contains `deprecated` and `ignoring --transport`, and `/mcp` answers on 7878. Mark it `#[serial]`, because it uses the default port, and skip it with a printed reason if 7878 is taken.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-cli --test it cp_serve_mcp_smoke::`
  Expected: FAIL.
- [ ] **Step 3: Implement the flags, `CliContextFactory` and the alias.** Delete the stdio smoke test.
- [ ] **Step 4: Run them and confirm they pass.**
  Run: `cargo test -p rupu-cli --test it cp_serve_mcp_smoke::` and `cargo test -p rupu-cli --test it argv_roundtrip::`
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "cli: cp serve --no-web/--no-mcp/--tools; rupu mcp serve runs the HTTP MCP"`

### Task 10: Docs and live check

**Files:**
- Modify:
  - `docs/mcp.md` (rewrite)
  - `docs/control-plane.md` (serve modes)
  - `docs/cli-reference.md` (regenerate the way the repo does)
  - `CLAUDE.md`: the `rupu-mcp` entry becomes "MCP protocol core (`McpServer` over `ToolSource`s, `ToolFilter`, `CatalogSource`); HTTP only, served by `rupu cp serve` at `/mcp`". The `rupu-cp` entry gains one sentence on `mcp/` (operator tool table over the in-process API, `wait`, the lockstep).
  - `CHANGELOG.md`
- [ ] **Step 1: Rewrite `docs/mcp.md`.** Cover:
  - the full-access stance
  - serve modes
  - connecting Claude Code: `claude mcp add --transport http …`, with and without a token
  - Cursor
  - Claude Desktop via a connector URL or `mcp-remote`
  - the 13 operator tools and what Plans 2–4 add
  - `wait` with an example loop
  - catalog tools and the `workspace` argument
  - `--tools`
  - attribution
  - the `rupu mcp serve` deprecation
- [ ] **Step 2: Live check.**
  - Start `rupu cp serve --no-web --bind 127.0.0.1:7879` with an isolated `RUPU_HOME` (copy `~/.rupu/config.toml` only).
  - `claude mcp add --transport http rupu-test http://127.0.0.1:7879/mcp`.
  - From `claude -p`: list the tools; `workflows.run` a sample workflow with an approval gate from `.rupu/workflows/`; `wait {run_gate}`; `runs.approve`; `wait {run_settled}`; `messages.list`.
  - Record the transcript excerpt in the PR description.
  - Remove the test server with `claude mcp remove rupu-test`.
- [ ] **Step 3: Commit.**
  `git commit -m "docs: MCP over cp serve, serve modes, wait"`

---

## Self-review notes

- **Spec coverage:**
  - M1 → Tasks 4 and 9. M2 → Task 4. M4 machinery → Tasks 5–6. M5 → Task 3 (+9). M6 → Task 2. M7 (local runs and messages) → Task 8. Decision 7 (messages) → Task 7. Docs → Task 10.
  - M3 needs nothing.
  - Deferred to Plan 2, by the spec's delivery order:
    - the remaining routes (`PENDING_PLAN_2`)
    - `wait` for findings, session turns, agentiflows, agent runs and remote hosts
    - `client: mcp` on finding tags (the tag tool arrives in Plan 2)
  - Plans 3–4 cover M8 and M9.
- **Spec deviation, recorded:** M4 said in-process calls are "marked with a request extension". This plan gets the same guarantee more simply. In-process calls go through `inner_api()`, a router with no token layer that is never bound to a socket. No marker can be spoofed, because there is none.
