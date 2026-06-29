# Multi-host Slice 2c — SSH transport Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fourth `HostConnector` transport — plain SSH — so the central CP can dispatch/observe/control runs on a host it can only reach over `ssh`.

**Architecture:** A new `SshHostConnector` dispatches via one-shot `ssh host rupu …` (through an injectable `RemoteExec` seam) and observes by mirroring the remote run's artifacts into the central `RunStore` via a per-run `ssh tail -f` pump — reusing the Tunnel mirror + the shared mirror-backed observation read-path. Control (cancel/approve/reject) reuses the same remote `rupu workflow …` commands Slice 2.5 drives. Auth is delegated entirely to the system `ssh`.

**Tech Stack:** Rust 2021, tokio (`tokio::process::Command` shelling out to `ssh`), serde, async-trait, thiserror (libs) / anyhow (CLI). NO ssh-library crate.

## Global Constraints

- Workspace deps only — versions in the ROOT `Cargo.toml`, never a crate `Cargo.toml`. This slice adds NO new dependencies: `ssh` is shelled out via `tokio::process::Command`, matching how the codebase already shells out to `git` / `gh` / `rg`.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden (no `unsafe`).
- Library errors use `thiserror`; the CLI binary uses `anyhow`.
- Per-file `rustfmt` only — `main` is fmt-dirty; NEVER run workspace-wide `cargo fmt`.
- `rupu-cli` has a PRE-EXISTING red toolchain baseline; verify only that NEW code compiles and its tests pass.
- Auth: rupu NEVER handles key material. It invokes `ssh` and the system resolves auth (ssh-agent / `~/.ssh/config` / default keys). The host record stores only `host` / `port` / `identity_file`; no `token_hash`, no secrets.
- Security: every remote argument is shell-escaped before being joined into the remote command (ssh re-parses remote args through the remote login shell). `ssh -o BatchMode=yes` so a missing key fails fast instead of hanging.
- This branch (`worktree-multi-host-slice-2c`) is STACKED on the Slice 2.5 branch (PR #418). It extends the 2.5 resume-worker filter and shares the mirror-backed observation; those 2.5 changes are present on this branch.
- Remote-environment assumptions (documented, out of scope to parameterize in 2c): the remote host uses the default `~/.rupu` runs root, and `rupu` is on the remote login shell's PATH.

---

## File Structure

- `crates/rupu-workspace/src/host_store.rs` — `HostTransport::Ssh` variant + `add_ssh_host` helper. (Task 1)
- Exhaustive `HostTransport` matches updated to keep the build green: `crates/rupu-cli/src/cmd/host.rs` (list display), `crates/rupu-cp/src/api/hosts.rs` (transport_fields), `crates/rupu-cp/src/host/registry.rs` (build_connector — placeholder until Task 5). (Task 1)
- `crates/rupu-cp/src/host/ssh.rs` — NEW: `RemoteExec` trait + `SshExec` real impl + pure builders (`build_remote_command`, `ssh_argv`, `parse_tail_marker`) + `SshHostConnector`. (Tasks 2, 4)
- `crates/rupu-cp/src/host/mod.rs` — `pub mod ssh;`. (Task 2)
- `crates/rupu-cp/src/host/connector.rs` — extract shared mirror-backed observation helpers. (Task 3)
- `crates/rupu-cp/src/host/tunnel.rs` — refactor to call the shared helpers. (Task 3)
- `crates/rupu-cp/src/host/registry.rs` — `build_connector` `Ssh` arm. (Task 5)
- `crates/rupu-cli/src/cmd/cp.rs` — extend the resume-worker filter to SSH hosts. (Task 6)
- `crates/rupu-cli/src/cmd/host.rs` — `rupu host add --ssh`. (Task 7)

---

## Task 1: `HostTransport::Ssh` variant + `add_ssh_host`

**Files:**
- Modify: `crates/rupu-workspace/src/host_store.rs` (enum variant + helper + tests)
- Modify (keep build green — add `Ssh` match arms): `crates/rupu-cli/src/cmd/host.rs` (list display), `crates/rupu-cp/src/api/hosts.rs` (transport_fields), `crates/rupu-cp/src/host/registry.rs` (`build_connector` placeholder)

**Interfaces:**
- Produces: `HostTransport::Ssh { host: String, port: Option<u16>, identity_file: Option<std::path::PathBuf> }` (serde tag `kind = "ssh"`); `add_ssh_host(store: &HostStore, name: &str, host: &str, port: Option<u16>, identity_file: Option<PathBuf>) -> Result<Host, HostStoreError>` assigning `host_<ULID>`, no `token_hash`.

- [ ] **Step 1: Write the failing tests**

In the `#[cfg(test)]` module of `crates/rupu-workspace/src/host_store.rs` (follow the existing `tunnel_transport_serde_roundtrip` style):

```rust
#[test]
fn ssh_transport_serde_roundtrip() {
    let host = Host {
        id: "host_01ABC".to_string(),
        name: "edge".to_string(),
        transport: HostTransport::Ssh {
            host: "deploy@edge.example".to_string(),
            port: Some(2222),
            identity_file: Some(std::path::PathBuf::from("/keys/id_ed25519")),
        },
        token_hash: None,
        created_at: "2026-06-29T00:00:00Z".to_string(),
        last_seen_at: None,
    };
    let toml = toml::to_string(&host).unwrap();
    assert!(toml.contains(r#"kind = "ssh""#));
    assert!(toml.contains(r#"host = "deploy@edge.example""#));
    let back: Host = toml::from_str(&toml).unwrap();
    assert_eq!(back, host);
}

#[test]
fn add_ssh_host_persists_no_token() {
    let tmp = tempfile::tempdir().unwrap();
    let store = HostStore { root: tmp.path().to_path_buf() };
    let h = add_ssh_host(&store, "edge", "deploy@edge.example", None, None).unwrap();
    assert!(h.id.starts_with("host_"));
    assert!(matches!(h.transport, HostTransport::Ssh { .. }));
    assert!(h.token_hash.is_none());
    let loaded = store.load(&h.id).unwrap();
    assert_eq!(loaded.transport, h.transport);
}
```

Note: `Host` must `derive(PartialEq)` for `assert_eq!` (the existing tunnel roundtrip test uses it, so it already does).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-workspace ssh_transport_serde_roundtrip add_ssh_host_persists_no_token`
Expected: FAIL — no `Ssh` variant / no `add_ssh_host`.

- [ ] **Step 3: Add the variant**

In the `HostTransport` enum in `crates/rupu-workspace/src/host_store.rs` (after `Tunnel`):

```rust
    /// Reachable over plain SSH. The CP shells out to `ssh` to dispatch and
    /// observe runs. `host` is an `ssh` destination (`user@hostname`) or a
    /// `~/.ssh/config` alias. No secrets are stored — auth is whatever the
    /// system `ssh` resolves.
    Ssh {
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity_file: Option<std::path::PathBuf>,
    },
```

- [ ] **Step 4: Add the `add_ssh_host` helper**

Near `enroll_node` in `crates/rupu-workspace/src/host_store.rs` (match the existing add/enroll style; confirm the real `Host` construction + `store.save` signature from `enroll_node`):

```rust
/// Create and persist an `Ssh` host record. No secret is stored — auth is
/// delegated to the system `ssh`.
pub fn add_ssh_host(
    store: &HostStore,
    name: &str,
    host: &str,
    port: Option<u16>,
    identity_file: Option<std::path::PathBuf>,
) -> Result<Host, HostStoreError> {
    let record = Host {
        id: format!("host_{}", ulid::Ulid::new()),
        name: name.to_string(),
        transport: HostTransport::Ssh {
            host: host.to_string(),
            port,
            identity_file,
        },
        token_hash: None,
        created_at: now_rfc3339(),
        last_seen_at: None,
    };
    store.save(&record)?;
    Ok(record)
}
```

Use the REAL timestamp helper the crate already uses (e.g. `now_rfc3339()` / the same call `enroll_node` uses — read it and match). Confirm `ulid` is already a dep of rupu-workspace (it is — `enroll_node` uses ULID-style ids; if it uses a different id scheme, match that).

- [ ] **Step 5: Keep the build green — add `Ssh` arms to exhaustive matches**

These three Rust matches on `HostTransport` are exhaustive and will fail to compile once the variant exists. Add minimal `Ssh` arms:

1. `crates/rupu-cli/src/cmd/host.rs` list display (the `match … { HttpCp … => …, Tunnel … => format!("tunnel:{node_id}") }`):
```rust
            HostTransport::Ssh { host, port, .. } => match port {
                Some(p) => format!("ssh:{host}:{p}"),
                None => format!("ssh:{host}"),
            },
```

2. `crates/rupu-cp/src/api/hosts.rs` `transport_fields` (or wherever `HostTransport` is matched to build `HostView`): add an arm setting `transport_kind = "ssh"` and the address field to the `host` string (follow the shape the Tunnel/HttpCp arms use — read it and mirror; expose `host`/`port` as the display fields, never any secret).

3. `crates/rupu-cp/src/host/registry.rs` `build_connector`: add a TEMPORARY placeholder arm (real connector lands in Task 5):
```rust
            HostTransport::Ssh { .. } => Err(HostConnectorError::Invalid(
                "ssh transport not yet wired (slice 2c task 5)".to_string(),
            )),
```

- [ ] **Step 6: Verify**

Run: `cargo test -p rupu-workspace ssh_transport_serde_roundtrip add_ssh_host_persists_no_token` → PASS.
Run: `cargo build -p rupu-workspace -p rupu-cp -p rupu-cli` → compiles.
Run: `cargo clippy -p rupu-workspace -p rupu-cp` → clean.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-workspace/src/host_store.rs crates/rupu-cli/src/cmd/host.rs crates/rupu-cp/src/api/hosts.rs crates/rupu-cp/src/host/registry.rs
git commit -m "feat(workspace): HostTransport::Ssh + add_ssh_host"
```

---

## Task 2: SSH command builders + `RemoteExec` seam

**Files:**
- Create: `crates/rupu-cp/src/host/ssh.rs`
- Modify: `crates/rupu-cp/src/host/mod.rs` (add `pub mod ssh;`)

**Interfaces:**
- Produces:
  - `fn shell_escape(arg: &str) -> String` (POSIX single-quote escaping)
  - `fn build_remote_command(argv: &[String]) -> String` (shell-escapes each token, joins with spaces)
  - `fn ssh_argv(host: &str, port: Option<u16>, identity_file: Option<&std::path::Path>, remote_command: &str) -> Vec<String>` (the args after the `ssh` program)
  - `fn parse_tail_marker(line: &str) -> Option<&str>` (returns the path inside a `==> <path> <==` header, else None)
  - `#[async_trait] trait RemoteExec: Send + Sync { async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError>; fn spawn_lines(&self, remote: &str) -> Result<LineStream, RemoteExecError>; }`
  - `struct SshExec { host, port, identity_file }` impl `RemoteExec` (real, shells out to `ssh`)
  - `struct RemoteOutput { stdout: String, stderr: String, success: bool }`; `type LineStream = ...` (a `Pin<Box<dyn Stream<Item = std::io::Result<String>> + Send>>`)
  - `#[derive(thiserror::Error)] enum RemoteExecError { Spawn(String), … }`

- [ ] **Step 1: Write the failing builder tests**

In `crates/rupu-cp/src/host/ssh.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn shell_escape_wraps_and_escapes_quotes() {
    assert_eq!(shell_escape("plain"), "'plain'");
    assert_eq!(shell_escape("a b"), "'a b'");
    assert_eq!(shell_escape("it's"), r#"'it'\''s'"#);
    assert_eq!(shell_escape("a;rm -rf /"), "'a;rm -rf /'");
    assert_eq!(shell_escape("$HOME"), "'$HOME'");
}

#[test]
fn build_remote_command_escapes_each_token() {
    let argv = vec![
        "rupu".to_string(), "workflow".to_string(), "run".to_string(),
        "my workflow".to_string(), "--run-id".to_string(), "run_1".to_string(),
    ];
    assert_eq!(
        build_remote_command(&argv),
        "'rupu' 'workflow' 'run' 'my workflow' '--run-id' 'run_1'"
    );
}

#[test]
fn ssh_argv_includes_flags_in_order() {
    let argv = ssh_argv("deploy@edge", Some(2222), Some(std::path::Path::new("/k/id")), "'true'");
    // BatchMode always on; -i and -p present; host then remote command last.
    assert!(argv.contains(&"-oBatchMode=yes".to_string()) || argv.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
    assert!(argv.iter().any(|a| a == "-i") && argv.iter().any(|a| a == "/k/id"));
    assert!(argv.iter().any(|a| a == "-p") && argv.iter().any(|a| a == "2222"));
    assert_eq!(argv.last().unwrap(), "'true'");
    let pos_host = argv.iter().position(|a| a == "deploy@edge").unwrap();
    let pos_cmd = argv.len() - 1;
    assert!(pos_host < pos_cmd, "host must precede the remote command");
}

#[test]
fn ssh_argv_omits_optional_flags() {
    let argv = ssh_argv("edge", None, None, "'true'");
    assert!(!argv.iter().any(|a| a == "-i"));
    assert!(!argv.iter().any(|a| a == "-p"));
    assert!(argv.iter().any(|a| a == "edge"));
}

#[test]
fn parse_tail_marker_extracts_path() {
    assert_eq!(parse_tail_marker("==> /r/run_1/events.jsonl <=="), Some("/r/run_1/events.jsonl"));
    assert_eq!(parse_tail_marker("{\"some\":\"json\"}"), None);
    assert_eq!(parse_tail_marker(""), None);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-cp --lib host::ssh`
Expected: FAIL — module / fns not defined.

- [ ] **Step 3: Implement the pure builders**

Create `crates/rupu-cp/src/host/ssh.rs` (top portion; connector comes in Task 4):

```rust
//! SSH transport: dispatch/observe/control runs on a host reachable over `ssh`.
//!
//! Auth is delegated entirely to the system `ssh` (ssh-agent / `~/.ssh/config`
//! / default keys); rupu stores no key material. Every remote argument is
//! shell-escaped before being joined into the remote command, because `ssh`
//! re-parses remote args through the remote login shell.
#![deny(clippy::all)]

use std::path::Path;

/// POSIX single-quote escaping: wrap in single quotes, replacing each embedded
/// `'` with `'\''`.
pub(crate) fn shell_escape(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('\'');
    for ch in arg.chars() {
        if ch == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Join an argv into a single shell command string with each token escaped.
pub(crate) fn build_remote_command(argv: &[String]) -> String {
    argv.iter()
        .map(|a| shell_escape(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the args (after the `ssh` program) to run `remote_command` on `host`.
/// `BatchMode=yes` makes a missing key fail fast instead of hanging on a prompt.
pub(crate) fn ssh_argv(
    host: &str,
    port: Option<u16>,
    identity_file: Option<&Path>,
    remote_command: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "-o".to_string(), "BatchMode=yes".to_string(),
        "-o".to_string(), "ConnectTimeout=10".to_string(),
    ];
    if let Some(id) = identity_file {
        argv.push("-i".to_string());
        argv.push(id.to_string_lossy().into_owned());
    }
    if let Some(p) = port {
        argv.push("-p".to_string());
        argv.push(p.to_string());
    }
    argv.push(host.to_string());
    argv.push(remote_command.to_string());
    argv
}

/// If `line` is a `tail` file-header (`==> <path> <==`), return `<path>`.
pub(crate) fn parse_tail_marker(line: &str) -> Option<&str> {
    let t = line.trim();
    let inner = t.strip_prefix("==> ")?.strip_suffix(" <==")?;
    if inner.is_empty() { None } else { Some(inner) }
}
```

(Adjust the `ssh_argv` test to whatever exact representation you choose for `-o BatchMode=yes` — the test above accepts either `-o BatchMode=yes` as two args or `-oBatchMode=yes` as one; the implementation here uses two args, so keep that form consistent in the test.)

- [ ] **Step 4: Run the builder tests to verify they pass**

Run: `cargo test -p rupu-cp --lib host::ssh`
Expected: PASS.

- [ ] **Step 5: Add the `RemoteExec` trait + `SshExec` real impl**

Append to `crates/rupu-cp/src/host/ssh.rs`:

```rust
use futures_util::stream::Stream;
use std::pin::Pin;

#[derive(Debug, Clone)]
pub(crate) struct RemoteOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoteExecError {
    #[error("ssh spawn failed: {0}")]
    Spawn(String),
}

pub(crate) type LineStream = Pin<Box<dyn Stream<Item = std::io::Result<String>> + Send>>;

/// Port: run a command on the remote host. Real impl shells out to `ssh`;
/// tests inject a fake.
#[async_trait::async_trait]
pub(crate) trait RemoteExec: Send + Sync {
    async fn run(&self, remote_command: &str) -> Result<RemoteOutput, RemoteExecError>;
    fn spawn_lines(&self, remote_command: &str) -> Result<LineStream, RemoteExecError>;
}

pub(crate) struct SshExec {
    pub host: String,
    pub port: Option<u16>,
    pub identity_file: Option<std::path::PathBuf>,
}

#[async_trait::async_trait]
impl RemoteExec for SshExec {
    async fn run(&self, remote_command: &str) -> Result<RemoteOutput, RemoteExecError> {
        let argv = ssh_argv(&self.host, self.port, self.identity_file.as_deref(), remote_command);
        let out = tokio::process::Command::new("ssh")
            .args(&argv)
            .output()
            .await
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        Ok(RemoteOutput {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        })
    }

    fn spawn_lines(&self, remote_command: &str) -> Result<LineStream, RemoteExecError> {
        use tokio::io::AsyncBufReadExt;
        let argv = ssh_argv(&self.host, self.port, self.identity_file.as_deref(), remote_command);
        let mut child = tokio::process::Command::new("ssh")
            .args(&argv)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        let stdout = child.stdout.take().ok_or_else(|| RemoteExecError::Spawn("no stdout".into()))?;
        let reader = tokio::io::BufReader::new(stdout);
        // Keep the child alive for the stream's lifetime by moving it into the stream state.
        let lines = tokio_stream::wrappers::LinesStream::new(reader.lines());
        // Attach the child so it is killed when the stream is dropped.
        let stream = async_stream::stream! {
            let _child_guard = child; // dropped (killed via kill_on_drop) when stream ends
            futures_util::pin_mut!(lines);
            while let Some(item) = futures_util::StreamExt::next(&mut lines).await {
                yield item;
            }
        };
        Ok(Box::pin(stream))
    }
}
```

CRITICAL dep check: this uses `futures_util`, `tokio_stream`, and `async_stream`. `futures_util` and `tokio_stream` are already workspace deps (used by `open_run_events_tail` / SSE). VERIFY before using; if `async_stream` is NOT already a root workspace dep, do NOT add it — instead implement `spawn_lines` without the macro (e.g. return the `LinesStream` and store the child in the connector's per-run task so it's killed on cancel, OR set `.kill_on_drop(true)` on the Command and return `LinesStream` boxed while the task that drains it owns the child). Pick the approach that adds NO new dependency. Document in your report which you used. Set `.kill_on_drop(true)` on the spawn so a dropped pump kills the ssh child.

- [ ] **Step 6: Verify build + tests**

Run: `cargo test -p rupu-cp --lib host::ssh` → PASS.
Run: `cargo clippy -p rupu-cp` → clean. (No new deps in `crates/rupu-cp/Cargo.toml` unless it's a `{ workspace = true }` reference to an already-pinned root dep.)

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/src/host/ssh.rs crates/rupu-cp/src/host/mod.rs
git commit -m "feat(cp): ssh command builders + RemoteExec seam"
```

---

## Task 3: Extract shared mirror-backed observation

**Files:**
- Modify: `crates/rupu-cp/src/host/connector.rs` (add shared helpers)
- Modify: `crates/rupu-cp/src/host/tunnel.rs` (refactor to call them)

**Interfaces:**
- Produces three `pub(crate)` helpers (worker-scoped, mirror-backed), reused by Tunnel + SSH:
  - `fn mirror_list_runs(run_store: &RunStore, worker_id: &str, params: &RunListQuery, pricing: &PricingConfig) -> Result<Vec<serde_json::Value>, HostConnectorError>`
  - `fn mirror_get_run(run_store: &RunStore, worker_id: &str, run_id: &str, pricing: &PricingConfig) -> Result<serde_json::Value, HostConnectorError>`
  - `async fn mirror_stream_run_events(run_store: &Arc<RunStore>, worker_id: &str, run_id: &str) -> Result<EventByteStream, HostConnectorError>`

- [ ] **Step 1: Add the shared helpers (refactor — behavior preserved)**

In `crates/rupu-cp/src/host/connector.rs`, move the bodies currently inside `TunnelHostConnector::{list_runs,get_run,stream_run_events}` into these free functions (verbatim logic, parameterized by `worker_id` instead of `self.node_id`). Use the EXACT current logic from `tunnel.rs` (the `query_run_rows(..., Some(worker_id), pricing)` call for list; the `load` + `worker_id` guard + `query_run_detail` for get; the `load` + guard + `open_run_events_tail` for stream). Example for `mirror_get_run`:

```rust
pub(crate) fn mirror_get_run(
    run_store: &RunStore,
    worker_id: &str,
    run_id: &str,
    pricing: &rupu_config::PricingConfig,
) -> Result<serde_json::Value, HostConnectorError> {
    let record = run_store.load(run_id).map_err(|e| match e {
        rupu_orchestrator::RunStoreError::NotFound(_) => HostConnectorError::NotFound(run_id.to_string()),
        other => HostConnectorError::Invalid(other.to_string()),
    })?;
    if record.worker_id.as_deref() != Some(worker_id) {
        return Err(HostConnectorError::NotFound(run_id.to_string()));
    }
    query_run_detail(run_store, run_id, pricing).map_err(|e| HostConnectorError::Invalid(e.to_string()))
}
```

Write `mirror_list_runs` and `mirror_stream_run_events` the same way, lifting the exact current Tunnel bodies. (`mirror_list_runs` takes `&RunListQuery` and reproduces the `workflow_only`/`query_run_rows`/serialize logic.)

- [ ] **Step 2: Refactor `TunnelHostConnector` to delegate**

In `crates/rupu-cp/src/host/tunnel.rs`, replace the three method bodies with one-line delegations, e.g.:

```rust
async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
    crate::host::connector::mirror_get_run(&self.run_store, &self.node_id, run_id, &self.pricing)
}
async fn list_runs(&self, params: RunListQuery) -> Result<Vec<serde_json::Value>, HostConnectorError> {
    crate::host::connector::mirror_list_runs(&self.run_store, &self.node_id, &params, &self.pricing)
}
async fn stream_run_events(&self, run_id: &str) -> Result<EventByteStream, HostConnectorError> {
    crate::host::connector::mirror_stream_run_events(&self.run_store, &self.node_id, run_id).await
}
```

- [ ] **Step 2b: Run the existing tunnel tests to verify NO behavior change**

Run: `cargo test -p rupu-cp --test node_tunnel`
Expected: PASS (all existing tunnel observation tests — proves the refactor preserved behavior).
Run: `cargo clippy -p rupu-cp` → clean.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cp/src/host/connector.rs crates/rupu-cp/src/host/tunnel.rs
git commit -m "refactor(cp): extract shared mirror-backed observation helpers"
```

---

## Task 4: `SshHostConnector`

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs` (add the connector + tests)

**Interfaces:**
- Consumes: `RemoteExec` (Task 2), `build_remote_command`/`parse_tail_marker` (Task 2), the shared `mirror_*` helpers (Task 3), `NodeMirror` (`create_run`/`append`/`finish`), `RunStore`, the `HostConnector` trait, `LaunchRequest { workflow, inputs, mode, target, working_dir }`, `AgentLaunchRequest { agent, prompt, mode, target, working_dir }`, `ArtifactFile`.
- Produces: `pub(crate) struct SshHostConnector { host_id: String, exec: Arc<dyn RemoteExec>, mirror: Arc<NodeMirror>, run_store: Arc<RunStore>, pricing: PricingConfig }` impl `HostConnector`; `SshHostConnector::new(...)`.

- [ ] **Step 1: Write failing connector tests (via a `RemoteExec` fake)**

In `crates/rupu-cp/src/host/ssh.rs` tests, add a fake that records the remote commands and yields canned tail lines:

```rust
struct FakeExec {
    commands: std::sync::Mutex<Vec<String>>,
    tail_lines: Vec<String>,
}
#[async_trait::async_trait]
impl RemoteExec for FakeExec {
    async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
        self.commands.lock().unwrap().push(remote.to_string());
        Ok(RemoteOutput { stdout: String::new(), stderr: String::new(), success: true })
    }
    fn spawn_lines(&self, remote: &str) -> Result<LineStream, RemoteExecError> {
        self.commands.lock().unwrap().push(remote.to_string());
        let lines: Vec<std::io::Result<String>> =
            self.tail_lines.iter().cloned().map(Ok).collect();
        Ok(Box::pin(futures_util::stream::iter(lines)))
    }
}

#[tokio::test]
async fn launch_run_mints_creates_mirror_and_dispatches() {
    let tmp = tempfile::tempdir().unwrap();
    let run_store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
    let mirror = std::sync::Arc::new(crate::node::NodeMirror::new(std::sync::Arc::clone(&run_store)));
    let exec = std::sync::Arc::new(FakeExec { commands: Default::default(), tail_lines: vec![] });
    let conn = SshHostConnector::new("host_abc", exec.clone(), mirror, std::sync::Arc::clone(&run_store), rupu_config::PricingConfig::default());

    let run_id = conn.launch_run(crate::launcher::LaunchRequest {
        workflow: "deploy".into(),
        inputs: Default::default(),
        mode: Some("bypass".into()),
        target: None,
        working_dir: None,
    }).await.unwrap();

    assert!(run_id.starts_with("run_"));
    // mirror run exists, attributed to host_abc
    let rec = run_store.load(&run_id).unwrap();
    assert_eq!(rec.worker_id.as_deref(), Some("host_abc"));
    // dispatched a detached remote `rupu workflow run … --run-id <id> --plain`
    let cmds = exec.commands.lock().unwrap();
    assert!(cmds.iter().any(|c| c.contains("'workflow'") && c.contains("'run'")
        && c.contains(&format!("'{run_id}'")) && c.contains("'--plain'")));
    assert!(cmds.iter().any(|c| c.contains("setsid") || c.contains("nohup")));
}

#[tokio::test]
async fn cancel_approve_reject_issue_remote_commands() {
    // build a connector as above; call cancel_run / approve_run / reject_run;
    // assert exec.commands contain `'workflow' 'cancel' '<id>'`,
    // `'workflow' 'approve' '<id>' '--mode' 'bypass'`,
    // `'workflow' 'reject' '<id>' '--reason' 'nope'` respectively.
}

#[tokio::test]
async fn offline_host_run_failure_surfaces_unreachable() {
    // FakeExec whose run() returns success:false with stderr; assert info()/launch
    // map a failed ssh to HostConnectorError::Unreachable.
}
```

(Write the `cancel_approve_reject_issue_remote_commands` and `offline_host_run_failure_surfaces_unreachable` bodies fully — they mirror the first test's setup.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-cp --lib host::ssh`
Expected: FAIL — `SshHostConnector` not defined.

- [ ] **Step 3: Implement `SshHostConnector`**

Append to `crates/rupu-cp/src/host/ssh.rs`. Key methods:

```rust
use std::sync::Arc;
use rupu_orchestrator::RunStore;
use crate::node::NodeMirror;
use crate::node::protocol::{RunSpec, RunSpecKind, ArtifactFile};
use crate::host::connector::{HostConnector, HostConnectorError, HostInfo, HostCapabilities,
    LaunchRequest_PLACEHOLDER /* import real */ };

pub(crate) struct SshHostConnector {
    pub host_id: String,
    pub exec: Arc<dyn RemoteExec>,
    pub mirror: Arc<NodeMirror>,
    pub run_store: Arc<RunStore>,
    pub pricing: rupu_config::PricingConfig,
}

impl SshHostConnector {
    pub fn new(host_id: impl Into<String>, exec: Arc<dyn RemoteExec>, mirror: Arc<NodeMirror>,
               run_store: Arc<RunStore>, pricing: rupu_config::PricingConfig) -> Self {
        Self { host_id: host_id.into(), exec, mirror, run_store, pricing }
    }

    /// Remote argv for a workflow run (mirrors cp_launcher::build_run_argv).
    fn workflow_argv(req: &crate::launcher::LaunchRequest, run_id: &str) -> Vec<String> {
        let mut a = vec!["rupu".into(), "workflow".into(), "run".into(), req.workflow.clone()];
        if let Some(t) = &req.target { a.push(t.clone()); }
        a.push("--run-id".into()); a.push(run_id.to_string()); a.push("--plain".into());
        for (k, v) in &req.inputs { a.push("--input".into()); a.push(format!("{k}={v}")); }
        if let Some(m) = &req.mode { a.push("--mode".into()); a.push(m.clone()); }
        a
    }

    /// Remote argv for an agent run (mirrors cp_agent_launcher).
    fn agent_argv(req: &crate::agent_launcher::AgentLaunchRequest, run_id: &str) -> Vec<String> {
        let mut a = vec!["rupu".into(), "run".into(), req.agent.clone()];
        if let Some(t) = &req.target { a.push(t.clone()); }
        a.push("--run-id".into()); a.push(run_id.to_string());
        if let Some(m) = &req.mode { a.push("--mode".into()); a.push(m.clone()); }
        if let Some(p) = &req.prompt { a.push("--prompt".into()); a.push(p.clone()); }
        if req.target.is_some() { a.push("--tmp".into()); }
        a
    }

    /// Wrap an escaped remote command so the remote run is detached and
    /// survives the ssh session closing.
    fn detach(remote_cmd: &str) -> String {
        format!("setsid {remote_cmd} </dev/null >/dev/null 2>&1 &")
    }

    fn spawn_tail_pump(&self, run_id: String) {
        let exec = Arc::clone(&self.exec);
        let mirror = Arc::clone(&self.mirror);
        let host_id = self.host_id.clone();
        let dir = format!("$HOME/.rupu/runs/{run_id}");
        // tail the three jsonl files with file headers; poll run.json separately.
        let tail_argv = vec![
            "tail".to_string(), "-n".into(), "+1".into(), "-F".into(),
            format!("{dir}/events.jsonl"),
            format!("{dir}/step_results.jsonl"),
            format!("{dir}/unit_checkpoints.jsonl"),
        ];
        let tail_cmd = build_remote_command(&tail_argv);
        tokio::spawn(async move {
            let mut current: Option<ArtifactFile> = None;
            if let Ok(stream) = exec.spawn_lines(&tail_cmd) {
                futures_util::pin_mut!(stream);
                while let Some(Ok(line)) = futures_util::StreamExt::next(&mut stream).await {
                    if let Some(path) = parse_tail_marker(&line) {
                        current = if path.ends_with("events.jsonl") { Some(ArtifactFile::Events) }
                            else if path.ends_with("step_results.jsonl") { Some(ArtifactFile::StepResults) }
                            else if path.ends_with("unit_checkpoints.jsonl") { Some(ArtifactFile::UnitCheckpoints) }
                            else { None };
                        continue;
                    }
                    if line.trim().is_empty() { continue; }
                    if let Some(file) = current {
                        let _ = mirror.append(&run_id, &host_id, file, &line);
                    }
                }
            }
            // pump ended; best-effort terminal sync via run.json (see note).
            let cat = build_remote_command(&["cat".to_string(), format!("{dir}/run.json")]);
            if let Ok(out) = exec.run(&cat).await {
                if out.success && !out.stdout.trim().is_empty() {
                    let _ = mirror.append(&run_id, &host_id, ArtifactFile::RunJson, out.stdout.trim());
                    if let Ok(rec) = serde_json::from_str::<serde_json::Value>(out.stdout.trim()) {
                        if let Some(s) = rec.get("status").and_then(|v| v.as_str()) {
                            let _ = mirror.finish(&run_id, &host_id, s);
                        }
                    }
                }
            }
        });
    }
}
```

Then the trait impl: `launch_run`/`launch_agent` mint `run_<ULID>`, `mirror.create_run(&run_id, &self.host_id, &spec)`, build argv → `build_remote_command` → `Self::detach(..)` → `exec.run(detached).await` (map `!success` → `Unreachable` with stderr), then `self.spawn_tail_pump(run_id.clone())`, return run_id. (Build the `RunSpec` the same way `TunnelHostConnector` does so `create_run` records workflow/agent kind + name.) Control:

```rust
async fn cancel_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
    self.remote_workflow(&["cancel", run_id]).await
}
async fn approve_run(&self, run_id: &str, mode: &str) -> Result<(), HostConnectorError> {
    if mode.is_empty() { self.remote_workflow(&["approve", run_id]).await }
    else { self.remote_workflow(&["approve", run_id, "--mode", mode]).await }
}
async fn reject_run(&self, run_id: &str, reason: Option<&str>) -> Result<(), HostConnectorError> {
    match reason {
        Some(r) => self.remote_workflow(&["reject", run_id, "--reason", r]).await,
        None => self.remote_workflow(&["reject", run_id]).await,
    }
}
```

where `remote_workflow(&self, tail: &[&str]) -> Result<(), HostConnectorError>` builds `["rupu","workflow", ...tail]`, escapes via `build_remote_command`, calls `exec.run`, and maps `!success`/spawn-error to `HostConnectorError::Unreachable(stderr)`. Observation delegates to the Task-3 shared helpers with `&self.host_id` as `worker_id`; `get_transcript` → `read_transcript_file`; `proxy_get_json` → `Invalid` (as Tunnel does); `start_session`/`send_session_turn` → `Invalid("sessions not supported over ssh (slice 2c)")`. `info()`:

```rust
async fn info(&self) -> Result<HostInfo, HostConnectorError> {
    let probe = build_remote_command(&["true".to_string()]);
    let reachable = matches!(self.exec.run(&probe).await, Ok(o) if o.success);
    Ok(HostInfo { reachable, version: None, capabilities: HostCapabilities::default() })
}
```

Fix the placeholder import line — import the real `LaunchRequest`/`AgentLaunchRequest`/`HostConnector` types from their actual modules (read `tunnel.rs`'s `use` block and mirror it).

- [ ] **Step 4: Run the connector tests to verify they pass**

Run: `cargo test -p rupu-cp --lib host::ssh`
Expected: PASS.
Run: `cargo clippy -p rupu-cp` → clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/host/ssh.rs
git commit -m "feat(cp): SshHostConnector (dispatch + mirror tail pump + control)"
```

---

## Task 5: Registry resolution — `Ssh` arm

**Files:**
- Modify: `crates/rupu-cp/src/host/registry.rs` (`build_connector` `Ssh` arm — replace the Task-1 placeholder)
- Test: `crates/rupu-cp/tests/node_tunnel.rs` (or `host_registry.rs` — wherever the Tunnel resolution test lives; match it)

**Interfaces:**
- Consumes: `SshHostConnector::new` (Task 4), the registry's existing `run_store`/`pricing` deps (already injected via `with_tunnel_deps`), and the `Ssh` host transport fields.

- [ ] **Step 1: Write the failing resolution test**

Mirroring the existing Tunnel resolution test (`tunnel_host_resolves_*`), add one that: builds a `HostRegistry` with `with_tunnel_deps(...)` (which supplies `node_registry`/`mirror`/`run_store`/`pricing` — SSH needs `mirror`+`run_store`+`pricing`), `add_ssh_host`s a host into the store, then `registry.resolve(&host.id)` → `Ok`, and `connector.info().await` returns without panicking (reachability will be false in test since `ssh` to a fake host fails — assert `Ok(_)` or `reachable == false`). Also assert that resolving an SSH host WITHOUT the deps wired returns `Invalid` (mirrors the tunnel negative test).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp ssh_host_resolves`
Expected: FAIL — `build_connector` returns the Task-1 placeholder `Invalid`.

- [ ] **Step 3: Implement the `Ssh` arm**

In `crates/rupu-cp/src/host/registry.rs` `build_connector`, replace the placeholder `Ssh` arm:

```rust
            HostTransport::Ssh { host, port, identity_file } => {
                match (&self.node_mirror, &self.run_store) {
                    (Some(mir), Some(store)) => {
                        let exec = std::sync::Arc::new(crate::host::ssh::SshExec {
                            host: host.clone(),
                            port: *port,
                            identity_file: identity_file.clone(),
                        });
                        Ok(std::sync::Arc::new(crate::host::ssh::SshHostConnector::new(
                            host.id_PLACEHOLDER, // use the host record id — see note
                            exec,
                            std::sync::Arc::clone(mir),
                            std::sync::Arc::clone(store),
                            self.pricing.clone(),
                        )))
                    }
                    _ => Err(HostConnectorError::Invalid(
                        "ssh deps not wired (call HostRegistry::with_tunnel_deps)".to_string(),
                    )),
                }
            }
```

NOTE: `build_connector(&self, host: &Host)` receives the whole `host`, so use `host.id.clone()` for the connector's `host_id` (the `worker_id` the mirror attributes runs to). Replace `host.id_PLACEHOLDER` with `host.id.clone()`. Make `SshExec` / `SshHostConnector` / `ssh` module items `pub(crate)` so the registry can construct them.

- [ ] **Step 4: Run the resolution test to verify it passes**

Run: `cargo test -p rupu-cp ssh_host_resolves`
Expected: PASS.
Run: `cargo clippy -p rupu-cp` → clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/host/registry.rs crates/rupu-cp/src/host/ssh.rs crates/rupu-cp/tests/*.rs
git commit -m "feat(cp): resolve Ssh transport to SshHostConnector"
```

---

## Task 6: Resume-worker filter — skip SSH hosts

**Files:**
- Modify: `crates/rupu-cli/src/cmd/cp.rs` (extend the `tunnel_nodes` set built in the resume worker)
- Test: `crates/rupu-cp/tests/node_tunnel.rs` (extend the invariant test for an SSH-attributed run)

**Interfaces:**
- Consumes: `rupu_workspace::HostTransport::{Tunnel, Ssh}`; `Host.id`; `RunRecord.worker_id`.

- [ ] **Step 1: Write the failing invariant test**

Extend the Slice-2.5 `mirrored_awaiting_run_is_not_pending_resume` style test: create an `AwaitingApproval` run with `worker_id = Some("host_ssh_1")` and `resume_requested_at = None`, and a hosts store containing an `Ssh` host whose `id == "host_ssh_1"`. Then build the same `tunnel_nodes`/skip set logic the worker uses (or call the worker's filter helper if extracted) and assert the run is excluded. Since the worker loop isn't directly unit-testable, the locking test asserts the PRIMARY invariant via `list_pending_resume` (the SSH run has no marker, so it's already excluded). Mirror the exact shape of the existing 2.5 test, changing the worker_id + adding an Ssh host.

- [ ] **Step 2: Run to verify it passes (primary invariant) / extend the worker**

The primary-invariant test passes immediately (no marker → not listed). Now extend the defense-in-depth set.

- [ ] **Step 3: Extend the filter set in the worker**

In `crates/rupu-cli/src/cmd/cp.rs`, the resume worker currently builds `tunnel_nodes` from `Tunnel { node_id }`. Generalize it to collect the `worker_id` of BOTH tunnel and ssh hosts (rename to `remote_workers`):

```rust
        // Defense-in-depth: never resume a run that belongs to a REMOTE host
        // (tunnel or ssh); its real run lives on that host and is resumed via
        // the transport, not by this local worker.
        let remote_workers: std::collections::HashSet<String> = hosts
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|h| match h.transport {
                rupu_workspace::HostTransport::Tunnel { node_id } => Some(node_id),
                rupu_workspace::HostTransport::Ssh { .. } => Some(h.id),
                _ => None,
            })
            .collect();
```

and update the per-run check to use `remote_workers.contains(w)` (update the `tracing::debug!` message to "skipping remote-host run"). NOTE the asymmetry: a Tunnel run's `worker_id` is the `node_id`; an SSH run's `worker_id` is the host record `id` — that is why the Tunnel arm yields `node_id` and the Ssh arm yields `h.id`.

- [ ] **Step 4: Verify**

Run: `cargo build -p rupu-cli` → compiles.
Run: `cargo test -p rupu-cp --test node_tunnel mirrored_awaiting` → PASS.
Run: `cargo clippy -p rupu-cp` → clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/cmd/cp.rs crates/rupu-cp/tests/node_tunnel.rs
git commit -m "fix(cp): resume worker skips ssh-host runs (defense-in-depth)"
```

---

## Task 7: CLI — `rupu host add --ssh`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/host.rs` (`AddArgs` + `add_inner` + an ssh add path)
- Test: `crates/rupu-cli/src/cmd/host.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `rupu_workspace::host_store::add_ssh_host` (Task 1).
- Produces: `rupu host add --ssh <name> <user@host> [--port N] [--identity <path>]`.

- [ ] **Step 1: Write the failing test**

In `crates/rupu-cli/src/cmd/host.rs` tests (next to `add_list_remove_roundtrip`), add a test that calls the ssh-add path against a temp hosts dir and asserts a `Ssh` host is persisted with the right `host`/`port`/`identity_file` and no token. Match the existing test's helper style (it calls `add_host(dir, name, url, token)`; you'll call the new `add_ssh_host_cli(dir, name, host, port, identity)` or the workspace `add_ssh_host`).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli --lib cmd::host`
Expected: FAIL — no ssh add path.

- [ ] **Step 3: Add the CLI surface**

In `crates/rupu-cli/src/cmd/host.rs`, extend `AddArgs` (read its current clap shape first) to support SSH. Minimal approach — add flags:

```rust
pub struct AddArgs {
    pub name: String,
    /// HTTP base URL (HttpCp transport). Mutually exclusive with --ssh.
    #[arg(long, conflicts_with = "ssh")]
    pub url: Option<String>,
    /// SSH destination (user@host or ~/.ssh/config alias) — selects the Ssh transport.
    #[arg(long)]
    pub ssh: Option<String>,
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long)]
    pub identity: Option<std::path::PathBuf>,
    // ...existing token field, if any...
}
```

(Adapt to the ACTUAL current `AddArgs` — it currently has `name` + `url` (positional or flag) + token. Keep the HttpCp path working; add the `--ssh` branch.) In `add_inner`, branch: if `args.ssh.is_some()` call `rupu_workspace::host_store::add_ssh_host(&store, &args.name, ssh, args.port, args.identity)`; else the existing HttpCp `add_host` path. Print the new host id like the HttpCp path does.

- [ ] **Step 4: Verify**

Run: `cargo test -p rupu-cli --lib cmd::host` → PASS.
Run: `cargo build -p rupu-cli` → compiles. (Ignore the pre-existing rupu-cli baseline; assert only this module's tests pass.)

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/cmd/host.rs
git commit -m "feat(cli): rupu host add --ssh"
```

---

## Self-Review

**Spec coverage:**
- `HostTransport::Ssh` + `add_ssh_host` → Task 1. ✅
- `RemoteExec` seam + ssh command builders + shell-escaping → Task 2. ✅
- Shared mirror-backed observation (no duplication) → Task 3. ✅
- `SshHostConnector` (launch/tail-pump/control/observe/info) → Task 4. ✅
- Registry `Ssh` resolution → Task 5. ✅
- Resume-worker filter extended to SSH → Task 6. ✅
- CLI `rupu host add --ssh` → Task 7. ✅
- Mirror reuse (NodeMirror) — used in Tasks 4/5, not re-implemented. ✅
- Out of scope honored: no ControlMaster, no web UI, no CI ssh e2e, no ssh crate. ✅

**Type consistency:** `SshHostConnector { host_id, exec, mirror, run_store, pricing }` and `SshHostConnector::new(host_id, exec, mirror, run_store, pricing)` consistent across Tasks 4/5. `RemoteExec::{run, spawn_lines}` consistent Tasks 2/4. The mirror `worker_id` for SSH is the host record `id` (Tasks 4 create_run, 5 resolution, 6 filter all agree). Shared helper names `mirror_list_runs`/`mirror_get_run`/`mirror_stream_run_events` consistent Tasks 3/4.

**Placeholder scan:** the only intentional placeholders are clearly labeled and replaced within the same plan — Task 1's `build_connector` `Ssh` arm returns a temporary `Invalid` replaced in Task 5; `LaunchRequest_PLACEHOLDER`/`host.id_PLACEHOLDER` in Task 4/5 carry explicit "use the real import / `host.id.clone()`" notes. The `spawn_lines` real impl carries an explicit "verify `async_stream` is a dep; if not, use `kill_on_drop` + `LinesStream` instead — add NO new dep" instruction.

---

## Process Note

Branch + single PR per repo convention. Branch `worktree-multi-host-slice-2c` is STACKED on the 2.5 branch (PR #418). Build subagent-driven (TDD). After all tasks: final whole-branch review (opus), then finishing-a-development-branch → push + PR (no self-merge). When #418 merges to main, rebase this branch onto main before/after the PR so its diff is SSH-only.
