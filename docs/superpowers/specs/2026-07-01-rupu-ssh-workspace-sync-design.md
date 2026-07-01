# rupu multi-host — SSH workspace sync (Slice 3c follow-up)

Status: approved (design), pending implementation plan
Date: 2026-07-01

## Context

Slice 3c (shipped v0.32.0) added cross-host workspace sync: a step with
`workspace: sync` has the coordinator's file workspace made available on the
remote host it runs on, and the resulting file changes propagate back. The
mechanism is an auto git-or-tar **codec** in `rupu-workspace`
(`pack` / `stage` / `collect_delta` / `apply_deltas`), routed through the
orchestrator's opaque `UnitDispatcher` port (`WorkspaceDelta` / `WorkspaceConflict`
+ `apply_workspace_deltas`), with per-transport staging on `rupu-cp`'s
`HostConnector` (`stage_workspace(payload) -> working_dir`,
`collect_workspace_delta(working_dir) -> bytes`).

3c fully implemented the **Local** and **HttpCp** transports. **SSH** was
deferred to a loud `Unsupported` (the SSH `HostConnector` methods return
`HostConnectorError::Unsupported` with a warn log). This follow-up implements SSH
workspace sync so it reaches parity with Local/HttpCp.

The SSH transport delegates auth entirely to the system `ssh`
(ssh-agent / `~/.ssh/config` / an optional `identity_file`); rupu stores no key
material. It exposes a `RemoteExec` port (`SshExec` shells out to `ssh`;
`build_remote_command` shell-escapes argv; `ssh_argv` builds the invocation).
The remote host already runs the same `rupu` binary for `launch_agent`, and
`git2`/`tar` are vendored in it — so no host-side git/tar/rsync tooling is
required.

## Spine decisions (approved)

1. **Move bytes over the existing system-`ssh` channel (pipe stdin/stdout).**
   Not rsync, not scp/sftp. SSH rides the **same `rupu-workspace` codec** as
   Local/HttpCp — it only moves the codec's opaque payload/delta bytes — so the
   git 3-way-merge and cross-unit conflict-detection guarantees are identical on
   every transport. (rsync would be more transfer-efficient but would bypass the
   codec, losing the merge/conflict semantics and adding a host dependency;
   efficiency is a future cross-cutting codec-layer concern, not a reason to fork
   one transport. This mirrors HttpCp, which already ships the whole payload in a
   single HTTP body.)
2. **A binary-capable `RemoteExec` method.** Today `RemoteExec::run` returns a
   UTF-8-lossy `String` and pipes no stdin — unusable for binary. Add
   `run_bytes(remote_command, stdin: Option<Vec<u8>>) -> Result<Vec<u8>, RemoteExecError>`:
   write `stdin` to the ssh child, capture stdout as **raw bytes**, check exit
   status. The text `run` / `spawn_lines` are untouched.
3. **A hidden `rupu __workspace stage|collect` helper subcommand** invoked over
   ssh, backed by the **same** `rupu_workspace` codec the Local transport uses:
   - `rupu __workspace stage` — reads the payload from **stdin** →
     `rupu_workspace::stage` into a scratch dir under the remote rupu cache →
     writes the codec `Baseline` to a **sidecar** file → prints the working-dir
     path to **stdout**.
   - `rupu __workspace collect <working_dir>` — reloads the sidecar `Baseline` →
     `rupu_workspace::collect_delta` → writes the delta bytes to **stdout**.
   `hide = true` (not in `--help`). Not a new privilege surface: the SSH
   transport already ssh-execs arbitrary `rupu` subcommands (that is how
   `launch_agent` runs); this adds two codec-backed ones.
4. **Reuse the transport-agnostic composition.** The `FleetUnitDispatcher`
   already composes pack → `stage_workspace` → `launch_agent(working_dir =
   staged)` → poll → `collect_workspace_delta` → `apply_workspace_deltas` (3c).
   SSH only needs its two `HostConnector` methods to work; the composition is
   unchanged.
5. **Uniform scratch cleanup.** Remote scratch dir + sidecar are removed after
   `collect` and, best-effort, on the error/timeout paths. 3c left a known
   error-path scratch leak for Local/HttpCp; this follow-up fixes cleanup
   **uniformly** across Local / HttpCp / SSH so there is one correct cleanup
   story (approved scope decision — best long-term over an SSH-only patch).

## Goals

- A `workspace: sync` step on an **SSH** host runs its agent against the synced
  workspace and its file changes propagate back, identical in behavior to Local
  / HttpCp (git 3-way-merge / tar disjoint; conflicts surface as typed step
  failures honoring `continue_on_error`).
- No host-side git/tar/rsync required — the vendored `rupu` binary carries the
  codec.
- Auth unchanged (system `ssh`, no stored keys); no new secrets.
- Failures (unreachable host, nonzero remote exit, oversize/malformed output)
  surface as clean `HostConnectorError`s → proper step failures.
- Remote scratch never leaks on the happy path, and best-effort on error paths,
  across all three transports.

## Non-goals (later)

- Bucket / Tunnel workspace sync (remain `Unsupported`).
- Transfer efficiency (rsync-style block deltas / content-addressed incremental
  payloads) — a future cross-transport codec-layer optimization, not per-SSH.
- Resume of a `workspace: sync` workflow (still refused, per 3c).
- mTLS (Slice 4) / sessions (Slice 5).

## Architecture

### Binary remote exec (`crates/rupu-cp/src/host/ssh.rs`)

Add to the `RemoteExec` trait:

```rust
/// Run `remote_command`, writing `stdin` to it (if any) and returning its
/// raw stdout bytes. Binary-safe (unlike `run`, which lossily decodes UTF-8).
async fn run_bytes(
    &self,
    remote_command: &str,
    stdin: Option<Vec<u8>>,
) -> Result<Vec<u8>, RemoteExecError>;
```

`SshExec::run_bytes` spawns `ssh` (via the existing `ssh_argv`) with
`stdin(piped)` when `stdin` is `Some`, `stdout(piped)`, writes the bytes, closes
stdin, awaits the child, and returns stdout bytes on exit 0 — else a
`RemoteExecError` carrying the exit code + stderr. A fake `RemoteExec` in tests
implements `run_bytes` (the existing fakes already implement the trait; they gain
the new method).

### Remote helper (`crates/rupu-cli`)

A hidden `__workspace` subcommand group with `stage` and `collect`, dispatched in
the thin CLI layer to a small handler that calls `rupu_workspace::{stage,
collect_delta}`. It resolves the remote rupu **cache root** (the same base the
Local transport uses for staging), creates/reads scratch dirs beneath it, and
confines every path under that root (canonicalize + `starts_with`; reject `..` /
absolute `working_dir`), mirroring the HttpCp handler's guard. `stage` reads
stdin to EOF (bounded by `MAX_WORKSPACE_BYTES`); `collect` writes the delta to
stdout.

### SSH `HostConnector` (`crates/rupu-cp/src/host/ssh.rs`)

Replace the `Unsupported` stubs:

```rust
async fn stage_workspace(&self, payload: Vec<u8>) -> Result<String, HostConnectorError> {
    // size-guard payload, then:
    let out = self.exec.run_bytes("rupu __workspace stage", Some(payload)).await?;
    // parse the single working-dir line from `out`
}
async fn collect_workspace_delta(&self, working_dir: &str) -> Result<Vec<u8>, HostConnectorError> {
    let cmd = build_remote_command(&["rupu".into(), "__workspace".into(), "collect".into(), working_dir.into()]);
    let bytes = self.exec.run_bytes(&cmd, None).await?;
    // size-guard the returned delta
}
```

The remote `rupu` path and any working-dir shell-escaping reuse the SSH
transport's existing argv-building (`build_remote_command` / the same remote-rupu
invocation `launch_agent` uses).

### Errors & security

- `RemoteExecError` (ssh spawn / connection failure) → `HostConnectorError::Unreachable`.
- Nonzero remote exit → `HostConnectorError::Remote(code, stderr)`.
- Oversize (either direction) or unparseable working-dir line →
  `HostConnectorError::Invalid(msg)`. No silent fallback.
- `MAX_WORKSPACE_BYTES` enforced on the payload before send and the delta after
  receive (reuses the 3c const).
- Remote helper confines all paths under the rupu cache root; rejects
  traversal — the helper cannot read/write outside the cache.
- Auth is system `ssh`; no new secrets, no stored keys.
- Cleanup: `collect` removes the working dir + sidecar; the connector best-effort
  cleans on error/timeout. The Local/HttpCp error-path cleanup gap is closed in
  the same change.
- `#![deny(clippy::all)]`; no `unsafe`; library errors `thiserror`, CLI `anyhow`;
  workspace deps only; the orchestrator is untouched (SSH is a transport detail
  behind the `HostConnector` — hexagonal boundary preserved).

## Testing

- **`run_bytes` (fake + real-shape):** a fake `RemoteExec` round-trips stdin →
  stdout bytes; `SshExec::run_bytes` argv/stdin wiring is unit-tested where
  feasible (the ssh spawn itself is integration-only).
- **Helper subcommands:** invoke the `__workspace stage` / `collect` handlers
  directly with stdin bytes against a tempdir cache; assert a git workspace and a
  tar workspace both round-trip (stage → simulate remote edit → collect → the
  delta reflects the change), and that the sidecar baseline persists between the
  two calls.
- **Confinement:** `collect` with a `working_dir` outside the cache root is
  rejected; `stage`/`collect` reject `..`/absolute paths.
- **SSH `HostConnector`:** with a fake `RemoteExec` that actually runs the helper
  logic, `stage_workspace` + `collect_workspace_delta` produce a working dir and
  a delta; a fake that returns a nonzero exit maps to `Remote`; a spawn failure
  maps to `Unreachable`; an oversize payload/delta maps to `Invalid`.
- **Cleanup:** the remote scratch is removed after a successful collect and on a
  simulated error path (all three transports).
- **End-to-end:** exercise the full SSH path with a fake exec running the real
  helper — pack → (ssh) stage → launch → collect → apply — over both a git and a
  non-git workspace, proving parity with the 3c Local/HttpCp e2e.

## Open questions

- **Remote rupu path:** whether the remote command is a bare `rupu` (on the
  host's `PATH`) or a configured absolute path per host. Resolve in the plan by
  reusing exactly what `launch_agent`'s remote invocation already assumes (bare
  `rupu` unless the SSH host record already carries a path) — no new config.
- **stage/collect as one round-trip vs two:** whether to keep `stage` and
  `collect` as two ssh invocations (matches the `HostConnector` two-method shape)
  or fuse them. Keep two — it matches the port and the Local/HttpCp shape; the
  agent run happens between them anyway.
