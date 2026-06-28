# rupu multi-host orchestration — Slice 1: federated control (HTTP transport)

Status: approved (design), pending implementation plan
Date: 2026-06-28

## Context

Today the rupu Control Panel can launch and control work only on the machine it
runs on. `rupu cp serve` installs five per-capability ports — `RunLauncher`,
`AgentLauncher`, `SessionStarter`, `SessionSender`, `RepoLister` — each backed by
a **local-subprocess** adapter (`crates/rupu-cli/src/cp_*`). Run state is
**file-based** under `~/.rupu/runs/<id>/{run.json,events.jsonl,step_results.jsonl,…}`
and observed by polling (`FileTailRunSource`, 250 ms). `WorkerRecord` already
carries a `host` field (the local hostname) and every run records `worker_id` /
`backend_id`. So there is a clean abstraction seam, but state currently assumes
files on the same machine.

We want to **control rupu agents/workflows running on other hosts from a central
CP**, and to support **multiple ways to connect** to those hosts — modelled after
Okesu, where the Node is the root of a distributed model with a pluggable
transport layer (push WebSocket+mTLS tunnel, pull S3 dead-drop, CP-local
subprocess) and workflows that fan out across nodes.

This is a multi-subsystem effort. It is decomposed into slices (below); **this
spec covers Slice 1 only**.

## Spine decisions (approved)

1. **Hybrid / pluggable transports.** The central CP is the hub; a Host is
   reached via a chosen *transport*. New transports are added as new
   implementations of one port (below) without changes above it.
2. **Host is a top-level peer entity** (not the root, not a mere tag). It sits
   alongside Projects. Local execution becomes **host[0] ("this host")**.
   Projects are unchanged; runs/sessions gain host attribution.
3. **First transport = HTTP federation** to a host running `rupu cp serve`. The
   central CP becomes a *client* of the remote CP's already-existing API.
4. **State model = proxy / live-query.** The central CP stores no remote run
   state; it calls each host's API on demand and merges results.

## Goals (Slice 1)

- Register and health-check remote hosts that run `rupu cp serve`.
- From the central CP: launch runs / agents / sessions on a chosen host; send
  session turns; observe a host's runs (list, detail, live events, transcript);
  control runs (approve / reject / cancel) — all via the remote's HTTP API.
- A "Fleet → Hosts" surface in the CP; host attribution + filtering on run lists;
  a host selector in the launcher.
- Degrade gracefully when a host is unreachable.

## Non-goals (Slice 1 — see Future slices)

- Node-agent / tunnel / pull transports.
- Host-targeted **distributed workflow steps** + cross-host fan-out.
- Mirror/sync of remote state + offline history.
- Capability-based auto-placement / scheduling.
- mTLS / enrollment / SSH-key hardening.

## Architecture

### The transport port: `HostConnector`

A single trait represents "a way to drive + observe **one** host." It subsumes,
for the *remote* case, what the five per-capability ports do locally:

```text
trait HostConnector (async, Send + Sync):
  // control
  launch_run(LaunchRequest) -> run_id
  launch_agent(AgentLaunchRequest) -> run_id
  start_session(SessionStartRequest) -> session_id
  send_session_turn(SendMessageRequest) -> run_id
  approve_run(run_id, mode); reject_run(run_id, reason); cancel_run(run_id)
  // observe
  list_runs(ListParams) -> Vec<RunListRow>
  get_run(run_id) -> RunDetail (+ graph/usage as today)
  stream_run_events(run_id) -> SSE/byte stream
  get_transcript(path) -> events
  // health
  info() -> HostInfo { reachable, version, capabilities }
```

Lives in `rupu-cp` (the port layer), beside the existing port traits. The request
types reuse the existing `LaunchRequest` / `AgentLaunchRequest` /
`SessionStartRequest` / `SendMessageRequest`.

### Two implementations (Slice 1)

- **`LocalHostConnector`** — wraps the *existing* local subprocess adapters +
  local `RunStore` reads (and the existing SSE tailing). This is **host[0]**;
  current CP behaviour is unchanged, just relabelled as the local host. The
  existing per-capability port fields on `AppState` continue to back it.
- **`HttpHostConnector`** — a client of a remote `rupu cp serve`, mapping each
  method to endpoints that **already exist** on the remote:
  - `launch_run` → `POST /api/workflows/:name/run`
  - `launch_agent` → `POST /api/agents/:name/run`
  - `start_session` → `POST /api/agents/:name/session`
  - `send_session_turn` → `POST /api/sessions/:id/send`
  - `list_runs` / `get_run` → `GET /api/runs…` / `GET /api/runs/:id` (+graph/usage)
  - `stream_run_events` → `GET /api/events/stream?run=<id>` (SSE pass-through)
  - `get_transcript` → `GET /api/transcript?path=…`
  - `approve_run`/`reject_run`/`cancel_run` → `POST /api/runs/:id/{approve,reject,cancel}`
  - `info` → `GET /healthz` (+ a small host-info endpoint; see Open questions)
  Auth: `Authorization: Bearer <token>`.

Transport is selected per host by its `transport` field (cf. Okesu
`Node.Transport`).

### Registry + state wiring

- **`HostRegistry`** (`Arc`, in `AppState`) resolves `host_id → Arc<dyn HostConnector>`.
  Built at `cp serve` startup from the `HostStore` plus the always-present
  `local` host. Read-only `rupu cp` builds a registry containing only `local`.
- `AppState` gains `hosts: Arc<HostRegistry>`. Existing port fields remain and
  back the local connector (minimal churn this slice).

## Host entity + registry

```text
Host {
  id: String,            // "local" | host_<ULID>
  name: String,
  transport: HostTransport,
  capabilities: HostCapabilities,   // backends / scm_hosts / permission_modes (from remote)
  status: Online | Offline | Stale, // derived from last successful info()
  version: Option<String>,
  created_at, last_seen_at
}
HostTransport = Local | HttpCp { base_url: String, token_ref: SecretRef }
```

- **`HostStore`** — file-backed at `~/.rupu/hosts/<id>.toml`, mirroring
  `WorkerStore` (atomic write, list, load, delete). The **`local` host is
  implicit** and always present (not persisted, or persisted as a sentinel).
- **Tokens** are stored as a **keychain secret reference** via `rupu-auth`
  (never plaintext in the TOML).
- **Registration:** a CP "Add host" form (name, base URL, token) **and** a CLI
  `rupu host add|list|remove`. The CLI is thin (arg parse + delegate), per the
  architecture rules.
- **Health:** `info()` is called on demand and on a periodic ping
  (a few-second cadence); status → online / offline / stale, with version +
  capabilities cached on the `Host` for display.

## Data flow (proxy / live-query)

- **Launch.** The existing launch endpoints (`/api/workflows/:name/run`,
  `/api/agents/:name/{run,session}`, `/api/sessions/:id/send`) gain an optional
  `host` field (default `local`). The handler resolves the connector from the
  registry and calls it; returns `{ host_id, run_id }`.
- **Run lists.** A new aggregating path fans out `list_runs` across all hosts
  **concurrently**, tags each row with `host_id`, and merges — reusing the
  tolerate-per-failure pattern from the events firehose
  (`tail_all_events_sse`). An unreachable host yields an "offline" marker, never
  a broken list. The default view stays "this host" so single-host setups are
  unchanged.
- **Run detail / events / transcript.** A run is addressed by
  **`(host_id, run_id)`**. Detail, the SSE event stream, and transcript
  **proxy straight through** to the owning host's API. Central endpoints gain a
  `host` selector (e.g. `?host=`); when omitted, `local`.
- **Control.** `approve` / `reject` / `cancel` and `session send` proxy to the
  owning host's endpoints.

## Local host + deployment requirements

- **Host[0] = local**, always present, backed by today's adapters + `RunStore` +
  local SSE tailing. Existing single-host behaviour is unchanged.
- **Federation requires the full `rupu cp serve` runtime** (it installs the
  `HostRegistry` with HTTP connectors). Read-only `rupu cp` shows hosts but
  cannot control them.
- Each **remote** host must run `rupu cp serve`, reachable from the center, with
  a bearer token. `https` is expected for remote base URLs.

## CP UI

- **New top-level "Fleet" sidebar group → Hosts page:** a `SortableTable`
  (Name · Transport · Status [online/offline/stale via the status tokens] ·
  Version · Active runs · Last seen) + an **Add Host** form (name, base URL,
  token). `local` pinned first.
- **Host detail:** that host's runs (the run list scoped to `host_id`) +
  connection/health info.
- **Run lists** gain a **Host column + filter**; "this host" remains the default.
- **Launcher** (TargetPicker / LauncherSheet) gains a **host selector**
  (defaults to local). **Run detail** shows the owning host. Run rows/detail
  address `(host_id, run_id)` so deep-links remain stable.

## Errors + security

- **Unreachable host:** list fan-out degrades per host (offline chip, page
  intact); launch/control to an offline host returns a clear error; SSE
  auto-reconnects (reuse existing client logic).
- **Auth:** per-host bearer token, stored as a keychain secret ref, sent as
  `Authorization: Bearer`. Warn when a remote base URL is plaintext `http`.
- **Trust:** adding a host is privileged — the central CP can now control that
  machine. Token scope is the remote `cp serve`'s concern. The central CP adds
  **no new inbound surface** this slice; it is purely a client, and the remote's
  API is already token-gated.

## Testing

- `HttpHostConnector` against a **mock remote CP** (httpmock/wiremock): every
  method incl. SSE parsing, the bearer header, and error mapping
  (offline / 401 / 404 / 5xx).
- `HostRegistry` resolution; concurrent list fan-out; **per-host failure
  tolerance** (one offline host doesn't break the merge).
- `LocalHostConnector` **parity** — existing local behaviour unchanged.
- Frontend (vitest): Hosts table, host selector, host-attributed run rows,
  offline state rendering.
- One integration test: launch a real `rupu cp serve` as a "remote," federate to
  it from another instance, and launch → observe (events) → cancel.

## Future slices (out of scope here)

- **Slice 2 — more transports:** node-agent dial-home tunnel (WebSocket+mTLS)
  and/or pull dead-drop (bucket), plus SSH; each is a new `impl HostConnector`.
- **Slice 3 — distributed workflow steps:** workflow steps that target a host /
  host-set / selector, cross-host fan-out + per-host result aggregation
  (cf. Okesu `node` / `nodes` / `nodes_selector`).
- **Slice 4 — fleet polish:** heartbeat-based health at scale, capability-based
  auto-placement, enrollment + mTLS hardening, optional mirror/sync for central
  offline history.

## Open questions (to resolve during planning)

- **Host-info endpoint:** reuse `/healthz` (version only) vs add a small
  `GET /api/host/info` returning version + capabilities. Leaning toward a tiny
  dedicated endpoint so capabilities/version are first-class.
- **Composite run addressing on the wire:** `?host=<id>` query param (kept
  separate) vs a composite `host:run` id. Leaning toward a separate `host`
  param to avoid changing the run-id shape.
- **Registry refresh:** rebuild on host add/remove vs hot-reload the registry
  without restarting `cp serve`. Leaning toward hot-reload so adding a host
  doesn't require a restart.
