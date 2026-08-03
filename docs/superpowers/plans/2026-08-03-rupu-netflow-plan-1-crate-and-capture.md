# Netflow Plan 1 — `rupu-netflow` crate and HTTP capture

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `rupu-netflow` crate — flow record, sink port, append-only ledger, ASN enrichment, and the instrumented `reqwest` client — then migrate `rupu-providers` onto it and lock the choke point with a clippy guard.

**Architecture:** A new dependency-light lib crate defines a `FlowRecord` and the `FlowSink` port. The phase-1 capture adapter is a `reqwest-middleware` that pushes into a sink, gated behind an `http` cargo feature so the schema half of the crate carries no HTTP dependency. Records land in an append-only JSONL ledger under `.rupu/netflow/`; ASN enrichment is resolved at *read* time from an auto-refreshed table, never stamped into the record.

**Note on `Ulid::from_parts`:** its signature is `(timestamp_ms: u64, random: u128)`. Tests below pass `u64` values for both; the second argument needs an explicit `as u128`. Watch for this — it is the most likely compile error in Tasks 2–4.

**Tech Stack:** Rust 2021, `tokio`, `async-trait`, `serde`/`serde_json`, `chrono`, `ulid`, `thiserror`, `reqwest` + `reqwest-middleware`, `httpmock` + `tempfile` for tests.

**Spec:** `docs/superpowers/specs/2026-08-03-rupu-netflow-observability-design.md`

## Global Constraints

- **Workspace deps only.** Versions are pinned in the root `Cargo.toml`; never in a crate `Cargo.toml`. Adding `reqwest-middleware` means adding it to the root `[workspace.dependencies]` first.
- **`#![deny(clippy::all)]` and `unsafe_code = "forbid"`** apply workspace-wide. Every new crate `Cargo.toml` ends with `[lints]\nworkspace = true`.
- **Capture must never break a request.** No `unwrap()`/`expect()`/panic on any path reachable from the middleware. Sink writes are best-effort.
- **Query strings and headers are NEVER stored.** `FlowRecord.path` is query-stripped. There is no opt-out.
- **No TLS version/cipher fields.** `reqwest` does not expose them; a permanently-`None` field must not exist.
- **No `asn` field on `FlowRecord`.** ASN is resolved at read time.
- **`Origin` variants carry `String`, not `&'static str`** — the record must `Deserialize` from the ledger.
- **Never run package-wide `cargo fmt`** on this repo (main is fmt-dirty under the pinned toolchain). Format only the files you touched: `rustfmt --edition 2021 <path>`.
- **Never use bare `git stash` / `git stash pop`** — the stash stack is shared across worktrees.

---

### Task 1: Crate skeleton and `FlowRecord`

**Files:**
- Create: `crates/rupu-netflow/Cargo.toml`
- Create: `crates/rupu-netflow/src/lib.rs`
- Create: `crates/rupu-netflow/src/record.rs`
- Create: `crates/rupu-netflow/src/ctx.rs`
- Modify: `Cargo.toml` (workspace `members` + `[workspace.dependencies]` entry for `rupu-netflow`)

**Interfaces:**
- Consumes: nothing.
- Produces: `FlowId` (alias for `ulid::Ulid`), `Fidelity`, `Outcome`, `Origin`, `FlowCtx`, `FlowRecord`, `LedgerLine`. Every later task depends on these exact names.

- [ ] **Step 1: Add the crate to the workspace**

In the root `Cargo.toml`, add `"crates/rupu-netflow"` to `[workspace] members`, and under `[workspace.dependencies]`:

```toml
rupu-netflow = { path = "crates/rupu-netflow", default-features = false }
```

- [ ] **Step 2: Write `crates/rupu-netflow/Cargo.toml`**

```toml
[package]
name = "rupu-netflow"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[features]
default = []
http = ["dep:reqwest", "dep:reqwest-middleware", "dep:http"]

[dependencies]
async-trait.workspace = true
chrono.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true
ulid.workspace = true
reqwest = { workspace = true, optional = true }
reqwest-middleware = { workspace = true, optional = true }
http = { workspace = true, optional = true }

[dev-dependencies]
tempfile.workspace = true
httpmock.workspace = true
tokio = { workspace = true, features = ["test-util"] }

[lints]
workspace = true
```

If `reqwest-middleware` or `http` are absent from the root `[workspace.dependencies]`, add them:

```toml
reqwest-middleware = "0.4"
http = "1"
```

**Use 0.4, not 0.5 — this is verified, not a guess.** The workspace pins `reqwest = "0.12"`. `reqwest-middleware` 0.5.x requires `reqwest ^0.13`, so pinning 0.5 resolves BOTH `reqwest 0.12.28` and `reqwest 0.13.4` into `Cargo.lock`, and `ClientWithMiddleware` then wraps a `reqwest 0.13` `Client` that is type-incompatible with every existing call site. 0.4.x is the line that targets `reqwest 0.12`.

After adding it, confirm `rupu-netflow` resolves against the workspace `reqwest`:

```bash
cargo tree -p rupu-netflow --features http -i reqwest
```

Every line must show `reqwest v0.12.x`. If `reqwest-middleware` appears under a `v0.13.x` root, the pin is wrong — stop and report rather than building on a split graph.

Do **not** grep the whole `Cargo.lock` for this. The lockfile legitimately contains two `reqwest` majors already: `object_store 0.14` (via `rupu-cp` → `rupu-cli`) pulls `reqwest 0.13.4`, and has since before this plan started. That split is pre-existing, unrelated to netflow, and out of scope — the two never interoperate. A lockfile-wide check reports it as a failure and sends you chasing someone else's dependency.

If `cargo build` disagrees with any signature in this plan, follow the compiler and note the deviation in your report; the trait's exact shape is the compiler's to state, not this document's.

- [ ] **Step 3: Write the failing test**

Create `crates/rupu-netflow/src/record.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample() -> FlowRecord {
        FlowRecord {
            id: FlowId::from_parts(1, 2),
            ts: chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            ctx: FlowCtx {
                run_id: Some("run-1".into()),
                step_id: Some("step-1".into()),
                agent: Some("reviewer".into()),
                workspace_id: Some("ws-1".into()),
                origin: Origin::Provider("anthropic".into()),
            },
            fidelity: Fidelity::Http,
            method: "POST".into(),
            scheme: "https".into(),
            host: "api.anthropic.com".into(),
            port: 443,
            path: "/v1/messages".into(),
            peer_ip: Some("160.79.104.10".parse().unwrap()),
            resolved_ips: vec!["160.79.104.10".parse().unwrap()],
            http_version: Some("HTTP/1.1".into()),
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: Some(1234),
            bytes_in: None,
            body_complete: false,
            ttfb_ms: Some(42),
            duration_ms: None,
        }
    }

    #[test]
    fn flow_record_round_trips_through_json() {
        let r = sample();
        let json = serde_json::to_string(&r).unwrap();
        let back: FlowRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn origin_is_externally_tagged_with_kind_and_name() {
        let json = serde_json::to_value(Origin::Scm("github".into())).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "scm", "name": "github"}));

        let json = serde_json::to_value(Origin::Update).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "update"}));
    }

    #[test]
    fn ledger_line_round_trips_all_variants() {
        let lines = vec![
            LedgerLine::Flow(Box::new(sample())),
            LedgerLine::Complete {
                id: FlowId::from_parts(1, 2),
                bytes_in: 9999,
                duration_ms: 1500,
            },
            LedgerLine::Dropped {
                count: 7,
                ts: chrono::Utc.timestamp_opt(1_700_000_001, 0).unwrap(),
            },
        ];
        for line in lines {
            let json = serde_json::to_string(&line).unwrap();
            let back: LedgerLine = serde_json::from_str(&json).unwrap();
            assert_eq!(line, back);
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test -p rupu-netflow`
Expected: FAIL to compile — `cannot find type FlowRecord in this scope`.

- [ ] **Step 5: Write `ctx.rs`**

```rust
//! Attribution context. Bound when a client is CONSTRUCTED, never
//! discovered at request time — `tokio` task-locals and `tracing` spans
//! both lose context across `tokio::spawn`, which would silently
//! mis-attribute flows. See the design spec §4.

use serde::{Deserialize, Serialize};

/// Which subsystem opened the connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Origin {
    /// LLM provider, by provider name (`anthropic`, `openai`, …).
    Provider(String),
    /// SCM / issue connector, by platform (`github`, `gitlab`, …).
    Scm(String),
    /// MCP server, by configured server name.
    Mcp(String),
    Webhook,
    Update,
    Cp,
    System,
}

/// Attribution for every flow a client produces.
///
/// `run_id` is `None` for process-global clients (the update checker,
/// CP's host registry). Those flows reach the ledger but no transcript —
/// that is the honest shape of the data, not a gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowCtx {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub origin: Origin,
}

impl FlowCtx {
    /// A context with no run attribution, for process-global clients.
    pub fn system(origin: Origin) -> Self {
        Self {
            run_id: None,
            step_id: None,
            agent: None,
            workspace_id: None,
            origin,
        }
    }
}
```

- [ ] **Step 6: Write `record.rs` above the test module**

```rust
//! The canonical flow record and the ledger line envelope.

use crate::ctx::FlowCtx;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

pub use ulid::Ulid as FlowId;

/// How much of this record is actually known, by capture backend.
///
/// Rendered as a badge in every CP view. The subsystem never claims
/// coverage it does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    /// Host, outcome and timing only — recorded at a connector boundary
    /// whose HTTP stack we do not own (`octocrab`).
    Coarse,
    /// Exact request/response metadata from the instrumented client.
    Http,
    /// Frame-level capture from the microVM backend. Not emitted yet.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    HttpError,
    TransportError,
    Timeout,
}

/// One outbound request.
///
/// Deliberate omissions (spec §5.1): no query string, no headers, no TLS
/// version, no ASN. Query strings routinely carry tokens; ASN is resolved
/// at read time so a late-arriving dataset improves history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRecord {
    pub id: FlowId,
    pub ts: DateTime<Utc>,
    pub ctx: FlowCtx,
    pub fidelity: Fidelity,

    pub method: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// Query-stripped. Never contains `?`.
    pub path: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_ip: Option<IpAddr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_ips: Vec<IpAddr>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_out: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_in: Option<u64>,
    /// `false` while a streamed body is still draining. Flipped by a
    /// later `LedgerLine::Complete` folded in at read time.
    pub body_complete: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// One line of the append-only ledger.
///
/// `Flow` is boxed because it dwarfs the other variants — clippy's
/// `large_enum_variant` denies otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LedgerLine {
    Flow(Box<FlowRecord>),
    /// Finalizes a streamed body written earlier at header time.
    Complete {
        id: FlowId,
        bytes_in: u64,
        duration_ms: u64,
    },
    /// Records visible loss when the writer channel overflowed.
    Dropped { count: u64, ts: DateTime<Utc> },
}
```

- [ ] **Step 7: Write `lib.rs`**

```rust
//! Network egress observability for rupu.
//!
//! Phase 1 captures rupu's OWN outbound HTTP — provider APIs, SCM
//! connectors, MCP, webhooks, the update checker. It does NOT cover
//! traffic from the agent's `bash` subprocesses; that arrives with the
//! microVM backend (spec §9). Every record carries a [`Fidelity`] so no
//! view ever claims coverage it does not have.

pub mod ctx;
pub mod record;

pub use ctx::{FlowCtx, Origin};
pub use record::{Fidelity, FlowId, FlowRecord, LedgerLine, Outcome};
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p rupu-netflow`
Expected: PASS — 3 tests.

- [ ] **Step 9: Verify lints are clean**

Run: `cargo clippy -p rupu-netflow --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 10: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/lib.rs crates/rupu-netflow/src/record.rs crates/rupu-netflow/src/ctx.rs
git add Cargo.toml crates/rupu-netflow/
git commit -m "feat(netflow): FlowRecord, FlowCtx and ledger line types"
```

---

### Task 2: `FlowSink` port and its in-process adapters

**Files:**
- Create: `crates/rupu-netflow/src/sink.rs`
- Modify: `crates/rupu-netflow/src/lib.rs`

**Interfaces:**
- Consumes: `FlowRecord`, `FlowId` from Task 1.
- Produces: `FlowSink` trait, `NullSink`, `FanoutSink`, `MemorySink`. Task 3's `LedgerSink` implements `FlowSink`; Task 8's middleware calls it.

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-netflow/src/sink.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Origin;
    use crate::record::{Fidelity, Outcome};

    fn flow(id: FlowId) -> FlowRecord {
        FlowRecord {
            id,
            ts: chrono::Utc::now(),
            ctx: FlowCtx::system(Origin::Update),
            fidelity: Fidelity::Http,
            method: "GET".into(),
            scheme: "https".into(),
            host: "example.test".into(),
            port: 443,
            path: "/".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: None,
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: None,
            bytes_in: None,
            body_complete: true,
            ttfb_ms: None,
            duration_ms: None,
        }
    }

    #[tokio::test]
    async fn memory_sink_collects_records_and_completions() {
        let sink = MemorySink::default();
        let id = FlowId::from_parts(1, 1);
        sink.record(flow(id)).await;
        sink.complete(id, 512, 30).await;

        assert_eq!(sink.records().len(), 1);
        assert_eq!(sink.completions(), vec![(id, 512, 30)]);
    }

    #[tokio::test]
    async fn fanout_delivers_to_every_child() {
        let a = Arc::new(MemorySink::default());
        let b = Arc::new(MemorySink::default());
        let fan = FanoutSink::new(vec![a.clone(), b.clone()]);

        fan.record(flow(FlowId::from_parts(2, 2))).await;

        assert_eq!(a.records().len(), 1);
        assert_eq!(b.records().len(), 1);
    }

    #[tokio::test]
    async fn null_sink_is_inert() {
        let sink = NullSink;
        sink.record(flow(FlowId::from_parts(3, 3))).await;
        sink.complete(FlowId::from_parts(3, 3), 1, 1).await;
        // No panic, no state. Nothing to assert beyond reaching here.
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-netflow sink`
Expected: FAIL to compile — `cannot find trait FlowSink`.

- [ ] **Step 3: Write the implementation above the test module**

```rust
//! The `FlowSink` port and its in-process adapters.

use crate::ctx::FlowCtx;
use crate::record::{FlowId, FlowRecord};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// Where flow records go.
///
/// Implementations MUST be non-panicking and best-effort: a sink failure
/// must never surface into the request path that produced the record.
#[async_trait]
pub trait FlowSink: Send + Sync {
    /// Record a flow. Called at response-header time.
    async fn record(&self, flow: FlowRecord);

    /// Finalize a streamed body recorded earlier. Sinks that cannot
    /// express completion may ignore this.
    async fn complete(&self, id: FlowId, bytes_in: u64, duration_ms: u64);
}

/// Capture disabled.
pub struct NullSink;

#[async_trait]
impl FlowSink for NullSink {
    async fn record(&self, _flow: FlowRecord) {}
    async fn complete(&self, _id: FlowId, _bytes_in: u64, _duration_ms: u64) {}
}

/// Delivers to every child in order.
pub struct FanoutSink {
    children: Vec<Arc<dyn FlowSink>>,
}

impl FanoutSink {
    pub fn new(children: Vec<Arc<dyn FlowSink>>) -> Self {
        Self { children }
    }
}

#[async_trait]
impl FlowSink for FanoutSink {
    async fn record(&self, flow: FlowRecord) {
        for child in &self.children {
            child.record(flow.clone()).await;
        }
    }

    async fn complete(&self, id: FlowId, bytes_in: u64, duration_ms: u64) {
        for child in &self.children {
            child.complete(id, bytes_in, duration_ms).await;
        }
    }
}

/// Test double. Also used by CP for the live broadcast buffer.
#[derive(Default)]
pub struct MemorySink {
    inner: Mutex<MemoryState>,
}

#[derive(Default)]
struct MemoryState {
    records: Vec<FlowRecord>,
    completions: Vec<(FlowId, u64, u64)>,
}

impl MemorySink {
    pub fn records(&self) -> Vec<FlowRecord> {
        self.inner
            .lock()
            .map(|s| s.records.clone())
            .unwrap_or_default()
    }

    pub fn completions(&self) -> Vec<(FlowId, u64, u64)> {
        self.inner
            .lock()
            .map(|s| s.completions.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl FlowSink for MemorySink {
    async fn record(&self, flow: FlowRecord) {
        if let Ok(mut s) = self.inner.lock() {
            s.records.push(flow);
        }
    }

    async fn complete(&self, id: FlowId, bytes_in: u64, duration_ms: u64) {
        if let Ok(mut s) = self.inner.lock() {
            s.completions.push((id, bytes_in, duration_ms));
        }
    }
}
```

Note the `FlowCtx` import is used by the test module only; if clippy flags it as unused in the non-test build, move it into the test module's `use super::*` neighbours.

- [ ] **Step 4: Export from `lib.rs`**

```rust
pub mod sink;
pub use sink::{FanoutSink, FlowSink, MemorySink, NullSink};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-netflow`
Expected: PASS — 6 tests.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/sink.rs crates/rupu-netflow/src/lib.rs
git add crates/rupu-netflow/
git commit -m "feat(netflow): FlowSink port with null, fanout and memory adapters"
```

---

### Task 3: Ledger paths and the background writer

**Files:**
- Create: `crates/rupu-netflow/src/ledger/mod.rs`
- Create: `crates/rupu-netflow/src/ledger/paths.rs`
- Create: `crates/rupu-netflow/src/ledger/writer.rs`
- Modify: `crates/rupu-netflow/src/lib.rs`

**Interfaces:**
- Consumes: `FlowSink` (Task 2), `LedgerLine`/`FlowRecord`/`FlowId` (Task 1).
- Produces: `NetflowPaths::new(workspace) -> NetflowPaths` with field `flows: PathBuf`; `NetflowWriterHandle::spawn(NetflowPaths) -> io::Result<NetflowWriterHandle>` with fields `writer: Arc<NetflowWriter>` and method `shutdown(self)`; `NetflowWriter` implements `FlowSink` and exposes `dropped() -> u64`.

This mirrors `crates/rupu-coverage/src/ledger/{paths,writer}.rs` — read those first; the shape is deliberately identical.

- [ ] **Step 1: Write the failing test for paths**

Create `crates/rupu-netflow/src/ledger/paths.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_layout_under_dotrupu_netflow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::new(tmp.path());
        assert_eq!(paths.root, tmp.path().join(".rupu/netflow"));
        assert_eq!(paths.flows, paths.root.join("flows.jsonl"));
    }

    #[test]
    fn ensure_dir_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::new(tmp.path());
        paths.ensure_dir().unwrap();
        paths.ensure_dir().unwrap();
        assert!(paths.root.is_dir());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-netflow paths`
Expected: FAIL to compile — `cannot find struct NetflowPaths`.

- [ ] **Step 3: Implement `paths.rs` above the tests**

```rust
use std::path::{Path, PathBuf};

/// Canonical on-disk layout of a workspace's netflow ledger.
#[derive(Debug, Clone)]
pub struct NetflowPaths {
    pub root: PathBuf,
    pub flows: PathBuf,
}

impl NetflowPaths {
    pub fn new(workspace: &Path) -> Self {
        let root = workspace.join(".rupu").join("netflow");
        Self {
            flows: root.join("flows.jsonl"),
            root,
        }
    }

    pub fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)
    }
}
```

- [ ] **Step 4: Write the failing test for the writer**

Create `crates/rupu-netflow/src/ledger/writer.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::{FlowCtx, Origin};
    use crate::record::{Fidelity, Outcome};

    fn flow(id: FlowId) -> FlowRecord {
        FlowRecord {
            id,
            ts: chrono::Utc::now(),
            ctx: FlowCtx::system(Origin::Update),
            fidelity: Fidelity::Http,
            method: "GET".into(),
            scheme: "https".into(),
            host: "example.test".into(),
            port: 443,
            path: "/releases".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: None,
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: None,
            bytes_in: None,
            body_complete: false,
            ttfb_ms: None,
            duration_ms: None,
        }
    }

    #[tokio::test]
    async fn writer_appends_flow_and_complete_lines() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::new(tmp.path());
        let handle = NetflowWriterHandle::spawn(paths.clone()).unwrap();

        let id = FlowId::from_parts(9, 9);
        handle.writer.record(flow(id)).await;
        handle.writer.complete(id, 4096, 250).await;
        // Bounded: a shutdown deadlock must FAIL the test, not hang the
        // suite forever. An `Arc` cycle between `NetflowWriter` (which
        // owns the channel sender) and the writer task produces exactly
        // that hang, and a bare `.await` here would wedge CI instead of
        // reporting it.
        tokio::time::timeout(std::time::Duration::from_secs(10), handle.shutdown())
            .await
            .expect("writer shutdown deadlocked");

        let text = std::fs::read_to_string(&paths.flows).unwrap();
        let lines: Vec<LedgerLine> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        assert_eq!(lines.len(), 2);
        assert!(matches!(&lines[0], LedgerLine::Flow(f) if f.id == id));
        assert!(matches!(
            lines[1],
            LedgerLine::Complete { id: got, bytes_in: 4096, duration_ms: 250 } if got == id
        ));
    }

    #[tokio::test]
    async fn dropped_count_is_recorded_not_silent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::new(tmp.path());
        let handle = NetflowWriterHandle::spawn_with_capacity(paths.clone(), 1).unwrap();

        // Flood far past the channel capacity. The writer task cannot
        // keep up, so some sends hit the bounded-channel backstop.
        for i in 0..5_000u64 {
            handle
                .writer
                .record(flow(FlowId::from_parts(i, i as u128)))
                .await;
        }
        let dropped = handle.writer.dropped();
        tokio::time::timeout(std::time::Duration::from_secs(10), handle.shutdown())
            .await
            .expect("writer shutdown deadlocked");

        let text = std::fs::read_to_string(&paths.flows).unwrap();

        // This is DETERMINISTIC, not scheduler-dependent, so assert it
        // unconditionally. `#[tokio::test]` is current-thread by default
        // and `record()` contains no await point (`try_send` is sync), so
        // the writer task cannot run until this task yields at
        // `shutdown()`. With capacity 1, exactly the first send is
        // buffered and the other 4,999 overflow.
        assert!(dropped > 0, "flooding a capacity-1 channel must drop");

        let dropped_line = text
            .lines()
            .filter_map(|l| serde_json::from_str::<LedgerLine>(l).ok())
            .find_map(|l| match l {
                LedgerLine::Dropped { count, .. } => Some(count),
                _ => None,
            })
            .expect("loss must leave a Dropped line in the ledger");
        assert_eq!(dropped_line, dropped, "the ledger must account for every lost record");
    }
}
```

- [ ] **Step 5: Run to verify it fails**

Run: `cargo test -p rupu-netflow writer`
Expected: FAIL to compile — `cannot find struct NetflowWriterHandle`.

- [ ] **Step 6: Implement `writer.rs` above the tests**

```rust
use crate::ledger::paths::NetflowPaths;
use crate::record::{FlowId, FlowRecord, LedgerLine};
use crate::sink::FlowSink;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug)]
enum WriteRequest {
    Line(Box<LedgerLine>),
    Flush(tokio::sync::oneshot::Sender<()>),
    /// Explicit termination. `shutdown` MUST NOT rely on the last
    /// `Sender` being dropped: `NetflowWriterHandle::writer` is a public
    /// `Arc`, and any caller holding a clone (a `FanoutSink`, an
    /// instrumented client) would otherwise keep `rx.recv()` alive
    /// forever and hang shutdown.
    Stop,
}

/// Best-effort append-only ledger sink.
///
/// Writes go through a BOUNDED channel to a background task. When the
/// channel is full the record is dropped and counted — visible loss,
/// surfaced by the UI, never silent loss. Capture must never block or
/// break the request that produced it.
#[derive(Debug)]
pub struct NetflowWriter {
    tx: mpsc::Sender<WriteRequest>,
    /// Shared with the writer task. This MUST be a separate `Arc`, not
    /// reached via `Arc<NetflowWriter>` — see `spawn_with_capacity`.
    dropped: Arc<AtomicU64>,
}

impl NetflowWriter {
    /// Records lost to channel overflow since process start.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn offer(&self, line: LedgerLine) {
        if self.tx.try_send(WriteRequest::Line(Box::new(line))).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[async_trait]
impl FlowSink for NetflowWriter {
    async fn record(&self, flow: FlowRecord) {
        self.offer(LedgerLine::Flow(Box::new(flow)));
    }

    async fn complete(&self, id: FlowId, bytes_in: u64, duration_ms: u64) {
        self.offer(LedgerLine::Complete {
            id,
            bytes_in,
            duration_ms,
        });
    }
}

pub struct NetflowWriterHandle {
    pub writer: Arc<NetflowWriter>,
    task: JoinHandle<()>,
}

impl NetflowWriterHandle {
    pub fn spawn(paths: NetflowPaths) -> std::io::Result<Self> {
        Self::spawn_with_capacity(paths, CHANNEL_CAPACITY)
    }

    pub fn spawn_with_capacity(paths: NetflowPaths, capacity: usize) -> std::io::Result<Self> {
        paths.ensure_dir()?;
        let (tx, rx) = mpsc::channel(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = Arc::new(NetflowWriter {
            tx,
            dropped: dropped.clone(),
        });
        // Hand the writer task ONLY the counter, never `writer.clone()`.
        // `NetflowWriter` owns `tx`; if the task held an `Arc<NetflowWriter>`
        // that sender would outlive `shutdown`'s `drop(self.writer)`, so
        // `rx.recv()` would never yield `None`, the task would never exit,
        // and `self.task.await` would hang forever. This is a real deadlock
        // that was hit and fixed during implementation — do not "simplify"
        // it back.
        let task = tokio::spawn(run_writer(paths, rx, dropped));
        Ok(Self { writer, task })
    }

    /// Flush pending writes, emit a final `Dropped` line if anything was
    /// lost, then stop the task.
    pub async fn shutdown(self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.writer.tx.send(WriteRequest::Flush(tx)).await;
        let _ = rx.await;
        // Explicit stop, not sender-drop. Callers legitimately hold
        // `Arc<NetflowWriter>` clones (see `WriteRequest::Stop`).
        let _ = self.writer.tx.send(WriteRequest::Stop).await;
        drop(self.writer);
        let _ = self.task.await;
    }
}

async fn run_writer(
    paths: NetflowPaths,
    mut rx: mpsc::Receiver<WriteRequest>,
    dropped_counter: Arc<AtomicU64>,
) {
    let mut file = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.flows)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, path = ?paths.flows, "netflow ledger unavailable; flows will not be persisted");
            return;
        }
    };

    let mut last_dropped = 0u64;

    while let Some(req) = rx.recv().await {
        match req {
            WriteRequest::Line(line) => {
                write_line(&mut file, &line, &dropped_counter).await;
            }
            WriteRequest::Flush(ack) => {
                last_dropped = flush_dropped(&mut file, &dropped_counter, last_dropped).await;
                let _ = file.flush().await;
                let _ = ack.send(());
            }
            WriteRequest::Stop => {
                // Close first, THEN drain. `Stop` is FIFO-ordered against
                // concurrent producers, so a clone's `try_send` can land
                // behind it; breaking immediately would discard that
                // record with no counter bump — silent loss, the exact
                // defect this ledger exists to prevent.
                //
                // `close()` makes every subsequent `try_send` fail, so
                // `offer()` counts those as dropped (visible), while
                // `recv()` still yields everything already queued.
                rx.close();
                while let Some(pending) = rx.recv().await {
                    if let WriteRequest::Line(line) = pending {
                        write_line(&mut file, &line, &dropped_counter).await;
                    }
                }
                break;
            }
        }
    }

    flush_dropped(&mut file, &dropped_counter, last_dropped).await;
    let _ = file.flush().await;
}

/// Append a `Dropped` line for anything lost since `last_dropped`.
/// Returns the new watermark. One implementation, two call sites.
async fn flush_dropped(
    file: &mut tokio::fs::File,
    dropped_counter: &AtomicU64,
    last_dropped: u64,
) -> u64 {
    let dropped = dropped_counter.load(Ordering::Relaxed);
    if dropped > last_dropped {
        let line = LedgerLine::Dropped {
            count: dropped - last_dropped,
            ts: chrono::Utc::now(),
        };
        write_line(file, &line, dropped_counter).await;
    }
    dropped
}

/// Append one line. A failure here is REAL LOSS and must be counted —
/// the design's whole point is that a lost record leaves a trace. The
/// channel-overflow counter is not the only way records go missing: a
/// full disk or a revoked permission loses them here instead.
async fn write_line(file: &mut tokio::fs::File, line: &LedgerLine, dropped: &AtomicU64) {
    let json = match serde_json::to_string(line) {
        Ok(mut j) => {
            j.push('\n');
            j
        }
        Err(e) => {
            dropped.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(error = %e, "netflow ledger serialize failed; record lost");
            return;
        }
    };
    if let Err(e) = file.write_all(json.as_bytes()).await {
        dropped.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(error = %e, "netflow ledger write failed; record lost");
    }
}
```

- [ ] **Step 7: Write `ledger/mod.rs`**

```rust
pub mod paths;
pub mod writer;

pub use paths::NetflowPaths;
pub use writer::{NetflowWriter, NetflowWriterHandle};
```

- [ ] **Step 8: Export from `lib.rs`**

```rust
pub mod ledger;
pub use ledger::{NetflowPaths, NetflowWriter, NetflowWriterHandle};
```

- [ ] **Step 9: Run tests**

Run: `cargo test -p rupu-netflow`
Expected: PASS — 10 tests.

- [ ] **Step 10: Verify lints**

Run: `cargo clippy -p rupu-netflow --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 11: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/ledger/mod.rs crates/rupu-netflow/src/ledger/paths.rs crates/rupu-netflow/src/ledger/writer.rs crates/rupu-netflow/src/lib.rs
git add crates/rupu-netflow/
git commit -m "feat(netflow): append-only ledger writer with visible drop accounting"
```

---

### Task 4: Ledger read views and aggregation

**Files:**
- Create: `crates/rupu-netflow/src/ledger/views.rs`
- Modify: `crates/rupu-netflow/src/ledger/mod.rs`

**Interfaces:**
- Consumes: `NetflowPaths` (Task 3), `FlowRecord`/`LedgerLine` (Task 1).
- Produces: `read_flows(&Path) -> io::Result<Vec<FlowRecord>>`; `HostRollup { host, port, calls, bytes_in, bytes_out, errors, p50_ms, p95_ms }`; `host_rollup(&[FlowRecord]) -> Vec<HostRollup>`; `GraphView { nodes, edges }`, `GraphNode { id, label, side }`, `NodeSide::{Source, Endpoint}`, `GraphEdge { from, to, calls, bytes, errors }`; `graph_view(&[FlowRecord]) -> GraphView`; `read_dropped_total(&Path) -> io::Result<u64>`. Plan 3's CP API calls all of these.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-netflow/src/ledger/views.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::{FlowCtx, Origin};
    use crate::record::{Fidelity, Outcome};

    fn flow(id: u64, host: &str, run: Option<&str>, ms: u64, ok: bool) -> FlowRecord {
        FlowRecord {
            id: FlowId::from_parts(id, id as u128),
            ts: chrono::Utc::now(),
            ctx: FlowCtx {
                run_id: run.map(str::to_string),
                step_id: Some("s1".into()),
                agent: Some("reviewer".into()),
                workspace_id: Some("ws".into()),
                origin: Origin::Provider("anthropic".into()),
            },
            fidelity: Fidelity::Http,
            method: "POST".into(),
            scheme: "https".into(),
            host: host.into(),
            port: 443,
            path: "/v1/messages".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: None,
            status: Some(if ok { 200 } else { 500 }),
            outcome: if ok { Outcome::Ok } else { Outcome::HttpError },
            error: None,
            bytes_out: Some(100),
            bytes_in: Some(200),
            body_complete: true,
            ttfb_ms: Some(10),
            duration_ms: Some(ms),
        }
    }

    fn write_lines(path: &std::path::Path, lines: &[LedgerLine]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body: String = lines
            .iter()
            .map(|l| format!("{}\n", serde_json::to_string(l).unwrap()))
            .collect();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn read_flows_folds_complete_into_its_flow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("flows.jsonl");

        let mut streamed = flow(1, "api.anthropic.com", Some("r1"), 0, true);
        streamed.bytes_in = None;
        streamed.body_complete = false;
        let id = streamed.id;

        write_lines(
            &path,
            &[
                LedgerLine::Flow(Box::new(streamed)),
                LedgerLine::Complete {
                    id,
                    bytes_in: 8192,
                    duration_ms: 4200,
                },
            ],
        );

        let flows = read_flows(&path).unwrap();
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].bytes_in, Some(8192));
        assert_eq!(flows[0].duration_ms, Some(4200));
        assert!(flows[0].body_complete);
    }

    #[test]
    fn read_flows_skips_malformed_lines_without_failing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("flows.jsonl");
        std::fs::create_dir_all(tmp.path()).unwrap();
        let good = serde_json::to_string(&LedgerLine::Flow(Box::new(flow(
            1,
            "example.test",
            Some("r1"),
            5,
            true,
        ))))
        .unwrap();
        std::fs::write(&path, format!("{{not json\n{good}\n\n")).unwrap();

        let flows = read_flows(&path).unwrap();
        assert_eq!(flows.len(), 1);
    }

    #[test]
    fn read_flows_on_missing_file_is_empty_not_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let flows = read_flows(&tmp.path().join("absent.jsonl")).unwrap();
        assert!(flows.is_empty());
    }

    #[test]
    fn host_rollup_groups_and_computes_percentiles() {
        let flows = vec![
            flow(1, "api.anthropic.com", Some("r1"), 10, true),
            flow(2, "api.anthropic.com", Some("r1"), 20, true),
            flow(3, "api.anthropic.com", Some("r1"), 100, false),
            flow(4, "api.github.com", Some("r1"), 50, true),
        ];
        let mut rollup = host_rollup(&flows);
        rollup.sort_by(|a, b| a.host.cmp(&b.host));

        assert_eq!(rollup.len(), 2);
        assert_eq!(rollup[0].host, "api.anthropic.com");
        assert_eq!(rollup[0].calls, 3);
        assert_eq!(rollup[0].errors, 1);
        assert_eq!(rollup[0].bytes_in, Some(600));
        assert_eq!(rollup[0].bytes_out, Some(300));
        assert_eq!(rollup[0].p50_ms, 20);
        assert_eq!(rollup[0].p95_ms, 100);
        assert_eq!(rollup[1].host, "api.github.com");
        assert_eq!(rollup[1].calls, 1);
    }

    #[test]
    fn one_unknown_byte_count_makes_the_host_total_unknown() {
        // A Coarse record cannot be summed into a total that claims to
        // be complete. `None` says "we could not see it"; 600 would be
        // a false claim and 0 would be worse.
        let mut coarse = flow(1, "api.github.com", Some("r1"), 10, true);
        coarse.fidelity = Fidelity::Coarse;
        coarse.bytes_in = None;
        coarse.bytes_out = None;

        let flows = vec![flow(2, "api.github.com", Some("r1"), 20, true), coarse];
        let rollup = host_rollup(&flows);

        assert_eq!(rollup[0].calls, 2, "calls are still exact");
        assert_eq!(rollup[0].bytes_in, None);
        assert_eq!(rollup[0].bytes_out, None);
        // Durations are [10, 20]; nearest-rank p50 = ceil(0.5*2) = rank 1
        // = the LOWER of the two. Timings stay exact even though bytes
        // became unknowable.
        assert_eq!(rollup[0].p50_ms, 10, "timings are still exact");
    }

    #[test]
    fn an_unknown_bytes_in_does_not_blank_a_known_bytes_out() {
        // The ordinary in-flight streaming shape: response body still
        // draining, so `bytes_in` is unknown while `bytes_out` (the
        // request we sent) is perfectly well known. A single shared
        // "bytes known" flag would wrongly blank the host's bytes_out
        // total too, discarding data we actually have.
        let mut streaming = flow(1, "api.anthropic.com", Some("r1"), 10, true);
        streaming.bytes_in = None;
        streaming.body_complete = false;

        let flows = vec![flow(2, "api.anthropic.com", Some("r1"), 20, true), streaming];
        let rollup = host_rollup(&flows);

        assert_eq!(rollup[0].bytes_in, None, "one unknown in makes the in-total unknown");
        assert_eq!(
            rollup[0].bytes_out,
            Some(200),
            "every bytes_out was known, so the out-total must survive"
        );
    }

    #[test]
    fn graph_view_is_bipartite_source_to_endpoint() {
        let flows = vec![
            flow(1, "api.anthropic.com", Some("r1"), 10, true),
            flow(2, "api.anthropic.com", Some("r1"), 20, false),
            flow(3, "api.github.com", Some("r2"), 30, true),
        ];
        let g = graph_view(&flows);

        let sources: Vec<_> = g.nodes.iter().filter(|n| n.side == NodeSide::Source).collect();
        let endpoints: Vec<_> = g
            .nodes
            .iter()
            .filter(|n| n.side == NodeSide::Endpoint)
            .collect();
        assert_eq!(sources.len(), 2, "one per run");
        assert_eq!(endpoints.len(), 2, "one per host:port");

        let e = g
            .edges
            .iter()
            .find(|e| e.to == "api.anthropic.com:443")
            .unwrap();
        assert_eq!(e.calls, 2);
        assert_eq!(e.errors, 1);
        assert_eq!(e.bytes, 600);
    }

    #[test]
    fn unattributed_flows_group_under_a_system_source_node() {
        let flows = vec![flow(1, "api.github.com", None, 10, true)];
        let g = graph_view(&flows);
        let source = g.nodes.iter().find(|n| n.side == NodeSide::Source).unwrap();
        assert_eq!(source.id, "system");
    }

    #[test]
    fn read_dropped_total_sums_dropped_lines() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("flows.jsonl");
        write_lines(
            &path,
            &[
                LedgerLine::Dropped {
                    count: 3,
                    ts: chrono::Utc::now(),
                },
                LedgerLine::Dropped {
                    count: 4,
                    ts: chrono::Utc::now(),
                },
            ],
        );
        assert_eq!(read_dropped_total(&path).unwrap(), 7);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-netflow views`
Expected: FAIL to compile — `cannot find function read_flows`.

- [ ] **Step 3: Implement `views.rs` above the tests**

```rust
//! Read-side views over the append-only ledger.
//!
//! Nothing here resolves ASN — enrichment happens in the CP API at
//! render time (spec §6.2), so a dataset that arrives late improves
//! every historical record with no backfill.

use crate::record::{FlowId, FlowRecord, LedgerLine, Outcome};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Read every flow, folding `Complete` lines into their flow.
///
/// A missing file is an empty ledger, not an error. Malformed lines are
/// skipped — a torn write at the tail must not lose the whole history.
pub fn read_flows(path: &Path) -> std::io::Result<Vec<FlowRecord>> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut flows: Vec<FlowRecord> = Vec::new();
    let mut index: HashMap<FlowId, usize> = HashMap::new();

    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<LedgerLine>(&line) else {
            continue;
        };
        match parsed {
            LedgerLine::Flow(f) => {
                index.insert(f.id, flows.len());
                flows.push(*f);
            }
            LedgerLine::Complete {
                id,
                bytes_in,
                duration_ms,
            } => {
                if let Some(&i) = index.get(&id) {
                    flows[i].bytes_in = Some(bytes_in);
                    flows[i].duration_ms = Some(duration_ms);
                    flows[i].body_complete = true;
                }
            }
            LedgerLine::Dropped { .. } => {}
        }
    }

    Ok(flows)
}

/// Total records lost to writer-channel overflow.
pub fn read_dropped_total(path: &Path) -> std::io::Result<u64> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };

    let mut total = 0u64;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if let Ok(LedgerLine::Dropped { count, .. }) = serde_json::from_str::<LedgerLine>(&line) {
            total += count;
        }
    }
    Ok(total)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRollup {
    pub host: String,
    pub port: u16,
    pub calls: u64,
    /// `None` when ANY contributing flow had an unknown byte count — a
    /// `Coarse` record cannot be summed into a total that claims to be
    /// complete. Summing `unwrap_or(0)` would silently turn "we could
    /// not see it" into "it was zero".
    pub bytes_in: Option<u64>,
    pub bytes_out: Option<u64>,
    pub errors: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
}

fn is_error(f: &FlowRecord) -> bool {
    !matches!(f.outcome, Outcome::Ok)
}

/// Nearest-rank percentile — the standard definition: the smallest value
/// at or below which at least `pct` of the samples fall, i.e. index
/// `ceil(pct * n) - 1`. `pct` in 0.0..=1.0. `sorted` must be ascending.
///
/// Do NOT "fix" this to `ceil(pct * (n - 1))` to make a test pass. That
/// is a different, nonstandard definition; if a test disagrees with
/// nearest-rank, the test's expected value is what is wrong.
fn percentile(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (pct * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

/// `bytes_in` and `bytes_out` are tracked INDEPENDENTLY. A single shared
/// "known" flag would be wrong: an in-flight streamed flow has a known
/// `bytes_out` and an unknown `bytes_in`, and one shared flag would blank
/// the host's `bytes_out` total too — discarding data we actually have.
struct RollupAcc {
    calls: u64,
    /// `None` once any contributor's own `bytes_in` is unknown.
    bytes_in: Option<u64>,
    /// `None` once any contributor's own `bytes_out` is unknown.
    bytes_out: Option<u64>,
    errors: u64,
    ms: Vec<u64>,
}

impl Default for RollupAcc {
    fn default() -> Self {
        Self {
            calls: 0,
            bytes_in: Some(0),
            bytes_out: Some(0),
            errors: 0,
            ms: Vec::new(),
        }
    }
}

/// Add one flow's contribution, collapsing to `None` on its own gap only.
fn accumulate(total: Option<u64>, sample: Option<u64>) -> Option<u64> {
    match (total, sample) {
        (Some(a), Some(b)) => Some(a + b),
        _ => None,
    }
}

pub fn host_rollup(flows: &[FlowRecord]) -> Vec<HostRollup> {
    let mut acc: HashMap<(String, u16), RollupAcc> = HashMap::new();

    for f in flows {
        let entry = acc
            .entry((f.host.clone(), f.port))
            .or_insert_with(RollupAcc::default);
        entry.calls += 1;
        // Per-field: one unobservable contributor makes THAT total
        // unknowable, and only that one.
        entry.bytes_in = accumulate(entry.bytes_in, f.bytes_in);
        entry.bytes_out = accumulate(entry.bytes_out, f.bytes_out);
        if is_error(f) {
            entry.errors += 1;
        }
        if let Some(ms) = f.duration_ms {
            entry.ms.push(ms);
        }
    }

    acc.into_iter()
        .map(|((host, port), mut a)| {
            a.ms.sort_unstable();
            HostRollup {
                host,
                port,
                calls: a.calls,
                bytes_in: a.bytes_in,
                bytes_out: a.bytes_out,
                errors: a.errors,
                p50_ms: percentile(&a.ms, 0.50),
                p95_ms: percentile(&a.ms, 0.95),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeSide {
    Source,
    Endpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub side: NodeSide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub calls: u64,
    pub bytes: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphView {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Bipartite topology: sources (runs, or `system` for unattributed
/// process-global egress) on one side, `host:port` endpoints on the other.
pub fn graph_view(flows: &[FlowRecord]) -> GraphView {
    let mut nodes: HashMap<String, GraphNode> = HashMap::new();
    let mut edges: HashMap<(String, String), GraphEdge> = HashMap::new();

    for f in flows {
        let source_id = f.ctx.run_id.clone().unwrap_or_else(|| "system".to_string());
        let endpoint_id = format!("{}:{}", f.host, f.port);

        nodes.entry(source_id.clone()).or_insert_with(|| GraphNode {
            id: source_id.clone(),
            label: source_id.clone(),
            side: NodeSide::Source,
        });
        nodes
            .entry(endpoint_id.clone())
            .or_insert_with(|| GraphNode {
                id: endpoint_id.clone(),
                label: f.host.clone(),
                side: NodeSide::Endpoint,
            });

        let edge = edges
            .entry((source_id.clone(), endpoint_id.clone()))
            .or_insert_with(|| GraphEdge {
                from: source_id.clone(),
                to: endpoint_id.clone(),
                calls: 0,
                bytes: 0,
                errors: 0,
            });
        edge.calls += 1;
        // Edge weight is a VISUAL scale for stroke thickness, not a
        // reported total — unlike `HostRollup::bytes_in`, which must
        // stay `None` when unknown. A Coarse flow contributes 0 here,
        // so its edge simply renders thin. Never surface this number
        // as a byte count in a table.
        edge.bytes += f.bytes_in.unwrap_or(0) + f.bytes_out.unwrap_or(0);
        if is_error(f) {
            edge.errors += 1;
        }
    }

    GraphView {
        nodes: nodes.into_values().collect(),
        edges: edges.into_values().collect(),
    }
}
```

- [ ] **Step 4: Export from `ledger/mod.rs`**

```rust
pub mod views;
pub use views::{
    graph_view, host_rollup, read_dropped_total, read_flows, GraphEdge, GraphNode, GraphView,
    HostRollup, NodeSide,
};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-netflow`
Expected: PASS — 18 tests.

- [ ] **Step 6: Verify lints**

Run: `cargo clippy -p rupu-netflow --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/ledger/views.rs crates/rupu-netflow/src/ledger/mod.rs
git add crates/rupu-netflow/
git commit -m "feat(netflow): ledger read views, host rollup and bipartite graph"
```

---

### Task 5: ASN table — compaction and lookup

**Files:**
- Create: `crates/rupu-netflow/src/asn/mod.rs`
- Create: `crates/rupu-netflow/src/asn/table.rs`
- Modify: `crates/rupu-netflow/src/lib.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `AsnInfo { asn: u32, org: String }`; `AsnTable` with `compact_from_tsv<R: BufRead>(R) -> io::Result<AsnTable>`, `write(&self, &Path) -> io::Result<()>`, `load(&Path) -> io::Result<AsnTable>`, `lookup(&self, IpAddr) -> Option<AsnInfo>`. Task 7 writes the table; Plan 3's CP API calls `lookup`.

The source format is iptoasn.com's `ip2asn-combined.tsv`: `range_start \t range_end \t AS_number \t country_code \t AS_description`. Unrouted ranges carry AS number `0` and description `Not routed` — those are skipped.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-netflow/src/asn/table.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const TSV: &str = "\
1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET
1.0.1.0\t1.0.3.255\t0\tNone\tNot routed
8.8.8.0\t8.8.8.255\t15169\tUS\tGOOGLE
2606:4700::\t2606:4700:ffff:ffff:ffff:ffff:ffff:ffff\t13335\tUS\tCLOUDFLARENET
";

    fn table() -> AsnTable {
        AsnTable::compact_from_tsv(Cursor::new(TSV)).unwrap()
    }

    #[test]
    fn looks_up_an_ipv4_inside_a_range() {
        let t = table();
        let got = t.lookup("1.0.0.42".parse().unwrap()).unwrap();
        assert_eq!(got.asn, 13335);
        assert_eq!(got.org, "CLOUDFLARENET");
    }

    #[test]
    fn range_boundaries_are_inclusive() {
        let t = table();
        assert!(t.lookup("1.0.0.0".parse().unwrap()).is_some());
        assert!(t.lookup("1.0.0.255".parse().unwrap()).is_some());
    }

    #[test]
    fn unrouted_ranges_are_not_indexed() {
        let t = table();
        assert!(t.lookup("1.0.2.1".parse().unwrap()).is_none());
    }

    #[test]
    fn unmapped_address_returns_none() {
        let t = table();
        assert!(t.lookup("192.0.2.1".parse().unwrap()).is_none());
    }

    #[test]
    fn looks_up_an_ipv6_address() {
        let t = table();
        let got = t.lookup("2606:4700::1111".parse().unwrap()).unwrap();
        assert_eq!(got.asn, 13335);
    }

    #[test]
    fn write_then_load_round_trips() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("asn.db");
        table().write(&path).unwrap();

        let loaded = AsnTable::load(&path).unwrap();
        assert_eq!(loaded.lookup("8.8.8.8".parse().unwrap()).unwrap().asn, 15169);
        assert_eq!(loaded.len(), table().len());
    }

    #[test]
    fn load_of_a_missing_file_is_an_error_callers_can_degrade_on() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = AsnTable::load(&tmp.path().join("absent.db")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn malformed_tsv_rows_are_skipped_not_fatal() {
        let t = AsnTable::compact_from_tsv(Cursor::new(
            "garbage\n1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n\n",
        ))
        .unwrap();
        assert_eq!(t.len(), 1);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-netflow asn`
Expected: FAIL to compile — `cannot find struct AsnTable`.

- [ ] **Step 3: Implement `table.rs` above the tests**

The on-disk form is JSON, not a bespoke binary: the compacted table is a few hundred thousand rows, `serde_json` reads it in well under a second, and a hand-rolled binary format would be a second thing to version and get wrong.

```rust
//! IP → ASN range table.
//!
//! Sorted ranges + binary search. v4 and v6 are held separately so the
//! v4 path compares `u32` rather than widening every address.

use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::net::IpAddr;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsnInfo {
    pub asn: u32,
    pub org: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Range<T> {
    start: T,
    end: T,
    asn: u32,
    /// Index into `AsnTable::orgs`.
    org: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AsnTable {
    v4: Vec<Range<u32>>,
    v6: Vec<Range<u128>>,
    orgs: Vec<String>,
}

impl AsnTable {
    pub fn len(&self) -> usize {
        self.v4.len() + self.v6.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Build from iptoasn.com's `ip2asn-combined.tsv`.
    ///
    /// Columns: start, end, AS number, country, description. Rows with
    /// AS number 0 are unrouted and are skipped. Malformed rows are
    /// skipped rather than failing the whole ingest.
    pub fn compact_from_tsv<R: BufRead>(reader: R) -> std::io::Result<Self> {
        let mut table = AsnTable::default();
        let mut org_index: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

        for line in reader.lines() {
            let line = line?;
            let mut cols = line.split('\t');
            let (Some(start), Some(end), Some(asn), Some(_cc), Some(desc)) = (
                cols.next(),
                cols.next(),
                cols.next(),
                cols.next(),
                cols.next(),
            ) else {
                continue;
            };
            let Ok(asn) = asn.parse::<u32>() else {
                continue;
            };
            if asn == 0 {
                continue;
            }
            let (Ok(start), Ok(end)) = (start.parse::<IpAddr>(), end.parse::<IpAddr>()) else {
                continue;
            };

            let next_idx = org_index.len() as u32;
            let org = *org_index.entry(desc.to_string()).or_insert(next_idx);
            if org as usize == table.orgs.len() {
                table.orgs.push(desc.to_string());
            }

            match (start, end) {
                (IpAddr::V4(s), IpAddr::V4(e)) => table.v4.push(Range {
                    start: s.into(),
                    end: e.into(),
                    asn,
                    org,
                }),
                (IpAddr::V6(s), IpAddr::V6(e)) => table.v6.push(Range {
                    start: s.into(),
                    end: e.into(),
                    asn,
                    org,
                }),
                _ => continue,
            }
        }

        table.v4.sort_unstable_by_key(|r| r.start);
        table.v6.sort_unstable_by_key(|r| r.start);
        Ok(table)
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("db.tmp");
        let json = serde_json::to_vec(self)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn lookup(&self, ip: IpAddr) -> Option<AsnInfo> {
        match ip {
            IpAddr::V4(a) => find(&self.v4, u32::from(a)).map(|r| self.info(r.asn, r.org)),
            IpAddr::V6(a) => find(&self.v6, u128::from(a)).map(|r| self.info(r.asn, r.org)),
        }
    }

    fn info(&self, asn: u32, org: u32) -> AsnInfo {
        AsnInfo {
            asn,
            org: self
                .orgs
                .get(org as usize)
                .cloned()
                .unwrap_or_else(|| format!("AS{asn}")),
        }
    }
}

/// Last range whose `start <= needle`, then an inclusive `end` check.
fn find<T: Ord + Copy>(ranges: &[Range<T>], needle: T) -> Option<&Range<T>> {
    let idx = ranges.partition_point(|r| r.start <= needle);
    let candidate = ranges.get(idx.checked_sub(1)?)?;
    (needle <= candidate.end).then_some(candidate)
}
```

- [ ] **Step 4: Write `asn/mod.rs`**

```rust
pub mod table;
pub use table::{AsnInfo, AsnTable};
```

- [ ] **Step 5: Export from `lib.rs`**

```rust
pub mod asn;
pub use asn::{AsnInfo, AsnTable};
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p rupu-netflow`
Expected: PASS — 26 tests.

- [ ] **Step 7: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/asn/mod.rs crates/rupu-netflow/src/asn/table.rs crates/rupu-netflow/src/lib.rs
git add crates/rupu-netflow/
git commit -m "feat(netflow): ASN range table with TSV ingest and binary-search lookup"
```

---

### Task 6: `NetflowConfig` in `rupu-config`

**Files:**
- Modify: `crates/rupu-config/src/lib.rs` (or the module that defines `CpConfig` — locate it with `grep -rn "struct CpConfig" crates/rupu-config/src`)
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `NetflowConfig { asn_auto_refresh: bool, asn_refresh_interval_days: u64, asn_source_url: String }` with `Default`, reachable as the `netflow` field on the top-level config struct. Task 7 and Plan 2's `cp serve` tick read it.

- [ ] **Step 1: Locate the pattern to copy**

Run: `grep -rn "struct CpConfig" -A 30 crates/rupu-config/src`
Read how `CpConfig` declares serde defaults and how it is hung off the root config struct. Mirror it exactly — including whether defaults come from `#[serde(default = "...")]` functions or a `Default` impl.

Also run `grep -n "^toml" crates/rupu-config/Cargo.toml` — the test below parses TOML directly. If `toml` is only a normal dependency and not a dev-dependency, that is fine; if it is absent entirely, add `toml.workspace = true` under `[dev-dependencies]`.

- [ ] **Step 2: Write the failing test**

Add to the same file's test module:

```rust
#[test]
fn netflow_defaults_are_auto_refresh_weekly() {
    let cfg = NetflowConfig::default();
    assert!(cfg.asn_auto_refresh);
    assert_eq!(cfg.asn_refresh_interval_days, 7);
    assert!(cfg.asn_source_url.contains("iptoasn.com"));
}

#[test]
fn netflow_section_parses_from_toml() {
    let toml = r#"
[netflow]
asn_auto_refresh = false
asn_refresh_interval_days = 30
"#;
    let cfg: NetflowConfig = toml::from_str::<toml::Value>(toml)
        .unwrap()
        .get("netflow")
        .unwrap()
        .clone()
        .try_into()
        .unwrap();
    assert!(!cfg.asn_auto_refresh);
    assert_eq!(cfg.asn_refresh_interval_days, 30);
    // Unspecified keys still take their default.
    assert!(cfg.asn_source_url.contains("iptoasn.com"));
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-config netflow`
Expected: FAIL to compile — `cannot find struct NetflowConfig`.

- [ ] **Step 4: Implement**

```rust
/// `[netflow]` — network egress observability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NetflowConfig {
    /// Acquire and refresh the ASN table automatically. The operator
    /// should never have to run a command to get enrichment working.
    pub asn_auto_refresh: bool,
    /// Refresh cadence. BGP prefixes move slowly; weekly is ample.
    pub asn_refresh_interval_days: u64,
    /// Source for the combined IPv4+IPv6 prefix→ASN table.
    pub asn_source_url: String,
}

impl Default for NetflowConfig {
    fn default() -> Self {
        Self {
            asn_auto_refresh: true,
            asn_refresh_interval_days: 7,
            asn_source_url: "https://iptoasn.com/data/ip2asn-combined.tsv.gz".to_string(),
        }
    }
}
```

Then add the field to the root config struct alongside `cp`:

```rust
    #[serde(default)]
    pub netflow: NetflowConfig,
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-config`
Expected: PASS, including the two new tests.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-config/src/lib.rs
git add crates/rupu-config/
git commit -m "feat(config): [netflow] section for automatic ASN refresh"
```

---

### Task 7: Automatic ASN acquisition

**Files:**
- Create: `crates/rupu-netflow/src/asn/acquire.rs`
- Modify: `crates/rupu-netflow/src/asn/mod.rs`
- Modify: `crates/rupu-netflow/Cargo.toml` (add `flate2` — the source is `.tsv.gz`)
- Modify: `Cargo.toml` (workspace dep `flate2 = "1"` if absent)

**Interfaces:**
- Consumes: `AsnTable` (Task 5).
- Produces: `asn_db_path() -> Option<PathBuf>`; `is_stale(&Path, u64) -> bool`; `ingest_gz<R: Read>(R) -> io::Result<AsnTable>`; `refresh(url: &str, dest: &Path, client: &reqwest_middleware::ClientWithMiddleware) -> Result<(), AsnError>`. Plan 2's `cp serve` tick calls `refresh`.

`refresh` is gated behind the `http` feature; `is_stale`, `asn_db_path` and `ingest_gz` are not, so the freshness check compiles without an HTTP stack.

- [ ] **Step 1: Add `flate2`**

Root `Cargo.toml` under `[workspace.dependencies]` (skip if present):

```toml
flate2 = "1"
```

`crates/rupu-netflow/Cargo.toml` under `[dependencies]`:

```toml
flate2.workspace = true
```

- [ ] **Step 2: Write the failing tests**

Create `crates/rupu-netflow/src/asn/acquire.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const TSV: &str = "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n";

    fn gzipped(body: &str) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(body.as_bytes()).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn ingest_gz_decompresses_and_builds_the_table() {
        let table = ingest_gz(std::io::Cursor::new(gzipped(TSV))).unwrap();
        assert_eq!(table.lookup("1.0.0.7".parse().unwrap()).unwrap().asn, 13335);
    }

    #[test]
    fn a_missing_db_is_stale() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(is_stale(&tmp.path().join("absent.db"), 7));
    }

    #[test]
    fn a_freshly_written_db_is_not_stale() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("asn.db");
        std::fs::write(&path, b"{}").unwrap();
        assert!(!is_stale(&path, 7));
    }

    #[test]
    fn a_zero_day_interval_makes_everything_stale() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("asn.db");
        std::fs::write(&path, b"{}").unwrap();
        assert!(is_stale(&path, 0));
    }

    #[tokio::test]
    async fn refresh_downloads_decompresses_and_writes_the_table() {
        let server = httpmock::MockServer::start_async().await;
        let body = gzipped(TSV);
        let mock = server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET).path("/ip2asn.tsv.gz");
                then.status(200).body(body.clone());
            })
            .await;

        let tmp = tempfile::TempDir::new().unwrap();
        let dest = tmp.path().join("asn.db");
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();

        refresh(&server.url("/ip2asn.tsv.gz"), &dest, &client)
            .await
            .unwrap();

        mock.assert_async().await;
        let table = AsnTable::load(&dest).unwrap();
        assert_eq!(table.lookup("1.0.0.7".parse().unwrap()).unwrap().asn, 13335);
    }

    #[tokio::test]
    async fn refresh_leaves_an_existing_db_intact_when_the_source_fails() {
        let server = httpmock::MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET).path("/bad");
                then.status(500);
            })
            .await;

        let tmp = tempfile::TempDir::new().unwrap();
        let dest = tmp.path().join("asn.db");
        AsnTable::default().write(&dest).unwrap();
        let before = std::fs::read(&dest).unwrap();

        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
        let err = refresh(&server.url("/bad"), &dest, &client).await;

        assert!(err.is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), before);
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-netflow --features http acquire`
Expected: FAIL to compile — `cannot find function ingest_gz`.

- [ ] **Step 4: Implement `acquire.rs` above the tests**

```rust
//! Automatic acquisition of the ASN table.
//!
//! The operator never has to run a command. Two triggers drive this:
//! the `rupu cp serve` sweep loop, and any netflow read that finds the
//! DB missing or stale (Plan 2). Failure is always best-effort — log
//! once, never block a read, never block a request.

use crate::asn::table::AsnTable;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, thiserror::Error)]
pub enum AsnError {
    #[error("asn source returned HTTP {0}")]
    Status(u16),
    #[error("asn source request failed: {0}")]
    Transport(String),
    #[error("asn table io: {0}")]
    Io(#[from] std::io::Error),
}

/// `~/.rupu/netflow/asn.db`. `None` when the home directory is unknown.
pub fn asn_db_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".rupu").join("netflow").join("asn.db"))
}

/// True when the table is absent, unreadable, or older than the interval.
/// An interval of 0 means always refresh.
pub fn is_stale(path: &Path, max_age_days: u64) -> bool {
    if max_age_days == 0 {
        return true;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return true;
    };
    let Ok(modified) = meta.modified() else {
        return true;
    };
    let max_age = Duration::from_secs(max_age_days.saturating_mul(86_400));
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age > max_age)
        .unwrap_or(true)
}

/// Decompress a gzipped iptoasn TSV into a compacted table.
pub fn ingest_gz<R: Read>(reader: R) -> std::io::Result<AsnTable> {
    let decoder = flate2::read::GzDecoder::new(reader);
    AsnTable::compact_from_tsv(BufReader::new(decoder))
}

/// Download, decompress and atomically replace the table at `dest`.
///
/// On any failure the existing table is left untouched — a failed
/// refresh must never degrade enrichment that already works.
#[cfg(feature = "http")]
pub async fn refresh(
    url: &str,
    dest: &Path,
    client: &reqwest_middleware::ClientWithMiddleware,
) -> Result<(), AsnError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| AsnError::Transport(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(AsnError::Status(status.as_u16()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AsnError::Transport(e.to_string()))?;

    let table = ingest_gz(std::io::Cursor::new(bytes))?;
    table.write(dest)?;
    Ok(())
}
```

- [ ] **Step 5: Export from `asn/mod.rs`**

```rust
pub mod acquire;
pub use acquire::{asn_db_path, ingest_gz, is_stale, AsnError};
#[cfg(feature = "http")]
pub use acquire::refresh;
```

- [ ] **Step 6: Run tests both ways**

Run: `cargo test -p rupu-netflow --features http`
Expected: PASS — 32 tests.

Run: `cargo test -p rupu-netflow`
Expected: PASS — the `http`-gated tests are compiled out; the rest still pass. This proves the default feature set carries no HTTP dependency.

- [ ] **Step 7: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/asn/acquire.rs crates/rupu-netflow/src/asn/mod.rs
git add Cargo.toml crates/rupu-netflow/
git commit -m "feat(netflow): automatic ASN table acquisition with atomic replace"
```

---

### Task 8: The instrumented client and middleware

**Files:**
- Create: `crates/rupu-netflow/src/http/mod.rs`
- Create: `crates/rupu-netflow/src/http/middleware.rs`
- Create: `crates/rupu-netflow/src/http/resolver.rs`
- Create: `crates/rupu-netflow/tests/capture.rs`
- Modify: `crates/rupu-netflow/src/lib.rs`

**Interfaces:**
- Consumes: `FlowCtx`, `FlowRecord`, `FlowId`, `Fidelity`, `Outcome`, `FlowSink` (Tasks 1–2).
- Produces: `init(Arc<dyn FlowSink>)`; `sink() -> Arc<dyn FlowSink>`; `client(FlowCtx) -> ClientWithMiddleware`; `client_from(FlowCtx, reqwest::ClientBuilder) -> reqwest::Result<ClientWithMiddleware>`; `complete(FlowId, u64, u64)`; `NetflowMiddleware`. Task 10 migrates `rupu-providers` onto `client_from`.

- [ ] **Step 1: Write the failing integration test**

Create `crates/rupu-netflow/tests/capture.rs`:

```rust
#![cfg(feature = "http")]

use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

#[tokio::test]
async fn records_a_successful_request() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200).body("hello");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx {
            run_id: Some("run-7".into()),
            step_id: Some("step-2".into()),
            agent: Some("reviewer".into()),
            workspace_id: Some("ws".into()),
            origin: Origin::Provider("anthropic".into()),
        },
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    let resp = client.get(server.url("/v1/models")).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let records = sink.records();
    assert_eq!(records.len(), 1);
    let r = &records[0];
    assert_eq!(r.method, "GET");
    assert_eq!(r.path, "/v1/models");
    assert_eq!(r.status, Some(200));
    assert_eq!(r.ctx.run_id.as_deref(), Some("run-7"));
    assert!(matches!(r.outcome, rupu_netflow::Outcome::Ok));
    assert!(r.peer_ip.is_some(), "remote_addr must be captured");
    assert!(r.ttfb_ms.is_some());
}

#[tokio::test]
async fn never_stores_the_query_string() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/search");
            then.status(200);
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Update),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    let url = format!("{}?api_key=SUPERSECRET&q=x", server.url("/search"));
    client.get(url).send().await.unwrap();

    let r = &sink.records()[0];
    assert_eq!(r.path, "/search");
    assert!(!r.path.contains('?'));
    let json = serde_json::to_string(r).unwrap();
    assert!(
        !json.contains("SUPERSECRET"),
        "no part of the record may carry query values"
    );
}

#[tokio::test]
async fn records_an_http_error_status_as_http_error() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/boom");
            then.status(503);
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Cp),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    client.get(server.url("/boom")).send().await.unwrap();

    let r = &sink.records()[0];
    assert_eq!(r.status, Some(503));
    assert!(matches!(r.outcome, rupu_netflow::Outcome::HttpError));
}

#[tokio::test]
async fn records_a_transport_failure_with_no_status() {
    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::System),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    // Port 1 on loopback refuses connections.
    let result = client.get("http://127.0.0.1:1/nope").send().await;
    assert!(result.is_err());

    let r = &sink.records()[0];
    assert_eq!(r.status, None);
    assert!(matches!(
        r.outcome,
        rupu_netflow::Outcome::TransportError | rupu_netflow::Outcome::Timeout
    ));
    assert!(r.error.is_some());
}

#[tokio::test]
async fn a_caller_supplied_flow_id_is_used_so_it_can_complete_later() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200).body("stream");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Provider("anthropic".into())),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    let id = rupu_netflow::FlowId::from_parts(42, 42);
    client
        .post(server.url("/v1/messages"))
        .with_extension(id)
        .send()
        .await
        .unwrap();

    let records = sink.records();
    assert_eq!(records[0].id, id, "middleware must honour the caller's id");
    assert!(!records[0].body_complete, "a streamed body is not complete at header time");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-netflow --features http --test capture`
Expected: FAIL to compile — `could not find http in rupu_netflow`.

- [ ] **Step 3: Implement `resolver.rs`**

```rust
//! DNS resolver that records every answer.
//!
//! Delegates to the system resolver via `tokio::net::lookup_host` — the
//! same `getaddrinfo` path reqwest's default `GaiResolver` uses — and
//! keeps the most recent answer per host so the middleware can attach
//! `resolved_ips`.
//!
//! Caveat, deliberately not papered over: under concurrent requests to
//! the SAME host the map holds the latest answer, so `resolved_ips` is
//! "the most recent resolution for this host", not "the resolution this
//! request used". `peer_ip` is always exact — prefer it when the two
//! disagree.

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct RecordingResolver {
    seen: Arc<Mutex<HashMap<String, Vec<IpAddr>>>>,
}

impl RecordingResolver {
    pub fn answers_for(&self, host: &str) -> Vec<IpAddr> {
        self.seen
            .lock()
            .ok()
            .and_then(|m| m.get(host).cloned())
            .unwrap_or_default()
    }
}

impl Resolve for RecordingResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let seen = self.seen.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if let Ok(mut m) = seen.lock() {
                m.insert(host, addrs.iter().map(|s| s.ip()).collect());
            }
            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}
```

- [ ] **Step 4: Implement `middleware.rs`**

```rust
//! The capture middleware.
//!
//! Hard rule: this code must never panic and never fail a request. Every
//! fallible step degrades to a less complete record.

use crate::ctx::FlowCtx;
use crate::http::resolver::RecordingResolver;
use crate::record::{Fidelity, FlowId, FlowRecord, Outcome};
use crate::sink::FlowSink;
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MwResult};
use std::sync::Arc;
use std::time::Instant;

pub struct NetflowMiddleware {
    pub(crate) ctx: FlowCtx,
    pub(crate) sink: Arc<dyn FlowSink>,
    pub(crate) resolver: RecordingResolver,
}

#[async_trait::async_trait]
impl Middleware for NetflowMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> MwResult<Response> {
        // The caller may mint the id itself (via `with_extension`) so it
        // can finalize a streamed body later. Otherwise generate one.
        let id = extensions.get::<FlowId>().copied().unwrap_or_else(FlowId::new);

        let url = req.url().clone();
        let method = req.method().to_string();
        let host = url.host_str().unwrap_or_default().to_string();
        let port = url.port_or_known_default().unwrap_or(0);
        let scheme = url.scheme().to_string();
        // Query-stripped. Query strings routinely carry tokens.
        let path = url.path().to_string();
        let bytes_out = req.body().and_then(|b| b.as_bytes()).map(|b| b.len() as u64);

        let started = Instant::now();
        let result = next.run(req, extensions).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;

        let mut record = FlowRecord {
            id,
            ts: chrono::Utc::now(),
            ctx: self.ctx.clone(),
            fidelity: Fidelity::Http,
            method,
            scheme,
            resolved_ips: self.resolver.answers_for(&host),
            host,
            port,
            path,
            peer_ip: None,
            http_version: None,
            status: None,
            outcome: Outcome::TransportError,
            error: None,
            bytes_out,
            bytes_in: None,
            body_complete: false,
            ttfb_ms: Some(elapsed_ms),
            duration_ms: None,
        };

        match &result {
            Ok(resp) => {
                record.status = Some(resp.status().as_u16());
                record.http_version = Some(format!("{:?}", resp.version()));
                record.peer_ip = resp.remote_addr().map(|a| a.ip());
                record.outcome = if resp.status().is_success() || resp.status().is_redirection() {
                    Outcome::Ok
                } else {
                    Outcome::HttpError
                };
                // Known length means the body is fully accounted for now;
                // a streamed body is finalized later via `complete`.
                if let Some(len) = resp.content_length() {
                    record.bytes_in = Some(len);
                    record.body_complete = true;
                    record.duration_ms = Some(elapsed_ms);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                record.outcome = if msg.to_ascii_lowercase().contains("timed out")
                    || msg.to_ascii_lowercase().contains("timeout")
                {
                    Outcome::Timeout
                } else {
                    Outcome::TransportError
                };
                record.error = Some(msg);
                record.duration_ms = Some(elapsed_ms);
                record.body_complete = true;
            }
        }

        self.sink.record(record).await;
        result
    }
}
```

- [ ] **Step 5: Implement `http/mod.rs`**

```rust
//! The instrumented client — the ONE door for rupu's outbound HTTP.
//!
//! `clippy.toml` denies `reqwest::Client::new` and `::builder` outside
//! this crate (Task 11), so this factory cannot be bypassed by accident.

pub mod middleware;
pub mod resolver;

use crate::ctx::FlowCtx;
use crate::record::FlowId;
use crate::sink::{FlowSink, NullSink};
use middleware::NetflowMiddleware;
use resolver::RecordingResolver;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use std::sync::{Arc, OnceLock};

static SINK: OnceLock<Arc<dyn FlowSink>> = OnceLock::new();

/// Install the process-wide sink. First call wins; later calls are
/// ignored so a test or a second runtime cannot silently re-point it.
pub fn init(sink: Arc<dyn FlowSink>) {
    let _ = SINK.set(sink);
}

/// The process-wide sink, or a no-op one when capture was never
/// initialised (a library consumer, or a unit test).
pub fn sink() -> Arc<dyn FlowSink> {
    SINK.get().cloned().unwrap_or_else(|| Arc::new(NullSink))
}

/// An instrumented client with default settings.
pub fn client(ctx: FlowCtx) -> ClientWithMiddleware {
    client_from(ctx, default_builder())
        .unwrap_or_else(|_| ClientBuilder::new(reqwest::Client::new()).build())
}

/// Start from a caller-tuned builder — timeouts, `http1_only`, proxies.
/// The resolver is installed here, so callers must not set their own.
pub fn client_from(
    ctx: FlowCtx,
    builder: reqwest::ClientBuilder,
) -> reqwest::Result<ClientWithMiddleware> {
    client_with(ctx, builder, sink())
}

/// As `client_from`, with an explicit sink. Used by tests.
pub fn client_with(
    ctx: FlowCtx,
    builder: reqwest::ClientBuilder,
    sink: Arc<dyn FlowSink>,
) -> reqwest::Result<ClientWithMiddleware> {
    let resolver = RecordingResolver::default();
    let inner = builder.dns_resolver(Arc::new(resolver.clone())).build()?;
    Ok(ClientBuilder::new(inner)
        .with(NetflowMiddleware {
            ctx,
            sink,
            resolver,
        })
        .build())
}

#[allow(clippy::disallowed_methods)]
fn default_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
}

/// Finalize a streamed body recorded earlier at header time.
///
/// The caller minted the `FlowId` and attached it with
/// `RequestBuilder::with_extension`, so it already knows which record to
/// close. See the spec §7.2 — the alternative is estimating byte counts,
/// which this design does not do.
pub async fn complete(id: FlowId, bytes_in: u64, duration_ms: u64) {
    sink().complete(id, bytes_in, duration_ms).await;
}
```

- [ ] **Step 6: Export from `lib.rs`**

```rust
#[cfg(feature = "http")]
pub mod http;
```

- [ ] **Step 7: Run the integration tests**

Run: `cargo test -p rupu-netflow --features http --test capture`
Expected: PASS — 5 tests.

- [ ] **Step 8: Verify the default feature set still builds without HTTP**

Run: `cargo build -p rupu-netflow`
Expected: success, and `cargo tree -p rupu-netflow | grep reqwest` prints nothing.

- [ ] **Step 9: Verify lints**

Run: `cargo clippy -p rupu-netflow --all-features --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 10: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/http/mod.rs crates/rupu-netflow/src/http/middleware.rs crates/rupu-netflow/src/http/resolver.rs crates/rupu-netflow/tests/capture.rs crates/rupu-netflow/src/lib.rs
git add crates/rupu-netflow/
git commit -m "feat(netflow): instrumented reqwest client with capture middleware"
```

---

### Task 9: Two-phase completion for streamed bodies

**Files:**
- Modify: `crates/rupu-netflow/tests/capture.rs`
- Modify: `crates/rupu-netflow/src/ledger/writer.rs` (no code change expected — verify only)

**Interfaces:**
- Consumes: `complete` (Task 8), `read_flows` (Task 4), `NetflowWriterHandle` (Task 3).
- Produces: no new API. This task proves the header-time record plus a later `Complete` folds into one finished flow end to end.

- [ ] **Step 1: Write the failing end-to-end test**

Append to `crates/rupu-netflow/tests/capture.rs`:

```rust
#[tokio::test]
async fn a_streamed_body_is_finalized_through_the_ledger() {
    use rupu_netflow::{NetflowPaths, NetflowWriterHandle};

    let server = httpmock::MockServer::start_async().await;
    // Chunked: no Content-Length, so the middleware cannot know the size
    // at header time. This is the SSE shape every provider chat path has.
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("transfer-encoding", "chunked")
                .body("data: one\n\ndata: two\n\n");
        })
        .await;

    let tmp = tempfile::TempDir::new().unwrap();
    let paths = NetflowPaths::new(tmp.path());
    let handle = NetflowWriterHandle::spawn(paths.clone()).unwrap();

    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Provider("anthropic".into())),
        reqwest::Client::builder(),
        handle.writer.clone(),
    )
    .unwrap();

    let id = rupu_netflow::FlowId::new();
    let mut resp = client
        .post(server.url("/v1/messages"))
        .with_extension(id)
        .send()
        .await
        .unwrap();

    // Drain exactly the way the provider's SSE loop does.
    let started = std::time::Instant::now();
    let mut total = 0u64;
    while let Some(chunk) = resp.chunk().await.unwrap() {
        total += chunk.len() as u64;
    }
    handle
        .writer
        .complete(id, total, started.elapsed().as_millis() as u64)
        .await;
    handle.shutdown().await;

    let flows = rupu_netflow::ledger::read_flows(&paths.flows).unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].id, id);
    assert_eq!(flows[0].bytes_in, Some(total));
    assert!(total > 0);
    assert!(flows[0].body_complete, "Complete must fold into the flow");
}
```

You will need `use rupu_netflow::FlowSink;` at the top of the test file for `handle.writer.complete(...)` to resolve.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-netflow --features http --test capture streamed`
Expected: FAIL — either a compile error on the missing `FlowSink` import, or an assertion failure if `Complete` folding is wrong.

- [ ] **Step 3: Fix whatever the test exposes**

Most likely fixes, in order of likelihood:
1. Add the `FlowSink` import to the test.
2. `crates/rupu-netflow/src/lib.rs` does not re-export `ledger::read_flows` — add it.
3. `httpmock` sets a `Content-Length` despite the chunked header, so the middleware marks `body_complete: true` at header time. If so, the `Complete` line must still win: confirm `read_flows` overwrites `bytes_in`/`duration_ms` unconditionally when a `Complete` matches (it does — that is the fold's job), and keep the test asserting the final folded value.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-netflow --features http`
Expected: PASS — all tests.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/tests/capture.rs
git add crates/rupu-netflow/
git commit -m "test(netflow): end-to-end two-phase completion for streamed bodies"
```

---

### Task 10: Migrate `rupu-providers` onto the instrumented client

**Files:**
- Modify: `crates/rupu-providers/Cargo.toml` (add `rupu-netflow` with the `http` feature)
- Modify: `crates/rupu-providers/src/tuning.rs:89-95` (`http_client_builder`)
- Modify: `crates/rupu-providers/src/anthropic.rs:264-290` (`build_http_client`, `build_http_client_with_timeout`)
- Modify: `crates/rupu-providers/src/anthropic.rs:1316-1345` (the SSE chunk loop)
- Modify: `crates/rupu-providers/src/local.rs:25-40`, `openai_compatible.rs`, `github_copilot.rs`, `openai_codex.rs`, `google_gemini.rs`, `broker_client.rs`
- Test: `crates/rupu-providers/tests/netflow_capture.rs` (create)

**Interfaces:**
- Consumes: `client_from`, `client_with`, `complete`, `FlowCtx`, `Origin` (Task 8).
- Produces: providers whose HTTP goes through the instrumented client. Plan 2 migrates the remaining six crates the same way.

Each provider currently builds a bare `reqwest::Client`. The change is uniform: keep the existing builder tuning, hand it to `client_from`, and store a `ClientWithMiddleware` instead of a `reqwest::Client`. The request-builder API is identical, so call sites downstream do not change.

- [ ] **Step 1: Add the dependency**

`crates/rupu-providers/Cargo.toml`:

```toml
rupu-netflow = { workspace = true, features = ["http"] }
```

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-providers/tests/netflow_capture.rs`:

```rust
//! Proves provider egress reaches the netflow sink with provider
//! attribution — the whole point of the migration.

use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

#[tokio::test]
async fn provider_client_records_flows_with_provider_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200).body("{}");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx {
            run_id: Some("run-1".into()),
            step_id: None,
            agent: None,
            workspace_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        rupu_providers::tuning::ProviderTuning::default().http_client_builder(),
        sink.clone(),
    )
    .unwrap();

    client.get(server.url("/v1/models")).send().await.unwrap();

    let records = sink.records();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].ctx.origin,
        Origin::Provider("anthropic".to_string())
    );
    assert_eq!(records[0].ctx.run_id.as_deref(), Some("run-1"));
}
```

Adjust `ProviderTuning::default()` to whatever the real constructor is — check `crates/rupu-providers/src/tuning.rs:89` for the surrounding type and how it is normally built.

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-providers --test netflow_capture`
Expected: FAIL to compile — `rupu_netflow` is not a dependency yet, or `http_client_builder` is not public.

- [ ] **Step 4: Migrate `anthropic.rs`'s client builders**

Replace the bodies of `build_http_client` / `build_http_client_with_timeout` so the tuning survives and the client is instrumented:

```rust
fn build_http_client_with_timeout(
    ctx: rupu_netflow::FlowCtx,
    timeout: Option<std::time::Duration>,
) -> reqwest_middleware::ClientWithMiddleware {
    let mut builder = reqwest::Client::builder().http1_only();
    if let Some(t) = timeout {
        builder = builder.timeout(t);
    }
    rupu_netflow::http::client_from(ctx, builder)
        .unwrap_or_else(|_| rupu_netflow::http::client(rupu_netflow::FlowCtx::system(
            rupu_netflow::Origin::Provider("anthropic".into()),
        )))
}
```

Change the struct field holding the client from `reqwest::Client` to `reqwest_middleware::ClientWithMiddleware` and thread a `FlowCtx` in from wherever the provider is constructed. Where no run context is available at construction, use `FlowCtx::system(Origin::Provider("anthropic".into()))` — accurate, and Plan 2 can thread the real run id when the provider factory is touched.

- [ ] **Step 5: Instrument the SSE loop**

At `crates/rupu-providers/src/anthropic.rs:1316`, the loop reads:

```rust
let mut parser = SseParser::new();
// ...
Ok(chunk_result) => match chunk_result? {
    Some(chunk) => {
        for event in parser.feed(&chunk)? {
```

Mint the id before the request, accumulate, and finalize after the loop:

```rust
let flow_id = rupu_netflow::FlowId::new();
let flow_started = std::time::Instant::now();
let mut flow_bytes_in: u64 = 0;
// ... on the request builder for this call:
//     .with_extension(flow_id)

// inside the chunk loop, immediately after `Some(chunk) => {`:
flow_bytes_in += chunk.len() as u64;

// after the loop exits (every exit path, including the error paths —
// use a guard or an explicit call on each branch):
rupu_netflow::http::complete(
    flow_id,
    flow_bytes_in,
    flow_started.elapsed().as_millis() as u64,
)
.await;
```

The idle-restart loop re-sends a stalled stream. Mint a **fresh** `flow_id` per attempt — each re-send is genuinely a separate connection and must appear as its own flow. Do not reuse the id across retries.

- [ ] **Step 6: Migrate the remaining providers**

For each of `local.rs`, `openai_compatible.rs`, `github_copilot.rs`, `openai_codex.rs`, `google_gemini.rs`, `broker_client.rs`:
1. Change the stored client type to `reqwest_middleware::ClientWithMiddleware`.
2. Replace `reqwest::Client::new()` with `rupu_netflow::http::client(FlowCtx::system(Origin::Provider("<name>".into())))`, using the provider's own name.
3. Where a `ClientBuilder` with tuning already exists (`tuning.rs:89`), pass it through `client_from` instead.

- [ ] **Step 7: Run the provider test suite**

Run: `cargo test -p rupu-providers`
Expected: PASS. Existing tests should be unaffected — `ClientWithMiddleware` exposes the same request-builder API.

- [ ] **Step 8: Verify lints**

Run: `cargo clippy -p rupu-providers --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
rustfmt --edition 2021 crates/rupu-providers/src/anthropic.rs crates/rupu-providers/src/tuning.rs crates/rupu-providers/src/local.rs crates/rupu-providers/src/openai_compatible.rs crates/rupu-providers/src/github_copilot.rs crates/rupu-providers/src/openai_codex.rs crates/rupu-providers/src/google_gemini.rs crates/rupu-providers/src/broker_client.rs crates/rupu-providers/tests/netflow_capture.rs
git add crates/rupu-providers/
git commit -m "feat(providers): route provider egress through the netflow client"
```

---

### Task 11: Lock the choke point

**Files:**
- Create: `clippy.toml` (repo root)
- Create: `crates/rupu-netflow/tests/choke_point.rs`
- Modify: `Cargo.toml` (`[workspace.lints.clippy]`)

**Interfaces:**
- Consumes: nothing.
- Produces: a build-time guarantee that `reqwest::Client::new` and `::builder` appear nowhere outside `rupu-netflow`. Plan 2 relies on this to keep migrated crates migrated.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-netflow/tests/choke_point.rs`:

```rust
//! rupu has exactly ONE door for outbound HTTP. This test fails if a new
//! one appears, so a regression is a red build rather than a review miss.
//!
//! Paired with the `clippy.toml` `disallowed-methods` lint: the lint
//! catches it at compile time in CI, this catches it even when someone
//! runs a bare `cargo test`.

use std::path::Path;

/// Files permitted to construct a raw reqwest client.
const ALLOWED: &[&str] = &[
    // The factory itself.
    "crates/rupu-netflow/src/http/mod.rs",
    // Its `#[cfg(test)]` module builds a throwaway client to exercise
    // `refresh`. Inline test modules live in `src/`, so the `/tests/`
    // skip below does not cover them.
    "crates/rupu-netflow/src/asn/acquire.rs",
];

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/<name> has a grandparent")
        .to_path_buf()
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name == "node_modules" || name == ".git" {
                continue;
            }
            walk(&path, out);
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_raw_reqwest_client_outside_rupu_netflow() {
    let root = repo_root();
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);

    let mut offenders = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");

        // Tests may build throwaway clients; they are not rupu's egress.
        if rel.contains("/tests/") || ALLOWED.contains(&rel.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            if code.contains("reqwest::Client::new") || code.contains("reqwest::Client::builder") {
                offenders.push(format!("{rel}:{}", i + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "raw reqwest clients bypass netflow capture — route them through \
         rupu_netflow::http::client / client_from instead:\n  {}",
        offenders.join("\n  ")
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-netflow --test choke_point`
Expected: FAIL, listing the not-yet-migrated sites in `rupu-auth`, `rupu-scm`, `rupu-update`, `rupu-webhook`, `rupu-cp`, `rupu-cli`.

- [ ] **Step 3: Scope the test to what Plan 1 migrated**

Plan 2 migrates the rest. Until then, the guard must pass without lying about coverage. Add an explicit, documented exemption list — a visible debt register, not a silent pass:

```rust
/// NOT yet migrated. Plan 2 empties this list; it must never grow.
/// Every entry here is a client whose egress is currently invisible.
const PENDING_PLAN_2: &[&str] = &[
    "crates/rupu-auth/src/oauth/device.rs",
    "crates/rupu-auth/src/oauth/callback.rs",
    "crates/rupu-auth/src/resolver.rs",
    "crates/rupu-update/src/github.rs",
    "crates/rupu-cp/src/host/http.rs",
    "crates/rupu-cli/src/cmd/cp.rs",
    "crates/rupu-scm/src/client_options.rs",
    "crates/rupu-scm/src/connectors/gitlab/events.rs",
    "crates/rupu-scm/src/connectors/linear/events.rs",
    "crates/rupu-scm/src/connectors/jira/events.rs",
    "crates/rupu-scm/src/connectors/jira/issues.rs",
];
```

Add `|| PENDING_PLAN_2.contains(&rel.as_str())` to the skip condition, and a second test that the debt register only shrinks:

```rust
#[test]
fn pending_migration_list_matches_reality() {
    let root = repo_root();
    for rel in PENDING_PLAN_2 {
        assert!(
            root.join(rel).exists(),
            "{rel} is listed as pending migration but does not exist — \
             remove it from PENDING_PLAN_2"
        );
    }
}
```

Run the actual file list from Step 2's failure output rather than trusting the list above verbatim — the repo may have moved on.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-netflow --test choke_point`
Expected: PASS — 2 tests.

- [ ] **Step 5: Add the clippy lint**

Create `clippy.toml` at the repo root:

```toml
# rupu has exactly one door for outbound HTTP: rupu_netflow::http::client
# / client_from. A raw reqwest client bypasses netflow capture entirely.
# See docs/superpowers/specs/2026-08-03-rupu-netflow-observability-design.md §7.3
disallowed-methods = [
    { path = "reqwest::Client::new", reason = "use rupu_netflow::http::client(ctx) so the flow is captured" },
    { path = "reqwest::Client::builder", reason = "use rupu_netflow::http::client_from(ctx, builder) so the flow is captured" },
]
```

In the root `Cargo.toml` under `[workspace.lints.clippy]`:

```toml
disallowed_methods = "warn"
```

Set it to `"warn"` while `PENDING_PLAN_2` is non-empty, and raise it to `"deny"` in Plan 2's final task once the list is empty. A `deny` now would break the build on unmigrated crates.

- [ ] **Step 6: Verify the lint fires where expected**

Run: `cargo clippy -p rupu-auth --all-targets 2>&1 | grep -c "disallowed"`
Expected: a non-zero count — the lint is live and pointing at real unmigrated sites.

Run: `cargo clippy -p rupu-netflow --all-features --all-targets -- -D warnings`
Expected: no warnings — the factory's own `#[allow(clippy::disallowed_methods)]` covers it.

- [ ] **Step 7: Run the whole workspace**

Run: `cargo test --workspace`
Expected: PASS. Investigate any failure before committing — a broken workspace test here means the provider migration changed behaviour.

- [ ] **Step 8: Commit**

```bash
git add clippy.toml Cargo.toml crates/rupu-netflow/tests/choke_point.rs
git commit -m "feat(netflow): lock the HTTP choke point with a clippy lint and guard test"
```

---

## Done when

- `cargo test --workspace` passes.
- `cargo build -p rupu-netflow` pulls in no `reqwest` (verify with `cargo tree`).
- Provider egress appears in `.rupu/netflow/flows.jsonl` when a run executes.
- `clippy.toml` exists and `PENDING_PLAN_2` is the only list of unmigrated clients.

Plan 2 migrates the remaining six crates, adds the `Coarse` adapter for `octocrab`, wires `Event::NetFlow` into the transcript and live stream, and raises the lint to `deny`. Plan 3 builds the CP API and views.
