# rupu multi-host — Slice 2.5: finish the tunnel (approve / reject / resume)

Status: approved (design), pending implementation plan
Date: 2026-06-29

## Context

Slice 2 (shipped in v0.28.0) gave the central CP a dial-home **tunnel**
transport: a `rupu node` agent on a NAT'd box dials the CP over a persistent
WebSocket, executes dispatched workflow/agent runs locally, and streams their
artifacts back into the central `RunStore` (mirrored, host-attributed). The
central CP observes those runs through the same host-aware `HostConnector` port
as HTTP hosts, and can **cancel** them. See
`docs/superpowers/specs/2026-06-28-rupu-multi-host-slice-2-design.md`.

But `TunnelHostConnector` still returns `Invalid` for the approval-gate
operations: `approve_run`, `reject_run`, and (interactive) `start_session` /
`send_session_turn`. So a node run that pauses on an `approval:` gate can be
*seen* from the central CP but not *unblocked* — the operator has to SSH to the
box and run `rupu workflow approve` by hand.

Slice 2.5 closes that gap for approval gates: route **approve / reject / resume**
over the existing tunnel `Frame` protocol, mirroring exactly how `cancel`
already works. Interactive **sessions** are a much larger surface (a persistent
session worker on the node, a `session_id → node_id` mapping across reconnects,
and request/response multiplexing the half-duplex tunnel lacks) and are
explicitly deferred to **Slice 5**.

This is the first of three sequential slices, each its own PR: **2.5** (this
spec), then **2c** (SSH transport), then **2b** (pull/bucket transport).

## Spine decisions (approved)

1. **Scope = approval-gate approve / reject / resume only.** Sessions →
   Slice 5. Standalone `rupu workflow resume` of *terminal* (failed / cancelled
   / rejected) runs is also out of scope — this slice is approval-gate resume.
2. **Direct-frame, node-must-be-online.** The connector sends a control frame
   over the live connection, exactly like `cancel`. If the node is offline the
   call returns a clear `Unreachable` error and the operator retries on
   reconnect. No durable/queued approvals, no new delivery worker.
3. **Reuse the node's existing local commands.** The node shells out to the
   `rupu workflow approve` / `rupu workflow reject` commands it already has; the
   resumed run's new events + terminal status flow back through the artifact
   tail that Slice 2 already runs. No new streaming code.

## Goals (Slice 2.5)

- From the central CP, **approve** a tunnel-node run that is paused at an
  approval gate (with a resume `mode`), and have the node resume it locally.
- From the central CP, **reject** such a run (with an optional reason).
- The resumed/rejected run's outcome streams back into the central mirror and
  shows in the existing host-aware run lists / RunDetail / live events.
- Offline node → a clear, actionable error; no silent no-op.

## Non-goals (later)

- Interactive **sessions** over the tunnel (`start_session` /
  `send_session_turn`) — **Slice 5**.
- **Durable / queued** approvals (approve while the node is offline, delivered
  on reconnect).
- Re-running **terminal** runs via `rupu workflow resume` over the tunnel.
- SSH (2c) / pull-bucket (2b) transports — their own slices.

## Architecture

Three small changes; all mirror the proven Slice 2 `cancel` path.

### 1. Wire protocol (`crates/rupu-cp/src/node/protocol.rs`)

Two new CP→node `Frame` variants, modeled exactly on `Frame::Cancel { run_id }`:

- `Frame::Approve { run_id: String, mode: String }`
- `Frame::Reject  { run_id: String, reason: Option<String> }`

`mode` is the resume mode string already used elsewhere (`"ask"` / `"bypass"` /
`"readonly"`; empty string means "unspecified — node uses the run's stored
mode / default"). These slot into the existing
`#[serde(tag = "type", rename_all = "snake_case")]` enum and round-trip as
`{"type":"approve",...}` / `{"type":"reject",...}`. No change to `Auth`,
`RunSpec`, or `ArtifactFile`. The new variants are added to every exhaustive
`match` on `Frame` (the node's inbound dispatch; any CP-side handling).

### 2. CP side — `TunnelHostConnector` (`crates/rupu-cp/src/host/tunnel.rs`)

Replace the two `Invalid` stubs with live-frame sends, line-for-line like
`cancel_run`:

```rust
async fn approve_run(&self, run_id: &str, mode: &str) -> Result<(), HostConnectorError> {
    let conn = self.live_conn()?;                  // Unreachable if offline
    conn.send(Frame::Approve { run_id: run_id.to_string(), mode: mode.to_string() })
        .await
        .map_err(|_| HostConnectorError::Unreachable(
            format!("node {} disconnected before Approve frame could be sent", self.node_id)))
}

async fn reject_run(&self, run_id: &str, reason: Option<&str>) -> Result<(), HostConnectorError> {
    let conn = self.live_conn()?;
    conn.send(Frame::Reject { run_id: run_id.to_string(), reason: reason.map(str::to_string) })
        .await
        .map_err(|_| HostConnectorError::Unreachable(
            format!("node {} disconnected before Reject frame could be sent", self.node_id)))
}
```

The central HTTP handlers (`POST /api/runs/:id/approve?host=<node>` /
`/reject?host=<node>`) already resolve the host via `HostRegistry` → this
connector — no handler changes.

### 3. Node side — `rupu node` agent (`crates/rupu-cli/src/cmd/node.rs`)

Handle the two new inbound frames in the existing `match frame { … }`:

- `Frame::Approve { run_id, mode }` → spawn a detached child:
  `rupu workflow approve <run_id> [--mode <mode>]` (omit `--mode` when empty).
  This is the existing command; it performs the real two-phase
  `store.approve()` (status flip `AwaitingApproval → Running`) +
  `resume::resume_run()` (re-enter the orchestrator) against the node's local
  run directory.
- `Frame::Reject { run_id, reason }` → spawn
  `rupu workflow reject <run_id> [--reason <reason>]`. This flips the local run
  to `Rejected` immediately.

**Child-handle re-pointing.** When a run pauses at a gate, the original
`rupu workflow run` child has already exited (awaiting is suspended on disk),
but the node keeps the run in its `active` map and keeps tailing its files
(`read_terminal_status` does not treat `awaiting_approval` as terminal — correct
and unchanged). On `Approve`/`Reject`, after spawning the new subprocess, the
node **re-points `active[run_id].child`** to the new child handle, so a
subsequent `Cancel` kills the resumed run (not the dead original). If the
`run_id` is not in `active` (unknown / already finished), log a warning and
ignore (same posture as `Cancel` for an unknown id).

The resumed run writes new `events.jsonl` / `step_results.jsonl` / `run.json`
lines into the same run dir; the existing 250 ms artifact tail streams them back
as `Artifact` frames and emits `RunFinished` when the run reaches a terminal
status — **no new streaming code**.

## Resume-worker invariant (correctness)

The central CP's background resume worker (in `rupu cp serve`) polls
`RunStore::list_pending_resume` and, for **local** runs, spawns
`rupu workflow approve <id>` itself. Tunnel runs must **never** go through this
worker — the central run directory is a *mirror*; the real run lives on the
node, and spawning a local approve against a mirror dir is wrong.

This is **naturally satisfied** by the direct-frame design: tunnel
`approve_run` sends a `Frame::Approve` and does **not** call
`request_resume_approval`, so it never sets the `resume_requested_at` marker
that `list_pending_resume` filters on. Tunnel runs therefore never appear in the
worker's queue.

As **defense in depth** (so a future change can't regress this), the resume
worker additionally skips any run attributed to a tunnel node — i.e. a run
whose `worker_id` matches a host of `transport: Tunnel`. The spec records this
as an explicit invariant; the plan adds a focused test that a mirrored
awaiting-approval run is not picked up by the worker.

## Observation already works

The mirror already streams `run.json` carrying `status: "awaiting_approval"`,
`awaiting_step_id`, `approval_prompt`, and `awaiting_since`, so a gated tunnel
run already surfaces in the host-aware run lists, RunDetail, and the inline
Approve/Reject affordances exactly like a local run. Slice 2.5 only adds the
**action** behind those affordances for tunnel hosts.

## Errors & security

- Offline node (no live conn) → `HostConnectorError::Unreachable` with a clear
  "node <id> disconnected" message; the operator retries when it reconnects.
- The new frames carry only a `run_id` the node already owns. The node acts on
  its **local** run store by id (same trust model as `Cancel`); a
  bad/unknown id is logged and ignored — `rupu workflow approve/reject` itself
  validates the run exists and is in an approvable state.
- No new inbound surface, no new auth path — these are CP→node frames on the
  already-authenticated, token-gated tunnel.
- `#![deny(clippy::all)]`; no `unsafe`; library errors `thiserror`, CLI
  `anyhow`.

## Testing

- **Protocol:** `Frame::Approve` / `Frame::Reject` JSON round-trip
  (`{"type":"approve","run_id":…,"mode":…}`, reject with and without `reason`).
- **Connector:** `approve_run` / `reject_run` send the correct frame over a fake
  conn (assert the frame read off the fake node's rx); offline node →
  `Unreachable`.
- **Node argv:** an `Approve` frame builds `rupu workflow approve <id> --mode m`
  (and omits `--mode` when empty); a `Reject` frame builds
  `rupu workflow reject <id> --reason r` (and omits `--reason` when none).
- **Resume-worker guard:** a mirrored `awaiting_approval` run (worker_id = a
  tunnel node) is not returned to the resume worker / not acted on locally.
- **e2e (extend `crates/rupu-cp/tests/node_tunnel.rs`):** spin the CP, connect a
  fake node, dispatch a run, have the fake node report `awaiting_approval` via a
  `run.json` artifact; central `POST /api/runs/<id>/approve?host=<node>` →
  assert the fake node receives `Frame::Approve { run_id, mode }`; then the fake
  node streams a resumed-to-`completed` `run.json` + `RunFinished` and the
  central mirror shows the run completed. A parallel case for reject →
  `Frame::Reject` → `RunFinished{rejected}`.

## Open questions

None — the design mirrors the approved Slice 2 `cancel` path end-to-end.
