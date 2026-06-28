# Task 8 Report: Host-aware run observation (proxy + fan-out)

## What was implemented

### 1. `HostConnector` trait — `get_transcript` method added
`crates/rupu-cp/src/host/connector.rs`: added `get_transcript(path: &str) -> Result<Value, HostConnectorError>` to the trait (the `EventByteStream` type alias was already present from the spec).

### 2. `LocalHostConnector::get_transcript`
`crates/rupu-cp/src/host/local.rs`: basic safety checks (`.jsonl` only, no `..`), then reads from disk using `rupu_transcript::JsonlReader`. Returns `{events:[], summary:null}` if file doesn't exist yet. Intentionally does NOT reproduce the full `allowed_roots` check (that is the HTTP handler's security boundary; the connector is trusted in-process).

### 3. `HttpHostConnector::get_transcript`
`crates/rupu-cp/src/host/http.rs`: forwards `GET /api/transcript?path=<path>` to the remote host and returns the JSON body.

### 4. `api/runs.rs` — fan-out list + host-aware get_run
- **Added** `FAN_OUT_LIMIT = 10_000` constant.
- **Added** `resolve_host(s, host_id)` helper (maps `HostConnectorError::NotFound` → 404).
- **Added** `fan_out_list_runs(s, kind, lifecycle)` async helper: calls `list_runs` on every host concurrently via `futures_util::future::join_all`, tags each row with `host_id`, merges, sorts newest-first by `started_at` (ISO-8601 lexicographic), and tolerates per-host failure (warn + skip).
- **`list_runs`**: new `RunsListQuery` struct adds `?host=<id>`. No `?host=` → fan-out all hosts + paginate merged result. `?host=<id>` → single-host with pass-through pagination. Return type changed to `Json<Vec<serde_json::Value>>` (was `Json<Vec<RunListRow>>`).
- **`list_workflow_runs`**: same pattern; `WorkflowRunsQuery` gets `host: Option<String>`.
- **`get_run`**: new `RunDetailQuery` with `host: Option<String>`. `?host=local` or absent → local path (unchanged). `?host=<remote>` → proxies via connector.

### 5. `api/events.rs` — host-aware SSE proxy
- Added `proxy_event_byte_stream(stream: EventByteStream) -> Response` helper: builds a `Response` with `Body::from_stream(stream)` and `text/event-stream` header.
- `events_stream` now extracts `?host=`. For a remote host: resolves connector, calls `stream_run_events(run_id)` (requires `?run=`), pipes the `EventByteStream` through `proxy_event_byte_stream`. For local/absent: keeps today's code path verbatim.

### 6. `api/transcript.rs` — host-aware proxy
- `PathQ` struct gains `host: Option<String>`.
- `get_transcript` routes remote requests to `connector.get_transcript(path)`, local requests through the existing `validate_transcript_path` + disk read path.

## Backward-compat parity preserved

- **Local-only fan-out**: with only the built-in `local` host registered, `GET /api/runs` calls `LocalHostConnector::list_runs` (which calls `query_run_rows`), tags each row with `host_id: "local"`, and re-paginates over the same sorted data. The existing tests (`list_runs_returns_seeded_run`, `list_runs_carries_trigger_field`, `list_runs_empty_when_no_runs`, `list_workflow_runs_*`) all pass without modification — they check specific fields, not exact-equality of the whole row object, so the additive `host_id` field is transparent to them.
- **`get_run` (no `?host=`)**: unchanged code path — still calls `query_run_detail` on the local store.
- **`events_stream` (no `?host=`)**: unchanged code path — still delegates to `tail_events_sse` or `tail_all_events_sse`.
- **`get_transcript` (no `?host=`)**: unchanged code path — still validates via `validate_transcript_path` + reads from disk.

## Existing tests touched

- `crates/rupu-cp/tests/host_registry.rs`: `StubLocal::get_transcript` stub added to satisfy the new trait method. One line, returns `unimplemented!()`.

No other existing test files were modified.

## New tests (`tests/run_observation.rs`) — 13 tests

1. `run_list_fan_out_merges_local_and_remote` — httpmock remote + local seed; both runs appear with correct `host_id` tags.
2. `run_list_fan_out_offline_host_does_not_fail` — host at port 1 registered; local run still appears, 200 status.
3. `run_list_single_local_host_tagged_with_local` — single-host fan-out tags rows `host_id: "local"`.
4. `run_list_host_param_scopes_to_one_host` — `?host=<remote>` returns only remote runs.
5. `run_list_unknown_host_returns_404`.
6. `get_run_proxies_to_remote_host` — `?host=<remote>` returns mock's detail JSON.
7. `get_run_unknown_host_returns_404`.
8. `get_run_local_no_host_param_unchanged` — regression: local get_run still works.
9. `events_stream_proxies_to_remote_host` — SSE frames from mock arrive in the proxied stream.
10. `events_stream_unknown_host_returns_404`.
11. `get_transcript_proxies_to_remote_host` — returns mock's transcript JSON.
12. `get_transcript_unknown_host_returns_404`.
13. `get_transcript_local_no_host_param_unchanged` — path outside roots → 400 (regression).

## Concerns / notes

- **Transcript security**: The local connector's `get_transcript` does basic `.jsonl` + no-`..` checks but NOT the full `allowed_roots` check from the HTTP handler. This is intentional: the connector is in-process trusted code; the handler-level check remains in place for all local requests (the connector path is only reached for remote host routing).
- **Fan-out pagination**: Uses `FAN_OUT_LIMIT = 10_000` per host to avoid re-pagination defeating the purpose. For runs in the tens of thousands this would be slow; a cursor API is the right fix but is out of scope for this slice.
- **`run_streams.rs`**: The autoflow/agent run endpoints in this file were NOT host-aware-ified; they have fundamentally different data sources (JSONL scan, not the RunStore) and are not mentioned in the task brief's endpoint list.

## Fix pass (review fixes)

Three fixes applied post-review:

1. **Fix 1 — `HttpHostConnector::list_runs` scoped to `host=local`** (`crates/rupu-cp/src/host/http.rs`): Added `("host", "local")` to the query tuple in the single `.query(&[...])` call that covers both `RunKind::All` and `RunKind::Workflow` branches. Prevents recursive fan-out when the remote CP is itself host-aware. Updated `list_runs_all_forwards_offset_and_limit` and `list_runs_workflow_hits_workflows_path` mocks in `tests/host_http.rs` to assert `.query_param("host", "local")`.

2. **Fix 2 — deferral documented** (`crates/rupu-cp/src/api/run_streams.rs`): Added a top-of-file `NOTE(multi-host slice 1)` comment explaining why `/api/runs/agents` and `/api/runs/autoflows` are local-only and what future work unlocks host-awareness. Appended a `TODO.md` entry under a new `## Multi-host (fleet)` section: `- [ ] multi-host: make /api/runs/agents + /api/runs/autoflows host-aware (needs HostConnector::list_agent_runs/list_autoflow_runs)`.

3. **Fix 3 — SSE proxy builder no longer panics** (`crates/rupu-cp/src/api/events.rs`): Changed `proxy_event_byte_stream` return type from `Response` to `Result<Response, ApiError>`. Replaced `.expect("valid response builder")` with `.map(|r| r.into_response()).map_err(|e| ApiError::internal(format!("event proxy response: {e}")))`. Updated call site from `Ok(proxy_event_byte_stream(stream))` to `proxy_event_byte_stream(stream)?`-style direct return via `return proxy_event_byte_stream(stream);`.

**Test result:** `cargo test -p rupu-cp --test host_http --test run_observation` — 24/24 pass. `cargo clippy -p rupu-cp` — clean. `cargo test -p rupu-cp` — 153/153 pass, no regressions.

## Final-review fix pass (backend)

Four fixes applied from the whole-branch review (I1, M4, M2, M1):

### I1 — `GET /api/runs/:id/log` host-aware SSE proxy

`crates/rupu-cp/src/api/runs.rs`: `get_run_log` now accepts `Query<RunDetailQuery>` (the same `host: Option<String>` struct already used by `get_run`). When `host` is present and != `"local"`, it resolves the connector via `resolve_host` and calls `conn.stream_run_events(run_id)`, then passes the `EventByteStream` through `proxy_event_byte_stream` — exactly mirroring `events_stream`'s remote-proxy path. Local path (absent or `"local"`) is unchanged. Unknown host → 404.

`crates/rupu-cp/src/api/events.rs`: `proxy_event_byte_stream` promoted from `fn` to `pub(crate) fn` so `runs.rs` can reuse it without duplication.

**New tests** (`tests/run_observation.rs`, 2 added, 15 total):
- `get_run_log_proxies_to_remote_host` — httpmock answers `GET /api/events/stream?run=remote_log_r1`; the test drives `GET /api/runs/remote_log_r1/log?host=<id>` and verifies the SSE frame arrives with the correct `run_id`.
- `get_run_log_unknown_host_returns_404` — unknown host id → 404.

### M4 — Warn on plaintext `http://` host URL

`crates/rupu-cp/src/host/registry.rs`: `HostRegistry::add_host` now emits `tracing::warn!` when `base_url` starts with `http://` before saving the record. The add is not blocked; the warning satisfies spec §Errors+security.

### M2 — `rupu host remove` lenient on keychain failure

`crates/rupu-cli/src/cmd/host.rs`: `remove_host` changed from propagating `delete_host_token` errors (which could fail the command post-deletion) to a best-effort `if let Err(e) = delete_host_token(&id) { tracing::warn!(...) }` pattern, matching `HostRegistry::remove_host`'s existing behaviour.

### M1 — Transcript unsafe-input caveat documented

`crates/rupu-cp/src/host/local.rs`: added a `// SAFETY/CAVEAT:` block comment above `LocalHostConnector::get_transcript` explaining that the HTTP handler enforces `allowed_roots` before reaching the connector, and that this method must not be called with untrusted paths.

**Test result:** `cargo test -p rupu-cp` — 165/165 pass (15 in `run_observation`, 0 regressions). `cargo clippy -p rupu-cp` — clean (no warnings). `cargo test -p rupu-cli --lib host` — 4/4 pass.
