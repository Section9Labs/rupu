# Multi-Host Slice 1.5 (Finish Federation) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make remote runs/sessions fully usable from the central CP — remote run graph/usage, host-aware agent/autoflow/session lists+detail, remote session launch — and theme the new Fleet/Host UI for dark mode.

**Architecture:** Adds ONE generic `HostConnector::proxy_get_json(path_and_query)` primitive (HTTP GET passthrough); each read handler reads `?host=` and either runs today's local logic (parity) or proxies to the resolved host. Run/session LISTS fan out across hosts and tag `host_id` (Slice 1 pattern). Web threads an optional `host` param and un-gates remote run detail. No new architecture — builds on Slice 1's `HostConnector` port + proxy/live-query model.

**Tech Stack:** Rust (axum, tokio, reqwest, async-trait, serde, thiserror), React + TypeScript + Tailwind (semantic theme tokens via `data-theme`), vitest, httpmock.

## Global Constraints

- Hexagonal: `proxy_get_json` is a `HostConnector` port method; `HttpHostConnector` is the adapter; `LocalHostConnector` returns `Invalid` for it (never called for local). `#![deny(clippy::all)]`; `unsafe_code` forbidden; library errors `thiserror`; workspace deps only (no versions in crate Cargo.toml).
- Default `?host=` is `"local"`. Local path = today's in-process logic UNCHANGED (parity; existing tests pass; `host_id` is additive on list rows).
- **Single-resource proxy** (run graph/usage, session detail/runs/usage): forward the path to the remote WITHOUT `?host=` (the remote serves its own local resource). **List proxy** (agent/autoflow/session lists): forward with `?host=local` so the remote does not itself fan out.
- Fan-out tolerates per-host failure (offline host contributes nothing, `tracing::warn`, never 500) — reuse the pattern in `crates/rupu-cp/src/sse.rs` `tail_all_events_sse` and Slice 1's `fan_out_list_runs` in `crates/rupu-cp/src/api/runs.rs`.
- Unknown `?host=` → `ApiError::not_found`. `proxy_get_json` error mapping matches the other HTTP methods (connect→Unreachable, 401→Unauthorized, 404→NotFound, ≥400→Remote).
- Web: theme tokens only for the migrated UI — `bg/panel/surface/border/ink{,-dim,-mute}/err{,-bg}/ok{,-bg}/warn{,-bg}/info{,-bg}/status.*/sev.*`; dark mode is `[data-theme="dark"]` on `<html>` (no `dark:` needed — tokens flip via CSS vars). Web build/test from `crates/rupu-cp/web`: `npx vitest run <file>`, `npx tsc -b`; the lead runs the consolidated `npm run build` at the end.

---

## File Structure

**Backend (rupu-cp)**
- `src/host/connector.rs` — add `proxy_get_json` to the `HostConnector` trait.
- `src/host/http.rs` — implement `proxy_get_json` (reuse the private `send`).
- `src/host/local.rs` — implement `proxy_get_json` → `Err(Invalid(..))`.
- `src/api/runs.rs` — `?host=` on `get_run_graph`, `get_run_usage_timeline`.
- `src/api/run_streams.rs` — `?host=` + fan-out on `/api/runs/agents`, `/api/runs/autoflows`, `/api/runs/autoflows/events`; update the Slice-1 NOTE.
- `src/api/sessions.rs` — `?host=` on `list_sessions` (fan-out), `get_session`, `get_session_runs`, `get_session_usage_timeline`.
- Tests: `tests/host_http.rs` (proxy_get_json), extend `tests/run_observation.rs` + `tests/sessions_*` (or a new `tests/host_reads.rs`), extend `tests/federation_e2e.rs`.

**Web (`crates/rupu-cp/web/src`)**
- `lib/api.ts` — optional `host` on the listed helpers.
- `pages/RunDetail.tsx` — remove remote gating.
- `pages/runs/AgentRuns.tsx`, `pages/runs/AutoflowRuns.tsx`, `pages/Sessions.tsx` — Host column + filter (reuse `WorkflowRuns.tsx` pattern).
- `components/AgentLauncherSheet.tsx` — re-enable remote sessions + nav.
- `pages/SessionDetail.tsx` — thread `?host=`.
- `components/ui/HostStatusBadge.tsx` (new) — shared host-status chip; used by `pages/Hosts.tsx`, `pages/HostDetail.tsx`, and list Host columns.
- Theme migration in `Hosts.tsx`, `HostDetail.tsx`, `HostSelect.tsx`.

---

## Task 1: `HostConnector::proxy_get_json`

**Files:** Modify `src/host/connector.rs`, `src/host/http.rs`, `src/host/local.rs`. Test `tests/host_http.rs`.

**Interfaces:**
- Produces: `async fn proxy_get_json(&self, path_and_query: &str) -> Result<serde_json::Value, HostConnectorError>` on `HostConnector`. `HttpHostConnector`: `GET {base_url}{path_and_query}` via the existing private `send(...)` (bearer + error mapping), parse JSON body. `LocalHostConnector`: `Err(HostConnectorError::Invalid("local host is served in-process".into()))`.

- [ ] **Step 1: Failing test** (`tests/host_http.rs`)

```rust
#[tokio::test]
async fn proxy_get_json_forwards_path_with_bearer() {
    let server = httpmock::MockServer::start_async().await;
    let m = server.mock(|when, then| {
        when.method("GET").path("/api/runs/agents").query_param("limit", "5")
            .header("authorization", "Bearer tok");
        then.status(200).json_body(serde_json::json!([{"run_id":"r1"}]));
    });
    let c = HttpHostConnector::new(server.base_url(), Some("tok".into()));
    let v = c.proxy_get_json("/api/runs/agents?limit=5").await.unwrap();
    assert_eq!(v[0]["run_id"], "r1");
    m.assert();
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rupu-cp --test host_http proxy_get_json` → FAIL (method missing).
- [ ] **Step 3: Implement** the trait method + both impls. HttpHostConnector reuses `send` (build `self.client.get(format!("{}{}", self.base_url, path_and_query))`, send, `resp.json().await` mapped to `HostConnectorError::Remote` on parse failure).
- [ ] **Step 4: Run** → PASS; `cargo clippy -p rupu-cp` clean.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp): HostConnector::proxy_get_json generic GET passthrough"`

---

## Task 2: Host-aware run graph + usage-timeline

**Files:** Modify `src/api/runs.rs` (`get_run_graph`, `get_run_usage_timeline`). Test extend `tests/run_observation.rs`.

**Interfaces:**
- Consumes: `proxy_get_json` (Task 1), `resolve_host` (existing in runs.rs).
- Both handlers gain `Query<RunDetailQuery>` (the `host: Option<String>` struct already exists in runs.rs from the Slice-1.x SSE fix — reuse it). Local/absent → existing logic. Remote → `resolve_host(&s, host)?.proxy_get_json(&format!("/api/runs/{id}/graph"))` (or `/usage-timeline`) — **no `?host=`** forwarded — and return `Json(value)`.

- [ ] **Step 1: Failing test** — `GET /api/runs/<id>/graph?host=<mock-remote>` returns the remote's graph JSON; `…/usage-timeline?host=<mock>` returns the remote's series. (Register the remote via `registry.add_host` + httpmock, mirroring existing `run_observation.rs` tests.)
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** — add `Query` to both handlers; branch local vs remote-proxy. Keep local path byte-for-byte.
- [ ] **Step 4: Run** `cargo test -p rupu-cp` (new + existing) + clippy → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp): host-aware run graph + usage-timeline (proxy)"`

---

## Task 3: Host-aware agent + autoflow run lists (fan-out)

**Files:** Modify `src/api/run_streams.rs` (`list_agent_runs`, `list_autoflow_runs`, `list_autoflow_events` + the NOTE comment). Test extend `tests/run_observation.rs` or new `tests/host_reads.rs`.

**Interfaces:**
- Consumes: `proxy_get_json`, `s.hosts` registry, the existing local collectors (`collect_standalone_runs`, `collect_session_runs_from_dir`, `AutoflowHistoryStore`).
- Each handler gains `host: Option<String>` in its query struct. Behavior:
  - **absent or "all":** fan out — for each `registry.list_hosts()`: local → existing collector rows; remote → `proxy_get_json("/api/runs/agents?<orig query>&host=local")` → `Vec<Value>`. Merge; set `host_id` on each row (a JSON field). Tolerate per-host failure.
  - **"local":** today's local logic, with `host_id:"local"` added to each row.
  - **single remote id:** `proxy_get_json("/api/runs/agents?<orig query>&host=local")` for that host; tag `host_id`.
  Return type becomes `Json<Vec<serde_json::Value>>` (rows + `host_id`) — OR keep the typed row and add an `#[serde] host_id` field. Prefer adding `host_id: Option<String>` (skip_serializing_if None defaults to none → fill "local"/id) to `AgentRunRow`/`AutoflowCycleRow`/`AutoflowEventRow` so the typed shape is preserved and existing tests stay green.

- [ ] **Step 1: Failing test** — agent-list fan-out merges local + a mock remote's agent runs, each tagged `host_id`; an offline host doesn't break it. Same for autoflow cycles.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** — add the `host_id` field to the three row structs; write a small `fan_out_proxy(s, path, local_rows_fn)` helper (or reuse/generalize Slice 1's fan-out) used by all three handlers. Update the file's Slice-1 "LOCAL-ONLY" NOTE to reflect these are now host-aware (claims still local).
- [ ] **Step 4: Run** `cargo test -p rupu-cp` + clippy → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp): host-aware agent + autoflow run lists (fan-out)"`

---

## Task 4: Host-aware sessions (list fan-out + detail/runs/usage proxy)

**Files:** Modify `src/api/sessions.rs` (`list_sessions`, `get_session`, `get_session_runs`, `get_session_usage_timeline`). Test new `tests/host_reads.rs` or extend.

**Interfaces:**
- `list_sessions`: add `host` to `SessionsQuery`; fan-out (local `collect_sessions` + remote `proxy_get_json("/api/sessions?<query>&host=local")`), tag each session object with `host_id`. `get_session`/`get_session_runs`/`get_session_usage_timeline`: add `Query<{host:Option<String>}>`; remote → `proxy_get_json("/api/sessions/{id}")` / `…/runs` / `…/usage-timeline` (no `?host=`); local unchanged. Unknown host → not_found.

- [ ] **Step 1: Failing test** — `GET /api/sessions?host=<all>` merges local + mock remote sessions (tagged `host_id`); `GET /api/sessions/:id?host=<mock>` proxies; offline host tolerated in the list.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** — reuse the Task-3 fan-out helper for the list; single-proxy for detail/runs/usage. Local paths unchanged.
- [ ] **Step 4: Run** + clippy → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp): host-aware sessions (list fan-out + detail/runs/usage proxy)"`

---

## Task 5: Web api helpers — thread `host`

**Files:** Modify `crates/rupu-cp/web/src/lib/api.ts`. Test `src/lib/api.host15.test.ts`.

**Interfaces:** add optional `host` to `getAgentRuns`, `getAutoflowRuns`, `getAutoflowEvents`, `getSessions`, `getSession`, `getSessionRuns`, `getRunGraph`, `getRunUsageTimeline`, and the session usage-timeline helper. Append `?host=<id>` (or merge into existing query) when provided; omit when not (additive — existing callers unaffected). Mirror the Slice-1 host-threading already present on `getRuns`/`getWorkflowRuns`/`getRun`.

- [ ] **Step 1: Failing test** — `getAgentRuns({host:'h1'})` hits `…?…host=h1`; omitted → no host; `getSession(id,{host})` appends `?host=`.
- [ ] **Step 2–4:** implement; `npx vitest run src/lib/api.host15.test.ts` + `npx tsc -b` → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp/web): thread host through agent/autoflow/session + graph/usage helpers"`

---

## Task 6: RunDetail — un-gate remote graph/usage

**Files:** Modify `pages/RunDetail.tsx`. Test extend `pages/RunDetail.test.tsx`.

**Interfaces:** Remove the Slice-1 `isRemote` gating that skipped `getRunGraph`/`getRunUsageTimeline` and rendered the "not available for remote" note. Now always call them, passing the run's `host` (from `?host=`). Keep the loading/error states.

- [ ] **Step 1: Failing test** — for a remote `?host=`, RunDetail NOW calls `getRunGraph` (mocked) and does NOT render the remote-unavailable note (invert the Slice-1 test).
- [ ] **Step 2–4:** implement; `npx vitest run src/pages/RunDetail.test.tsx` + `tsc -b` → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp/web): render graph + usage for remote runs"`

---

## Task 7: Host column + filter on Agent/Autoflow/Sessions lists

**Files:** Modify `pages/runs/AgentRuns.tsx`, `pages/runs/AutoflowRuns.tsx` (cycles + events tabs), `pages/Sessions.tsx`. Test extend each page's test.

**Interfaces:** Reuse the EXACT pattern from `pages/runs/WorkflowRuns.tsx` (Slice 1): a host filter defaulting to **"This host" (local)** that drives the fetch (`getAgentRuns({host})`, etc.), an **"All hosts"** option (omit host → fan-out), and a **Host** column showing `row.host_id ?? 'local'`. Rows deep-link with `?host=`.

- [ ] **Step 1: Failing test** — AgentRuns default fetch passes `host:'local'`; "All hosts" omits host; Host column renders `host_id`. (Mirror the WorkflowRuns Slice-1 tests.)
- [ ] **Step 2–4:** implement on all three pages; `npx vitest run` on the three + `tsc -b` → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp/web): Host column + filter on agent/autoflow/session lists"`

---

## Task 8: Remote sessions in the web

**Files:** Modify `components/AgentLauncherSheet.tsx`, `pages/SessionDetail.tsx`. Test extend both tests.

**Interfaces:**
- `AgentLauncherSheet`: REMOVE the Slice-1 gate that disabled the Session kind for remote hosts (and its "sessions run locally" note + the force-back-to-single-run effect). On a remote session launch, navigate to `/sessions/${id}?host=${encodeURIComponent(host)}` (local → `/sessions/${id}`).
- `SessionDetail`: read `?host=` via `useSearchParams`; thread it into `getSession(id,{host})`, `getSessionRuns(id,{host})`, the usage-timeline call, and `sendSessionMessage(id, text, host)` (host arg already supported from Slice 1). Show the owning host in the header.

- [ ] **Step 1: Failing test** — AgentLauncherSheet: Session kind is ENABLED for a remote host and a remote session launch navigates to `…?host=h1`; SessionDetail with `?host=h1` calls `getSession` with `{host:'h1'}`.
- [ ] **Step 2–4:** implement; `npx vitest run` on both + `tsc -b` → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp/web): remote sessions — launch + host-aware SessionDetail"`

---

## Task 9: Theme the new Fleet/Host UI

**Files:** Create `components/ui/HostStatusBadge.tsx`; modify `pages/Hosts.tsx`, `pages/HostDetail.tsx`, `components/HostSelect.tsx`, and the Host columns (from Task 7) to use it. Test `components/ui/HostStatusBadge.test.tsx`.

**Interfaces:** `HostStatusBadge({ status }: { status: 'online'|'offline'|'stale' })` → a `Chip` using semantic tokens: online → `bg-ok-bg text-ok ring-ok/30` (or the project's chip convention), stale → `warn`, offline → `err`. Replace the duplicated `HOST_STATUS_CLASS` maps in `Hosts.tsx`+`HostDetail.tsx` with this. Migrate the remaining raw classes: transport chip (`bg-blue-50/slate-100`) → `info`/neutral tokens; error banners `border-red-200 bg-red-50 text-red-700` → `border-err/30 bg-err-bg text-err`; `text-red-600`/`text-amber-600` → `text-err`/`text-warn`; `HostSelect.tsx` `bg-white` → `bg-panel`; `bg-slate-100` → `bg-surface`.

- [ ] **Step 1: Failing test** — `HostStatusBadge` renders the right token class per status (assert the className contains `ok`/`warn`/`err`); deduped helper used by both pages.
- [ ] **Step 2–4:** implement + migrate; `npx vitest run src/components/ui/HostStatusBadge.test.tsx` (+ Hosts test still green) + `tsc -b` → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp/web): theme Fleet/Host UI (dark mode) + shared HostStatusBadge"`

---

## Task 10: Extend federation e2e — remote session + remote graph

**Files:** Modify `crates/rupu-cp/tests/federation_e2e.rs`.

**Interfaces:** Extend the existing two-in-process-server test: after registering the remote, seed/start a session on the remote and assert the central `GET /api/sessions?host=<all>` lists it, `GET /api/sessions/:id?host=<remote>` loads it; and seed a remote run + assert `GET /api/runs/:id/graph?host=<remote>` returns the remote's graph via the central.

- [ ] **Step 1: Write the test** (extend the existing one or add a sibling `central_proxies_remote_session_and_graph`).
- [ ] **Step 2: Run** `cargo test -p rupu-cp --test federation_e2e` → PASS; `cargo test -p rupu-cp` full suite green.
- [ ] **Step 3: Commit** — `git commit -m "test(cp): federation e2e — remote session + remote run graph"`

---

## Final verification (lead, end of batch)

- [ ] `cargo test -p rupu-cp -p rupu-workspace` + `cargo clippy -p rupu-cp -p rupu-workspace` clean.
- [ ] `cd crates/rupu-cp/web && npx vitest run && npm run build` clean.
- [ ] Manual (matt): two `rupu cp serve` instances; from the central, open a remote workflow run (graph + usage now render), browse remote Agent/Autoflow runs + Sessions (Host column/filter), launch a **remote session** and open its detail + send a turn; toggle dark mode and confirm the Fleet/Hosts UI + host chips look right.

## Self-review (coverage)

- Spec §"generic proxy primitive" → Task 1. §"run graph/usage" → Task 2 + web Task 6. §"agent/autoflow lists" → Task 3 + web Task 7. §"sessions" → Task 4 + web Tasks 7 (list) + 8 (launch/detail). §"web api threading" → Task 5. §"theming" → Task 9. §"testing/e2e" → per-task + Task 10.
- Open questions resolved: single-resource proxies forward WITHOUT `?host=` (Tasks 2/4); list proxies forward `?host=local` (Tasks 3/4); default filter "this host" (Task 7).
