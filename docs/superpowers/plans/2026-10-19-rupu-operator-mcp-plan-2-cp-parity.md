# Operator MCP, Plan 2: CP parity — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every CP route that is not on the spec's M4 exclusion list becomes an operator MCP tool. `wait` gains findings, session turns, agentiflows, agent-run ends, remote hosts and progress notifications. The loose ends Plan 1 left are closed.

**Architecture:**
- The operator tool table grows from 13 entries to 138. It is split into one file per area under `crates/rupu-cp/src/mcp/ops/` and built with a small builder.
- Each entry still points at a CP route. Its input schema comes from the handler's own query and body structs (`schemars`).
- Calls still go through the in-process router. Three things are added on that path:
  - a route guard, so a call only reaches the route its entry names;
  - forwarding of the `/mcp` request's `Host` header;
  - response mapping for `x-rupu-*` headers and byte bodies.
- `wait` is split into modules: one per condition family, plus a remote module that reads through the host connectors.
- `rupu-mcp` gains embedded-resource results, annotated results, a `Host`-carrying call context and progress notifications. `/mcp` answers a tool call with SSE when the client asked for progress.

**Tech Stack:** Rust (workspace MSRV), axum 0.7, tower 0.5, tokio, `schemars` 0.8, `base64` 0.22 (workspace deps), MCP streamable-HTTP transport.

**Spec:** `docs/superpowers/specs/2026-10-19-rupu-operator-mcp-design.md`. This plan implements "Delivery order" item 2: M4's full tool table (the lockstep holds only the M4 exclusions afterwards), and M7's remaining conditions, remote hosts and progress. It also does decision 7 for finding tags. It builds on Plan 1, `docs/superpowers/plans/2026-10-19-rupu-operator-mcp-plan-1-foundation.md`, merged as #884. M8 (Plan 3) and M9 (Plan 4) are out of scope.

## Global Constraints

- **Full access, no guardrail layer.** No permission mode, admin tier or confirmation step on any tool. Hints are informational and gate nothing.
- **No new CP endpoints and no new handler logic.** The tools carry the API's request and response shapes as they are (spec non-goal). Allowed handler-side changes:
  - `#[derive(schemars::JsonSchema)]` on query and body structs, with `#[schemars(...)]` attributes where a field's type needs one;
  - `pub(crate)` visibility on those structs so `mcp/ops/` can name them;
  - taking `crate::mcp::ViaMcp` where attribution is recorded (Task 9 only);
  - one extractor change: `GET /api/events`' `Query<HashMap<String, String>>` becomes a typed struct (Task 5).
- **No web UI changes.** The CP is for humans, the MCP is for agents (matt, 2026-10-19).
- **Lockstep:** when this plan is done, every registered route is in `ops()` or in `EXCLUDED`, and `EXCLUDED` holds exactly the spec's M4 exclusions plus `POST /mcp`. `PENDING_PLAN_2` no longer exists.
- **Tool names:** `<area>.<verb>`, lowercase, `_` inside a word (`runs.usage_timeline`). Each description's first line says what the tool is for on its own, ends with a period, and is at most 120 characters (spec M6: tool search ranks on it).
- **Hint defaults by method:**
  - `GET`: read_only + idempotent
  - `PUT`: idempotent
  - `DELETE`: destructive
  - `POST`: none
  - Per-entry overrides follow spec M4's hints table: destructive for cancel, reject, discard, host remove, config writes and prune; open_world for launches and SCM-touching calls.
- **Attribution (decision 7):** an additive optional `client: "mcp"` field next to `via: cp`. The `via` value never changes on the wire, and a write forwarded to a remote host keeps that host's own attribution.
- **`wait` timing:** default timeout 30 s, capped at 55 s (unchanged). Remote reads issue one request per host at a time, never a burst.
- **Process rules:**
  - Workspace deps only. `#![deny(clippy::all)]`. No `unsafe`.
  - Integration tests are modules under `crates/<crate>/tests/it/`. Run one module with `cargo test -p <crate> --test it <module>::`.
  - Format with per-file `rustfmt <file>` only, never package-wide `cargo fmt`.
  - Run targeted tests only, never a cold full-workspace `cargo test`.
  - Never `git stash`.
  - Never bind or touch port 7878, where matt's live CP runs.
  - Every change lands through a PR.

## Review Focus

These are the inputs most likely to bite an agent that no happy-path test exercises. Each is pinned by a test in the task named.

1. **A path argument that equals a sibling route's literal segment.** Examples: `findings.get {id: "tags"}`, `coverage.get {target: "templates"}`. It must be refused before any handler runs. It must never be answered by the sibling (`GET /api/findings/tags`), and above all never run a sibling's write. Pinned in Task 4.
2. **A binary or oversize export.** `findings.export {format: "pdf"}` must come back as a base64 `blob` resource with `application/pdf`, not as lossy text. A body over 16 MiB must be an error that names the size and the API path to fetch it from, not a truncated resource. Pinned in Tasks 1, 4 and 9.
3. **`wait {findings}` with no cursor must not dump the whole ledger history.** A bad `q` must be an `isError` result carrying the query error's `token` and `code`, not a hang or a 500. Pinned in Task 15.
4. **A remote host that is down, too old, or goes away mid-wait.**
   - Down at the start, or a peer whose features lack `run.messages`: `isError` naming the host and what it can't do.
   - Lost mid-wait: the call still returns by its timeout, with a `warnings` entry, never hanging past it.

   Pinned in Task 16.
5. **`wait {agent_run_end}` / `{session_turn}` called right after `agents.run` / `sessions.send`,** before the transcript or turn record exists. The call waits out a short grace for it to appear. It must not answer "unknown run" for a run that is starting. Pinned in Task 14.

---

## File Structure

**`crates/rupu-mcp/src/`**

| File | Change |
|---|---|
| `source.rs` | `ToolResult::{Annotated, Resource}`, `EmbeddedResource`, `CallCx.{host, progress}`, `Progress` |
| `server.rs` | `handle_with` (+ `HandleCx`), rendering of the new results, progress notifications, `run` forwards notifications |
| `filter.rs` | did-you-mean in `ServeError::Filter` |
| `Cargo.toml` | `base64` |

**`crates/rupu-cp/src/mcp/`**

| File | Change |
|---|---|
| `ops.rs` → `ops/mod.rs` | `OpSpec` (+ `bytes`), the builder, `ops()` concatenating the area files, `EXCLUDED`; `PENDING_PLAN_2` shrinks per area task and is deleted in Task 12 |
| `ops/{runs,sessions,agentiflows,definitions,findings,observe,fleet,projects}.rs` (new) | One area's entries each; Plan 1's 13 entries move into their areas |
| `operator_source.rs` | Route guard, `Host` forwarding, `x-rupu-*` notes, byte bodies → resources |
| `mod.rs` | `ExpectedRoute` + the guard middleware in `inner_api`, `served_names()` |
| `endpoint.rs` | SSE reply for a tool call carrying a progress token |
| `wait.rs` → `wait/{mod.rs,parse.rs,local.rs,turns.rs,findings.rs,remote.rs}` | Split, then new conditions |

**Other**

| File | Change |
|---|---|
| `crates/rupu-cp/src/api/*.rs` | `JsonSchema` derives + `pub(crate)` on query/body structs (per area task); `tag_findings` takes `ViaMcp` |
| `crates/rupu-cp/src/sse.rs` | `parse_run_frames` (SSE bytes → `RunFrame`s) |
| `crates/rupu-coverage/src/ledger/tags.rs` | `OperatorAttribution.client` |
| `crates/rupu-tools/src/grant.rs` | `edit_distance` made `pub` (re-exported) |
| `crates/rupu-cli/src/cmd/cp.rs` | `FsCatalog` on the MCP assemblers, startup line filtered by `served_names` |
| `crates/rupu-cp/tests/it/mcp_ops_<area>.rs` (new, one per area) | Tool tests per area |
| `crates/rupu-cp/tests/fixtures/mcp/tool_names.json` (new) | The pinned tool-name list (`BLESS=1` rewrites) |
| `docs/mcp.md`, `docs/cp-api.md`, `CLAUDE.md`, `CHANGELOG.md` | Task 18 |

---

### Task 1: `rupu-mcp` — resource and annotated results, the caller's `Host`

**Files:**
- Modify: `crates/rupu-mcp/src/source.rs`, `crates/rupu-mcp/src/server.rs`, `crates/rupu-mcp/src/lib.rs`, `crates/rupu-mcp/Cargo.toml` (`base64.workspace = true`)
- Test: `crates/rupu-mcp/tests/it/sessions.rs`

**Interfaces:**
- **Produces:**
  ```rust
  pub struct EmbeddedResource {
      pub uri: String,          // e.g. rupu://api/findings/F1/export?format=pdf
      pub mime_type: String,
      pub name: Option<String>, // from Content-Disposition's filename
      pub bytes: bytes::Bytes,
  }
  pub enum ToolResult {
      Json(Value), Text(String), Error(String),   // unchanged
      /// A JSON result plus notes the caller must see.
      Annotated { value: Value, notes: Vec<String>, meta: Value },
      /// A byte body.
      Resource(EmbeddedResource),
  }
  pub struct CallCx {
      pub session: Option<String>,
      /// The `Host` header of the request the call arrived on.
      pub host: Option<String>,
      pub progress: Option<Progress>,   // Task 2; `None` until then
  }
  pub struct HandleCx<'a> { pub session: Option<&'a str>, pub host: Option<&'a str> } // Task 2 adds `notify`
  impl McpServer {
      pub async fn handle_with(&self, msg: Value, cx: HandleCx<'_>) -> Option<Value>;
      // `handle(msg, session)` stays and calls `handle_with` with `host: None`.
  }
  ```
- **Rendering in `Core::call`:**
  - `Annotated`: `content` is the JSON text block, then one `{"type":"text","text": note}` per note. `structuredContent` follows `Json`'s rule (objects only). `_meta` is `meta`.
  - `Resource`: `content: [{"type":"resource","resource":{"uri","mimeType","text"|"blob"}}]`, with `name` in `resource._meta.name` when present. It is `text` when the MIME type is `text/*`, `application/json` or ends in `+json`/`+xml`, and the bytes are valid UTF-8. Otherwise it is `blob`: standard base64 with padding.
- **Consumed by:** Task 4 (operator source), Task 2 (`progress`).

- [ ] **Step 1: Write the failing tests** in `sessions.rs`, with new `StubSource` tools:
  - `a_text_resource_is_embedded_as_text`: `stub.file` returns `Resource{mime "text/markdown; charset=utf-8", bytes "# hi"}` → `content[0].type == "resource"`, `resource.text == "# hi"`, no `blob`.
  - `a_binary_resource_is_embedded_as_base64`: bytes `[0x25,0x50,0x44,0x46,0xff]`, mime `application/pdf` → `resource.blob == "JVBERv8="`, no `text`.
  - `annotated_results_carry_notes_and_meta`: `Annotated{value: [1], notes: ["x-rupu-hosts-unfilterable: h:status — …"], meta: {"rupu/headers": {...}}}` → `content.len() == 2`, `content[1].text` starts with `x-rupu-hosts-unfilterable`, no `structuredContent` (an array), and `_meta["rupu/headers"]` present.
  - `the_callers_host_reaches_the_source`: `handle_with(call stub.host, HandleCx{host: Some("127.0.0.1:7879")})` → the stub echoes `cx.host`.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-mcp --test it sessions::`
  Expected: compile errors (`ToolResult::Resource`, `handle_with`).
- [ ] **Step 3: Implement** the types, `handle_with` threading `host` into `CallCx`, and the rendering. Update the re-exports.
- [ ] **Step 4: Run them and confirm they pass,** with Plan 1's `serve::`, `filter::` and `catalog::` modules unchanged.
  Run: `cargo test -p rupu-mcp --test it`
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "rupu-mcp: embedded-resource and annotated results; the caller's Host in CallCx"`

### Task 2: Progress notifications (`rupu-mcp` + `/mcp` SSE replies)

**Files:**
- Modify: `crates/rupu-mcp/src/{source.rs,server.rs}`, `crates/rupu-cp/src/mcp/endpoint.rs`
- Test: `crates/rupu-mcp/tests/it/sessions.rs`, `crates/rupu-cp/tests/it/mcp.rs`

**Interfaces:**
- **Produces:**
  ```rust
  /// A call's progress channel: present only when the request carried
  /// `params._meta.progressToken` and the transport can deliver notifications.
  #[derive(Clone)]
  pub struct Progress { /* token: Value, tx: mpsc::UnboundedSender<Value> */ }
  impl Progress {
      /// Sends `notifications/progress {progressToken, progress, total?, message?}`;
      /// a closed channel is ignored.
      pub fn report(&self, progress: f64, total: Option<f64>, message: Option<&str>);
  }
  pub struct HandleCx<'a> { pub session: Option<&'a str>, pub host: Option<&'a str>,
                            pub notify: Option<mpsc::UnboundedSender<Value>> }
  ```
- **`run` (in-process transport):** passes a `notify` that forwards each notification to the transport, so tests see them.
- **`/mcp` endpoint:**
  - When the request's `Accept` includes `text/event-stream` and the body is a single `tools/call` request with `params._meta.progressToken`, the reply is `200 text/event-stream`. Each notification is sent as `event: message\ndata: <json>\n\n`. Then the JSON-RPC response goes the same way, and the stream ends.
  - Every other request is answered as today (JSON or 202).
  - A client that drops the stream leaves the call running (Plan 1's rule). Its notifications are discarded.
- **Consumed by:** Task 17 (`wait` progress).

- [ ] **Step 1: Write the failing tests.**
  - rupu-mcp `progress_notifications_precede_the_reply`: `stub.ticks {n: 3}` reports 1, 2, 3 then returns. Over `InProcessTransport` with `_meta.progressToken: "t1"`, the client reads three `notifications/progress` with `progressToken == "t1"` and `progress` 1, 2, 3, then the result.
  - rupu-mcp `no_token_no_progress`: the same call without a token gets only the result.
  - rupu-cp `a_progress_call_is_answered_as_sse`: POST `/mcp` with `Accept: application/json, text/event-stream`, calling a test-only `ToolSource` that reports twice. `content-type` starts with `text/event-stream`. The body holds two `notifications/progress` frames, then the reply.
  - rupu-cp `without_sse_in_accept_the_reply_is_json`: same call, `Accept: application/json` → `application/json`, the result only.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-mcp --test it -- sessions::progress sessions::no_token` and `cargo test -p rupu-cp --test it -- mcp::a_progress_call mcp::without_sse`
  Expected: FAIL.
- [ ] **Step 3: Implement** `Progress`, `notify` threading (only `tools/call` with a token gets a `Progress`), `run`'s forwarding, and the endpoint's SSE branch (`axum::response::sse` over an mpsc stream: the notifications, then the final reply).
- [ ] **Step 4: Run them and confirm they pass,** plus `cargo test -p rupu-cp --test it mcp::`.
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "mcp: notifications/progress, sent as SSE on /mcp when the client asks"`

### Task 3: Serve-time loose ends — did-you-mean, `FsCatalog`, the startup line

**Files:**
- Modify:
  - `crates/rupu-tools/src/grant.rs`: `pub fn edit_distance`, re-exported from `lib.rs`
  - `crates/rupu-mcp/src/filter.rs`
  - `crates/rupu-cp/src/mcp/mod.rs`
  - `crates/rupu-cli/src/cmd/cp.rs`
- Test: `crates/rupu-mcp/tests/it/filter.rs`, `crates/rupu-cp/tests/it/mcp.rs`, `crates/rupu-cli/tests/it/cp_serve_mcp_smoke.rs`

**Interfaces:**
- **`ServeError::Filter`'s message** gains `; did you mean `<x>`?` when an entry is within edit distance 2 (case-insensitive) of a served name, an alias, a `<ns>.*` or `core.*`. The closest match wins; on a tie, the first in sorted order. Without a match the message is unchanged.
- **Produces** `pub fn served_names(catalog: Option<&Arc<dyn ToolSource>>) -> BTreeSet<String>` in `rupu_cp::mcp`: the names `sources()` serves (after shadowing), built without an `AppState`, as `validate_tools` does. `validate_tools` uses it.
- **`CliContextFactory`:** both its global assembler and each project assembler set `catalog: Some(Arc::new(rupu_orchestrator::FsCatalog::for_project_root(&global, project_root.as_deref())))`. The context then provides `Service::Catalog`. `catalog.search` is served, and `agents.{list,get}` / `workflows.{list,get}` are built but shadowed by the operator tools.
- **The `mcp: not served (service missing)` info line** names only unavailable tools that no source serves: `catalog.unavailable()` minus `served_names(Some(&catalog))`. Nothing is logged when that is empty.

- [ ] **Step 1: Write the failing tests.**
  - `filter::a_near_miss_suggests_the_name`: `runs.lsit` over `[runs.list, runs.get]` → the message contains ``did you mean `runs.list`?``.
  - `filter::a_namespace_typo_suggests_the_wildcard`: `run.*` → ``did you mean `runs.*`?``.
  - `filter::a_far_miss_has_no_suggestion`: `zzzz` → no `did you mean`.
  - `mcp::served_names_counts_a_shadowed_catalog_tool_once`: with a catalog stub serving `agents.list` and `stub.only`, both names appear in `served_names`, `agents.list` once.
  - `cp_serve_mcp_smoke::catalog_search_is_served`: `tools/list` includes `catalog.search`.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-mcp --test it filter::`, `cargo test -p rupu-cp --test it mcp::served_names`, `cargo test -p rupu-cli --test it cp_serve_mcp_smoke::`
  Expected: FAIL.
- [ ] **Step 3: Implement** them.
- [ ] **Step 4: Run them and confirm they pass,** plus `cargo test -p rupu-tools --lib grant::`.
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "mcp: --tools did-you-mean, catalog.search via FsCatalog, startup line names only unserved tools"`

### Task 4: Operator-source machinery — builder, area files, route guard, `Host`, headers, bytes

**Files:**
- Move: `crates/rupu-cp/src/mcp/ops.rs` → `ops/mod.rs`. Create the eight area files with Plan 1's entries moved in:
  - `runs.rs`: `runs.list`, `runs.get`, `runs.approve`, `runs.reject`, `runs.cancel`
  - `definitions.rs`: `workflows.list`, `workflows.run`, `agents.list`, `agents.run`
  - `sessions.rs`: `sessions.start`, `sessions.send`, `messages.list`, `messages.send`
  - `agentiflows.rs`, `findings.rs`, `observe.rs`, `fleet.rs`, `projects.rs`: empty `pub(super) fn ops() -> Vec<OpSpec>` for now
- Modify: `crates/rupu-cp/src/mcp/{operator_source.rs,mod.rs}`
- Create: `crates/rupu-cp/tests/fixtures/mcp/tool_names.json`
- Test: `crates/rupu-cp/tests/it/mcp.rs`, `crates/rupu-cp/tests/it/mcp_routes.rs`

**Interfaces:**
- **Produces** (in `ops/mod.rs`; area files call `op(...)`):
  ```rust
  pub struct OpSpec { /* Plan 1's fields */ pub bytes: bool }
  /// Hints from the method (Global Constraints); description must follow the first-line rule.
  pub(crate) fn op(name: &'static str, method: http::Method, path: &'static str,
                   description: &'static str) -> OpSpec;
  impl OpSpec {
      pub(crate) fn query<T: schemars::JsonSchema>(self) -> Self;
      pub(crate) fn body<T: schemars::JsonSchema>(self) -> Self;
      pub(crate) fn destructive(self) -> Self;   // sets destructive
      pub(crate) fn open_world(self) -> Self;    // sets open_world
      pub(crate) fn read_only(self) -> Self;     // a POST that changes nothing (validate, preview, export)
      pub(crate) fn bytes(self) -> Self;         // 2xx body is returned as a resource
  }
  pub fn ops() -> &'static [OpSpec];   // concatenation of the area files, in this order:
                                       // runs, sessions, agentiflows, definitions, findings, observe, fleet, projects
  ```
- **Route guard** (`mod.rs`):
  - The operator source inserts the request extension `ExpectedRoute(&'static str)`. Its value is the template the router registers (for `{root}`, the expanded `/api/<segment>/:id/messages`).
  - `inner_api` adds `route_layer(from_fn(check_expected_route))`. It reads `axum::extract::MatchedPath` and answers 400 `{"error": "`<uri>` routes to `<matched>`, not `<expected>`"}` **before the handler runs** when the two differ.
  - A request without the extension passes.
  - Plan 1's `runs.*` `valid_run_id` check stays: it gives the nicer error for the common case.
- **`Host`:** when `cx.host` is `Some`, the in-process request carries it as its `Host` header. `POST /api/hosts/node` builds the node's dial address from it.
- **Response mapping additions:**
  - **2xx with any `x-rupu-*` response header:** `ToolResult::Annotated`.
    - `value` is the body.
    - `notes`: one line per header, `"<name>: <value> — <explanation>"`. The explanations are fixed: `x-rupu-hosts-unfilterable` → "these hosts were left out: they can't filter on the named field (host:field)"; `x-rupu-hosts-without-customer` → "these hosts were left out: they can't report a customer"; any other header → no explanation.
    - `meta`: `{"rupu/headers": {name: value}}`.
  - **2xx on an op with `bytes`:** `ToolResult::Resource`.
    - `uri` = `rupu://api` + the in-process path and query.
    - `mime_type` = the response's `Content-Type` (default `application/octet-stream`).
    - `name` = the `filename="…"` of `Content-Disposition`.
  - **A body over `MAX_RESPONSE_BYTES` (16 MiB) on any op:** `ToolResult::Error("HTTP <code>: the response is over 16 MiB; fetch GET <path> from the API with its token instead")`.
- **Tool-name pin:** `tests/fixtures/mcp/tool_names.json` is the sorted array of `ops()` names. A test compares it, and `BLESS=1` rewrites it (the `tests/fixtures/list_query/` convention).

- **Test seams** (`pub(crate)`, used by the unit tests below):
  - `OperatorSource::with_ops(inner: axum::Router, ops: Vec<OpSpec>) -> Self`, the constructor `new` delegates to with `ops()`.
  - `mcp::guarded(router: axum::Router) -> axum::Router`, which adds the guard. `inner_api` is `guarded(api_routes().with_state(..))` plus the unsafe-segment layer.

- [ ] **Step 1: Write the failing tests.**
  - Unit tests in `operator_source.rs`, over small test routers wrapped in `guarded`:
    - `the_guard_refuses_before_the_handler_runs`: a router with `POST /t/:id` (its handler bumps an `AtomicUsize`) and `POST /t/literal`, and an op for `/t/:id`. Called with `{id: "literal"}` → `isError`, the text contains `routes to`, and the counter is 0.
    - `the_guard_passes_a_matching_route`: the same op with `{id: "x"}` → the handler ran once.
    - `the_callers_host_is_the_requests_host`: a route echoing its `Host` header, called with `CallCx { host: Some("10.1.2.3:7879"), .. }` → the result is `"10.1.2.3:7879"`.
    - `a_bytes_op_returns_a_resource_named_from_content_disposition`: a route answering `application/pdf` with `Content-Disposition: attachment; filename="r.pdf"` → `Resource { mime_type: "application/pdf", name: Some("r.pdf"), uri: "rupu://api/t/x?format=pdf" }`.
    - `an_oversize_body_names_the_api_path`: a route answering 17 MiB → `Error` containing `over 16 MiB` and `/t/x`.
    - `x_rupu_headers_become_notes`: a route setting `x-rupu-hosts-unfilterable: peer:status` → `Annotated`. Its note starts `x-rupu-hosts-unfilterable: peer:status — these hosts were left out`, and `meta["rupu/headers"]["x-rupu-hosts-unfilterable"] == "peer:status"`.
  - Integration tests in `mcp_routes.rs`:
    - `tool_names_are_pinned`: names == the fixture.
    - `every_schema_is_flat_and_described`: for every op, `input_schema` has `type: object`, `required ⊆ properties`, no `$ref` anywhere, and the description's first line is ≤ 120 chars and ends with `.`.
  - Rewrite `mcp::tools_list_has_the_13_operator_tools_with_schemas` to compare against the fixture (rename it `tools_list_matches_the_pinned_names`).
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-cp --test it -- mcp:: mcp_routes::`
  Expected: FAIL.
- [ ] **Step 3: Implement** the builder and the area split (Plan 1's 13 entries rewritten with `op(...)`, behavior unchanged), the guard, `Host`, the header notes, the bytes path and the oversize error. Bless the fixture with `BLESS=1`.
- [ ] **Step 4: Run them and confirm they pass,** plus `cargo test -p rupu-cp --lib mcp::operator_source`.
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "cp mcp: op builder and area files, route guard, Host forwarding, header notes, byte results"`

---

### Area tasks (5–12): the shared recipe

Each area task does these steps, for the routes in its table:

1. **Derives.** Add `#[derive(schemars::JsonSchema)]` to each query/body struct its table names, and make the struct `pub(crate)`. Where a field type has no schema, add `#[schemars(with = "...")]`. Where a field is `deserialize_with` from a string, add `#[schemars(with = "Option<String>")]`. Shared structs (`ScopeQuery`, `SessionHostQuery`, `TimeRangeQuery`, `GetCoverageQuery`, `ConfigWriteBody`, `WorkflowWriteBody`, …) are derived once, by the first task that needs them.
2. **Entries.** Add one `op(...)` per row to the area file, with a description that follows the first-line rule. Its second paragraph names the arguments that matter and what the tool returns.
3. **Pending list.** Remove those `(method, path)` pairs from `PENDING_PLAN_2`.
4. **Bless.** Re-bless `tool_names.json`.
5. **Tests.** Write `tests/it/mcp_ops_<area>.rs` (add it to `main.rs`):
   - `<area>_tools_are_listed_with_their_required_arguments`: a table of `(name, &[required args])` asserted against `tools/list`.
   - `<area>_reads_answer_what_the_api_answers`: for at least three GET tools of the table (the ones the task's seeded fixture can fill), the MCP result's JSON (`structuredContent`, else the parsed text) equals a plain HTTP GET of the same route on the same server.
   - The task's own tests, listed in the task.
6. **Run** `cargo test -p rupu-cp --test it -- mcp_ops_<area>:: mcp_routes:: mcp::` and expect PASS.
7. **Commit** `cp mcp: <area> tools`.

The hints column lists only overrides of the method default.

### Task 5: Runs area — workflow runs, agent runs, transcripts, events, autoflow runs (23 tools)

**File:** `ops/runs.rs`. **Test:** `mcp_ops_runs.rs`.

| Tool | Route | Query / body | Hints |
|---|---|---|---|
| `runs.list_all` | GET `/api/runs` | `runs::RunsListQuery` | |
| `runs.archived` | GET `/api/runs/archived` | `runs::ArchivedQuery` | |
| `runs.delete` | DELETE `/api/runs/:id` | `runs::RunControlQuery` | |
| `runs.archive` | POST `/api/runs/:id/archive` | `RunControlQuery` | |
| `runs.restore` | POST `/api/runs/:id/restore` | `RunControlQuery` | |
| `runs.pause` | POST `/api/runs/:id/pause` | `RunControlQuery` | |
| `runs.resume` | POST `/api/runs/:id/resume` | `RunControlQuery` | open_world |
| `runs.graph` | GET `/api/runs/:id/graph` | `runs::RunDetailQuery` | |
| `runs.children` | GET `/api/runs/:id/children` | — | |
| `runs.usage` | GET `/api/runs/:id/usage` | `runs::RunUsageQuery` (`epoch`: `#[schemars(with = "Option<String>")]`) | |
| `runs.usage_timeline` | GET `/api/runs/:id/usage-timeline` | `RunDetailQuery` | |
| `runs.source` | GET `/api/runs/:id/source` | `source::SourceQuery` | |
| `runs.ast` | GET `/api/runs/:id/ast` | `source::AstQuery` | |
| `runs.autoflow` | GET `/api/runs/:id/autoflow` | — | |
| `agent_runs.list` | GET `/api/runs/agents` | `run_streams::AgentRunsQuery` | |
| `transcripts.get` | GET `/api/transcript` | `transcript::PathQ` | |
| `transcripts.archive` | POST `/api/transcripts/:id/archive` | `transcripts::TranscriptHostQuery` | |
| `transcripts.delete` | DELETE `/api/transcripts/:id` | `TranscriptHostQuery` | |
| `events.recent` | GET `/api/events` | new `events::RecentEventsQuery { limit, before_ts, before_run, before_pos, q }` (all `Option`, the keys the handler reads today) | |
| `autoflow_runs.list` | GET `/api/runs/autoflows/runs` | `autoflow_runs::AutoflowRunsQuery` | |
| `autoflow_runs.problems` | GET `/api/runs/autoflows/problems` | `autoflow_runs::ProblemsQuery` | |
| `autoflow_runs.cycles` | GET `/api/runs/autoflows` | `run_streams::AutoflowRunsQuery` (a different struct of the same name) | |
| `autoflow_runs.events` | GET `/api/runs/autoflows/events` | `run_streams::AutoflowEventsQuery` | |

Note on descriptions:
- `runs.list` (Plan 1): hand-started workflow runs.
- `runs.list_all`: every orchestrator run, any trigger.
- `agent_runs.list`: standalone agent runs.

Each description says this, so an agent picks the right list.

**Task-specific tests:**
- `events_recent_reads_the_same_keys_as_before`: `GET /api/events?limit=2&q=…` over HTTP answers the same before and after the extractor change. Run it before the change, keep it.
- `runs_pause_resume_record_through_mcp`: on a seeded paused run, `runs.resume` → `structuredContent.ok == true`, and the resume marker records actor `mcp` (Plan 1's `decided_by` helper pattern).
- `runs_delete_is_marked_destructive`.

### Task 6: Sessions area (7 tools)

**File:** `ops/sessions.rs`. **Test:** `mcp_ops_sessions.rs`.

| Tool | Route | Query / body | Hints |
|---|---|---|---|
| `sessions.list` | GET `/api/sessions` | `sessions::SessionsQuery` | |
| `sessions.get` | GET `/api/sessions/:id` | `sessions::SessionHostQuery` | |
| `sessions.turns` | GET `/api/sessions/:id/runs` | `SessionHostQuery` | |
| `sessions.usage_timeline` | GET `/api/sessions/:id/usage-timeline` | `SessionHostQuery` | |
| `sessions.archive` | POST `/api/sessions/:id/archive` | `SessionHostQuery` | |
| `sessions.restore` | POST `/api/sessions/:id/restore` | `SessionHostQuery` | |
| `sessions.delete` | DELETE `/api/sessions/:id` | `SessionHostQuery` | |

**Task-specific test:** `sessions_list_with_a_query_filters_like_the_api` (`q: "is:active"`).

### Task 7: Agentiflows area (6 tools)

**File:** `ops/agentiflows.rs`. **Test:** `mcp_ops_agentiflows.rs`.

| Tool | Route | Query / body | Hints |
|---|---|---|---|
| `agentiflows.list` | GET `/api/agentiflows` | — | |
| `agentiflows.get` | GET `/api/agentiflows/:id` | — | |
| `agentiflows.events` | GET `/api/agentiflows/:id/events` | — | |
| `agentiflows.children` | GET `/api/agentiflows/:id/children` | — | |
| `agentiflows.unit_output` | GET `/api/agentiflows/:id/units/:unit_id/output` | — | |
| `agentiflows.steer` | POST `/api/agentiflows/:id/steer` | `agentiflows::SteerRequest` | |

**Task-specific test:** `steer_through_mcp_records_client_mcp`. On a live agentiflow space (the fixture `messages.rs` uses), `agentiflows.steer {id, message: "go"}` → the log line has `"via":"cp"` and `"client":"mcp"`. The handler already threads `ViaMcp`.

### Task 8: Definitions area — agents, workflows, autoflows, claims, skills, tools, models (23 tools)

**File:** `ops/definitions.rs`. **Test:** `mcp_ops_definitions.rs`.

`repo_scope::ScopeKind` gets `#[derive(schemars::JsonSchema)]` (it serializes snake_case; the schema follows serde).

| Tool | Route | Query / body | Hints |
|---|---|---|---|
| `agents.get` | GET `/api/agents/:name` | — | |
| `agents.create` | POST `/api/agents` | `agents::AgentWriteBody` | |
| `agents.save` | PUT `/api/agents/:name` | `repo_scope::ScopeQuery` / `AgentWriteBody` | |
| `agents.delete` | DELETE `/api/agents/:name` | `ScopeQuery` | |
| `agents.generate` | POST `/api/agents/generate` | `agents::GenerateAgentBody` | open_world |
| `workflows.get` | GET `/api/workflows/:name` | — | |
| `workflows.create` | POST `/api/workflows` | `workflows::WorkflowWriteBody` | |
| `workflows.save` | PUT `/api/workflows/:name` | `ScopeQuery` / `WorkflowWriteBody` | |
| `workflows.delete` | DELETE `/api/workflows/:name` | `ScopeQuery` | |
| `workflows.validate` | POST `/api/workflows/validate` | `WorkflowWriteBody` | read_only |
| `workflows.generate` | POST `/api/workflows/generate` | `workflows::GenerateWorkflowBody` | open_world |
| `generate.models` | GET `/api/generate/models` | — | |
| `autoflows.list` | GET `/api/autoflows` | — | |
| `autoflows.enable` | POST `/api/autoflows/:name/enable` | `ScopeQuery` | |
| `autoflows.disable` | POST `/api/autoflows/:name/disable` | `ScopeQuery` | |
| `claims.list` | GET `/api/autoflows/claims` | — | |
| `claims.release` | POST `/api/autoflows/claims/release` | `autoflow_claims::ReleaseBody` | |
| `claims.requeue` | POST `/api/autoflows/claims/requeue` | `autoflow_claims::RequeueBody` | |
| `skills.list` | GET `/api/skills` | `skills::SkillsQuery` | |
| `skills.get` | GET `/api/skills/:layer/:name` | `SkillsQuery` | |
| `tools.list` | GET `/api/tools` | — | |
| `models.list` | GET `/api/models` | — | |
| `models.refresh` | POST `/api/models/refresh` | `models::RefreshBody` | open_world |

Notes:
- `agents.get` and `workflows.get` shadow the catalog's namesakes (`sources()` does it).
- `workflows.generate` shadows the catalog launch tool of the same name, which the MCP context can't build anyway.
- `models.refresh` reads its body as `Bytes`. The tool always sends a JSON object (`{}` when no arguments are given), which `parse_refresh_body` accepts.

**Task-specific tests:**
- `agent_create_save_get_delete_round_trip`: create `t-agent` from a minimal `raw`, then `agents.get` shows it, then `agents.save` with an edited description, then `agents.get` shows the edit, then `agents.delete`, then `agents.get` is `isError` `HTTP 404`.
- `workflows_validate_reports_errors_as_json`: invalid YAML → a 2xx or 4xx exactly as the API answers, compared through plain HTTP.
- `agents_get_is_the_operator_tool_not_the_catalogs`: with a catalog source that also serves `agents.get`, the call reaches the CP route (the result has the route's `AgentDetailDto` keys).

### Task 9: Findings, coverage, assets (16 tools) + `client: mcp` on tag events

**Files:** `ops/findings.rs`, `crates/rupu-coverage/src/ledger/tags.rs`, `crates/rupu-cp/src/api/findings.rs`. **Test:** `mcp_ops_findings.rs`, `crates/rupu-coverage/tests/it/finding_tags.rs`.

| Tool | Route | Query / body | Hints |
|---|---|---|---|
| `findings.list` | GET `/api/findings` | `findings::FindingsQuery` | |
| `findings.get` | GET `/api/findings/:id` | `findings::FindingWsQuery` | |
| `findings.tags` | GET `/api/findings/tags` | `findings::TagsQuery` | |
| `findings.tag` | POST `/api/findings/tags` | `findings::TagFindingsBody` (+ `FindingTarget` derives) | |
| `findings.export` | GET `/api/findings/:id/export` | `findings::ExportQuery` | bytes |
| `findings.export_many` | POST `/api/findings/export` | `findings::ExportBody` | read_only, bytes |
| `findings.artifact` | GET `/api/findings/:id/artifacts/:sha256` | `FindingWsQuery` | bytes |
| `coverage.list` | GET `/api/coverage` | — | |
| `coverage.get` | GET `/api/coverage/:target` | `coverage::GetCoverageQuery` | |
| `coverage.audit` | GET `/api/coverage/:target/audit` | `GetCoverageQuery` | |
| `coverage.catalog` | GET `/api/coverage/:target/catalog` | `GetCoverageQuery` | |
| `coverage.runs` | GET `/api/coverage/:target/runs` | `GetCoverageQuery` | |
| `coverage.diff` | GET `/api/coverage/:target/diff` | `coverage::DiffQuery` | |
| `coverage.templates` | GET `/api/coverage/templates` | — | |
| `coverage.template` | GET `/api/coverage/templates/:name` | — | |
| `assets.list` | GET `/api/assets` | `assets::AssetQuery` | |

**Shadowing:**
- `findings.tag` shadows the catalog's `findings.tag`. Its arguments are the CP's (`finding_ids` / `findings`, `add`, `remove`), and it is attributed to the operator.
- The catalog's `findings.query` stays served, beside `findings.list`.
- No name here collides with the catalog's `coverage.{status,remaining,mark,concerns.*}`.

**Attribution:**
- `OperatorAttribution` gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub client: Option<String>`.
- `TagActor::operator(via)` keeps its signature and sets `client: None`. Add `TagActor::operator_with_client(via, client: Option<&str>)`.
- `tag_findings` takes `crate::mcp::ViaMcp` and builds `TagActor::operator_with_client(OperatorSurface::Cp, via.is_some().then_some("mcp"))`.
- The two `OperatorAttribution { .. }` literals in tests (`tags.rs` test, `ingest.rs` test) get `client: None`.

**Task-specific tests:**
- rupu-coverage `an_operator_tag_line_without_client_parses_and_round_trips_without_it`: an older line `{"by":{"kind":"operator","user":"u","via":"cp"},…}` parses with `client == None` and re-serializes byte-equal.
- `tags_through_mcp_record_client_mcp_and_keep_via_cp`: `findings.tag {finding_ids: [F], add: ["needs-poc"]}` → the tag log line's `by` is `{"kind":"operator","user":…,"via":"cp","client":"mcp"}`. A plain HTTP POST writes no `client` key.
- `a_pdf_export_is_a_blob_resource` (skip with a printed reason when the `pdf` feature is off): `findings.export {id: F, format: "pdf"}` → `resource.mimeType == "application/pdf"`, `resource.blob` decodes to bytes starting `%PDF`, and `resource._meta.name` ends in `.pdf`.
- `a_markdown_export_is_a_text_resource`: `format: "md"` → `resource.text` contains the finding's title.
- `an_artifact_the_finding_does_not_reference_is_an_error`: `findings.artifact {id: F, sha256: <other>}` → `isError` `HTTP 404`.

### Task 10: Observability — netflow, usage, dashboard (13 tools)

**File:** `ops/observe.rs`. **Test:** `mcp_ops_observe.rs`.

| Tool | Route | Query / body | Hints |
|---|---|---|---|
| `netflow.run` | GET `/api/runs/:id/netflow` | `netflow::TimeRangeQuery` | |
| `netflow.agentiflow` | GET `/api/agentiflows/:id/netflow` | `TimeRangeQuery` | |
| `netflow.project` | GET `/api/projects/:id/netflow` | `TimeRangeQuery` | |
| `netflow.global` | GET `/api/netflow` | `TimeRangeQuery` | |
| `netflow.graph` | GET `/api/netflow/graph` | `netflow::GraphQuery` | |
| `netflow.explorer` | GET `/api/netflow/explorer` | `netflow::ExplorerQuery` | |
| `netflow.index` | GET `/api/netflow/index` | — | |
| `netflow.latest` | GET `/api/netflow/latest` | `netflow::LatestQuery` | |
| `usage.get` | GET `/api/usage` | `usage::UsageQuery` | |
| `usage.timeline` | GET `/api/usage/timeline` | `usage::TimelineQuery` | |
| `usage.runs` | GET `/api/usage/runs` | `usage::UsageRunsQuery` | |
| `usage.outliers` | GET `/api/usage/outliers` | `usage_outliers::OutliersQuery` | |
| `dashboard.get` | GET `/api/dashboard` | `dashboard::DashboardQuery` | |

**Descriptions:** the netflow tools' `host` argument is a flow filter (`host:port`), not a rupu host. Their descriptions say so, since every other tool's `host` picks a rupu host.

**Task-specific test:** `usage_runs_carries_the_without_customer_note`. With a peer host too old to report customers, `usage.runs {customer: "acme"}` → the `x-rupu-hosts-without-customer` note.

### Task 11: Fleet, config, customers, launch preview, repos (25 tools)

**File:** `ops/fleet.rs`. **Test:** `mcp_ops_fleet.rs`.

| Tool | Route | Query / body | Hints |
|---|---|---|---|
| `hosts.list` | GET `/api/hosts` | — | |
| `hosts.registered` | GET `/api/hosts/registered` | — | |
| `hosts.add` | POST `/api/hosts` | `hosts::AddHostBody` | |
| `hosts.add_ssh` | POST `/api/hosts/ssh` | `hosts::AddSshHostBody` | |
| `hosts.add_bucket` | POST `/api/hosts/bucket` | `hosts::AddBucketHostBody` | |
| `hosts.enroll_node` | POST `/api/hosts/node` | `hosts::EnrollNodeBody` | |
| `hosts.remove` | DELETE `/api/hosts/:id` | — | |
| `workers.list` | GET `/api/workers` | — | |
| `host.info` | GET `/api/host/info` | — | |
| `config.get` | GET `/api/config` | `config::ConfigQuery` | |
| `config.put_global` | PUT `/api/config/global` | `config::ConfigWriteBody` | destructive |
| `config.put_customer` | PUT `/api/config/customer/:slug` | `ConfigWriteBody` | destructive |
| `config.put_project` | PUT `/api/config/project/:id` | `ConfigWriteBody` | destructive |
| `config.put_policy` | PUT `/api/config/policy` | `config::PolicyBody` | destructive |
| `customers.list` | GET `/api/customers` | `customers::RangeQuery` | |
| `customers.create` | POST `/api/customers` | `customers::CreateBody` | |
| `customers.get` | GET `/api/customers/:slug` | `RangeQuery` | |
| `customers.update` | PATCH `/api/customers/:slug` | `customers::PatchBody` | |
| `customers.delete` | DELETE `/api/customers/:slug` | — | |
| `customers.archive` | POST `/api/customers/:slug/archive` | — | |
| `customers.unarchive` | POST `/api/customers/:slug/unarchive` | — | |
| `customers.assign_project` | PUT `/api/customers/:slug/projects/:ws_id` | — | |
| `customers.unassign_project` | DELETE `/api/customers/:slug/projects/:ws_id` | — | |
| `launch.preview` | POST `/api/launch/preview` | `launch_preview::PreviewBody` | read_only |
| `repos.list` | GET `/api/repos` | — | open_world |

**Task-specific tests:**
- `enroll_node_builds_its_command_from_the_mcp_host`: the `/mcp` request is sent with `Host: 10.1.2.3:7879`; `hosts.enroll_node {name: "n1"}` → the returned command names `10.1.2.3:7879`. Run it against `serve_on`'s `launcher` (501 without it), as the existing hosts tests do.
- `customers_delete_answers_204_as_status`: `customers.delete` on a customer with no projects → `structuredContent == {"status": 204}`.
- `config_writes_are_destructive_and_idempotent`.

### Task 12: Projects area (12 tools) — and the pending list goes

**File:** `ops/projects.rs`, `ops/mod.rs`. **Test:** `mcp_ops_projects.rs`, `mcp_routes.rs`.

| Tool | Route | Query / body |
|---|---|---|
| `projects.list` | GET `/api/projects` | `projects::ProjectsQuery` |
| `projects.get` | GET `/api/projects/:ws_id` | — |
| `projects.runs` | GET `/api/projects/:ws_id/runs` | `projects::ProjectListQuery` |
| `projects.sessions` | GET `/api/projects/:ws_id/sessions` | `ProjectListQuery` |
| `projects.agents` | GET `/api/projects/:ws_id/agents` | — |
| `projects.workflows` | GET `/api/projects/:ws_id/workflows` | — |
| `projects.autoflows` | GET `/api/projects/:ws_id/autoflows` | — |
| `projects.coverage` | GET `/api/projects/:ws_id/coverage` | — |
| `projects.coverage_assessed` | GET `/api/projects/:ws_id/coverage/assessed` | — |
| `projects.tree` | GET `/api/projects/:ws_id/tree` | `code::TreeQuery` |
| `projects.source` | GET `/api/projects/:ws_id/source` | `TreeQuery` |
| `projects.files` | GET `/api/projects/:ws_id/files` | — |

**Final step of this task:**
- Delete `PENDING_PLAN_2` and its use in `mcp_routes.rs`. The test then asserts every scanned pair is in `ops()` ∪ `EXCLUDED`.
- Add `excluded_is_exactly_the_spec_list`: `EXCLUDED`'s pairs equal a literal copy of the spec M4 exclusions plus `POST /mcp`.
- Recount: `ops().len() == 138`, and `tool_names.json` has 138 entries.

**Task-specific test:** `projects_source_returns_file_lines_as_json`. `projects.source {ws_id, path: "README.md"}` on a seeded workspace returns the handler's `FileContent` JSON (the route is JSON, not bytes).

---

### Task 13: `wait` — split into modules, project-local runs, string cursor values

**Files:**
- Move: `crates/rupu-cp/src/mcp/wait.rs` → `wait/mod.rs` (loop, `Got`, `WaitSource`, `tool_def`, `clamp_timeout`, constants), `wait/parse.rs` (`Request`, `Condition`, `Kind`, `input_schema`), `wait/local.rs` (local run/message feeds and state checks). The public paths `rupu_cp::mcp::wait::{clamp_timeout, DEFAULT_TIMEOUT, MAX_TIMEOUT, tool_def, WaitSource}` stay.
- Test: `crates/rupu-cp/tests/it/mcp_wait.rs`

**Behavior changes:**
- **A run condition resolves the run through `api::run_resolve::resolve_run_location`:**
  - `Global` → `state.run_store`
  - `ProjectLocal { path }` → that project's `RunStore`, built the way `api::runs` builds it for this variant
  - `Host { host_id }` → as if `host: <host_id>` was given (an error until Task 16)
  - `Unpersisted` → `run_settled` fires at once with `{status: "failed", unpersisted: true}`, and `run_gate` / `run_events` are errors naming the run

  Plan 1 read only the global store, so a project-local run answered "no run".
- **Cursor values** may be a non-negative integer (line ids) or a string (remote message ids, Task 16). The schema becomes `additionalProperties: {"type": ["integer", "string"], "minimum": 0}`. A local source given a string is an error naming the key.

- [ ] **Step 1: Write the failing tests:**
  - `a_project_local_run_is_found`: register a workspace whose `.rupu/runs/` holds a settled run; `wait {run_settled}` fires.
  - `a_string_cursor_on_a_local_source_is_refused`.
  - All of Plan 1's `mcp_wait::` tests stay unchanged.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-cp --test it mcp_wait::`
  Expected: the two new tests FAIL.
- [ ] **Step 3: Implement** the split (move first, then change) and the two behaviors.
- [ ] **Step 4: Run them and confirm they pass,** plus `cargo test -p rupu-cp --lib mcp::wait`.
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "cp mcp wait: modules, project-local runs, string cursor values"`

### Task 14: `wait` — `agentiflow`, `agent_run_end`, `session_turn`

**Files:** Create `wait/turns.rs`; modify `wait/{parse.rs,local.rs,mod.rs}`; make `api::sessions::load_session_file` `pub(crate)`; add `pub(crate) fn agent_run_transcript(global: &Path, run: &str) -> Option<PathBuf>` to `api/run_streams.rs`. That function looks for `<global>/transcripts/<run>.jsonl`, then `<project>/.rupu/transcripts/<run>.jsonl` for each registered workspace: the dirs `list_agent_runs` scans. **Test:** `mcp_wait.rs`.

**Conditions** (all local only; a `host` is an error, as spec M7 gives these no `host`):

| Condition | Source / check | Fires | `detail` |
|---|---|---|---|
| `{kind: "agentiflow", id, types?}` | `NumberedTail<serde_json::Value>` on `<agentiflows dir>/<id>/events.jsonl`, source `agentiflow:<id>`; `types` filters the line's `kind` | Collects lines | `{count}` |
| `{kind: "agent_run_end", run}` | The transcript from `agent_run_transcript`; re-checked every 1 s and on every line of the file's tail | It has a `run_complete` | `{status, error, total_tokens, final_answer}`, from `JsonlReader::summary` + `rupu_transcript::final_answer` |
| `{kind: "session_turn", session, run?}` | `<global>/sessions/<id>/session.json`, then `sessions-archive/` (`load_session_file`). The turn is `run` if given, else `active_run_id`, else the last `runs[]` entry. Re-checked every 1 s | The turn's transcript has `run_complete`, or the session `status` is not `"running"` | `{session, run_id, status, final_answer, error}`; `run_id` / `final_answer` `null` when the session ended with no turn |

**Appear grace:** `APPEAR_GRACE = 10 s`.
- For `agent_run_end`, an unknown transcript keeps being looked for until the grace runs out, then the call is an error: ``no agent run `<run>` ``.
- For `session_turn`, the same holds for a `run` not yet in `runs[]` and for an unknown session.
- `agentiflow` has no grace: the flow dir exists before its id is returned.

- [ ] **Step 1: Write the failing tests:**
  - `agentiflow_lines_are_returned_and_filtered_by_kind`: write `round` + `run_stopped` lines; `types: ["run_stopped"]` returns 1 event, and the cursor is `{"agentiflow:<id>": 2}`.
  - `an_ended_agent_run_fires_with_its_final_answer`: a transcript fixture ending in `run_complete` with an assistant message `"done."` → `detail.final_answer == "done."`.
  - `an_agent_run_that_appears_late_still_fires`: start the wait; after 1.5 s, write the transcript with `run_complete` → it fires, `isError` false.
  - `an_agent_run_that_never_appears_is_an_error_after_the_grace`: `timeout_ms: 30000` → `isError` within 12 s, naming the run.
  - `a_session_turn_fires_with_the_turns_answer`: a `session.json` with `status: "running"`, `active_run_id: R`, and `R`'s transcript completed → fires with `run_id == R` and the answer.
  - `a_session_that_stopped_fires_without_a_turn`: `status: "stopped"`, no runs → `run_id == null`.
  - `a_host_on_these_conditions_is_refused`.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-cp --test it mcp_wait::`
  Expected: FAIL.
- [ ] **Step 3: Implement** them, with the schema's `oneOf` extended by the three shapes and the description listing all kinds.
- [ ] **Step 4: Run them and confirm they pass.**
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "cp mcp wait: agentiflow events, agent run ends, session turns"`

### Task 15: `wait` — `findings`

**Files:**
- Create: `wait/findings.rs`
- Modify: `wait/{parse.rs,mod.rs}`
- In `api/findings.rs`, extract and make `pub(crate)` the code `list_findings` uses to turn a workspace's ledger records into `FindingOut`s and their slim list rows, so `wait` and the list share it: `fn finding_out(ws: &Workspace, target: &str, record: FindingRecord) -> FindingOut`, `fn list_row(f: &FindingOut) -> Value`.

**Test:** `mcp_wait.rs`.

**Condition:** `{kind: "findings", project?: <ws id>, run?: <run id>, q?: <findings query>}`. It is local only: the API has no per-host findings list, and remote units' findings are ingested into the coordinator's own ledgers.

- **Sources:** each `<ws path>/.rupu/coverage/<target>/findings.jsonl` of the scoped workspaces (`project`, else every registered one), source `findings:<ws_id>/<target>`. Target dirs are re-listed every 1 s, so a ledger created mid-wait is picked up from line 0.
- **Where reading starts:** without a cursor entry, a source starts at its current end, so a call returns only findings recorded after it began. Exception: when `run` is set, it starts at 0, since a run's findings are bounded and wanted whole.
- **Matching:**
  - New lines are parsed as `rupu_coverage::FindingRecord`; an unparseable line is skipped.
  - Each becomes a `finding_out`. `join_workflow_names` runs first.
  - Then the filters apply: `run` through `resolve_run_scope` (the run and everything under it), and `q` through `query_findings` over just the new records, with tags as the workspace's tag log has them now.
  - Each kept record is an event `{source, id: <line>, event: list_row(f)}`.
- **A bad `q`** is `isError` whose text is the same JSON the list answers on a 400 (`{error, token, code, start, end}`). It is checked before any wait.
- **Not covered:** a tag change on an older finding appends no finding line and so fires nothing. The tool description says so.

- [ ] **Step 1: Write the failing tests:**
  - `no_cursor_returns_only_findings_recorded_after_the_call_began`: seed 3 findings; start `wait {findings, project}`; append one → exactly 1 event.
  - `a_run_scope_reads_the_runs_findings_from_the_start`: 2 seeded findings declared by `run_a`, 1 by `run_b`; `{findings, run: "run_a", timeout_ms: 300}` → 2 events at once.
  - `q_filters_new_findings`: append a low and a high finding; `q: "severity>=high"` → 1 event.
  - `a_bad_q_is_an_error_with_its_token_and_code`: `q: "severity>>high"` → `isError`, the text parses as JSON with `code` and `token`.
  - `a_ledger_created_mid_wait_is_read_from_its_first_line`.
  - `the_cursor_resumes_per_ledger`.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-cp --test it mcp_wait::`
  Expected: FAIL.
- [ ] **Step 3: Implement** them (poll-based; `CATCH_UP_MAX` per source per read).
- [ ] **Step 4: Run them and confirm they pass,** plus `cargo test -p rupu-cp --lib api::findings`.
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "cp mcp wait: new finding records, scoped and queried"`

### Task 16: `wait` — remote hosts

**Files:** Create `wait/remote.rs`; modify `wait/{mod.rs,parse.rs}`, `crates/rupu-cp/src/sse.rs`. **Test:** `mcp_wait.rs` (peer served by `rupu_cp::server::router` and registered with `state.hosts.add_host("peer", url, None)`, as `list_query.rs` does).

**Interfaces:**
- **Produces** in `sse.rs`:
  ```rust
  /// The frames of a run event stream as `RunFrame::to_sse_bytes` writes them.
  /// Partial frames across chunks are buffered; an unknown event name is skipped.
  pub fn parse_run_frames(bytes: EventByteStream) -> impl Stream<Item = std::io::Result<RunFrame>> + Send;
  ```
- **Host resolution:** `host` other than `local` resolves through `api::runs::resolve_host`. An unknown host is `isError` ``unknown host `<id>` ``. Remote source keys are the local key plus `@<host>` (`run:<id>@peer`, `messages:<root>@peer`).

**Remote conditions:**

| Condition | How | Cursor |
|---|---|---|
| `run_events`, `run_settled` | One `stream_run_events_after(run, cursor)` per run per call, parsed with `parse_run_frames`. `Event` frames feed `run_events`; the `End` frame fires `run_settled`, with detail `{status}` from a single `get_run` | Integer line ids, as local |
| `run_gate` | `get_run(run)` at the start and every `REMOTE_POLL = 2 s`; gates from the JSON's `awaiting` (and legacy `awaiting_step_id`), same detail as local | — |
| `messages` | `list_messages(root, ViewQuery { after: cursor, limit: 200, kinds })` at the start and every `REMOTE_POLL` | String: the last message id returned |

**Pacing:** each host's poll requests in a round are issued one after another (at most one in flight per host). Event streams are long-lived and count as that host's one stream, not polls.

**Errors:**
- The first request for a source failing is `isError`. `HostConnectorError::Unsupported` / a 501 reads ``host `<id>` can't <what> (<reason>)``. A 404 reads ``no run `<id>` on host `<host>` ``.
- A later failure is retried next round. The result gains `warnings: ["<source>: <error>"]` (absent when empty). The call still returns at its timeout.
- A stream that ends without `End` is reopened from its cursor on the next round.

- [ ] **Step 1: Write the failing tests:**
  - `sse_frames_parse_across_chunk_boundaries` (unit, `sse.rs`): frames split mid-line across three chunks parse to `[Event{1}, Event{2}, End]`.
  - `remote_run_events_resume_with_the_cursor`: a run on the peer with 2 events → ids `[1, 2]` and cursor `{"run:<id>@peer": 2}`; append 1 on the peer, wait with the cursor → `[3]`.
  - `a_remote_settled_run_fires`.
  - `a_remote_parked_gate_fires`.
  - `remote_messages_poll_with_a_string_cursor`: send an operator message on the peer's live space → 1 event, cursor value is the `msg_…` id; a second wait with it → `timed_out` at 300 ms.
  - `an_unknown_host_is_an_error`.
  - `a_host_that_goes_away_mid_wait_returns_by_the_timeout_with_a_warning`: start `wait {run_events, host: peer, timeout_ms: 3000}`, stop the peer after 500 ms → returns within 4 s, `warnings` non-empty, no `isError`.
- [ ] **Step 2: Run them and confirm they fail.**
  Run: `cargo test -p rupu-cp --test it mcp_wait:: && cargo test -p rupu-cp --lib sse::`
  Expected: FAIL.
- [ ] **Step 3: Implement** `parse_run_frames` and `remote.rs`. Remove Plan 1's "remote not supported" error and update the schema's `host` description.
- [ ] **Step 4: Run them and confirm they pass.**
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "cp mcp wait: remote runs over the event stream, remote messages by polling"`

### Task 17: `wait` — progress while waiting

**Files:** `wait/mod.rs`. **Test:** `mcp_wait.rs`.

**Behavior:**
- When `cx.progress` is `Some`, the wait loop reports at the start and every `PROGRESS_EVERY = 5 s` until it returns: `report(elapsed_ms, Some(timeout_ms), Some("waiting: <kinds watched, comma-separated>"))`.
- `progress` grows strictly with each report.
- There is no report after the result is built.

- [ ] **Step 1: Write the failing test** `a_wait_with_a_progress_token_reports_while_waiting`: over `/mcp` with SSE accepted and `_meta.progressToken`, `wait {run_events on a quiet run, timeout_ms: 11000}` → at least 2 progress frames, then the result with `timed_out: true`.
- [ ] **Step 2: Run it and confirm it fails.**
  Run: `cargo test -p rupu-cp --test it mcp_wait::a_wait_with_a_progress`
  Expected: FAIL.
- [ ] **Step 3: Implement** it.
- [ ] **Step 4: Run it and confirm it passes,** plus `cargo test -p rupu-cp --test it mcp_wait::`.
  Expected: PASS.
- [ ] **Step 5: Commit.**
  `git commit -m "cp mcp wait: notifications/progress while waiting"`

### Task 18: Docs and live check

**Files:** `docs/mcp.md`, `docs/cp-api.md` (one paragraph pointing at the MCP for agents), `CLAUDE.md` (the `rupu-cp` entry's MCP sentence), `CHANGELOG.md`.

- [ ] **Step 1: Update `docs/mcp.md`:**
  - **Operator tools:** the tool areas with their tool names, generated from `tool_names.json` and grouped as the area files are.
  - **Results:** how `x-rupu-*` notes and byte results (embedded resources) come back.
  - **`wait`:** every condition kind, cursors (integers locally; message ids for remote messages), remote hosts, `warnings` and progress.
  - **Troubleshooting:** the "missing tool" row says a catalog tool whose service is missing is "logged at `info` level at startup (`RUST_LOG=info`)", instead of "logged at startup". Add the `--tools` did-you-mean.
- [ ] **Step 2: Live check.**
  - Start `rupu cp serve --no-web --bind 127.0.0.1:7879` with an isolated `RUPU_HOME` (copy `~/.rupu/config.toml` only). Then `claude mcp add --transport http rupu-test http://127.0.0.1:7879/mcp`.
  - From `claude -p`:
    1. `agents.create` a tiny agent, then `agents.run` it.
    2. `wait {agent_run_end}` and read its `final_answer`.
    3. `workflows.run` a gated sample workflow, `wait {run_gate}`, `runs.approve`, then `wait {run_settled}`.
    4. `findings.list`, then `findings.tag` one finding.
    5. `findings.export` it as markdown.
    6. Check that the tag log line has `client: mcp`.
  - Record the excerpt in the PR description, then `claude mcp remove rupu-test`.
- [ ] **Step 3: Commit.**
  `git commit -m "docs: operator MCP tool areas, wait conditions, results"`

---

## Self-review notes

**Spec coverage:**
- **M4's full table:** Tasks 5–12. The lockstep is reduced to the M4 exclusions in Task 12. The hints table is applied through the builder defaults plus the overrides in each table.
- **Byte results as embedded resources:** Tasks 1, 4 and 9.
- **Tool-name pin** (spec "Testing"): Task 4.
- **M7:**
  - `findings`: Task 15
  - `session_turn`, `agentiflow`, `agent_run_end`: Task 14
  - remote hosts: Task 16
  - progress: Tasks 2 and 17
- **Decision 7 for finding tags:** Task 9.
- **Plan 1 leftovers:**
  - `agents.get` / `workflows.get`: Task 8 (operator tools) and Task 3 (`FsCatalog`, which also serves `catalog.search`)
  - `x-rupu-*` headers: Task 4
  - tag `client: mcp`: Task 9
  - `--tools` did-you-mean, startup line: Task 3
  - the docs correction: Task 18

**Calls made here (check these):**
- **Remote hosts on `wait` are limited to `run_*` and `messages`**, the conditions spec M7 gives a `host`. `findings` is local only, although the delivery note mentions polling remote findings. The API has no per-host findings list to poll (`GET /api/findings` takes no `host`), and remote units' findings are already ingested into the coordinator's ledgers. A peer CP's own findings would need a new endpoint or `proxy_get_json` with no append-order cursor. Left for matt to rule on.
- **`messages`' `to_operator` filter from spec M7 is not built.** The bus refuses `to: operator`, so no line can match. `kinds: ["operator"]` already selects the operator's own lines.
- **`findings` without a cursor starts at the ledger's end, except when scoped to a run.** Run events and messages start at line 1 (Plan 1).
- **Appear grace is 10 s; remote poll interval is 2 s; progress every 5 s.**
- **`Unpersisted` runs fire `run_settled` as failed.**
- **The route guard** answers 400 before the handler, using `MatchedPath` in a `route_layer`.
- **The new tool names** are the ones in the tables: `runs.list_all`, `runs.archived`, `sessions.turns` for `/runs`, `findings.export_many`, `autoflow_runs.cycles`, `config.put_*`, `customers.assign_project`, and so on.
- **The startup line stays at `info` level**, with names filtered. The docs are corrected to match, rather than promoting the line to stdout.
- **No PDF size streaming:** byte results are read whole, up to the existing 16 MiB cap.
