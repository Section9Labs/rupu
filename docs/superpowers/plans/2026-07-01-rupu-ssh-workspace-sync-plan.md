# SSH Workspace-Sync (Slice 3c follow-up) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement workspace sync over the SSH transport (currently a loud `Unsupported`), reaching parity with the Local/HttpCp transports, by moving the 3c codec's opaque bytes over the existing system-`ssh` channel to a hidden remote `rupu __workspace` helper.

**Architecture:** Add a binary-safe `run_bytes` to the SSH `RemoteExec` port; extract the Local transport's stage/collect core into shared `rupu-cp` free functions (one staging implementation for Local/HttpCp/SSH); add a hidden `rupu __workspace stage|collect` CLI subcommand that calls the shared core over stdin/stdout; wire the SSH `HostConnector` to pipe the payload to `stage` and read the delta from `collect`. The orchestrator and the transport-agnostic `FleetUnitDispatcher` composition are untouched.

**Tech Stack:** Rust 2021 (MSRV 1.88), tokio (process/io), async-trait, thiserror (libs) / anyhow (CLI), clap, the vendored `rupu-workspace` git/tar codec.

## Global Constraints

- Backward compatible: Local/HttpCp behavior is **byte-identical** after the core extraction; `RemoteExec::run`/`spawn_lines` are untouched; the orchestrator gains no dependency (SSH is a `HostConnector` detail — hexagonal boundary preserved).
- Auth is the system `ssh` only (ssh-agent / `~/.ssh/config` / optional `identity_file`) — **no stored keys, no new secrets**.
- The remote `__workspace` helper is **confined to the rupu cache root** (canonicalize + `starts_with`; reject `..`/absolute) — no traversal, not a general RCE surface beyond stage/collect under the cache.
- `MAX_WORKSPACE_BYTES` enforced in **both** directions (payload before send, delta after receive).
- **No silent fallback**: unsupported / oversize / failed → a clear `HostConnectorError`.
- Binary-safe I/O: payloads and deltas are raw bytes — never lossy-UTF-8 decoded.
- `#![deny(clippy::all)]`; no `unsafe`; libraries `thiserror`, CLI `anyhow`; workspace deps only (no new dep is needed).
- Per-file `rustfmt` only (`rustfmt <path>`). **Never** a workspace-wide `cargo fmt` — `main` is fmt-dirty and a broad format polluted ~17 files during 3c. Before each commit run `git status --short` and `git restore` any stray drift by name.
- Clippy scoped with `--no-deps`: a pre-existing `items_after_test_module` in `rupu-orchestrator/src/runner.rs` (1.95-only, on `main`; pinned CI 1.88 clean) is unrelated (this work is rupu-cp/rupu-cli). `rupu-cli` also has pre-existing unrelated 1.95 clippy errors and `cmd::session::tests` failures — scope test runs to changed modules.

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `crates/rupu-cp/src/host/ssh.rs` | `run_bytes` on `RemoteExec` + `SshExec` + `FakeExec`; new `RemoteExecError` variant; SSH `HostConnector` stage/collect impls; SSH-side cleanup | 1, 4 |
| `crates/rupu-cp/src/host/workspace_stage.rs` (new) | shared `stage_to_dir` / `collect_from_dir` + confinement guard (the one staging implementation) | 2 |
| `crates/rupu-cp/src/host/local.rs` | refactor `stage_workspace`/`collect_workspace_delta` to call the shared core; cleanup-on-error | 2 |
| `crates/rupu-cp/src/host/http.rs`, `crates/rupu-cp/src/api/workspace.rs` | (optional dedup to shared core); cleanup-on-error | 2 |
| `crates/rupu-cp/src/host/mod.rs` (or `connector.rs`) | export the shared core | 2 |
| `crates/rupu-cli/src/lib.rs` | hidden `__workspace` subcommand + dispatch | 3 |
| `crates/rupu-cli/src/cmd/workspace_helper.rs` (new) | the `stage`/`collect` handler (stdin/stdout binary I/O) | 3 |
| `crates/rupu-cp/tests/` or in-module | SSH e2e parity test (git + tar) | 5 |

---

## Task 1: Binary-safe `run_bytes` on the SSH `RemoteExec`

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs` (`RemoteExecError` ~line 115; `RemoteExec` trait ~line 127; `SshExec` impl ~line 171; `FakeExec` ~line 779)
- Test: same file

**Interfaces:**
- Produces:
  - `RemoteExecError::NonZero { code: Option<i32>, stderr: String }` (new variant; `Spawn(String)` stays).
  - `RemoteExec::run_bytes(&self, remote_command: &str, stdin: Option<Vec<u8>>) -> Result<Vec<u8>, RemoteExecError>` (new trait method — **no default**; both `SshExec` and `FakeExec` implement it).

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod` in `ssh.rs` (where `FakeExec` lives). First extend `FakeExec` so it can script `run_bytes` (record the command + stdin, return scripted bytes or a scripted error). If `FakeExec` currently holds scripted `run` output, add parallel fields:

```rust
    // (extend FakeExec) — add fields:
    //   run_bytes_out: Mutex<Option<Result<Vec<u8>, RemoteExecError>>>,
    //   last_bytes_call: Mutex<Option<(String, Option<Vec<u8>>)>>,
    // with constructors `with_bytes_ok(Vec<u8>)` and `with_bytes_err(RemoteExecError)`.

    #[tokio::test]
    async fn run_bytes_pipes_stdin_and_returns_stdout_bytes() {
        let exec = FakeExec::with_bytes_ok(b"DELTA".to_vec());
        let out = exec
            .run_bytes("rupu __workspace stage", Some(b"PAYLOAD".to_vec()))
            .await
            .expect("ok");
        assert_eq!(out, b"DELTA");
        let (cmd, stdin) = exec.last_bytes_call.lock().unwrap().clone().unwrap();
        assert_eq!(cmd, "rupu __workspace stage");
        assert_eq!(stdin.as_deref(), Some(&b"PAYLOAD"[..]));
    }

    #[tokio::test]
    async fn run_bytes_nonzero_exit_is_error() {
        let exec = FakeExec::with_bytes_err(RemoteExecError::NonZero {
            code: Some(2),
            stderr: "boom".into(),
        });
        let err = exec.run_bytes("rupu __workspace collect /x", None).await.unwrap_err();
        assert!(matches!(err, RemoteExecError::NonZero { code: Some(2), .. }));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp --lib host::ssh -- run_bytes`
Expected: FAIL — `no method run_bytes` / `no variant NonZero`.

- [ ] **Step 3: Add the error variant**

In `RemoteExecError` (~line 115):

```rust
#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoteExecError {
    #[error("ssh spawn failed: {0}")]
    Spawn(String),
    #[error("remote command exited with {code:?}: {stderr}")]
    NonZero { code: Option<i32>, stderr: String },
}
```

- [ ] **Step 4: Add the trait method**

In the `RemoteExec` trait (~line 127), after `run`:

```rust
    /// Run `remote_command`, writing `stdin` to it (if any), and return its
    /// raw stdout bytes. Binary-safe — unlike `run`, which lossily decodes
    /// UTF-8. A spawn/connection failure is `Spawn`; a nonzero remote exit is
    /// `NonZero { code, stderr }`.
    async fn run_bytes(
        &self,
        remote_command: &str,
        stdin: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, RemoteExecError>;
```

- [ ] **Step 5: Implement on `SshExec`**

In `impl RemoteExec for SshExec` (~line 171). Uses `tokio::process::Command` + `tokio::io::AsyncWriteExt`:

```rust
    async fn run_bytes(
        &self,
        remote_command: &str,
        stdin: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, RemoteExecError> {
        use tokio::io::AsyncWriteExt;
        let argv = ssh_argv(
            &self.host,
            self.port,
            self.identity_file.as_deref(),
            remote_command,
        );
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.args(&argv)
            .stdin(if stdin.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        if let Some(bytes) = stdin {
            let mut si = child
                .stdin
                .take()
                .ok_or_else(|| RemoteExecError::Spawn("no stdin pipe".into()))?;
            si.write_all(&bytes)
                .await
                .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
            si.shutdown()
                .await
                .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
            drop(si);
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        if !out.status.success() {
            return Err(RemoteExecError::NonZero {
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        Ok(out.stdout)
    }
```

> Verify `self.host` / `self.port` / `self.identity_file` are the actual `SshExec` field names (check the struct at ~line 164) and match the `ssh_argv` signature; adjust the accessors if they differ.

- [ ] **Step 6: Implement on `FakeExec`** (return the scripted result, record the call). Add `run_bytes` to `impl RemoteExec for FakeExec` reading the fields added in Step 1.

- [ ] **Step 7: Run tests, format, lint, commit**

```bash
cargo test -p rupu-cp --lib host::ssh
rustfmt crates/rupu-cp/src/host/ssh.rs
cargo clippy -p rupu-cp --no-deps
git add crates/rupu-cp/src/host/ssh.rs
git commit -m "feat(multi-host): binary-safe run_bytes on SSH RemoteExec (ssh-ws T1)"
```
Expected: the 2 new tests pass; existing ssh tests green; clippy clean.

---

## Task 2: Shared stage/collect core + uniform cleanup

**Files:**
- Create: `crates/rupu-cp/src/host/workspace_stage.rs`
- Modify: `crates/rupu-cp/src/host/mod.rs` (declare + re-export the module), `crates/rupu-cp/src/host/local.rs` (call the shared core), `crates/rupu-cp/src/host/http.rs` + `crates/rupu-cp/src/api/workspace.rs` (cleanup-on-error; optional dedup)
- Test: `crates/rupu-cp/src/host/workspace_stage.rs`

**Interfaces:**
- Consumes: `rupu_workspace::{stage, collect_delta}`; the connector helpers `decode_payload`, `encode_delta`, `serialize_baseline`, `deserialize_baseline`, `MAX_WORKSPACE_BYTES` (in `crates/rupu-cp/src/host/connector.rs`).
- Produces:
  - `pub(crate) fn stage_to_dir(payload: &[u8], cache_root: &Path) -> Result<String, HostConnectorError>` — size-guards, `decode_payload`, stages into `<cache_root>/workspace-sync/<ulid>/work`, writes the `baseline.json` sidecar one level up, returns the `work` path string.
  - `pub(crate) fn collect_from_dir(working_dir: &str, cache_root: &Path) -> Result<Vec<u8>, HostConnectorError>` — confines `working_dir` under `<cache_root>/workspace-sync`, reads the sidecar baseline, `collect_delta`, `encode_delta`, removes the scratch, returns bytes.
  - `pub(crate) fn confine(path: &Path, root: &Path) -> Result<PathBuf, HostConnectorError>` — canonicalize + `starts_with(root)`; reject `..`/absolute-escape.

- [ ] **Step 1: Write the failing tests**

In `workspace_stage.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn stage_then_collect_round_trips_tar() {
        // build a tar payload from a non-git workspace via rupu_workspace::pack
        let ws = tempfile::tempdir().unwrap();
        fs::write(ws.path().join("a.txt"), "orig").unwrap();
        let payload = rupu_workspace::pack(ws.path()).unwrap();
        let encoded = crate::host::connector::encode_payload(&payload); // wire form used by decode_payload

        let cache = tempfile::tempdir().unwrap();
        let work = stage_to_dir(&encoded, cache.path()).unwrap();
        // simulate a remote edit
        fs::write(std::path::Path::new(&work).join("a.txt"), "EDITED").unwrap();
        let delta_bytes = collect_from_dir(&work, cache.path()).unwrap();
        let delta = crate::host::connector::decode_delta(&delta_bytes).unwrap();
        assert!(delta.changed.iter().any(|p| p == "a.txt"));
        // scratch cleaned
        assert!(!std::path::Path::new(&work).exists());
    }

    #[test]
    fn confine_rejects_traversal() {
        let root = tempfile::tempdir().unwrap();
        let escape = root.path().join("workspace-sync").join("..").join("..").join("etc");
        assert!(confine(&escape, &root.path().join("workspace-sync")).is_err());
    }

    #[test]
    fn collect_rejects_working_dir_outside_cache() {
        let cache = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("work");
        std::fs::create_dir_all(&outside).unwrap();
        let err = collect_from_dir(outside.to_str().unwrap(), cache.path());
        assert!(err.is_err());
    }
}
```

> Confirm the exact names of the payload/delta wire helpers in `connector.rs` (the plan assumes `encode_payload`/`decode_payload`/`encode_delta`/`decode_delta`). If a public `encode_payload` does not exist (only `decode_payload`), build the test payload by encoding through whatever the Local transport / T6-era bridge uses, or expose a `pub(crate) encode_payload` symmetric with `decode_payload`. The round-trip must use the SAME wire form the SSH path will use.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp --lib host::workspace_stage`
Expected: FAIL — module/functions not found.

- [ ] **Step 3: Implement the shared core**

Create `crates/rupu-cp/src/host/workspace_stage.rs` (mirrors `local.rs` lines 245–283 exactly, plus the confinement guard):

```rust
//! Shared workspace stage/collect core used by every HostConnector that
//! stages locally (Local, the remote `rupu __workspace` helper, HttpCp). One
//! implementation so the transports can't diverge.

use std::path::{Path, PathBuf};

use ulid::Ulid;

use crate::host::connector::{
    decode_payload, deserialize_baseline, encode_delta, serialize_baseline, HostConnectorError,
    MAX_WORKSPACE_BYTES,
};

/// Stage a packed workspace under `<cache_root>/workspace-sync/<ulid>/work`,
/// persisting the baseline sidecar one level up. Returns the `work` path.
pub(crate) fn stage_to_dir(payload: &[u8], cache_root: &Path) -> Result<String, HostConnectorError> {
    if payload.len() > MAX_WORKSPACE_BYTES {
        return Err(HostConnectorError::Invalid(format!(
            "workspace payload {} bytes exceeds limit {MAX_WORKSPACE_BYTES}",
            payload.len()
        )));
    }
    let decoded = decode_payload(payload)?;
    let base = cache_root.join("workspace-sync").join(Ulid::new().to_string());
    let work = base.join("work");
    let baseline =
        rupu_workspace::stage(&decoded, &work).map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
    std::fs::write(base.join("baseline.json"), serialize_baseline(&baseline)?)
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
    Ok(work.to_string_lossy().into_owned())
}

/// Reload the baseline, diff the working dir, return the encoded delta, and
/// remove the scratch. `working_dir` is confined under `<cache_root>/workspace-sync`.
pub(crate) fn collect_from_dir(
    working_dir: &str,
    cache_root: &Path,
) -> Result<Vec<u8>, HostConnectorError> {
    let sync_root = cache_root.join("workspace-sync");
    let work = confine(Path::new(working_dir), &sync_root)?;
    let base = work
        .parent()
        .ok_or_else(|| HostConnectorError::Invalid("invalid working dir".into()))?;
    let baseline_bytes = std::fs::read(base.join("baseline.json"))
        .map_err(|e| HostConnectorError::Invalid(format!("baseline missing: {e}")))?;
    let baseline = deserialize_baseline(&baseline_bytes)?;
    let delta = rupu_workspace::collect_delta(&work, &baseline)
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
    let bytes = encode_delta(&delta);
    let _ = std::fs::remove_dir_all(base);
    Ok(bytes)
}

/// Canonicalize `path` and confirm it stays under `root`. Rejects `..`/absolute
/// escapes.
pub(crate) fn confine(path: &Path, root: &Path) -> Result<PathBuf, HostConnectorError> {
    let canon = path
        .canonicalize()
        .map_err(|e| HostConnectorError::Invalid(format!("path: {e}")))?;
    let root_canon = root
        .canonicalize()
        .map_err(|e| HostConnectorError::Invalid(format!("root: {e}")))?;
    if !canon.starts_with(&root_canon) {
        return Err(HostConnectorError::Invalid(format!(
            "path escapes workspace-sync root: {}",
            canon.display()
        )));
    }
    Ok(canon)
}
```

Declare it in `crates/rupu-cp/src/host/mod.rs`: `pub(crate) mod workspace_stage;`. If the `connector` helpers aren't `pub(crate)`, widen their visibility to `pub(crate)` (they're currently imported within the crate, so this should already hold).

- [ ] **Step 4: Refactor `LocalHostConnector` to call the shared core**

In `local.rs`, replace the bodies of `stage_workspace`/`collect_workspace_delta` with calls to `stage_to_dir(&payload, &self.global_dir)` / `collect_from_dir(working_dir, &self.global_dir)`. Behavior is byte-identical (the shared core is lifted verbatim, plus the confinement guard — Local's working dirs are always under `global_dir/workspace-sync`, so they pass). Keep the doc comments.

- [ ] **Step 5: Uniform cleanup-on-error**

The 3c leak: when `launch_agent` (between stage and collect) fails, or the poll times out, `collect_from_dir` is never called, so the staged scratch leaks. The clean fix is in the **FleetUnitDispatcher** composition (rupu-cli, T4-adjacent) OR a best-effort sweep. Since the dispatcher is where stage/launch/collect are sequenced, add cleanup there: if `launch_agent` or the poll fails after a successful `stage_workspace`, call a new best-effort `HostConnector::discard_workspace(working_dir)` (default no-op; Local/SSH/HttpCp remove the scratch). Add the trait method (defaulted) here and implement the removal for Local (via `collect_from_dir`'s `remove_dir_all(base)` logic factored as `discard_dir(working_dir, cache_root)`), HttpCp (endpoint), and SSH (T4). 

> If adding a `discard_workspace` port method is deemed too broad for this task by the implementer/reviewer, the acceptable minimal alternative is: document that scratch is swept by the existing best-effort 7-day cache sweep and leave a `tracing::warn!` on the leak paths — BUT prefer the explicit `discard_workspace` since the spec's approved scope is "fix cleanup uniformly". Implement `discard_workspace` as a defaulted trait method (`Ok(())`) plus Local + a `discard_from_dir(working_dir, cache_root)` shared helper (confine + `remove_dir_all`), and call it from the dispatcher on the stage-succeeded-but-failed-later paths. Add a test that a simulated launch failure after stage removes the scratch.

- [ ] **Step 6: Run tests, format, lint, commit**

```bash
cargo test -p rupu-cp --lib host::workspace_stage host::local
rustfmt crates/rupu-cp/src/host/workspace_stage.rs crates/rupu-cp/src/host/local.rs crates/rupu-cp/src/host/mod.rs
cargo clippy -p rupu-cp --no-deps
git add crates/rupu-cp/src/host
git commit -m "refactor(multi-host): shared stage/collect core + uniform cleanup (ssh-ws T2)"
```
Expected: shared-core round-trip + confinement + Local-unchanged + cleanup tests pass; existing Local/HttpCp tests green.

---

## Task 3: Hidden `rupu __workspace stage|collect` helper

**Files:**
- Modify: `crates/rupu-cli/src/lib.rs` (`Cmd` enum ~line 47; dispatch ~line 211)
- Create: `crates/rupu-cli/src/cmd/workspace_helper.rs`; declare in `crates/rupu-cli/src/cmd/mod.rs`
- Test: `crates/rupu-cli/src/cmd/workspace_helper.rs`

**Interfaces:**
- Consumes: `rupu_cp::host::workspace_stage::{stage_to_dir, collect_from_dir}` (T2); the global-dir resolver the CP/CLI already use.
- Produces: `Cmd::Workspace { action: WorkspaceHelperAction }` (hidden); `cmd::workspace_helper::handle(action) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the failing test**

In `workspace_helper.rs`, test the handler core against a tempdir cache (factor the byte logic into a testable fn `stage_bytes(stdin: &[u8], cache_root: &Path) -> anyhow::Result<String>` and `collect_bytes(working_dir: &str, cache_root: &Path) -> anyhow::Result<Vec<u8>>` so the test doesn't need real stdin/stdout):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_stage_then_collect_round_trips() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("a.txt"), "orig").unwrap();
        let payload = rupu_workspace::pack(ws.path()).unwrap();
        let encoded = rupu_cp::host::connector::encode_payload(&payload);

        let cache = tempfile::tempdir().unwrap();
        let work = stage_bytes(&encoded, cache.path()).unwrap();
        std::fs::write(std::path::Path::new(&work).join("a.txt"), "EDITED").unwrap();
        let delta = collect_bytes(&work, cache.path()).unwrap();
        assert!(!delta.is_empty());
        let d = rupu_cp::host::connector::decode_delta(&delta).unwrap();
        assert!(d.changed.iter().any(|p| p == "a.txt"));
    }

    #[test]
    fn helper_stage_rejects_oversize() {
        let cache = tempfile::tempdir().unwrap();
        let huge = vec![0u8; rupu_cp::host::connector::MAX_WORKSPACE_BYTES + 1];
        assert!(stage_bytes(&huge, cache.path()).is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli --lib workspace_helper`
Expected: FAIL — module/functions not found.

- [ ] **Step 3: Implement the handler**

Create `crates/rupu-cli/src/cmd/workspace_helper.rs`:

```rust
//! Hidden `rupu __workspace stage|collect` helper — the remote side of SSH
//! workspace sync. Reads/writes raw bytes over stdin/stdout and delegates to
//! the shared rupu-cp staging core so remote staging is byte-identical to the
//! Local transport. Confined to the rupu cache root.

use std::io::{Read, Write};

use clap::Subcommand;

use rupu_cp::host::workspace_stage::{collect_from_dir, stage_to_dir};

#[derive(Subcommand, Debug)]
pub enum WorkspaceHelperAction {
    /// Stage a packed workspace read from stdin; print the working dir.
    Stage,
    /// Collect the change-delta from a staged working dir; write it to stdout.
    Collect { working_dir: String },
}

pub async fn handle(action: WorkspaceHelperAction) -> anyhow::Result<()> {
    let cache_root = cache_root()?;
    match action {
        WorkspaceHelperAction::Stage => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            let work = stage_bytes(&buf, &cache_root)?;
            println!("{work}");
        }
        WorkspaceHelperAction::Collect { working_dir } => {
            let delta = collect_bytes(&working_dir, &cache_root)?;
            std::io::stdout().write_all(&delta)?;
            std::io::stdout().flush()?;
        }
    }
    Ok(())
}

fn stage_bytes(stdin: &[u8], cache_root: &std::path::Path) -> anyhow::Result<String> {
    Ok(stage_to_dir(stdin, cache_root).map_err(|e| anyhow::anyhow!(e.to_string()))?)
}

fn collect_bytes(working_dir: &str, cache_root: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    Ok(collect_from_dir(working_dir, cache_root).map_err(|e| anyhow::anyhow!(e.to_string()))?)
}

/// The rupu global/cache dir — the SAME base the Local transport stages under.
fn cache_root() -> anyhow::Result<std::path::PathBuf> {
    // Reuse the existing global-dir resolver (identify it in the CLI: the same
    // one `rupu cp serve` / paths use for `global_dir`). Placeholder call:
    Ok(crate::paths::global_dir()?)
}
```

> Identify the real global-dir resolver used elsewhere in the CLI (e.g. `crate::paths::global_dir()` or a `rupu_config` call — grep for how `cp serve` / other subcommands resolve the global dir) and use it verbatim so the remote helper's cache root matches what the Local transport uses. Replace the placeholder `cache_root()` body accordingly.

- [ ] **Step 4: Wire the subcommand (hidden)**

In `crates/rupu-cli/src/lib.rs`, add to `Cmd` (with `hide`):

```rust
    /// Internal: remote workspace stage/collect helper (SSH workspace sync).
    #[command(hide = true)]
    Workspace {
        #[command(subcommand)]
        action: cmd::workspace_helper::WorkspaceHelperAction,
    },
```

Name the CLI token `__workspace` via `#[command(name = "__workspace")]` on the variant (so the remote invocation is `rupu __workspace ...`). And in `run()` dispatch (~line 211):

```rust
        Cmd::Workspace { action } => cmd::workspace_helper::handle(action).await,
```

Declare `pub mod workspace_helper;` in `crates/rupu-cli/src/cmd/mod.rs`.

- [ ] **Step 5: Run tests, format, lint, commit**

```bash
cargo test -p rupu-cli --lib workspace_helper
rustfmt crates/rupu-cli/src/cmd/workspace_helper.rs crates/rupu-cli/src/cmd/mod.rs crates/rupu-cli/src/lib.rs
cargo clippy -p rupu-cli --no-deps
git add crates/rupu-cli/src/cmd/workspace_helper.rs crates/rupu-cli/src/cmd/mod.rs crates/rupu-cli/src/lib.rs
git commit -m "feat(multi-host): hidden rupu __workspace stage/collect helper (ssh-ws T3)"
```
Expected: the 2 handler tests pass; `--help` does not list `__workspace` (hidden); clippy clean for the new file.

---

## Task 4: SSH `HostConnector` stage/collect via `run_bytes`

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs` (`stage_workspace`/`collect_workspace_delta` stubs ~lines 687/698; SSH `discard_workspace` from T2 if that path was chosen)
- Test: `crates/rupu-cp/src/host/ssh.rs`

**Interfaces:**
- Consumes: `RemoteExec::run_bytes` (T1); `build_remote_command` (~line 53); `MAX_WORKSPACE_BYTES`.
- Produces: working SSH `stage_workspace`/`collect_workspace_delta`.

- [ ] **Step 1: Write the failing tests**

Extend the `FakeExec` (from T1) so its scripted `run_bytes` can either return bytes or run the real shared helper logic. Add tests to `ssh.rs`:

```rust
    #[tokio::test]
    async fn ssh_stage_returns_working_dir_line() {
        // FakeExec scripted to return "path/to/work\n" as stage stdout.
        let exec = Arc::new(FakeExec::with_bytes_ok(b"/cache/workspace-sync/x/work\n".to_vec()));
        let conn = SshHostConnector::new(/* host, port, identity, */ exec.clone() /* ... */);
        let dir = conn.stage_workspace(b"PAYLOAD".to_vec()).await.unwrap();
        assert_eq!(dir, "/cache/workspace-sync/x/work");
        let (cmd, stdin) = exec.last_bytes_call.lock().unwrap().clone().unwrap();
        assert!(cmd.contains("__workspace") && cmd.contains("stage"));
        assert_eq!(stdin.as_deref(), Some(&b"PAYLOAD"[..]));
    }

    #[tokio::test]
    async fn ssh_stage_nonzero_maps_to_remote_error() {
        let exec = Arc::new(FakeExec::with_bytes_err(RemoteExecError::NonZero {
            code: Some(1),
            stderr: "helper failed".into(),
        }));
        let conn = SshHostConnector::new(/* ... */ exec);
        let err = conn.stage_workspace(b"x".to_vec()).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Remote(1, _)));
    }

    #[tokio::test]
    async fn ssh_stage_spawn_failure_maps_to_unreachable() {
        let exec = Arc::new(FakeExec::with_bytes_err(RemoteExecError::Spawn("no route".into())));
        let conn = SshHostConnector::new(/* ... */ exec);
        let err = conn.stage_workspace(b"x".to_vec()).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Unreachable(_)));
    }

    #[tokio::test]
    async fn ssh_stage_oversize_payload_rejected() {
        let exec = Arc::new(FakeExec::with_bytes_ok(Vec::new()));
        let conn = SshHostConnector::new(/* ... */ exec);
        let huge = vec![0u8; MAX_WORKSPACE_BYTES + 1];
        let err = conn.stage_workspace(huge).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Invalid(_)));
    }
```

> Use the SSH connector's real constructor/field names (check the struct + how `new` is called elsewhere in the ssh tests). If constructing `SshHostConnector` in tests is verbose, mirror how the existing ssh tests build it.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp --lib host::ssh -- ssh_stage`
Expected: FAIL — stubs still return `Unsupported`.

- [ ] **Step 3: Implement the stage/collect impls**

Replace the two stubs (~687/698):

```rust
    async fn stage_workspace(&self, payload: Vec<u8>) -> Result<String, HostConnectorError> {
        if payload.len() > MAX_WORKSPACE_BYTES {
            return Err(HostConnectorError::Invalid(format!(
                "workspace payload {} bytes exceeds limit {MAX_WORKSPACE_BYTES}",
                payload.len()
            )));
        }
        let cmd = build_remote_command(&[
            "rupu".into(),
            "__workspace".into(),
            "stage".into(),
        ]);
        let out = self
            .exec
            .run_bytes(&cmd, Some(payload))
            .await
            .map_err(map_remote_err)?;
        let line = String::from_utf8_lossy(&out);
        let dir = line.trim();
        if dir.is_empty() {
            return Err(HostConnectorError::Invalid(
                "remote stage returned no working dir".into(),
            ));
        }
        Ok(dir.to_string())
    }

    async fn collect_workspace_delta(
        &self,
        working_dir: &str,
    ) -> Result<Vec<u8>, HostConnectorError> {
        let cmd = build_remote_command(&[
            "rupu".into(),
            "__workspace".into(),
            "collect".into(),
            working_dir.to_string(),
        ]);
        let bytes = self.exec.run_bytes(&cmd, None).await.map_err(map_remote_err)?;
        if bytes.len() > MAX_WORKSPACE_BYTES {
            return Err(HostConnectorError::Invalid(format!(
                "workspace delta {} bytes exceeds limit {MAX_WORKSPACE_BYTES}",
                bytes.len()
            )));
        }
        Ok(bytes)
    }
```

Add the error mapper near the impl:

```rust
fn map_remote_err(e: RemoteExecError) -> HostConnectorError {
    match e {
        RemoteExecError::Spawn(m) => HostConnectorError::Unreachable(m),
        RemoteExecError::NonZero { code, stderr } => {
            HostConnectorError::Remote(code.unwrap_or(-1) as u16, stderr)
        }
    }
}
```

> Confirm `HostConnectorError::Remote`'s exact signature (from 3c it was `Remote(u16, String)`); adjust the cast if the code type differs. Confirm `self.exec` is the SSH connector's `RemoteExec` handle field name.

If T2 chose the `discard_workspace` port method, implement it for SSH here: `run_bytes(build_remote_command(["rupu","__workspace","discard",working_dir]), None)` — and add a `discard` action to the T3 helper. (Only if T2 added the port method; otherwise skip.)

- [ ] **Step 4: Run tests, format, lint, commit**

```bash
cargo test -p rupu-cp --lib host::ssh
rustfmt crates/rupu-cp/src/host/ssh.rs
cargo clippy -p rupu-cp --no-deps
git add crates/rupu-cp/src/host/ssh.rs
git commit -m "feat(multi-host): SSH workspace stage/collect via run_bytes (ssh-ws T4)"
```
Expected: the 4 new ssh tests pass; existing ssh tests green.

---

## Task 5: End-to-end SSH parity test

**Files:**
- Create: `crates/rupu-cp/tests/ssh_workspace_sync.rs` (or add to an existing host integration test module)
- Test: that file

**Interfaces:**
- Consumes: `SshHostConnector`, a `FakeExec` (or a test `RemoteExec`) that runs the **real** shared helper logic (`stage_to_dir`/`collect_from_dir` against a tempdir cache), `rupu_workspace::{pack, apply_deltas}`, the connector wire helpers.

- [ ] **Step 1: Write the e2e test**

A `RemoteExec` test double that interprets the remote command: on `... __workspace stage` it calls `stage_to_dir(stdin, cache)`; on `... __workspace collect <dir>` it calls `collect_from_dir(dir, cache)`. Then drive the SSH connector through the same shape as the 3c e2e:

```rust
// pseudocode shape — fill against the real APIs, mirroring 3c's workspace_sync_e2e.rs
#[tokio::test]
async fn ssh_workspace_sync_round_trips_git_and_tar() {
    for use_git in [true, false] {
        // 1. build a workspace (git repo or plain dir), pack it
        // 2. SshHostConnector with a HelperExec { cache: tempdir } as its RemoteExec
        // 3. dir = conn.stage_workspace(encoded_payload).await
        // 4. simulate the remote agent editing a file under `dir`
        // 5. delta = conn.collect_workspace_delta(&dir).await
        // 6. apply the decoded delta to a fresh copy of the coordinator workspace
        //    via rupu_workspace::apply_deltas and assert the edit landed
    }
}
```

> Fill the body concretely, reusing the harness shape from `crates/rupu-orchestrator/tests/workspace_sync_e2e.rs` (3c) and the connector helpers. The `HelperExec` must go through `stage_to_dir`/`collect_from_dir` (the real code), so this proves the SSH command wiring + the shared core together. Cover both a git workspace and a non-git (tar) workspace.

- [ ] **Step 2: Run, format, lint, commit**

```bash
cargo test -p rupu-cp --test ssh_workspace_sync
cargo test -p rupu-cp --lib host
rustfmt crates/rupu-cp/tests/ssh_workspace_sync.rs
cargo clippy -p rupu-cp --all-targets --no-deps
git add crates/rupu-cp/tests/ssh_workspace_sync.rs
git commit -m "test(multi-host): e2e SSH workspace sync parity — git + tar (ssh-ws T5)"
```
Expected: the e2e test passes (both git and tar); host module suite green.

---

## Self-Review

**Spec coverage:**
- Spine 1 (bytes over ssh pipe, same codec) → T1 (`run_bytes`) + T4 (SSH connector). ✅
- Spine 2 (binary-capable RemoteExec) → T1. ✅
- Spine 3 (hidden `__workspace` helper, codec-backed, stdin/stdout) → T3, backed by T2's shared core. ✅
- Spine 4 (reuse the transport-agnostic composition) → nothing to change; noted (orchestrator/dispatcher untouched). ✅
- Spine 5 (uniform cleanup) → T2 (Local/HttpCp + shared `discard`) + T4 (SSH). ✅
- Errors/security (failure mapping, confinement, size guard both ways, no stored keys, no silent fallback) → T2 (confine), T4 (map_remote_err + size guard), T1 (NonZero). ✅
- Testing (run_bytes; helper round-trip git+tar; confinement; SSH connector mapping; cleanup; e2e) → T1/T2/T3/T4/T5. ✅

**Placeholder scan:** the two deliberately-parameterized spots are the CLI global-dir resolver (`cache_root()` — the plan directs identifying the existing resolver and names the likely symbol) and the e2e body (directs reuse of the 3c harness). Every other code step is complete. The wire-helper names (`encode_payload`/`decode_delta`) are flagged for confirmation against `connector.rs`. No "TBD"/"handle errors"/vacuous-test placeholders.

**Type consistency:** `run_bytes(&str, Option<Vec<u8>>) -> Result<Vec<u8>, RemoteExecError>` and `RemoteExecError::NonZero{code,stderr}` (T1) are consumed by `map_remote_err` (T4); `stage_to_dir(&[u8],&Path)->Result<String,_>` / `collect_from_dir(&str,&Path)->Result<Vec<u8>,_>` / `confine` (T2) are consumed by T3 and the T5 double; `HostConnectorError::{Invalid,Unreachable,Remote}` used consistently; `MAX_WORKSPACE_BYTES` from `connector.rs` throughout.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-01-rupu-ssh-workspace-sync-plan.md`. Build via subagent-driven-development: fresh implementer per task, task review (spec + quality) after each, a broad whole-branch review at the end, then a single PR to `main` (no self-merge — matt reviews before merge).
