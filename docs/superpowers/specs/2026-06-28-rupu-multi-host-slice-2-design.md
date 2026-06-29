# rupu multi-host — Slice 2: `rupu node` dial-home tunnel

Status: approved (design), pending implementation plan
Date: 2026-06-28

## Context

Slices 1 + 1.5 (shipped in v0.27.0) gave the central CP federated control of
remote hosts that run a reachable `rupu cp serve`, over HTTP, via a
`HostConnector` port + `HostRegistry` and a proxy/live-query model (the CP makes
outbound calls and proxies). See
`docs/superpowers/specs/2026-06-28-rupu-multi-host-slice-1-design.md` (+ 1.5).

That model can't reach a **NAT'd / firewalled** host that can't expose a server.
Slice 2 adds the first **dial-home transport**: a lightweight `rupu node` agent
runs on such a host, dials *out* to the CP over a persistent WebSocket, executes
dispatched runs locally, and streams results back. This **inverts** Slice 1's
assumptions, so it introduces three new things — but they slot under the
existing `HostConnector` port so the central handlers and UI are unchanged.

Reference pattern (already studied): Okesu's tunnel — node dials CP over
WebSocket+mTLS; CP sends run/cancel frames; node spawns the agent and streams
stdout/exit back; one live connection per node, evict-on-redial, keepalive.

## Spine decisions (approved)

1. **Scope = tunnel only.** Pull/bucket (2b) and SSH (2c) are deferred to their
   own cycles.
2. **Auth = enrollment token now, mTLS-ready later.** The CP mints a per-node
   token; the node sends it on connect. The frame envelope carries an `auth`
   block so an mTLS upgrade (cert CN = node id, token omitted) slots in with no
   protocol change (mTLS itself is Slice 4).
3. **`TunnelHostConnector` satisfies the existing `HostConnector` port** via
   **mirror-for-reads + frames-for-control**.
4. **Node runs are mirrored as first-class runs** in the central `RunStore`,
   host-attributed — forced by NAT (the CP can't pull a node), scoped to tunnel
   hosts only.

## Goals (Slice 2)

- Enroll a node (mint token + create a `Tunnel` host record).
- `rupu node` dials the CP, authenticates, stays connected (reconnect on drop).
- From the central CP: launch a **workflow** or **agent** run on a tunnel node;
  the node executes locally and streams its run artifacts back; the CP mirrors
  them and shows the run in the existing host-aware lists / detail / live events;
  **cancel** a node run.
- The node shows online/offline/stale in Fleet/Hosts driven by the live
  connection (no outbound probe).

## Non-goals (later)

- Pull/bucket transport (2b); SSH transport (2c).
- Remote **sessions** over the tunnel (interactive/long-lived — defer).
- **Approval-gate** approve/reject over the tunnel (cancel IS in scope; resume
  over the tunnel is deferred).
- Distributed workflow steps (Slice 3); mTLS / cert rotation (Slice 4).

## Architecture

Five pieces; #1–#4 are new:

1. **Tunnel server** — `GET /api/node/connect` (axum WS upgrade; ensure axum
   `ws` feature) + a `NodeRegistry` (`Arc`, in `AppState`) holding live
   connections `node_id → NodeConn`. One connection per node (evict prior on
   redial); a keepalive `Ping`/`Pong` loop marks stale nodes. Token-gated at the
   handshake — the ONLY new inbound surface.
2. **`rupu node` agent** (`crates/rupu-cli`) — dials the CP, `Hello`-authenticates,
   then on each `Run` frame spawns `rupu workflow run`/`rupu run` **locally**
   (the same detached-subprocess launch the CP already uses via
   `cp_launcher`/`cp_agent_launcher`), tails that run's artifact files, and
   streams their lines back; `Cancel` kills the child. Exponential-backoff
   reconnect (cap ~60 s); re-`Hello` on reconnect.
3. **Mirror** — incoming run artifacts are written into the central `RunStore`
   under the node run's id (host-attributed), so observation reads local data.
4. **`TunnelHostConnector`** (impl `HostConnector`) — **observation**
   (`list_runs`/`get_run`/`stream_run_events`/graph/usage via `proxy_get_json`'s
   sibling reads) reads the **mirror**; **control** (`launch_run`/`launch_agent`/
   `cancel_run`) sends a frame over the node's live `NodeConn`. Registered for
   `transport: Tunnel` hosts in the `HostRegistry`, so the Fleet/Hosts page, run
   lists, RunDetail, and live events all work through the same code paths as
   HTTP hosts (no handler changes).

## Wire protocol (typed JSON frames over the WS)

A single tagged `Frame` enum (mirrors Okesu's typed-frame approach; reuses the
real `rupu_transcript`/executor event shapes — no new event vocabulary):

- **Handshake:** node→CP `Hello { node_id, auth: { token } | { mtls: true }, rupu_version, capabilities }`; CP authorizes (token→node lookup, constant-time compare of the token hash) and replies `Welcome { }` or closes with a reason. The `auth` enum is the mTLS-ready seam.
- **CP→node:** `Run { run_id, kind: "workflow" | "agent", name, inputs: map, prompt?, mode?, target? }`, `Cancel { run_id }`, `Ping`.
- **node→CP:** `Welcome`, `Artifact { run_id, file: "events" | "step_results" | "unit_checkpoints" | "run_json", line/body }` (verbatim lines from the run dir, so the existing graph/usage/events builders work on the mirror), `RunFinished { run_id, status }`, `Pong`.

The CP allocates `run_id` and includes it in `Run` (so the central holds the id
before the node starts). Streaming the step-results + unit-checkpoints (not just
events) lets the mirrored run render its **graph + usage** via the existing
builders.

## Node execution detail

On `Run`, the node: creates the local run via the same path `rupu workflow run --run-id <id>` uses (the CP-assigned id), spawns it detached, then tails
`<run>/{events.jsonl,step_results.jsonl,unit_checkpoints.jsonl}` + reads
`run.json`, streaming new lines as `Artifact` frames until the run reaches a
terminal status, then sends `RunFinished`. `Cancel` terminates the child
process group. The node is a **full rupu install** (it executes real runs); the
agent is just the tunnel + dispatch loop.

## Enrollment + Host model

- Add `HostTransport::Tunnel { node_id }` to the Slice-1 `Host` model.
- **Enroll:** CP "Add node" form (and `rupu host add --tunnel <name>` /
  `rupu node enroll <name>`): the CP generates a one-time **token**, stores only
  its **hash** (keyring or the host store, hashed — never plaintext), creates a
  `Host { transport: Tunnel { node_id }, status: offline }`, and displays the
  `rupu node --cp-url wss://<cp>:7878 --token <token>` command to run on the box
  (token shown once).
- **Status** flips **online** when the node's WS connects (NodeRegistry),
  **stale** on missed pings, **offline** on disconnect — derived from the live
  connection, no outbound probe.

## CP surface

- New inbound `GET /api/node/connect` WS (token-gated) — the only new inbound
  surface; the CP makes no new outbound connections for tunnel nodes.
- A tunnel node appears in **Fleet → Hosts** like any host (transport chip
  "tunnel"; online/offline/stale from the connection). Its runs appear in the
  host-aware run lists + RunDetail + live events via the mirror-backed
  `TunnelHostConnector` — no new pages. The "Add node" UI surfaces the
  enrollment command + one-time token.

## Errors + security

- Token stored **hashed**; handshake does a constant-time compare; bad/unknown
  token → close with a reason (no run dispatch). `wss://` (TLS) expected; warn on
  plaintext `ws://`. The frame envelope is mTLS-ready (Slice 4).
- The inbound WS is the only new attack surface and is token-gated; it reuses the
  CP's existing bearer-gated server (the WS route sits behind the same listener;
  the node token is separate from the CP API bearer).
- Node offline (no live conn) → control calls return a clear error
  ("node <id> is not connected"); observation still works (reads the mirror —
  last-known state). Reconnect with backoff; keepalive ping/pong detects dead
  connections. One conn per node (evict prior on redial). Bound the per-run
  artifact relay channel (backpressure, don't unbounded-buffer).
- `#![deny(clippy::all)]`; no `unsafe`; library errors `thiserror`, CLI `anyhow`.

## Testing

- `NodeRegistry`: register/evict-on-redial/keepalive-stale/lookup.
- Tunnel handshake: good token → `Welcome`; bad/unknown token → close, no
  dispatch (constant-time compare).
- `TunnelHostConnector`: observation reads the mirror; `launch_run`/`cancel_run`
  send the right frame over a fake conn; offline node → control returns the
  clear error.
- Mirror: streamed `Artifact` lines for a run produce a readable mirrored run
  (list/detail/events; graph from streamed step_results).
- Integration: spin the CP router, connect a **fake node WS client**, enroll +
  `Hello`, dispatch a `Run`, stream artifacts, assert the central run list/detail
  shows it host-attributed, then `Cancel` reaches the node.
- The `rupu node` agent loop (receive Run → spawn local run → tail → stream →
  finish) — an integration test with a trivial agent/workflow on the node side.

## Open questions (resolve in planning)

- **Token hashing location:** reuse the host-token keyring entry (store the
  hash there) vs a hash field in the host TOML. Leaning host TOML (`token_hash`)
  — it's a hash, not a secret, and avoids a keyring write at enroll time.
- **run_id allocation:** CP-allocated (in the `Run` frame) — confirmed, so the
  central holds the id pre-execution for the mirror.
- **Artifact granularity:** stream per-line `Artifact` frames vs periodic file
  snapshots. Leaning per-line (lowest latency, matches the live-events feel);
  bound the channel for backpressure.
