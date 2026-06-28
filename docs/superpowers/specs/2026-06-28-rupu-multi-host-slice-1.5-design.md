# rupu multi-host — Slice 1.5: finish federation

Status: approved (design), pending implementation plan
Date: 2026-06-28

## Context

Slice 1 (shipped in v0.26.0) gave the central CP federated control of remote
hosts running `rupu cp serve`: a `Host` entity + registry, a `HostConnector`
transport port (`LocalHostConnector` = host[0]; `HttpHostConnector` = client of
a remote CP), proxy/live-query state, and a Fleet/Hosts UI with host-attributed
**workflow** runs and remote launch/observe/control. See
`docs/superpowers/specs/2026-06-28-rupu-multi-host-slice-1-design.md`.

Slice 1 deliberately deferred four things that leave the federation half-usable:
1. Remote run **step-graph + usage-timeline** are gated off (those endpoints
   aren't host-aware) — a remote run's detail page is half-blind.
2. **Agent** and **autoflow** run *lists* are local-only (their endpoints read
   local filesystem sources the connector didn't model).
3. **Sessions** are local-only — the launcher disables remote sessions and
   `SessionDetail` isn't host-aware.
4. The new Fleet/Host UI components landed *after* the theming PR (#409), so
   they use raw color classes and don't adapt to dark mode.

Slice 1.5 finishes the federation: every remote run/session view works, and the
new UI is themed. It builds directly on Slice 1's `HostConnector` port and the
proxy/live-query model — no new architecture.

## Goals

- Remote runs' step-graph + usage-timeline render (un-gate RunDetail).
- Agent + autoflow run lists are host-aware (fan-out + host filter + Host
  column), matching the Workflow Runs list.
- Remote sessions work end-to-end: launch on a remote host, find it in the
  Sessions list, open its detail (turns/runs/usage), and send turns.
- The new Fleet/Host UI components render correctly in light **and** dark mode.

## Non-goals (later slices)

- Node-agent / pull / SSH transports (Slice 2).
- Distributed workflow steps (Slice 3).
- Central mirror/history; capability-based placement.
- Autoflow **claims** host-awareness (claims are an issue-tracker view, not
  runs) — they stay local.

## Architecture

### One generic proxy primitive

Slice 1 used *typed* connector methods (`get_run`, `list_runs`, …). Slice 1.5
needs ~8 more remote reads (run graph, usage-timeline, agent runs, autoflow
cycles + events, sessions list/detail/runs/usage-timeline). Rather than add a
typed method per endpoint, add **one** generic primitive to `HostConnector`:

```rust
async fn proxy_get_json(&self, path_and_query: &str)
    -> Result<serde_json::Value, HostConnectorError>;
```

- `HttpHostConnector` implements it: `GET {base}{path_and_query}` with the
  bearer token + the existing error mapping (connect→Unreachable, 401→
  Unauthorized, 404→NotFound, ≥400→Remote). `path_and_query` is an
  absolute API path including any query string (e.g.
  `/api/runs/agents?limit=200&lifecycle=active&host=local`).
- `LocalHostConnector` returns `HostConnectorError::Invalid("local host is
  served in-process")` — it is never called for local (handlers short-circuit
  local to their existing in-process logic).

Every host runs the *same* CP API, so "proxy this GET to host X verbatim" is the
natural primitive. SSE streaming keeps the existing `stream_run_events`.

### The handler pattern

Each newly host-aware read handler follows one shape:

- Read optional `?host=` (default `"local"`).
- **Local** (host absent or `"local"`): run today's in-process logic UNCHANGED
  (parity; existing tests pass).
- **Remote single-host** (`GET …:id…?host=<remote>`): `resolve_host(&s, host)`
  then `conn.proxy_get_json("<same path + query, with host stripped or set to
  local>")` and return the JSON verbatim. (When proxying a *single* run/session
  read, forward the path WITHOUT `?host=` so the remote serves its own local
  resource; for list endpoints forward `?host=local` so the remote does not
  itself fan out — mirrors Slice 1's `list_runs` `?host=local` scoping.)
- **List fan-out** (the run/session *lists*): iterate `registry.list_hosts()`
  concurrently; for each host, local → existing collector, remote →
  `proxy_get_json("<list path>?…&host=local")`; merge the row arrays, tag each
  row with `host_id`, and tolerate a per-host failure (offline host contributes
  nothing — never 500 the list). Reuse the tolerance pattern from
  `sse.rs::tail_all_events_sse` / Slice 1's `fan_out_list_runs`.

## Backend changes

Endpoints gaining `?host=` (path = `crates/rupu-cp/src/api/`):

- `runs.rs`: `GET /api/runs/:id/graph`, `GET /api/runs/:id/usage-timeline` —
  single-host proxy (un-gates remote RunDetail).
- `run_streams.rs`: `GET /api/runs/agents`, `GET /api/runs/autoflows`,
  `GET /api/runs/autoflows/events` — fan-out + `host_id` tagging. (Autoflow
  **claims** unchanged/local.) Update the file's existing "local-only" NOTE
  comment from Slice 1.
- `sessions.rs`: `GET /api/sessions` (fan-out + `host_id`),
  `GET /api/sessions/:id`, `GET /api/sessions/:id/runs`,
  `GET /api/sessions/:id/usage-timeline` — proxy. (`send` is already
  host-aware.)
- `host/connector.rs`: add `proxy_get_json` to the trait; `host/http.rs` +
  `host/local.rs` implement it.

Fan-out limit + offline tolerance reuse Slice 1's constants/pattern. Each
remote row is tagged `host_id` by the central handler (the remote returns rows
without it). `#![deny(clippy::all)]`; no `unsafe`; errors via `ApiError`.

## Web changes (`crates/rupu-cp/web/src`)

- `lib/api.ts`: thread an optional `host` through `getAgentRuns`,
  `getAutoflowRuns`, `getAutoflowEvents`, `getSessions`, `getSession`,
  `getSessionRuns`, `getRunGraph`, `getRunUsageTimeline`, and the session
  usage-timeline helper — `?host=` query, additive (omitted = local), exactly
  like Slice 1's other helpers.
- `pages/RunDetail.tsx`: **remove** the Slice-1 remote gating — call
  `getRunGraph`/`getRunUsageTimeline` with the run's `?host=` for remote runs;
  delete the "not available for remote" note. Remote run detail becomes fully
  featured.
- Host column + **"This host / All hosts"** filter (default this host) on
  `runs/AgentRuns.tsx`, `runs/AutoflowRuns.tsx` (cycles + events tabs), and
  `Sessions.tsx` — reuse the exact pattern from `runs/WorkflowRuns.tsx`
  (Slice 1). Rows deep-link carrying `?host=`.
- `components/AgentLauncherSheet.tsx`: **re-enable** the Session kind for remote
  hosts (remove the Slice-1 gate + the "sessions run locally" note), and
  navigate to `/sessions/${id}?host=<host>` after a remote session launch.
- `pages/SessionDetail.tsx`: read `?host=` and thread it into `getSession`,
  `getSessionRuns`, `sendSessionMessage`, and the usage-timeline call; show the
  owning host.

## Theming the new Fleet/Host UI

Migrate the raw color classes (that landed after #409's theme sweep) to the
semantic tokens (`bg/panel/surface/border/ink/err/ok/warn/info/status/sev`,
dark mode via `data-theme="dark"`):

- `pages/Hosts.tsx` + `pages/HostDetail.tsx`: the duplicated `HOST_STATUS_CLASS`
  (`bg-green-50/red-50/amber-50 …`) → extract ONE shared helper mapping
  online→`ok`, stale→`warn`, offline→`err` (with `-bg` tints); `TRANSPORT_CLASS`
  → `info`/neutral; error banners `border-red-200 bg-red-50 text-red-700` →
  `err`/`err-bg`; stray `bg-slate-100`/`text-red-600`/`text-amber-600` → tokens.
- `components/HostSelect.tsx`: `bg-white` → `bg-panel`.
- The Host column chips on the run/session lists use the same shared host-status
  helper.

## Error handling

Unknown `?host=` → `ApiError::not_found`. Offline/unreachable host in a fan-out
→ contributes nothing + a `tracing::warn` (never 500). Remote single-host read
on an offline host → the proxied error maps to the appropriate status (404/500)
and the web surfaces it (RunDetail/SessionDetail already have error states).
`proxy_get_json` never panics (error-mapped like the other HTTP methods).

## Testing

- Backend: `proxy_get_json` against an httpmock remote (bearer + error mapping);
  each newly host-aware handler proxies for a mock remote; fan-out merges
  local + a mock remote and tolerates an offline host (graph/usage, agent-list,
  autoflow-list, sessions-list, session-detail). Local paths unchanged
  (existing tests pass; `host_id` additive on list rows).
- Web (vitest): RunDetail calls graph/usage for a remote `?host=` (gating
  removed); AgentLauncherSheet allows a remote Session + navigates with
  `?host=`; SessionDetail threads host; Host column/filter render on the three
  lists.
- Extend `crates/rupu-cp/tests/federation_e2e.rs`: a remote **session**
  (start → appears in central sessions list → detail loads → send), and a
  remote run's **graph** loads via the central.

## Open questions

- **Single-read host stripping:** when proxying `GET /api/runs/:id/graph?host=
  <remote>`, forward the path to the remote without `?host=` (it serves its own
  local run). Confirmed approach; noted here so the implementer doesn't
  re-forward `?host=` and cause confusion.
- **Sessions list fan-out cost:** same proxy/live-query trade-off as Slice 1's
  run lists (default "this host" keeps single-host fast; "All hosts" fans out).
  No central mirror this slice.
