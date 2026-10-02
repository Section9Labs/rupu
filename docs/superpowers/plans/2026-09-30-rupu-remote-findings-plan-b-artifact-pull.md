# Remote findings — Plan B: artifact pull and serving Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A finding artifact that a remote unit recorded (`stored: external`, `host` set, from Plan A) can be downloaded from the coordinator CP. The first request pulls the blob from the host that holds it, over whichever transport that host uses, verifies it against the recorded sha256 and size, and stores it in the coordinator's content-addressed store.

**Architecture:**

- **New connector method.** A required `HostConnector::pull_finding_artifact(sha256, dest, max_bytes)` streams the host-store blob into a temp file. How each transport does it:

  | Transport | How the blob arrives |
  |---|---|
  | Local | Copies from the shared store |
  | SSH | Runs a hidden `rupu __findings artifact <sha>` and streams it through a new `RemoteExec::run_to_file` |
  | HTTP | Streams the remote CP's new `GET /api/findings/artifacts/:sha256` |
  | Tunnel | New `ArtifactPull` / `ArtifactChunk` / `ArtifactPullDone` frames |
  | Bucket | Downloads `artifacts/<sha256>`, which the worker uploaded when the run finished |

- **New coordinator endpoint.** `GET /api/findings/:id/artifacts/:sha256` checks that the sha belongs to that finding and serves the store blob. When the blob isn't there, it pulls it (one pull per sha), verifies it, renames it into the store, then serves it. A local over-cap artifact is served from the workspace after a re-hash.

**Tech Stack:** Rust 2021, axum 0.7, reqwest 0.12 (`stream`), tokio / tokio-util (`io`), object_store 0.14 (multipart put, streaming get), base64 0.22, sha2.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-remote-findings-transport-design.md` (Plan B is §B1–B2). **Requires Plan A** (`docs/superpowers/plans/2026-09-30-rupu-remote-findings-plan-a-coverage-transport.md`) merged into this branch first. Plan B uses its `StreamLine`, `STREAM_FILE` and `unit_coverage`, and the capability constants in `node/protocol.rs`.

## Global Constraints

**Repo rules**
- Every Plan A constraint applies: workspace deps only; `deny(clippy::all)`; no `unsafe`; no `cargo fmt`, so hand-format your own lines only; no bare `git stash`. Clippy runs with `-A clippy::question_mark` because of the pre-existing `completers.rs:127` hit.

**Validation and limits**
- A sha256 is exactly 64 lowercase hex characters. Validate it with `rupu_coverage::report::is_sha256_hex` before it touches any path. `ArtifactStore::blob_path` does no validation of its own.
- **Transfer cap:** every pull aborts once more than the recorded `ArtifactRef.size` bytes have arrived (`max_bytes`).
- **Verification:** the caller verifies the pulled bytes by size and sha256 before renaming them into the store.
- **Temp files:** each pull writes to `<store>/<aa>/.pull-<sha256>-<ulid>`, which is removed on any failure.

**Names**
- Capability and feature strings: `findings.artifact_blob` for the HTTP `features` list, and `findings.artifact_pull` for the tunnel `Hello.capabilities`.
- Tunnel chunk size: 1 MiB decoded (`ARTIFACT_CHUNK_BYTES = 1 << 20`).
- Timeouts: tunnel pull idle timeout 60 s; HTTP pull per-request timeout 30 min.
- The coordinator's store is `<global_dir>/findings/artifacts`.

**Serving and SSH**
- Response headers:
  - Text kinds get `Content-Type: text/plain; charset=utf-8` and `X-Content-Type-Options: nosniff`.
  - Everything else gets `Content-Type: application/octet-stream` and `Content-Disposition: attachment; filename="<basename>"`.
  - Never serve an artifact as HTML.
- An unavailable artifact returns 404 with body `{"unavailable": "<reason>"}`.
- SSH: one invocation per pull. Never burst connections at a host.

## Deviations from the spec (flag in the PR)

1. **`pull_finding_artifact` takes a third parameter, `max_bytes`** (the recorded size), so every transport can stop an oversize transfer as it streams, not after.
2. **`RemoteExec::run_to_file` has a default body.** It buffers through `run_bytes`, so the roughly 20 SSH test doubles compile unchanged. The production `SshExec` overrides it with a true stream. The default is correct, just not streaming, and only test doubles use it.

## File map

| File | Responsibility |
|---|---|
| `crates/rupu-coverage/src/report/artifacts.rs` | `is_sha256_hex`, public `sha256_file` |
| `crates/rupu-cli/src/cmd/findings_helper.rs` (new) + `lib.rs`, `cmd/mod.rs` | hidden `rupu __findings artifact <sha256>`; `blob_path`, `write_blob` |
| `crates/rupu-cp/src/host/ssh.rs` | `RemoteExec::run_to_file`, `RemoteExecError::TooLarge`, SSH `pull_finding_artifact` |
| `crates/rupu-cp/src/host/connector.rs` | trait method, `validate_sha256`, `copy_blob_capped`; test doubles |
| `crates/rupu-cp/src/host/{local,http,tunnel}.rs`, `host/bucket/{mod,object_store_bucket,connector}.rs` | per-transport pulls |
| `crates/rupu-cp/src/api/findings.rs` | remote blob endpoint; coordinator artifact endpoint |
| `crates/rupu-cp/src/node/{protocol,registry,server}.rs` | tunnel pull frames, `NodeConn` pull routing |
| `crates/rupu-cli/src/cmd/node.rs` | node answers `ArtifactPull`; bucket worker uploads referenced blobs at run end |
| `crates/rupu-cp/Cargo.toml`, `crates/rupu-cli/Cargo.toml` | `tokio-util` (`io`) normal dep for rupu-cp; `base64` for both |
| `docs/coverage.md`, `CLAUDE.md` | docs |

---

### Task 1: sha helpers, the hidden `rupu __findings artifact`, and `RemoteExec::run_to_file`

**Files:**
- Modify: `crates/rupu-coverage/src/report/artifacts.rs` (`sha256_file` at `:90`), `crates/rupu-coverage/src/report/mod.rs`
- Create: `crates/rupu-cli/src/cmd/findings_helper.rs`
- Modify: `crates/rupu-cli/src/cmd/mod.rs` (next to `pub mod workspace_helper;` at `:42`)
- Modify: `crates/rupu-cli/src/lib.rs` (the `Cmd` variant next to `Workspace` at `:226-231`; dispatch at `:389`; `ensure_output_format_supported` at `:494-498`)
- Modify: `crates/rupu-cp/src/host/ssh.rs` (`RemoteExecError` at `:538`; the trait at `:553`; `SshExec` at `:600`; `map_remote_err` at `:983`)

**Interfaces:**
- Produces:
  - `pub fn is_sha256_hex(s: &str) -> bool` and `pub fn sha256_file(p: &Path) -> std::io::Result<String>`, both re-exported from `rupu_coverage::report`.
  - `pub(crate) fn blob_path(global: &Path, sha256: &str) -> anyhow::Result<PathBuf>` and `fn write_blob(global: &Path, sha256: &str, out: &mut impl Write) -> anyhow::Result<u64>` in `findings_helper`.
  - `RemoteExec::run_to_file(&self, remote_command: &str, dest: &Path, max_bytes: u64) -> Result<u64, RemoteExecError>`
  - `RemoteExecError::TooLarge(u64)`

- [ ] **Step 1: Write the failing tests.**

In `crates/rupu-coverage/src/report/artifacts.rs` tests:

```rust
    #[test]
    fn is_sha256_hex_is_exact() {
        assert!(is_sha256_hex(&"ab".repeat(32)));
        assert!(!is_sha256_hex(&"AB".repeat(32)), "uppercase is rejected");
        assert!(!is_sha256_hex(&"ab".repeat(31)));
        assert!(!is_sha256_hex("../../etc/passwd"));
        assert!(!is_sha256_hex(""));
    }
```

Create `crates/rupu-cli/src/cmd/findings_helper.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_blob_copies_the_stored_bytes() {
        let global = tempfile::tempdir().unwrap();
        let sha = "cd".repeat(32);
        let p = blob_path(global.path(), &sha).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"poc bytes").unwrap();
        let mut out = Vec::new();
        assert_eq!(write_blob(global.path(), &sha, &mut out).unwrap(), 9);
        assert_eq!(out, b"poc bytes");
    }

    #[test]
    fn write_blob_refuses_a_missing_blob_and_a_bad_sha() {
        let global = tempfile::tempdir().unwrap();
        let mut out = Vec::new();
        let err = write_blob(global.path(), &"ee".repeat(32), &mut out).unwrap_err();
        assert!(err.to_string().contains("not in this host's store"), "{err}");
        assert!(write_blob(global.path(), "../x", &mut out).is_err());
    }
}
```

In `crates/rupu-cp/src/host/ssh.rs` tests, add a test of the default `run_to_file` through `FakeExec` (its `run_bytes` is scriptable via `with_bytes_ok`):

```rust
    #[tokio::test]
    async fn default_run_to_file_writes_and_caps() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("blob");
        let fake = FakeExec::with_bytes_ok(b"0123456789".to_vec());
        assert_eq!(fake.run_to_file("cmd", &dest, 10).await.unwrap(), 10);
        assert_eq!(std::fs::read(&dest).unwrap(), b"0123456789");
        let fake = FakeExec::with_bytes_ok(b"0123456789".to_vec());
        assert!(matches!(
            fake.run_to_file("cmd", &dest, 9).await,
            Err(RemoteExecError::TooLarge(9))
        ));
    }
```

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-coverage --lib is_sha256_hex; cargo test -p rupu-cli --lib findings_helper; cargo test -p rupu-cp --lib default_run_to_file`
Expected: compile errors.

- [ ] **Step 3: Add the sha helpers.** In `artifacts.rs`, make `sha256_file` `pub` (it's `pub(crate)` at `:90`) and add:

```rust
/// Exactly 64 lowercase hex characters — the only shape a store key takes.
/// Callers MUST check this before [`ArtifactStore::blob_path`], which does no
/// validation of its own.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
```

In `report/mod.rs`, extend the `pub use artifacts::{…}` line with `is_sha256_hex, sha256_file`.

- [ ] **Step 4: Add the hidden helper.** Put this above the tests in `findings_helper.rs`:

```rust
//! `rupu __findings` — hidden helper a coordinator runs over SSH to pull a
//! finding artifact from this host's store (spec
//! 2026-09-30-rupu-remote-findings-transport-design.md §B1).

use anyhow::Context as _;
use clap::Subcommand;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum FindingsHelperAction {
    /// Write the stored blob `<sha256>` to stdout; nonzero exit if absent.
    Artifact { sha256: String },
}

pub async fn handle(action: FindingsHelperAction) -> ExitCode {
    match handle_inner(action) {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

fn handle_inner(action: FindingsHelperAction) -> anyhow::Result<()> {
    match action {
        FindingsHelperAction::Artifact { sha256 } => {
            let global = crate::paths::global_dir()?;
            let mut out = std::io::stdout().lock();
            write_blob(&global, &sha256, &mut out)?;
            out.flush()?;
            Ok(())
        }
    }
}

/// `<global>/findings/artifacts/<aa>/<sha256>`, after validating the sha.
pub(crate) fn blob_path(global: &Path, sha256: &str) -> anyhow::Result<PathBuf> {
    if !rupu_coverage::report::is_sha256_hex(sha256) {
        anyhow::bail!("{sha256:?} is not a sha256 (64 lowercase hex characters)");
    }
    Ok(
        rupu_coverage::report::ArtifactStore::new(global.join("findings").join("artifacts"))
            .blob_path(sha256),
    )
}

/// Copy the stored blob to `out`; returns the byte count.
fn write_blob(global: &Path, sha256: &str, out: &mut impl Write) -> anyhow::Result<u64> {
    let path = blob_path(global, sha256)?;
    let mut f = std::fs::File::open(&path)
        .with_context(|| format!("artifact {sha256} is not in this host's store ({})", path.display()))?;
    Ok(std::io::copy(&mut f, out)?)
}
```

In `cmd/mod.rs`, add `pub mod findings_helper;`. In `lib.rs`, next to the `Workspace` variant, add:

```rust
    /// Internal: stream a finding artifact from this host's store (SSH artifact pull).
    #[command(name = "__findings", hide = true)]
    FindingsHelper {
        #[command(subcommand)]
        action: cmd::findings_helper::FindingsHelperAction,
    },
```

Add the dispatch arm `Cmd::FindingsHelper { action } => cmd::findings_helper::handle(action).await,`. Add this arm to `ensure_output_format_supported`:

```rust
        Cmd::FindingsHelper { .. } => output::formats::ensure_supported(
            "__findings",
            format,
            &[output::formats::OutputFormat::Table],
        ),
```

- [ ] **Step 5: Add `run_to_file`.** In `ssh.rs`, add a variant to `RemoteExecError`:

```rust
    #[error("remote output exceeded {0} bytes")]
    TooLarge(u64),
```

In `map_remote_err`, add `RemoteExecError::TooLarge(n) => HostConnectorError::Invalid(format!("remote output exceeded its expected {n} bytes")),`. Add this default method to the `RemoteExec` trait:

```rust
    /// Run `remote_command` and stream its stdout into `dest` (created or
    /// truncated), failing with `TooLarge` once more than `max_bytes` arrive.
    /// Returns the byte count. This default buffers through `run_bytes` and
    /// exists for test doubles; `SshExec` overrides it with a true stream.
    async fn run_to_file(
        &self,
        remote_command: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<u64, RemoteExecError> {
        let bytes = self.run_bytes(remote_command, None).await?;
        if bytes.len() as u64 > max_bytes {
            return Err(RemoteExecError::TooLarge(max_bytes));
        }
        tokio::fs::write(dest, &bytes)
            .await
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        Ok(bytes.len() as u64)
    }
```

Add this to `impl RemoteExec for SshExec`:

```rust
    async fn run_to_file(
        &self,
        remote_command: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<u64, RemoteExecError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let argv = ssh_argv(
            &self.host,
            self.port,
            self.identity_file.as_deref(),
            remote_command,
            SHORT_CALL_CONNECT_TIMEOUT_SECS,
        );
        let mut child = tokio::process::Command::new("ssh")
            .args(&argv)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| RemoteExecError::Spawn("no stdout pipe".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| RemoteExecError::Spawn("no stderr pipe".into()))?;
        let stderr_task = tokio::spawn(async move {
            let mut s = Vec::new();
            let _ = stderr.read_to_end(&mut s).await;
            s
        });
        let io = |e: std::io::Error| RemoteExecError::Spawn(e.to_string());
        let mut file = tokio::fs::File::create(dest).await.map_err(io)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut total: u64 = 0;
        loop {
            let n = stdout.read(&mut buf).await.map_err(io)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > max_bytes {
                let _ = child.start_kill();
                return Err(RemoteExecError::TooLarge(max_bytes));
            }
            file.write_all(&buf[..n]).await.map_err(io)?;
        }
        file.flush().await.map_err(io)?;
        let status = child.wait().await.map_err(io)?;
        let stderr = stderr_task.await.unwrap_or_default();
        if !status.success() {
            return Err(RemoteExecError::NonZero {
                code: status.code(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            });
        }
        Ok(total)
    }
```

Make sure `std::path::Path` is imported in `ssh.rs`; it is already used there.

- [ ] **Step 6: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-coverage --lib report::artifacts && cargo test -p rupu-cli --lib findings_helper && cargo test -p rupu-cp --lib host::ssh`
Expected: PASS.

- [ ] **Step 7: Commit.**

```bash
git add crates/rupu-coverage crates/rupu-cli crates/rupu-cp/src/host/ssh.rs
git commit -m "feat(findings): is_sha256_hex; hidden rupu __findings artifact; RemoteExec::run_to_file (streamed over ssh)"
```

---

### Task 2: `HostConnector::pull_finding_artifact` for local and SSH; test doubles

**Files:**
- Modify: `crates/rupu-cp/src/host/connector.rs` (the trait; the helpers `validate_sha256` and `copy_blob_capped`; the doubles at `:938`, `:1034`)
- Modify: `crates/rupu-cp/src/host/local.rs`, `crates/rupu-cp/src/host/ssh.rs`
- Temporary `Unsupported` bodies, replaced in Tasks 3–5: `crates/rupu-cp/src/host/{http,tunnel}.rs`, `host/bucket/connector.rs`
- Modify: the test doubles in `crates/rupu-cp/src/api/{graph.rs:620, usage.rs:1032, runs.rs:2654, runs.rs:2796}`, `crates/rupu-cp/tests/host_registry.rs:31`, and `crates/rupu-cli/src/fleet_unit_dispatcher.rs` (the five fakes)

**Interfaces:**
- Consumes: Task 1's `is_sha256_hex` and `RemoteExec::run_to_file`.
- Produces:
  - `async fn pull_finding_artifact(&self, sha256: &str, dest: &Path, max_bytes: u64) -> Result<(), HostConnectorError>`, a required method.
  - `pub fn validate_sha256(sha256: &str) -> Result<(), HostConnectorError>`
  - `pub async fn copy_blob_capped(src: &Path, dest: &Path, max_bytes: u64) -> Result<(), HostConnectorError>`

- [ ] **Step 1: Write the failing tests.** Add these to the `connector.rs` tests:

```rust
    #[tokio::test]
    async fn copy_blob_capped_copies_refuses_missing_and_oversize() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        std::fs::write(&src, b"12345").unwrap();
        copy_blob_capped(&src, &dest, 5).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"12345");
        assert!(matches!(
            copy_blob_capped(&src, &dest, 4).await,
            Err(HostConnectorError::Invalid(_))
        ));
        assert!(matches!(
            copy_blob_capped(&tmp.path().join("absent"), &dest, 5).await,
            Err(HostConnectorError::NotFound(_))
        ));
        assert!(validate_sha256("../x").is_err());
        assert!(validate_sha256(&"ab".repeat(32)).is_ok());
    }
```

Add this to the `ssh.rs` tests. It uses `make_conn` and a `FakeExec` scripted through `with_bytes_ok`:

```rust
    #[tokio::test]
    async fn pull_finding_artifact_runs_the_hidden_helper_once() {
        let fake = std::sync::Arc::new(FakeExec::with_bytes_ok(b"blob".to_vec()));
        let (conn, _store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let dest = tmp.path().join("pulled");
        let sha = "ab".repeat(32);
        conn.pull_finding_artifact(&sha, &dest, 4).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"blob");
        let (cmd, _stdin) = fake.last_bytes_call.lock().unwrap().clone().unwrap();
        assert_eq!(cmd, format!("'rupu' '__findings' 'artifact' '{sha}'"));
        assert!(conn.pull_finding_artifact("nope", &dest, 4).await.is_err());
    }
```

`make_conn` returns the connector, the store and a temp dir. If it returns a different tuple, use whatever temp directory it gives.

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-cp --lib copy_blob_capped pull_finding_artifact_runs`
Expected: compile errors.

- [ ] **Step 3: Add the trait method and helpers.** In `connector.rs`, add this to the trait, after `unit_coverage`:

```rust
    /// Stream the blob `sha256` from this host's finding-artifact store into
    /// `dest` (a temp file the caller created inside the coordinator's
    /// store), aborting once more than `max_bytes` (the recorded size) arrive.
    /// The caller verifies size + sha256 before using it (spec
    /// 2026-09-30-rupu-remote-findings-transport-design.md §B1). No default:
    /// every transport must say how, or refuse.
    async fn pull_finding_artifact(
        &self,
        sha256: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(), HostConnectorError>;
```

Add these free functions:

```rust
/// `Invalid` unless `sha256` is a store key (64 lowercase hex).
pub fn validate_sha256(sha256: &str) -> Result<(), HostConnectorError> {
    if rupu_coverage::report::is_sha256_hex(sha256) {
        Ok(())
    } else {
        Err(HostConnectorError::Invalid(format!(
            "{sha256:?} is not a sha256 (64 lowercase hex characters)"
        )))
    }
}

/// Copy `src` to `dest`, refusing a missing source (`NotFound`) or one
/// larger than `max_bytes` (`Invalid`).
pub async fn copy_blob_capped(
    src: &Path,
    dest: &Path,
    max_bytes: u64,
) -> Result<(), HostConnectorError> {
    let meta = match tokio::fs::metadata(src).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(HostConnectorError::NotFound(format!(
                "artifact is not in this host's store ({})",
                src.display()
            )))
        }
        Err(e) => return Err(HostConnectorError::Invalid(e.to_string())),
    };
    if meta.len() > max_bytes {
        return Err(HostConnectorError::Invalid(format!(
            "stored blob is {} bytes, more than its recorded {max_bytes}",
            meta.len()
        )));
    }
    tokio::fs::copy(src, dest)
        .await
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
    Ok(())
}
```

- [ ] **Step 4: Implement it for local and SSH, with temporary stubs for the rest.**

`local.rs`:

```rust
    async fn pull_finding_artifact(
        &self,
        sha256: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(), HostConnectorError> {
        crate::host::connector::validate_sha256(sha256)?;
        let src = rupu_coverage::report::ArtifactStore::new(
            self.global_dir.join("findings").join("artifacts"),
        )
        .blob_path(sha256);
        crate::host::connector::copy_blob_capped(&src, dest, max_bytes).await
    }
```

`ssh.rs`:

```rust
    async fn pull_finding_artifact(
        &self,
        sha256: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(), HostConnectorError> {
        crate::host::connector::validate_sha256(sha256)?;
        // One invocation per pull (no connection bursts at the host). An
        // older remote rupu has no `__findings` and fails with its clap error,
        // which reaches the caller as the unavailable reason.
        let cmd = build_remote_command(&[
            "rupu".into(),
            "__findings".into(),
            "artifact".into(),
            sha256.to_string(),
        ]);
        self.exec
            .run_to_file(&cmd, dest, max_bytes)
            .await
            .map_err(map_remote_err)?;
        Ok(())
    }
```

In `http.rs`, `tunnel.rs` and `bucket/connector.rs`, add a temporary stub. Tasks 3, 4 and 5 replace these:

```rust
    async fn pull_finding_artifact(
        &self,
        _sha256: &str,
        _dest: &Path,
        _max_bytes: u64,
    ) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "artifact pull over this transport is not implemented yet".into(),
        ))
    }
```

- [ ] **Step 5: Implement it for every test double.** Add this to each of the 11 test `impl HostConnector`:

```rust
        async fn pull_finding_artifact(
            &self,
            _sha256: &str,
            _dest: &std::path::Path,
            _max_bytes: u64,
        ) -> Result<(), HostConnectorError> {
            Err(HostConnectorError::Unsupported("test double".into()))
        }
```

- [ ] **Step 6: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-cp --lib host:: && cargo check --workspace --tests`
Expected: PASS, and the workspace compiles.

- [ ] **Step 7: Commit.**

```bash
git add crates/rupu-cp crates/rupu-cli/src/fleet_unit_dispatcher.rs
git commit -m "feat(cp): HostConnector::pull_finding_artifact (required) — local copies, SSH streams rupu __findings artifact"
```

---

### Task 3: HTTP hosts serve blobs by hash, and the connector streams them

**Files:**
- Modify: `crates/rupu-cp/Cargo.toml` (add `tokio-util = { workspace = true, features = ["io"] }` to `[dependencies]`; it's currently dev-only)
- Modify: `crates/rupu-cp/src/node/protocol.rs` (constant), `crates/rupu-cp/src/api/host_info.rs` (`host_features`)
- Modify: `crates/rupu-cp/src/api/findings.rs` (route + handler)
- Modify: `crates/rupu-cp/src/host/http.rs` (replace the Task 2 stub)
- Regenerate: `apps/rupu-macos/Fixtures/host_info.json`
- Test: `crates/rupu-cp/tests/host_http.rs`

**Interfaces:**
- Produces:
  - `pub const CAP_FINDINGS_ARTIFACT_BLOB: &str = "findings.artifact_blob"`
  - `GET /api/findings/artifacts/:sha256`: 200 `application/octet-stream`, streamed; 400 for a bad sha; 404 `{"error": …}` when the blob is absent.
  - `pub(crate) fn blob_file_response(path, content_type, disposition) -> Response`, a streaming body helper reused by Task 6.

- [ ] **Step 1: Write the failing tests.** Add these to `crates/rupu-cp/tests/host_http.rs`:

```rust
fn store_blob(global: &std::path::Path, sha: &str, body: &[u8]) {
    let p = rupu_coverage::report::ArtifactStore::new(global.join("findings").join("artifacts"))
        .blob_path(sha);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn serve_cp(global: &std::path::Path) -> std::net::SocketAddr {
    let state = rupu_cp::state::AppState::new(
        global.to_path_buf(),
        rupu_config::PricingConfig::default(),
    );
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn pull_finding_artifact_streams_a_real_remotes_blob() {
    let remote = tempfile::tempdir().unwrap();
    let sha = "ab".repeat(32);
    store_blob(remote.path(), &sha, b"remote poc");
    let addr = serve_cp(remote.path()).await;

    let c = HttpHostConnector::new(format!("http://{addr}"), None);
    let dest = tempfile::tempdir().unwrap();
    let out = dest.path().join("pulled");
    c.pull_finding_artifact(&sha, &out, 10).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"remote poc");

    // Over the recorded size: refused mid-stream.
    assert!(matches!(
        c.pull_finding_artifact(&sha, &out, 3).await,
        Err(HostConnectorError::Invalid(_))
    ));
    // Absent on the remote.
    assert!(matches!(
        c.pull_finding_artifact(&"cd".repeat(32), &out, 10).await,
        Err(HostConnectorError::NotFound(_))
    ));
}

#[tokio::test]
async fn pull_finding_artifact_refuses_a_remote_without_the_feature() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .json_body(serde_json::json!({"version": "0.70.0", "features": []}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let dest = tempfile::tempdir().unwrap();
    let err = c
        .pull_finding_artifact(&"ab".repeat(32), &dest.path().join("x"), 10)
        .await
        .unwrap_err();
    assert!(matches!(err, HostConnectorError::Unsupported(_)), "{err:?}");
}
```

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-cp --test host_http pull_finding_artifact`
Expected: FAIL, because of the Task 2 stub.

- [ ] **Step 3: Add the constant and the feature.** In `protocol.rs`:

```rust
/// HTTP `/api/host/info` `features` entry: this CP serves
/// `GET /api/findings/artifacts/:sha256` from its artifact store.
pub const CAP_FINDINGS_ARTIFACT_BLOB: &str = "findings.artifact_blob";
```

Add `crate::node::protocol::CAP_FINDINGS_ARTIFACT_BLOB.to_string(),` to `host_features()`.

- [ ] **Step 4: Add the remote blob endpoint.** In `api/findings.rs`, change `routes()` to:

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/findings", get(list_findings))
        .route("/api/findings/artifacts/:sha256", get(get_artifact_blob))
}
```

Add:

```rust
/// A streaming file body with the given content headers.
pub(crate) fn blob_file_response(
    file: tokio::fs::File,
    content_type: &'static str,
    disposition: Option<String>,
    nosniff: bool,
) -> axum::response::Response {
    use axum::http::header;
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let mut resp = axum::response::Response::new(body);
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, header::HeaderValue::from_static(content_type));
    if nosniff {
        h.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            header::HeaderValue::from_static("nosniff"),
        );
    }
    if let Some(d) = disposition.and_then(|d| header::HeaderValue::from_str(&d).ok()) {
        h.insert(header::CONTENT_DISPOSITION, d);
    }
    resp
}

/// `GET /api/findings/artifacts/:sha256` — a blob from THIS host's artifact
/// store by hash, for a coordinator pulling a placed unit's artifact (spec
/// 2026-09-30-rupu-remote-findings-transport-design.md §B1). The
/// coordinator's own `/api/findings/:id/artifacts/:sha256` is what checks the
/// sha belongs to a finding; this is the host-to-coordinator channel, behind
/// the CP's bearer token.
async fn get_artifact_blob(
    State(s): State<AppState>,
    Path(sha256): Path<String>,
) -> ApiResult<axum::response::Response> {
    if !rupu_coverage::report::is_sha256_hex(&sha256) {
        return Err(ApiError::bad_request(
            "sha256 must be 64 lowercase hex characters",
        ));
    }
    let path = rupu_coverage::report::ArtifactStore::new(
        s.global_dir.join("findings").join("artifacts"),
    )
    .blob_path(&sha256);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found(format!("artifact {sha256} is not in this host's store")))?;
    Ok(blob_file_response(file, "application/octet-stream", None, false))
}
```

Add `Path` to the file's `axum::extract` imports if it isn't there, plus `ApiError`, `ApiResult` and `State` as needed.

- [ ] **Step 5: Stream it in the HTTP connector.** Replace the Task 2 stub in `http.rs`:

```rust
    async fn pull_finding_artifact(
        &self,
        sha256: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(), HostConnectorError> {
        use tokio::io::AsyncWriteExt;
        crate::host::connector::validate_sha256(sha256)?;
        self.require_feature(
            crate::node::protocol::CAP_FINDINGS_ARTIFACT_BLOB,
            "this finding artifact cannot be pulled",
        )
        .await?;
        // The client's 30s total timeout would cut a large blob short; an
        // artifact pull gets its own bound.
        let resp = self
            .send(
                self.client
                    .get(self.url(&format!("/api/findings/artifacts/{sha256}")))
                    .timeout(Duration::from_secs(30 * 60)),
            )
            .await?;
        let io = |e: std::io::Error| HostConnectorError::Invalid(e.to_string());
        let mut file = tokio::fs::File::create(dest).await.map_err(io)?;
        let mut stream = resp.bytes_stream();
        let mut total: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
            total += chunk.len() as u64;
            if total > max_bytes {
                return Err(HostConnectorError::Invalid(format!(
                    "artifact {sha256} exceeds its recorded {max_bytes} bytes"
                )));
            }
            file.write_all(&chunk).await.map_err(io)?;
        }
        file.flush().await.map_err(io)?;
        Ok(())
    }
```

`futures_util::StreamExt` and `std::time::Duration` are already imported in `http.rs`. If `reqwest_middleware::RequestBuilder` has no `.timeout`, build the request with `reqwest::Client`'s builder the same way `new_with_timeout` does. The requirement is that this request is not bound by the 30 s default.

- [ ] **Step 6: Regenerate the fixture and run the tests.**

Run: `REGEN_FIXTURES=1 cargo test -p rupu-cp fixture_is_current && git diff apps/rupu-macos/Fixtures/host_info.json`. The only change should be `findings.artifact_blob` added to `features`.

Run: `cargo test -p rupu-cp`
Expected: PASS.

- [ ] **Step 7: Commit.**

```bash
git add crates/rupu-cp apps/rupu-macos/Fixtures/host_info.json
git commit -m "feat(cp): GET /api/findings/artifacts/:sha256 (findings.artifact_blob); HTTP pull_finding_artifact streams with a cap"
```

---

### Task 4: Tunnel artifact pull frames

**Files:**
- Modify: `crates/rupu-cp/src/node/protocol.rs` (three frames, one constant, `node_capabilities`)
- Modify: `crates/rupu-cp/src/node/registry.rs` (`NodeConn` pull routing)
- Modify: `crates/rupu-cp/src/node/server.rs` (read pump `:264-294`; capture `conn`)
- Modify: `crates/rupu-cp/src/host/tunnel.rs` (replace the Task 2 stub)
- Modify: `crates/rupu-cli/src/cmd/node.rs` (the exhaustive `match frame` at `:508-605`; a new `send_artifact_blob`)
- Modify: `crates/rupu-cp/Cargo.toml` and `crates/rupu-cli/Cargo.toml` (add `base64.workspace = true`)
- Test: `crates/rupu-cp/tests/node_tunnel.rs`, `crates/rupu-cli/src/cmd/node.rs` tests

**Interfaces:**
- Produces:
  - `Frame::ArtifactPull { req, sha256 }`, `Frame::ArtifactChunk { req, seq, data_b64 }`, `Frame::ArtifactPullDone { req, error }`
  - `pub const CAP_FINDINGS_ARTIFACT_PULL: &str = "findings.artifact_pull"`, and `pub const ARTIFACT_CHUNK_BYTES: usize = 1 << 20`
  - `pub enum PullMsg { Chunk { seq: u64, data: Vec<u8> }, Done(Option<String>) }`
  - `NodeConn::begin_pull(&self, req) -> mpsc::Receiver<PullMsg>`, `NodeConn::end_pull(&self, req)`, `NodeConn::route_pull(&self, req, msg)`

- [ ] **Step 1: Write the failing tests.**

In `crates/rupu-cp/tests/node_tunnel.rs`'s `tunnel_connector` module:
- Change `setup_with_capabilities` (from #675) to also return the `Arc<NodeConn>` that `register_with_hello` returns. It becomes `-> (TunnelHostConnector, mpsc::Receiver<Frame>, Arc<RunStore>, Arc<rupu_cp::node::NodeConn>)`, with `let node_conn = registry.register_with_hello(…);` and `(conn, rx, run_store, node_conn)`.
- Update its existing callers to bind `(conn, mut rx, _run_store, _node_conn)`.
- Then add these tests. They play the node side by answering the `ArtifactPull` on the connection's routing table:

```rust
    #[tokio::test]
    async fn pull_finding_artifact_reassembles_node_chunks() {
        let dir = tempdir().unwrap();
        let (conn, mut rx, _store, node_conn) = setup_with_capabilities(
            "node-pull-1",
            dir.path(),
            rupu_cp::node::protocol::node_capabilities(),
        );
        let sha = "ab".repeat(32);
        let dest = dir.path().join("pulled");
        let pull = {
            let sha = sha.clone();
            let dest = dest.clone();
            tokio::spawn(async move { conn.pull_finding_artifact(&sha, &dest, 6).await })
        };
        let req = match rx.recv().await.unwrap() {
            Frame::ArtifactPull { req, sha256 } => {
                assert_eq!(sha256, sha);
                req
            }
            other => panic!("expected ArtifactPull, got {other:?}"),
        };
        node_conn
            .route_pull(&req, rupu_cp::node::PullMsg::Chunk { seq: 0, data: b"abc".to_vec() })
            .await;
        node_conn
            .route_pull(&req, rupu_cp::node::PullMsg::Chunk { seq: 1, data: b"def".to_vec() })
            .await;
        node_conn
            .route_pull(&req, rupu_cp::node::PullMsg::Done(None))
            .await;
        pull.await.unwrap().unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"abcdef");
    }

    #[tokio::test]
    async fn an_out_of_order_chunk_fails_the_pull() {
        let dir = tempdir().unwrap();
        let (conn, mut rx, _store, node_conn) = setup_with_capabilities(
            "node-pull-2",
            dir.path(),
            rupu_cp::node::protocol::node_capabilities(),
        );
        let dest = dir.path().join("pulled");
        let pull = {
            let dest = dest.clone();
            tokio::spawn(async move {
                conn.pull_finding_artifact(&"ab".repeat(32), &dest, 6).await
            })
        };
        let Frame::ArtifactPull { req, .. } = rx.recv().await.unwrap() else {
            panic!("expected ArtifactPull")
        };
        node_conn
            .route_pull(&req, rupu_cp::node::PullMsg::Chunk { seq: 1, data: b"x".to_vec() })
            .await;
        assert!(matches!(
            pull.await.unwrap(),
            Err(HostConnectorError::Invalid(m)) if m.contains("out of order")
        ));
    }

    #[tokio::test]
    async fn pull_finding_artifact_refuses_a_node_without_the_capability() {
        let dir = tempdir().unwrap();
        let (conn, _rx, _store) = setup("node-pull-old", dir.path());
        let err = conn
            .pull_finding_artifact(&"ab".repeat(32), &dir.path().join("x"), 6)
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::Unsupported(_)), "{err:?}");
    }
```

In `crates/rupu-cli/src/cmd/node.rs` tests, add a `Vec`-backed sink with the node's sink error type, and a chunking test:

```rust
    /// Collects every message a node would send.
    #[derive(Default)]
    struct VecSink(Vec<Message>);

    impl futures_util::Sink<Message> for VecSink {
        type Error = tokio_tungstenite::tungstenite::Error;
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn start_send(mut self: std::pin::Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.0.push(item);
            Ok(())
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn artifact_blob_is_sent_as_ordered_chunks_then_done() {
        use base64::Engine as _;
        let global = tempfile::tempdir().unwrap();
        let sha = "cd".repeat(32);
        let p = crate::cmd::findings_helper::blob_path(global.path(), &sha).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let body = vec![7u8; rupu_cp::node::protocol::ARTIFACT_CHUNK_BYTES + 5];
        std::fs::write(&p, &body).unwrap();

        let mut sink = VecSink::default();
        send_artifact_blob(&mut sink, global.path(), "req1", &sha).await;
        let frames: Vec<Frame> = sink.0.iter().map(|m| parse_frame(m).unwrap()).collect();
        assert_eq!(frames.len(), 3, "{} frames", frames.len());
        let mut got = Vec::new();
        for (i, f) in frames[..2].iter().enumerate() {
            match f {
                Frame::ArtifactChunk { req, seq, data_b64 } => {
                    assert_eq!(req, "req1");
                    assert_eq!(*seq, i as u64);
                    got.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(data_b64)
                            .unwrap(),
                    );
                }
                other => panic!("expected a chunk, got {other:?}"),
            }
        }
        assert_eq!(got, body);
        assert_eq!(
            frames[2],
            Frame::ArtifactPullDone { req: "req1".into(), error: None }
        );
    }

    #[tokio::test]
    async fn a_missing_blob_is_a_done_frame_with_an_error() {
        let global = tempfile::tempdir().unwrap();
        let mut sink = VecSink::default();
        send_artifact_blob(&mut sink, global.path(), "req2", &"ef".repeat(32)).await;
        let frames: Vec<Frame> = sink.0.iter().map(|m| parse_frame(m).unwrap()).collect();
        assert!(matches!(
            &frames[..],
            [Frame::ArtifactPullDone { error: Some(e), .. }] if e.contains("not in this node's store")
        ));
    }
```

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-cp --test node_tunnel pull_finding_artifact && cargo test -p rupu-cli --lib artifact_blob_is_sent`
Expected: compile errors.

- [ ] **Step 3: Update the protocol.** In `protocol.rs`, add these to `Frame`:

```rust
    /// CP→node: stream the finding-artifact blob `sha256` from the node's
    /// store. Sent only to a node that advertised
    /// [`CAP_FINDINGS_ARTIFACT_PULL`] — an older node's frame parse is fatal.
    ArtifactPull { req: String, sha256: String },
    /// node→CP: one chunk of an [`Frame::ArtifactPull`] answer
    /// (base64, ≤ [`ARTIFACT_CHUNK_BYTES`] decoded), in `seq` order.
    ArtifactChunk { req: String, seq: u64, data_b64: String },
    /// node→CP: the pull ended; `error` set ⇒ it failed.
    ArtifactPullDone { req: String, error: Option<String> },
```

Then add the constants, and extend `node_capabilities()`:

```rust
/// `Hello.capabilities` entry: this node answers [`Frame::ArtifactPull`].
pub const CAP_FINDINGS_ARTIFACT_PULL: &str = "findings.artifact_pull";
/// Decoded bytes per [`Frame::ArtifactChunk`].
pub const ARTIFACT_CHUNK_BYTES: usize = 1 << 20;

pub fn node_capabilities() -> Vec<String> {
    vec![
        CAP_AGENT_FINDINGS_PROFILE.to_string(),
        CAP_FINDINGS_ARTIFACT_PULL.to_string(),
    ]
}
```

- [ ] **Step 4: Route pulls through `NodeConn`.** In `registry.rs`:

```rust
/// One message of an in-flight artifact pull, routed from the tunnel's read
/// pump to the connector waiting on it.
#[derive(Debug)]
pub enum PullMsg {
    Chunk { seq: u64, data: Vec<u8> },
    /// End of the pull; `Some` carries the node's error.
    Done(Option<String>),
}
```

- Add the field `pulls: Mutex<HashMap<String, tokio::sync::mpsc::Sender<PullMsg>>>` to `NodeConn`, initialised as `Mutex::new(HashMap::new())` in `NodeConn::new`.
- Add these methods:

```rust
    /// Register `req` and return the receiver its chunks arrive on.
    pub fn begin_pull(&self, req: &str) -> tokio::sync::mpsc::Receiver<PullMsg> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        self.pulls
            .lock()
            .expect("pulls lock poisoned")
            .insert(req.to_string(), tx);
        rx
    }

    /// Forget `req` (the waiting side finished or gave up).
    pub fn end_pull(&self, req: &str) {
        self.pulls.lock().expect("pulls lock poisoned").remove(req);
    }

    /// Deliver `msg` to `req`'s waiter; dropped if nobody is waiting.
    pub async fn route_pull(&self, req: &str, msg: PullMsg) {
        let tx = self
            .pulls
            .lock()
            .expect("pulls lock poisoned")
            .get(req)
            .cloned();
        if let Some(tx) = tx {
            let _ = tx.send(msg).await;
        }
    }
```

- Re-export `PullMsg` from `node/mod.rs` next to `NodeConn`.

- [ ] **Step 5: Route inbound frames on the CP.** In `server.rs`, right after `let conn = …register_with_hello(…)`, clone it for the read pump: `let conn_r = Arc::clone(&conn);`. Move `conn_r` into the read-pump future, then add these arms before the `other =>` arm:

```rust
                        Frame::ArtifactChunk { req, seq, data_b64 } => {
                            use base64::Engine as _;
                            let msg = match base64::engine::general_purpose::STANDARD
                                .decode(data_b64.as_bytes())
                            {
                                Ok(data) => crate::node::PullMsg::Chunk { seq, data },
                                Err(e) => crate::node::PullMsg::Done(Some(format!(
                                    "undecodable artifact chunk: {e}"
                                ))),
                            };
                            conn_r.route_pull(&req, msg).await;
                        }
                        Frame::ArtifactPullDone { req, error } => {
                            conn_r
                                .route_pull(&req, crate::node::PullMsg::Done(error))
                                .await;
                        }
```

- [ ] **Step 6: Pull from the tunnel.** Replace the Task 2 stub in `tunnel.rs`:

```rust
    async fn pull_finding_artifact(
        &self,
        sha256: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(), HostConnectorError> {
        use tokio::io::AsyncWriteExt;
        crate::host::connector::validate_sha256(sha256)?;
        let conn = self.live_conn()?;
        if !conn.supports(crate::node::protocol::CAP_FINDINGS_ARTIFACT_PULL) {
            return Err(HostConnectorError::Unsupported(format!(
                "node {} (rupu {}) cannot serve artifact pulls; upgrade rupu on that node",
                self.node_id,
                conn.rupu_version().unwrap_or("unknown version"),
            )));
        }
        let req = format!("pull_{}", Ulid::new());
        let mut rx = conn.begin_pull(&req);
        let node_id = self.node_id.clone();
        let result = async {
            conn.send(Frame::ArtifactPull {
                req: req.clone(),
                sha256: sha256.to_string(),
            })
            .await
            .map_err(|_| HostConnectorError::Unreachable(format!("node {node_id} disconnected")))?;
            let io = |e: std::io::Error| HostConnectorError::Invalid(e.to_string());
            let mut file = tokio::fs::File::create(dest).await.map_err(io)?;
            let (mut total, mut next_seq) = (0u64, 0u64);
            loop {
                let msg = tokio::time::timeout(std::time::Duration::from_secs(60), rx.recv())
                    .await
                    .map_err(|_| {
                        HostConnectorError::Unreachable(format!(
                            "node {node_id} stopped sending artifact {sha256}"
                        ))
                    })?;
                match msg {
                    None => {
                        return Err(HostConnectorError::Unreachable(format!(
                            "node {node_id} disconnected during the pull"
                        )))
                    }
                    Some(crate::node::PullMsg::Chunk { seq, data }) => {
                        if seq != next_seq {
                            return Err(HostConnectorError::Invalid(format!(
                                "artifact chunk {seq} arrived out of order (expected {next_seq})"
                            )));
                        }
                        next_seq += 1;
                        total += data.len() as u64;
                        if total > max_bytes {
                            return Err(HostConnectorError::Invalid(format!(
                                "artifact {sha256} exceeds its recorded {max_bytes} bytes"
                            )));
                        }
                        file.write_all(&data).await.map_err(io)?;
                    }
                    Some(crate::node::PullMsg::Done(None)) => {
                        file.flush().await.map_err(io)?;
                        return Ok(());
                    }
                    Some(crate::node::PullMsg::Done(Some(e))) => {
                        return Err(HostConnectorError::NotFound(format!("node {node_id}: {e}")))
                    }
                }
            }
        }
        .await;
        conn.end_pull(&req);
        result
    }
```

- [ ] **Step 7: Answer on the node.** In `crates/rupu-cli/src/cmd/node.rs`, add:

```rust
/// Answer an `ArtifactPull`: the blob as ordered base64 chunks, then
/// `ArtifactPullDone` (with the error, if the blob could not be read).
async fn send_artifact_blob<S>(sink: &mut S, global: &Path, req: &str, sha256: &str)
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    use base64::Engine as _;
    use rupu_cp::node::protocol::ARTIFACT_CHUNK_BYTES;
    use tokio::io::AsyncReadExt;
    let error = async {
        let path = crate::cmd::findings_helper::blob_path(global, sha256).map_err(|e| e.to_string())?;
        let mut f = tokio::fs::File::open(&path)
            .await
            .map_err(|e| format!("artifact {sha256} is not in this node's store: {e}"))?;
        let mut buf = vec![0u8; ARTIFACT_CHUNK_BYTES];
        let mut seq = 0u64;
        loop {
            // Fill a whole chunk (a read may return less).
            let mut n = 0;
            while n < buf.len() {
                let got = f.read(&mut buf[n..]).await.map_err(|e| e.to_string())?;
                if got == 0 {
                    break;
                }
                n += got;
            }
            if n == 0 {
                return Ok::<(), String>(());
            }
            let data_b64 = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
            send_frame(sink, &Frame::ArtifactChunk { req: req.to_string(), seq, data_b64 }).await;
            seq += 1;
            if n < buf.len() {
                return Ok(());
            }
        }
    }
    .await
    .err();
    send_frame(sink, &Frame::ArtifactPullDone { req: req.to_string(), error }).await;
}
```

In the exhaustive `match frame` in `connect_and_run`, add:

```rust
            Frame::ArtifactPull { req, sha256 } => {
                send_artifact_blob(&mut sink, &global, &req, &sha256).await;
            }
```

Also add `| Frame::ArtifactChunk { .. } | Frame::ArtifactPullDone { .. }` to the "unexpected server-sent frame" arm. `global` is in scope, bound after `Welcome` (`let global = crate::paths::global_dir()?;`). If it's declared after the loop, move that `let` above the loop.

- [ ] **Step 8: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-cp --test node_tunnel && cargo test -p rupu-cp --lib node:: && cargo test -p rupu-cli --lib cmd::node`
Expected: PASS.

- [ ] **Step 9: Commit.**

```bash
git add crates/rupu-cp crates/rupu-cli
git commit -m "feat(tunnel): ArtifactPull/ArtifactChunk/ArtifactPullDone frames; node streams blobs; findings.artifact_pull capability"
```

---

### Task 5: Bucket hosts upload referenced blobs when a run finishes; the connector downloads them

**Files:**
- Modify: `crates/rupu-cp/src/host/bucket/mod.rs` (trait + `key_artifact`), `object_store_bucket.rs` (impl + test), `connector.rs` (replace the stub; `FailingBucket` double)
- Modify: `crates/rupu-cli/src/cmd/node.rs` (`referenced_artifacts`, `upload_referenced_artifacts`; call before `put_finished`)

**Interfaces:**
- Consumes: Plan A's `StreamLine` and `STREAM_FILE`; Task 1's `blob_path` and `is_sha256_hex`.
- Produces:
  - `Bucket::artifact_exists(&self, sha256) -> Result<bool, BucketError>`
  - `Bucket::put_artifact_file(&self, sha256, src: &Path) -> Result<(), BucketError>`
  - `Bucket::get_artifact_to_file(&self, sha256, dest: &Path, max_bytes: u64) -> Result<(), BucketError>`
  - Key layout: `artifacts/<sha256>`

- [ ] **Step 1: Write the failing tests.** Add this to the `object_store_bucket.rs` tests:

```rust
    #[tokio::test]
    async fn artifact_put_exists_get_roundtrip_and_cap() {
        let b = mem_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("blob");
        std::fs::write(&src, b"artifact!").unwrap();
        let sha = "ab".repeat(32);
        assert!(!b.artifact_exists(&sha).await.unwrap());
        b.put_artifact_file(&sha, &src).await.unwrap();
        assert!(b.artifact_exists(&sha).await.unwrap());
        let dest = tmp.path().join("out");
        b.get_artifact_to_file(&sha, &dest, 9).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"artifact!");
        assert!(b.get_artifact_to_file(&sha, &dest, 8).await.is_err());
        assert!(matches!(
            b.get_artifact_to_file(&"cd".repeat(32), &dest, 9).await,
            Err(BucketError::NotFound(_))
        ));
    }
```

Add this to the `crates/rupu-cli/src/cmd/node.rs` tests:

```rust
    #[test]
    fn referenced_artifacts_lists_only_copied_blobs_from_findings() {
        use rupu_coverage::report::{ArtifactKind, ArtifactRef, ArtifactStorage, FindingReport};
        let tmp = tempfile::tempdir().unwrap();
        let stream = tmp.path().join(rupu_coverage::STREAM_FILE);
        let copied = "ab".repeat(32);
        let external = "cd".repeat(32);
        let mut report: FindingReport = serde_json::from_str(include_str!(
            "../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap();
        let art = |sha: &str, stored| ArtifactRef {
            path: "poc/x".into(),
            sha256: sha.into(),
            size: 1,
            kind: Some(ArtifactKind::Binary),
            stored: Some(stored),
            host: None,
        };
        report.artifacts = vec![
            art(&copied, ArtifactStorage::Copied),
            art(&external, ArtifactStorage::External),
        ];
        let record = rupu_coverage::FindingRecord {
            id: "f1".into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: rupu_coverage::FindingScope::File,
            summary: "s".into(),
            severity: rupu_coverage::Severity::High,
            concern_id: None,
            evidence: rupu_coverage::FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: rupu_coverage::Attribution {
                run_id: "r".into(),
                model: "m".into(),
                surface: rupu_coverage::Surface::Agent,
            },
            declared_at: chrono::Utc::now(),
            profile: rupu_coverage::FindingProfile::Full,
            report: Some(report),
        };
        let lines = [
            rupu_coverage::StreamLine::Begin { v: 1, run_id: "r".into() },
            rupu_coverage::StreamLine::Findings { scope_name: "sec".into(), record },
        ];
        let body: String = lines
            .iter()
            .map(|l| serde_json::to_string(l).unwrap() + "\n")
            .collect();
        std::fs::write(&stream, body).unwrap();
        assert_eq!(
            referenced_artifacts(&stream),
            std::collections::BTreeSet::from([copied])
        );
        assert!(referenced_artifacts(&tmp.path().join("absent")).is_empty());
    }
```

The `include_str!` path is relative to `crates/rupu-cli/src/cmd/node.rs`.

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-cp --lib artifact_put_exists && cargo test -p rupu-cli --lib referenced_artifacts`
Expected: compile errors.

- [ ] **Step 3: Extend the trait and key layout.** In `bucket/mod.rs`, add `//! artifacts/<sha256>            — finding-artifact blobs a worker uploaded at run end` to the key-layout doc, and add these to the trait:

```rust
    /// Whether `artifacts/<sha256>` exists.
    async fn artifact_exists(&self, sha256: &str) -> Result<bool, BucketError>;

    /// Upload the file at `src` to `artifacts/<sha256>` (streamed multipart).
    async fn put_artifact_file(&self, sha256: &str, src: &std::path::Path)
        -> Result<(), BucketError>;

    /// Stream `artifacts/<sha256>` into `dest`, failing past `max_bytes`.
    /// `NotFound` when no worker uploaded it.
    async fn get_artifact_to_file(
        &self,
        sha256: &str,
        dest: &std::path::Path,
        max_bytes: u64,
    ) -> Result<(), BucketError>;
```

Add the key helper:

```rust
/// `artifacts/<sha256>`
pub(crate) fn key_artifact(sha256: &str) -> String {
    format!("artifacts/{sha256}")
}
```

- [ ] **Step 4: Implement it for the object store.** In `object_store_bucket.rs`, import `key_artifact`, then:

```rust
    async fn artifact_exists(&self, sha256: &str) -> Result<bool, BucketError> {
        self.exists(&self.path(&key_artifact(sha256))).await
    }

    async fn put_artifact_file(
        &self,
        sha256: &str,
        src: &std::path::Path,
    ) -> Result<(), BucketError> {
        use tokio::io::AsyncReadExt;
        let io = |e: std::io::Error| BucketError::Io(e.to_string());
        let path = self.path(&key_artifact(sha256));
        let upload = self
            .store
            .put_multipart(&path)
            .await
            .map_err(|e| BucketError::Io(e.to_string()))?;
        let mut w = object_store::WriteMultipart::new(upload);
        let mut f = tokio::fs::File::open(src).await.map_err(io)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf).await.map_err(io)?;
            if n == 0 {
                break;
            }
            w.write(&buf[..n]);
        }
        w.finish().await.map_err(|e| BucketError::Io(e.to_string()))?;
        Ok(())
    }

    async fn get_artifact_to_file(
        &self,
        sha256: &str,
        dest: &std::path::Path,
        max_bytes: u64,
    ) -> Result<(), BucketError> {
        use futures_util::StreamExt as _;
        use tokio::io::AsyncWriteExt;
        let io = |e: std::io::Error| BucketError::Io(e.to_string());
        let path = self.path(&key_artifact(sha256));
        let got = match self.store.get(&path).await {
            Ok(g) => g,
            Err(object_store::Error::NotFound { .. }) => {
                return Err(BucketError::NotFound(key_artifact(sha256)))
            }
            Err(e) => return Err(BucketError::Io(e.to_string())),
        };
        let mut stream = got.into_stream();
        let mut file = tokio::fs::File::create(dest).await.map_err(io)?;
        let mut total: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| BucketError::Io(e.to_string()))?;
            total += chunk.len() as u64;
            if total > max_bytes {
                return Err(BucketError::Io(format!(
                    "artifact {sha256} exceeds its recorded {max_bytes} bytes"
                )));
            }
            file.write_all(&chunk).await.map_err(io)?;
        }
        file.flush().await.map_err(io)?;
        Ok(())
    }
```

If `put_multipart` or `get` live on `ObjectStoreExt` rather than `ObjectStore` in object_store 0.14, both traits are already imported in this file.

- [ ] **Step 5: Pull in the bucket connector.** Replace the Task 2 stub in `bucket/connector.rs`:

```rust
    async fn pull_finding_artifact(
        &self,
        sha256: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(), HostConnectorError> {
        crate::host::connector::validate_sha256(sha256)?;
        self.bucket
            .get_artifact_to_file(sha256, dest, max_bytes)
            .await
            .map_err(|e| match e {
                BucketError::NotFound(_) => HostConnectorError::NotFound(format!(
                    "artifact {sha256} was not uploaded to bucket host {} (its worker may \
                     predate artifact upload, or the upload failed)",
                    self.host_id
                )),
                other => bucket_err_to_unreachable(other),
            })
    }
```

Add the three trait methods to `FailingBucket` with `unimplemented!("FailingBucket::…")` bodies.

- [ ] **Step 6: The worker uploads when the run finishes.** In `crates/rupu-cli/src/cmd/node.rs`, add:

```rust
/// sha256s of `stored: copied` artifacts referenced by findings in a run's
/// coverage stream.
fn referenced_artifacts(stream: &Path) -> std::collections::BTreeSet<String> {
    let Ok(text) = std::fs::read_to_string(stream) else {
        return Default::default();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<rupu_coverage::StreamLine>(l).ok())
        .filter_map(|l| match l {
            rupu_coverage::StreamLine::Findings { record, .. } => record.report,
            _ => None,
        })
        .flat_map(|r| r.artifacts)
        .filter(|a| {
            a.stored == Some(rupu_coverage::report::ArtifactStorage::Copied)
                && rupu_coverage::report::is_sha256_hex(&a.sha256)
        })
        .map(|a| a.sha256)
        .collect()
}

/// Upload every blob this run's findings reference to `artifacts/<sha256>`,
/// so the coordinator can pull them later without this worker online (spec
/// 2026-09-30-rupu-remote-findings-transport-design.md §B1). Best-effort: a
/// blob that fails to upload is reported unavailable by the coordinator.
async fn upload_referenced_artifacts(bucket: &ObjectStoreBucket, global: &Path, run_dir: &Path) {
    for sha in referenced_artifacts(&run_dir.join(rupu_coverage::STREAM_FILE)) {
        match bucket.artifact_exists(&sha).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                warn!(sha = %sha, error = %e, "node pull: artifact existence check failed");
                continue;
            }
        }
        let Ok(src) = crate::cmd::findings_helper::blob_path(global, &sha) else {
            continue;
        };
        if let Err(e) = bucket.put_artifact_file(&sha, &src).await {
            warn!(sha = %sha, error = %e, "node pull: artifact upload failed; the coordinator will report it unavailable");
        }
    }
}
```

In `pull()`'s terminal block, call `upload_referenced_artifacts(&bucket, &global, &run_dir).await;` right before `bucket.put_finished(rid, &status)`, after Plan A's final coverage drain. `global` is already bound in `pull()`.

- [ ] **Step 7: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-cp --lib host::bucket && cargo test -p rupu-cli --lib cmd::node && cargo test -p rupu-cp --test bucket_e2e`
Expected: PASS.

- [ ] **Step 8: Commit.**

```bash
git add crates/rupu-cp/src/host/bucket crates/rupu-cli/src/cmd/node.rs
git commit -m "feat(bucket): worker uploads referenced artifact blobs at run end; connector streams them back"
```

---

### Task 6: `GET /api/findings/:id/artifacts/:sha256` on the coordinator

**Files:**
- Modify: `crates/rupu-cp/src/api/findings.rs` (route, handler, helpers)
- Modify: `crates/rupu-cp/Cargo.toml` (only if `ulid` isn't already a dependency; it's used in `host/tunnel.rs`, so it should be)
- Test: `crates/rupu-cp/tests/finding_artifacts.rs` (new)

**Interfaces:**
- Consumes: `collect_all_findings` (`findings.rs:181`), `store_for` (`:155`), `crate::api::runs::resolve_host`, `pull_finding_artifact` (Tasks 2–5), `blob_file_response` (Task 3), `rupu_coverage::report::{is_sha256_hex, sha256_file, ArtifactStore, ArtifactRef, ArtifactKind, ArtifactStorage}`.
- Produces: `GET /api/findings/:id/artifacts/:sha256`:

  | Case | Response |
  |---|---|
  | Malformed sha | 400 |
  | Unknown finding, or sha not on it | 404 `{"error": …}` |
  | Blob available | 200 streamed body |
  | Blob can't be got | 404 `{"unavailable": "<reason>"}` |

- [ ] **Step 1: Write the failing tests.** Create `crates/rupu-cp/tests/finding_artifacts.rs`:

```rust
//! The coordinator's artifact endpoint: store hits, remote pulls (verified,
//! one per sha), and honest `unavailable` reasons (spec
//! 2026-09-30-rupu-remote-findings-transport-design.md §B2).
#![allow(clippy::disallowed_methods)] // throwaway in-process test client

use rupu_coverage::report::{ArtifactKind, ArtifactRef, ArtifactStorage, ArtifactStore, FindingReport};
use rupu_coverage::{CoveragePaths, FindingRecord};
use std::path::Path;

fn sha_of(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex_lower(&sha2::Sha256::digest(bytes))
}

fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn register_workspace(global: &Path, id: &str, root: &Path) {
    std::fs::create_dir_all(global.join("workspaces")).unwrap();
    std::fs::write(
        global.join("workspaces").join(format!("{id}.toml")),
        format!(
            "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
            root.display()
        ),
    )
    .unwrap();
}

fn write_finding(ws: &Path, artifact: ArtifactRef) -> String {
    write_finding_with_id(ws, "find_ART1", artifact)
}

fn write_finding_with_id(ws: &Path, id: &str, artifact: ArtifactRef) -> String {
    let mut report: FindingReport = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
    .unwrap();
    report.artifacts = vec![artifact];
    let rec = FindingRecord {
        id: id.into(),
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: rupu_coverage::FindingScope::File,
        summary: "s".into(),
        severity: rupu_coverage::Severity::High,
        concern_id: None,
        evidence: rupu_coverage::FindingEvidence {
            code_excerpt: None,
            rationale: "r".into(),
            references: vec![],
        },
        declared_by: rupu_coverage::Attribution {
            run_id: "run_A".into(),
            model: "m".into(),
            surface: rupu_coverage::Surface::Agent,
        },
        declared_at: chrono::Utc::now(),
        profile: rupu_coverage::FindingProfile::Full,
        report: Some(report),
    };
    let paths = CoveragePaths::new(ws, "t1");
    paths.ensure_dir().unwrap();
    rupu_coverage::append_record(&paths, rupu_coverage::Ledger::Findings, &rec).unwrap();
    rec.id
}

fn store_blob(global: &Path, sha: &str, body: &[u8]) {
    let p = ArtifactStore::new(global.join("findings").join("artifacts")).blob_path(sha);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn serve(state: rupu_cp::state::AppState) -> std::net::SocketAddr {
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn state(global: &Path) -> rupu_cp::state::AppState {
    rupu_cp::state::AppState::new(global.to_path_buf(), rupu_config::PricingConfig::default())
}

fn artifact(sha: &str, size: u64, kind: ArtifactKind, stored: ArtifactStorage, host: Option<String>) -> ArtifactRef {
    ArtifactRef {
        path: "poc/exploit.py".into(),
        sha256: sha.into(),
        size,
        kind: Some(kind),
        stored: Some(stored),
        host,
    }
}

#[tokio::test]
async fn a_remote_artifact_is_pulled_verified_stored_then_served() {
    // The remote: a real CP whose store holds the blob.
    let remote = tempfile::tempdir().unwrap();
    let body = b"print('poc')\n";
    let sha = sha_of(body);
    store_blob(remote.path(), &sha, body);
    let remote_addr = serve(state(remote.path())).await;

    // The coordinator: a finding pointing at the remote.
    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    register_workspace(coord.path(), "ws_a", ws.path());
    let st = state(coord.path());
    let host = st
        .hosts
        .add_host("remote", &format!("http://{remote_addr}"), None)
        .unwrap();
    let id = write_finding(
        ws.path(),
        artifact(&sha, body.len() as u64, ArtifactKind::Text, ArtifactStorage::External, Some(host.id.clone())),
    );
    let addr = serve(st).await;

    let resp = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{sha}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()["content-type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    assert_eq!(resp.bytes().await.unwrap().as_ref(), body);
    // Now in the coordinator's own store.
    let local = ArtifactStore::new(coord.path().join("findings").join("artifacts")).blob_path(&sha);
    assert_eq!(std::fs::read(local).unwrap(), body);
}

/// A mock remote that advertises the blob feature; each test mocks the blob.
async fn mock_remote() -> httpmock::MockServer {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "9.9.9", "features": ["findings.artifact_blob"]
        }));
    });
    server
}

#[tokio::test]
async fn concurrent_first_views_share_one_pull() {
    static BODY: &[u8] = b"binary\x00poc";
    let sha = sha_of(BODY);
    let server = mock_remote().await;
    let blob = server.mock(|when, then| {
        when.method("GET").path(format!("/api/findings/artifacts/{sha}"));
        then.status(200).body(BODY);
    });

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    register_workspace(coord.path(), "ws_a", ws.path());
    let st = state(coord.path());
    let host = st.hosts.add_host("m", &server.base_url(), None).unwrap();
    let id = write_finding(
        ws.path(),
        artifact(&sha, BODY.len() as u64, ArtifactKind::Binary, ArtifactStorage::External, Some(host.id.clone())),
    );
    let addr = serve(st).await;
    let url = format!("http://{addr}/api/findings/{id}/artifacts/{sha}");
    let (a, b) = tokio::join!(reqwest::get(&url), reqwest::get(&url));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.status(), 200);
    assert_eq!(b.status(), 200);
    assert!(a.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment; filename=\"exploit.py\""));
    assert_eq!(a.bytes().await.unwrap().as_ref(), BODY);
    blob.assert_hits(1);
}

#[tokio::test]
async fn a_hash_mismatch_is_unavailable_and_stores_nothing() {
    // The finding recorded the sha of "expected!", but the host answers with
    // different bytes of the same length.
    let recorded = sha_of(b"expected!");
    let server = mock_remote().await;
    server.mock(|when, then| {
        when.method("GET").path(format!("/api/findings/artifacts/{recorded}"));
        then.status(200).body("tampered!");
    });

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    register_workspace(coord.path(), "ws_a", ws.path());
    let st = state(coord.path());
    let host = st.hosts.add_host("m", &server.base_url(), None).unwrap();
    let id = write_finding_with_id(
        ws.path(),
        "find_TAMPERED",
        artifact(&recorded, 9, ArtifactKind::Binary, ArtifactStorage::External, Some(host.id.clone())),
    );
    let addr = serve(st).await;
    let r = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{recorded}"))
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let v: serde_json::Value = r.json().await.unwrap();
    assert!(v["unavailable"].as_str().unwrap().contains("mismatch"), "{v}");
    let stored = ArtifactStore::new(coord.path().join("findings").join("artifacts"))
        .blob_path(&recorded);
    assert!(!stored.exists(), "a mismatched blob must never enter the store");
    let leftovers: Vec<_> = std::fs::read_dir(stored.parent().unwrap())
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "temp file removed: {leftovers:?}");
}

#[tokio::test]
async fn unknown_sha_bad_sha_and_unavailable_reasons() {
    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    register_workspace(coord.path(), "ws_a", ws.path());
    let sha = sha_of(b"gone");
    // An external artifact with no host whose workspace file does not exist.
    let id = write_finding(
        ws.path(),
        artifact(&sha, 4, ArtifactKind::Binary, ArtifactStorage::External, None),
    );
    let addr = serve(state(coord.path())).await;

    let r = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/not-a-sha"))
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let other = sha_of(b"other");
    let r = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{other}"))
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let v: serde_json::Value = r.json().await.unwrap();
    assert!(v.get("error").is_some(), "not on the finding is a plain 404: {v}");

    let r = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{sha}"))
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let v: serde_json::Value = r.json().await.unwrap();
    assert!(v["unavailable"].as_str().unwrap().contains("no longer exists"), "{v}");

    // Now create the workspace file with the recorded content: served.
    std::fs::create_dir_all(ws.path().join("poc")).unwrap();
    std::fs::write(ws.path().join("poc/exploit.py"), b"gone").unwrap();
    let r = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{sha}"))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // Changed since it was recorded: refused.
    std::fs::write(ws.path().join("poc/exploit.py"), b"edited").unwrap();
    let r = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{sha}"))
        .await
        .unwrap();
    let v: serde_json::Value = r.json().await.unwrap();
    assert!(v["unavailable"].as_str().unwrap().contains("changed"), "{v}");
}
```

**Dependencies.** `sha2`, `chrono`, `httpmock`, `reqwest` and `tempfile` must be dev-dependencies of rupu-cp. The first four are already used by its tests; add `sha2 = { workspace = true }` to `[dev-dependencies]` if it's missing.

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-cp --test finding_artifacts`
Expected: FAIL. The route doesn't exist, so it hits the JSON 404.

- [ ] **Step 3: Implement the endpoint.** In `api/findings.rs`, add `.route("/api/findings/:id/artifacts/:sha256", get(get_finding_artifact))` to `routes()`, then:

```rust
/// One in-flight pull per sha: a second first-view waits on the first
/// rather than pulling the same blob again.
fn pull_lock(sha256: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .expect("pull locks poisoned")
        .entry(sha256.to_string())
        .or_default()
        .clone()
}

fn unavailable(reason: impl Into<String>) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "unavailable": reason.into() })),
    )
        .into_response()
}

/// Serve `path` with the headers its `kind` calls for: text as escaped plain
/// text, everything else as an attachment. Never rendered as HTML.
async fn serve_artifact(path: &std::path::Path, a: &ArtifactRef) -> axum::response::Response {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => return unavailable(format!("stored blob is unreadable: {e}")),
    };
    if a.kind == Some(rupu_coverage::report::ArtifactKind::Text) {
        return blob_file_response(file, "text/plain; charset=utf-8", None, true);
    }
    let name = std::path::Path::new(&a.path)
        .file_name()
        .map(|n| n.to_string_lossy().replace(['"', '\\'], ""))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| a.sha256.clone());
    blob_file_response(
        file,
        "application/octet-stream",
        Some(format!("attachment; filename=\"{name}\"")),
        false,
    )
}

/// (size, sha256) of a file, off the async runtime.
async fn size_and_sha(path: std::path::PathBuf) -> Result<(u64, String), String> {
    tokio::task::spawn_blocking(move || {
        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        let sha = rupu_coverage::report::sha256_file(&path).map_err(|e| e.to_string())?;
        Ok((size, sha))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Pull `a` from `host` into the store at `blob` (verified), or say why not.
async fn pull_into_store(
    s: &AppState,
    host: &str,
    a: &ArtifactRef,
    blob: &std::path::Path,
) -> Result<(), String> {
    let conn = crate::api::runs::resolve_host(s, host)
        .map_err(|e| format!("host {host} is not registered with this control plane: {}", e.1))?;
    let dir = blob.parent().ok_or("store path has no parent")?;
    tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".pull-{}-{}", a.sha256, ulid::Ulid::new()));
    let pulled = conn.pull_finding_artifact(&a.sha256, &tmp, a.size).await;
    if let Err(e) = pulled {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!("host {host}: {e}"));
    }
    match size_and_sha(tmp.clone()).await {
        Ok((size, sha)) if size == a.size && sha == a.sha256 => {}
        Ok((size, sha)) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(format!(
                "hash mismatch: host {host} returned {size} bytes hashing to {sha}, \
                 but the finding recorded {} bytes hashing to {}",
                a.size, a.sha256
            ));
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(format!("could not verify the pulled blob: {e}"));
        }
    }
    tokio::fs::rename(&tmp, blob).await.map_err(|e| e.to_string())
}

/// `GET /api/findings/:id/artifacts/:sha256` — a finding's artifact bytes:
/// from this CP's store, pulled from the recording host on first view, or
/// (local over-cap files) from the workspace while it is unchanged (spec
/// 2026-09-30-rupu-remote-findings-transport-design.md §B2).
async fn get_finding_artifact(
    State(s): State<AppState>,
    Path((id, sha256)): Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !rupu_coverage::report::is_sha256_hex(&sha256) {
        return ApiError::bad_request("sha256 must be 64 lowercase hex characters")
            .into_response();
    }
    let global = s.global_dir.clone();
    let wanted = id.clone();
    let found = tokio::task::spawn_blocking(move || {
        collect_all_findings(&global)
            .into_iter()
            .find(|f| f.record.id == wanted)
    })
    .await
    .ok()
    .flatten();
    let Some(f) = found else {
        return ApiError::not_found(format!("unknown finding {id}")).into_response();
    };
    let Some(a) = f
        .record
        .report
        .as_ref()
        .and_then(|r| r.artifacts.iter().find(|a| a.sha256 == sha256))
        .cloned()
    else {
        return ApiError::not_found(format!("finding {id} has no artifact {sha256}"))
            .into_response();
    };

    let blob = rupu_coverage::report::ArtifactStore::new(
        s.global_dir.join("findings").join("artifacts"),
    )
    .blob_path(&sha256);
    if blob.is_file() {
        return serve_artifact(&blob, &a).await;
    }

    match (a.stored, a.host.as_deref()) {
        (_, Some(host)) => {
            let lock = pull_lock(&sha256);
            let _held = lock.lock().await;
            // A concurrent first view may have pulled it while we waited.
            if !blob.is_file() {
                if let Err(reason) = pull_into_store(&s, host, &a, &blob).await {
                    return unavailable(reason);
                }
            }
            serve_artifact(&blob, &a).await
        }
        (Some(rupu_coverage::report::ArtifactStorage::External), None) => {
            serve_local_external(&s, &f.ws_id, &a).await
        }
        _ => unavailable("the blob is not in this control plane's artifact store"),
    }
}

/// A local over-cap artifact: the workspace file, while it still hashes to
/// the recorded sha.
async fn serve_local_external(s: &AppState, ws_id: &str, a: &ArtifactRef) -> axum::response::Response {
    let rel = std::path::Path::new(&a.path);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return unavailable("the recorded artifact path is not workspace-relative");
    }
    let Some(ws) = store_for(&s.global_dir)
        .list()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.id == ws_id)
    else {
        return unavailable(format!("workspace {ws_id} is no longer registered"));
    };
    let path = std::path::Path::new(&ws.path).join(rel);
    if !path.is_file() {
        return unavailable(format!(
            "the workspace file {} no longer exists",
            a.path
        ));
    }
    match size_and_sha(path.clone()).await {
        Ok((size, sha)) if size == a.size && sha == a.sha256 => serve_artifact(&path, a).await,
        Ok(_) => unavailable(format!(
            "the workspace file {} changed since it was recorded",
            a.path
        )),
        Err(e) => unavailable(format!("could not read {}: {e}", a.path)),
    }
}
```

Imports:
- Add `use rupu_coverage::report::ArtifactRef;` and `use std::sync::Arc;` to `findings.rs`, and `Path` from `axum::extract` if it's missing.
- `ApiError`'s fields are public (`pub struct ApiError(pub StatusCode, pub String)`), so `e.1` is the message.
- `store_for(…).list()` returns records with `id` and `path`, the same ones `collect_all_findings` iterates.

- [ ] **Step 4: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-cp --test finding_artifacts && cargo test -p rupu-cp`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp
git commit -m "feat(cp): GET /api/findings/:id/artifacts/:sha256 — store hit, verified pull on first view, local external, unavailable reasons"
```

---

### Task 7: Docs, and the full gate

**Files:**
- Modify: `docs/coverage.md` (the remote-unit artifacts bullet from Plan A, and a new "Downloading artifacts" paragraph)
- Modify: `CLAUDE.md` (the `rupu-cp` bullet)

- [ ] **Step 1: `docs/coverage.md`.** Replace the last sentence of Plan A's remote-unit bullet ("…their blobs stay in the host's store") with "…their blobs stay in the host's store and are pulled on first download." Then add this after the Artifacts list:

```markdown
**Downloading artifacts.** `GET /api/findings/:id/artifacts/:sha256` returns
an artifact's bytes. The sha must be one of that finding's artifacts. A blob
already in this CP's store is served directly; an `external` artifact with a
`host` is pulled from that host on first download (SSH runs the hidden
`rupu __findings artifact`, HTTP hosts serve
`/api/findings/artifacts/:sha256`, tunnel nodes stream it over the tunnel,
bucket workers uploaded it when the run finished), verified against the
recorded size and sha256, and stored here for later downloads. A local
over-cap file is served from the workspace while it still hashes to the
recorded sha. Text artifacts are served as plain text; everything else as an
attachment. When the bytes cannot be had, the response is `404` with
`{"unavailable": "<reason>"}` (host unreachable, node offline, not in the
host's store, host too old, hash mismatch, workspace file gone or changed).
```

- [ ] **Step 2: `CLAUDE.md`.** Append this to the `rupu-cp` bullet:

"`HostConnector::pull_finding_artifact` (required) streams a host-store blob to a temp file capped at the recorded size: local copies, SSH runs `rupu __findings artifact <sha>` via `RemoteExec::run_to_file`, HTTP streams `/api/findings/artifacts/:sha256` (`findings.artifact_blob` feature), tunnel uses `ArtifactPull`/`ArtifactChunk`/`ArtifactPullDone` (`findings.artifact_pull` capability), and bucket reads `artifacts/<sha256>`, which the worker uploads at run end. `GET /api/findings/:id/artifacts/:sha256` pulls on first view, one pull per sha, verified by size and sha256."

Add Plan B to "Read first".

- [ ] **Step 3: Full gate.**

```bash
cargo test -p rupu-coverage
cargo test -p rupu-workspace
cargo test -p rupu-orchestrator
cargo test -p rupu-cp
cargo test -p rupu-cli
cargo clippy -p rupu-coverage -p rupu-workspace -p rupu-orchestrator -p rupu-cp -p rupu-cli --all-targets -- -D warnings -A clippy::question_mark
```

Expected: all PASS, and clippy is clean.

- [ ] **Step 4: Commit.**

```bash
git add docs/coverage.md CLAUDE.md
git commit -m "docs: finding artifact downloads — pulled from the recording host on first view"
```

## Execution notes (2026-10-01)

How execution departed from the task text above, and why. The task text is left as written.

- **Stacked on Plan A.** Plan A (the coverage stream) was an unmerged local branch cut before #677, which added the artifact endpoint this plan extends. Plan B was first built on a replay of Plan A's commits onto fresh `main`, then rebased onto Plan A's own branch (`claude/remote-findings-transport`, tip `7967237d`), with one adapt commit for Plan A's later fixes. This work is therefore stacked on Plan A, and so is its PR: it lands after Plan A, or is retargeted to `main` once Plan A merges.
- **Task 6 modifies the shipped endpoint instead of adding one.** `main` already had `GET /api/findings/:id/artifacts/:sha256` (#677, hardened in #683). Task 6 changed only its `external` + `host` arm. The 400/404 lookups, the copied-blob arm, the single-handle local-external arm (404 gone / 409 changed with the standard `{"error"}` body) and all existing tests are kept. The spec's "local external -> 404 unavailable" wording is superseded by that shipped behavior. `{"unavailable": reason}` is for remote artifacts only.
- **One response builder, always `nosniff` + `Content-Security-Policy: sandbox`.** Both the coordinator endpoint and the host blob endpoint use the header code lifted from `main`'s `get_artifact`: text is `text/plain; charset=utf-8` inline, everything else an attachment, 64 KiB streamed chunks. The plan's `blob_file_response` signature was dropped because it made `nosniff` optional and omitted the CSP.
- **Routes were added, not replaced.** Task 3's `routes()` snippet would have deleted five shipped finding routes; the host blob route is added alongside them.
- **Test placement and mechanical changes follow #699 and #691.** rupu-cp integration tests live in `crates/rupu-cp/tests/it/<file>.rs` with a `mod` line in `tests/it/main.rs`. Task 1 adds only `is_sha256_hex` (and `blob_path_checked` now uses it, so there is one predicate). The `tokio-util` dependency change and the macOS fixture regeneration were skipped (already present; fixtures are frozen). Tests hash with the existing `sha256_reader`/`sha256_file`, and literal `FindingRecord`/`Attribution` values carry `main`'s `codename`/`agent` fields.
- **The Task 2 test-double list became "every `impl HostConnector`".** Line numbers had moved; the compiler finds them.
- **The bucket worker uploads inside `finish_bucket_run`, on every outcome.** The upload runs after the final coverage drain and before `put_finished`, so succeeded, failed and cancelled runs all upload what their findings reference: uploaded before `finished`; a blob that still fails after 3 passes is logged and released (the coordinator reports it unavailable); coverage lines are held until they land. The `RecordingBucket` test double answers honestly from an in-memory map.
- **The store's private-root rule applies to pulled blobs.** The coordinator creates the `<aa>` directory through a store helper so a missing root is created `0700`, not `0755` by a bare `create_dir_all`.
- **The "one pull per sha" map holds in-flight pulls only.** An entry is removed when its pull ends, so the map cannot grow for the life of the process. A test proves two concurrent first views make one remote request.
- **Docs and the "Read first" list are updated in the branch.** CLAUDE.md's stale "no remote fetch yet" clause and the parent spec's matching sentence are replaced with the shipped behavior, and the transport spec plus Plans A and B are listed, so the docs merge with the code.
- **The end-to-end test lives in `rupu-cp`.** The spec's end-to-end (Plan A's test continued with a GET) cannot sit in `rupu-orchestrator`, which cannot depend on `rupu-cp`. The test builds a Plan A unit stream, runs `ingest_unit_stream` with a registered remote host, and fetches the artifact through the coordinator. Plan A's own end-to-end covers the dispatch path up to ingest.
- **The tunnel node streams pulls interleaved with its frame loop, round-robin.** The plan's inline `send_artifact_blob` would block Cancel/Run handling and run-file drains for the whole transfer of a blob of up to 500 MB. The node keeps a queue of pulls, sends one chunk per loop turn and rotates, so a small pull is not stuck behind a large one (a front-only queue timed out the small pull on the idle bound).
- **Bucket workers do not advertise the tunnel-only `findings.artifact_pull` capability.** They shared the node capability list, which made the marker untrue.
- **Task 6 is cancel-safe and the store owns its invariants.** The store gained `pull_temp_path` and `install_verified` (0700 root, exact size and sha256 check, fsync, rename). The pull runs in a spawned task shared by all waiters, keyed by destination blob path, and removes its in-flight entry itself, so a browser disconnect mid-pull neither strands a temp file nor aborts the pull for other viewers.
- **SSH pulls have idle bounds, including the exit wait.** A silent established connection, or a child that never exits after EOF, would otherwise pin the shared pull forever. Reads, the final `wait()` and the stderr join are each bounded at 60 s. Local write failures are labelled and map to an invalid-request error, not "host unreachable".
- **HTTP and tunnel pulls have idle bounds too.** The HTTP pull keeps its 30-minute per-request timeout as the whole-transfer ceiling (it is what overrides the client's 30 s total), and the response head and each body chunk must arrive within 60 s of the last, or the pull fails as unreachable. The tunnel pull's initial `ArtifactPull` send waits on the tunnel's bounded write channel, so it is held to the same 60 s.
- **The bucket upload is a bounded multipart.** The plan's loop buffered the whole blob and let parallel parts time out on slow uplinks. The worker now waits for capacity before each write (at most four 5 MiB parts in flight), aborts the upload on any error, and uses a plain PUT below one part. Reading the run's stream is lossy-decoded so a torn tail does not drop every reference of a cancelled run.
- **The bucket download uses ranged reads.** The connector reads 8 MiB ranges and checks the total against the object size, so large artifacts are not bound by the store client's 30 s whole-request timeout.
- **The web artifact browser is un-gated for remote artifacts (added Task 6b).** #677 hid preview and download for artifacts with a `host` only because the CP could not fetch them. Leaving that gate would have made Plan B unreachable from the UI. The browser now previews (text up to 256 KiB, on click) and offers Download for them with a "from host" note, and `apiErrorMessage` reads `{"unavailable"}` bodies.
- **Known limitations, accepted:**
  - A bucket worker's run-end upload needs about 5.6 Mbit/s of uplink (four 5 MiB parts inside the store client's 30 s request timeout); a blob that still fails after 3 passes is logged and released, and the coordinator reports it unavailable, not silent.
  - The bucket worker's drain loop is sequential, so a large run-end upload delays other runs' drains and cancel polling on that worker. The fix is to upload off-loop and gate `finished` on completion.
  - There is no `ArtifactPullCancel` frame, so an abandoned tunnel pull streams to completion (it no longer blocks other pulls).
  - A coordinator killed mid-pull leaves `.pull-*` temp files (no startup sweep).
  - A host binary's Download in the web UI is a plain browser navigation, so a failed pull shows the 404 JSON in the browser rather than in the page.
