# rupu multi-host — Slice 2c: SSH transport

Status: approved (design), pending implementation plan
Date: 2026-06-29

## Context

Slices so far gave the central CP three ways to reach a host through the
`HostConnector` port + `HostRegistry`:

- **Slice 1 / 1.5 — `HttpCp`:** the host runs a reachable `rupu cp serve`; the CP
  federates over HTTP (proxy/live-query).
- **Slice 2 — `Tunnel`:** a NAT'd host runs the dial-home `rupu node` agent over
  a WebSocket; node runs are mirrored into the central `RunStore`.
- **Slice 2.5 — finish the tunnel:** approve/reject/resume of gated tunnel runs
  over the tunnel.

Slice 2c adds a fourth transport: **plain SSH**. It targets a host that runs a
full rupu install but neither exposes a reachable `rupu cp serve` (rules out
`HttpCp`) nor runs the dial-home agent (rules out `Tunnel`) — yet the operator
can already `ssh` to it. The central CP connects **out** over SSH to dispatch,
observe, and control runs.

This is sequential after Slice 2.5 (shipped as PR #418) and **builds on it**:
2c extends the 2.5 resume-worker defense-in-depth filter and shares the
mirror-backed observation path. Slice 2b (pull/bucket) comes next. Ships as its
own PR.

## Spine decisions (approved)

1. **Observation = mirror.** Dispatch with `ssh host rupu workflow run
   --run-id <id>`; a per-run background `ssh host tail -f <run dir files>` pump
   feeds lines into the existing `NodeMirror` → central `RunStore`. This reuses
   the entire Tunnel observation stack (shared read helpers, live-events tail,
   host-attributed runs). All reads are local; no per-read SSH latency.
2. **Auth = system ssh.** rupu never touches key material — it invokes `ssh` and
   the system resolves auth (ssh-agent, `~/.ssh/config`, default keys). The host
   record stores only `host` / optional `port` / optional `identity_file`; `host`
   may be a `~/.ssh/config` alias (so ProxyJump/ControlMaster/keys come for free
   from the user's config). No secrets in rupu, no `token_hash`.
3. **Execution = per-op `ssh`, no rupu-managed ControlMaster.** Each control op
   is its own one-shot `ssh host rupu …`; each active run gets one long-lived
   `ssh tail -f` pump. rupu owns no socket lifecycle. Users who want connection
   multiplexing set `ControlMaster auto` in their ssh config for the host alias.
   The launch command **detaches the remote run** (`setsid … &`) so it survives
   the ssh session closing; the tail pump observes its progress.

## Goals (Slice 2c)

- Enroll an SSH host: `rupu host add --ssh <name> <user@host> [--port N]
  [--identity <path>]` — creates a `HostTransport::Ssh` host record.
- From the central CP: launch a **workflow** or **agent** run on an SSH host; the
  remote rupu executes it; its artifacts are mirrored into the central RunStore
  and shown in the existing host-aware lists / RunDetail / live events.
- **Cancel / approve / reject** an SSH-host run from the central CP (reusing the
  remote `rupu workflow cancel|approve|reject` commands).
- Host shows reachable/unreachable from a fast `ssh … true` probe.

## Non-goals (later)

- rupu-managed OpenSSH `ControlMaster` multiplexing (use the user's ssh config).
- Pull/bucket transport (Slice 2b); sshfs / rsync artifact transfer.
- A CI ssh-server end-to-end test (CI has no ssh server — see Testing).
- Interactive **sessions** over SSH (sessions are Slice 5, all transports).

## Architecture

Five pieces; the connector + the exec seam are the only substantial new code.

### 1. Host model (`crates/rupu-workspace/src/host_store.rs`)

Add a variant to `HostTransport` (`#[serde(tag = "kind", rename_all = "snake_case")]`):

```rust
Ssh {
    host: String,                 // "user@hostname" or a ~/.ssh/config alias
    port: Option<u16>,            // omitted = ssh default / config
    identity_file: Option<PathBuf>, // omitted = ssh-agent / config / default keys
},
```

Serializes as `kind = "ssh"` with the named fields. Add an `add_ssh_host(store,
name, host, port, identity_file) -> Host` helper mirroring the existing
`add_host` (HttpCp) pattern: assigns `host_<ULID>`, no `token_hash`, no secrets.
The CLI surface is `rupu host add --ssh <name> <user@host> [--port N]
[--identity <path>]`.

### 2. Remote-exec seam (`crates/rupu-cp/src/host/ssh.rs`)

A small trait so the connector is testable without a real ssh server:

```rust
#[async_trait]
trait RemoteExec: Send + Sync {
    /// Run a remote command to completion, returning its captured output.
    async fn run(&self, remote_argv: &[String]) -> Result<RemoteOutput, RemoteExecError>;
    /// Spawn a long-lived remote command (the tail pump), yielding its stdout lines.
    fn spawn_lines(&self, remote_argv: &[String]) -> Result<LineStream, RemoteExecError>;
}
```

- **Real impl `SshExec { host, port, identity_file }`** builds
  `ssh [-i <identity>] [-p <port>] -o BatchMode=yes [-o ConnectTimeout=…] <host>
  -- <remote-command>` via `tokio::process::Command::new("ssh")`. `BatchMode=yes`
  makes a missing key fail fast instead of hanging on a password prompt.
- **Test impl** records the commands and/or runs them against a local temp
  RunStore, so connector tests need no ssh.

**Command construction (security).** ssh re-parses the remote args through the
remote **login shell**, so the remote side is a single shell string. Each rupu
argument is **shell-escaped** (single-quote escaping) before being joined into
the remote command — this is the primary injection/quoting hazard and is handled
in one pure, unit-tested `build_remote_command(argv) -> String` helper. The
detach wrapper (`setsid … </dev/null >/dev/null 2>&1 &`) is composed around the
already-escaped command.

### 3. `SshHostConnector` (`crates/rupu-cp/src/host/ssh.rs`) impl `HostConnector`

Holds `{ host_id, exec: Arc<dyn RemoteExec>, mirror: Arc<NodeMirror>,
run_store: Arc<RunStore> }`.

- **`launch_run` / `launch_agent`:** mint `run_<ULID>` locally → `mirror.create_run(run_id, host_id, spec)` → `exec.run` a detached remote
  `setsid rupu workflow run <wf> [target] --run-id <id> --plain [--input k=v]…
  [--mode m] </dev/null >/dev/null 2>&1 &` (agent form: `rupu run <agent> …`) →
  start the tail pump for that run → return `run_id`.
- **Tail pump** (one background task per active run): `exec.spawn_lines` running
  `tail -n +1 -F <runs>/<id>/events.jsonl <…>/step_results.jsonl
  <…>/unit_checkpoints.jsonl`; parse the `==> <path> <==` markers `tail` emits to
  route each line to the right `ArtifactFile`, calling `NodeMirror::append`.
  Periodically `cat run.json` (or detect its marker) → on terminal status call
  `NodeMirror::finish` and stop the pump. The remote runs root is
  `~/.rupu/runs/<id>/` (resolved the same way the host's own rupu resolves
  `global_dir`); if a non-default `RUPU_HOME` is needed it is out of scope for 2c
  (documented assumption: default `~/.rupu`).
- **Observation** (`list_runs` / `get_run` / `stream_run_events` /
  `get_transcript`): read the **mirror** filtered to `worker_id == host_id` —
  **shared with `TunnelHostConnector`**. The common mirror-backed observation is
  extracted into a shared helper/struct so both connectors call it (no verbatim
  duplication).
- **Control:** `cancel_run` / `approve_run` / `reject_run` → one-shot `exec.run`
  of `rupu workflow cancel|approve|reject <id> [--mode m] [--reason r]` — the
  **same remote commands Slice 2.5 already drives** for the tunnel. `info()` →
  `reachable` from a fast `exec.run(["true"])` with a short `ConnectTimeout`;
  failure → `reachable = false`.
- Any ssh failure (unreachable, auth) → `HostConnectorError::Unreachable` with
  the captured stderr as context.

### 4. Registry resolution (`crates/rupu-cp/src/host/registry.rs`)

`build_connector` gains an `Ssh` arm constructing `SshHostConnector` with the
`SshExec` real impl + the shared `mirror` / `run_store` (the same dep bundle the
`Tunnel` arm already receives via `with_tunnel_deps`; `node_registry` is not
needed for SSH). Cache + invalidation behave as for other transports.

### 5. Resume-worker filter (`crates/rupu-cli/src/cmd/cp.rs`)

Slice 2.5 added a defense-in-depth filter so the central resume worker never
resumes a **Tunnel** run (the real run is remote; approve goes over the
transport, never the local worker). 2c **extends that filter to skip `Ssh`-
transport hosts too** — same invariant: an SSH-host run's `worker_id` is the
host id, and approve is dispatched over ssh. (Like Tunnel, SSH `approve_run`
sends the command directly and never sets the `resume_requested_at` marker, so
the run never enters `list_pending_resume` in the first place; the filter is
belt-and-suspenders.)

## Errors & security

- `-o BatchMode=yes` → no interactive password hang; missing/!authorized key
  fails fast → `Unreachable`.
- All remote rupu arguments are shell-escaped before joining into the remote
  command (single injection-safe `build_remote_command` helper, unit-tested).
- No secrets stored: rupu holds only `host` / `port` / `identity_file`; auth is
  whatever the system `ssh` resolves. `identity_file` is a path, not key
  material.
- The mirror write path is the same hardened one Slice 2.5 produced
  (`worker_id`-scoped, run_id-validated, `resume_*` nulled on import).
- `#![deny(clippy::all)]`; no `unsafe`; library errors `thiserror`, CLI `anyhow`;
  workspace deps only (no new crate — `ssh` is shelled out via
  `tokio::process::Command`, matching the codebase's `git`/`gh`/`rg` style).

## Testing

- **Host model:** `HostTransport::Ssh` serde round-trip (`kind = "ssh"` + fields,
  `port`/`identity_file` optional); `add_ssh_host`.
- **Command builder (TDD core):** `build_remote_command` shell-escapes args
  (spaces, quotes, `;`, `$`) and the ssh argv builder adds `-i`/`-p`/`BatchMode`
  correctly — pure-fn unit tests, no ssh.
- **Connector (via the `RemoteExec` fake):** `launch_run` mints a run_id, creates
  the mirror run, and issues the expected detached remote command; the tail pump
  routes `==>`-separated lines to the right `ArtifactFile` and `finish`es on
  terminal status; `cancel`/`approve`/`reject` issue the expected remote
  commands; observation reads the mirror (reuse the Tunnel observation tests).
- **Resume-worker filter:** an SSH-attributed mirrored awaiting-approval run is
  not picked up by the resume worker (extend the 2.5 test).
- **Optional localhost smoke (manual / gated):** against `ssh localhost`, enroll
  → launch a trivial workflow → observe it complete in the mirror → cancel.
  Not run in CI (no ssh server); documented for manual validation.

## Open questions

- **Remote `RUPU_HOME`:** 2c assumes the remote host uses the default `~/.rupu`
  for its runs root. A non-default remote home (custom `RUPU_HOME`) is out of
  scope for 2c; if needed it becomes an optional `Ssh { rupu_home }` field in a
  later iteration.
- **Remote `rupu` on PATH:** 2c assumes `rupu` is on the remote login shell's
  PATH. If not, the operator uses a `~/.ssh/config` / login-shell setup; an
  explicit remote binary path is a later optional field.
