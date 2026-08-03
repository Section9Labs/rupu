# Netflow Plan 2 — full migration, `Event::NetFlow`, and automatic ASN refresh

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route every remaining rupu HTTP client through the netflow factory, record `octocrab` at `Coarse` fidelity, stream flows into the run transcript as `Event::NetFlow`, wire automatic ASN refresh into `cp serve`, and raise the choke-point lint to `deny`.

**Architecture:** Five crates still build raw `reqwest` clients; each migrates to `rupu_netflow::http::client_from`, preserving its existing builder tuning. `octocrab` cannot be instrumented at the transport layer without a tower service rewrite, so it is recorded at its retry choke point with an honest `Fidelity::Coarse`. The transcript bridge lives in `rupu-transcript` — which depends on `rupu-netflow`, never the reverse — so flows reach the run's transcript JSONL, which CP's transcript viewer already tails.

**Tech Stack:** Rust 2021, `tokio`, `reqwest` + `reqwest-middleware`, `octocrab`, `httpmock` + `tempfile` for tests.

**Spec:** `docs/superpowers/specs/2026-08-03-rupu-netflow-observability-design.md`
**Depends on:** Plan 1 (`docs/superpowers/plans/2026-08-03-rupu-netflow-plan-1-crate-and-capture.md`) — complete.

## Global Constraints

- **Workspace deps only.** Versions pinned in the root `Cargo.toml`.
- **`[lints] workspace = true`** in every crate `Cargo.toml`; `unsafe_code` forbidden.
- **Capture must never break a request.** No `unwrap()`/`expect()`/panic reachable from a sink or middleware.
- **Query strings and headers are NEVER stored.** This applies to the `Coarse` adapter too — record the path, never the query.
- **`rupu-netflow` must never depend on `rupu-transcript`.** The dependency runs the other way. A cycle here is a hard failure.
- **Migration preserves existing tuning.** Timeouts, `http1_only`, connect/read timeouts and proxy settings must survive; pass the tuned `reqwest::ClientBuilder` into `client_from`, never discard it.
- **`PENDING_PLAN_2` in `crates/rupu-netflow/tests/choke_point.rs` must only ever shrink.** It is empty by the end of this plan.
- **Never run package-wide `cargo fmt`** — format ONLY files you touched, with `rustfmt --edition 2021 <path>`. `cargo fmt -- <path>` does NOT scope and reformats the whole workspace.
- **Never use bare `git stash` / `git stash pop`** — the stash stack is shared across worktrees.
- **`rupu-cp` already contains two `reqwest` majors.** `object_store 0.14` pulls `reqwest 0.13.4` while the workspace pins `0.12`; this predates netflow and is out of scope. When you migrate `rupu-cp` (Task 7), `rupu_netflow::http::client` returns a `reqwest 0.12`-based `ClientWithMiddleware` — that is correct and expected. Do not try to reconcile the two majors, and do not use a lockfile-wide `reqwest` count as a health check.

---

### Task 1: `Event::NetFlow` in the transcript schema

**Files:**
- Modify: `crates/rupu-transcript/Cargo.toml` (add `rupu-netflow`, default features off)
- Modify: `crates/rupu-transcript/src/event.rs`

**Interfaces:**
- Consumes: `FlowRecord` from `rupu-netflow` (Plan 1 Task 1).
- Produces: `Event::NetFlow { flow: Box<FlowRecord> }`. Task 2's `TranscriptSink` constructs it; Plan 3's CP transcript reader matches on it.

- [ ] **Step 1: Add the dependency**

`crates/rupu-transcript/Cargo.toml`:

```toml
rupu-netflow = { workspace = true }
```

Default features only — this must NOT pull in `reqwest`. That is the point of the feature split.

- [ ] **Step 2: Write the failing test**

Add to the test module in `crates/rupu-transcript/src/event.rs`:

```rust
#[test]
fn netflow_event_round_trips() {
    use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowRecord, Origin, Outcome};

    let flow = FlowRecord {
        id: FlowId::from_parts(5, 5),
        ts: chrono::Utc::now(),
        ctx: FlowCtx {
            run_id: Some("run-9".into()),
            step_id: Some("step-1".into()),
            agent: Some("reviewer".into()),
            workspace_id: Some("ws".into()),
            origin: Origin::Provider("anthropic".into()),
        },
        fidelity: Fidelity::Http,
        method: "POST".into(),
        scheme: "https".into(),
        host: "api.anthropic.com".into(),
        port: 443,
        path: "/v1/messages".into(),
        peer_ip: None,
        resolved_ips: vec![],
        http_version: Some("HTTP/1.1".into()),
        status: Some(200),
        outcome: Outcome::Ok,
        error: None,
        bytes_out: Some(10),
        bytes_in: Some(20),
        body_complete: true,
        ttfb_ms: Some(5),
        duration_ms: Some(50),
    };

    let event = Event::NetFlow {
        flow: Box::new(flow),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains(r#""type":"net_flow""#));

    let back: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(event, back);
}

#[test]
fn legacy_transcripts_without_netflow_still_parse() {
    // A transcript written before this variant existed must still read.
    let line = r#"{"type":"turn_start","data":{"turn_idx":0}}"#;
    let back: Event = serde_json::from_str(line).unwrap();
    assert!(matches!(back, Event::TurnStart { turn_idx: 0 }));
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-transcript netflow`
Expected: FAIL to compile — `no variant named NetFlow`.

- [ ] **Step 4: Add the variant**

In `crates/rupu-transcript/src/event.rs`, add to `enum Event` after `ToolAudit`:

```rust
    /// One outbound network flow attributed to this run.
    ///
    /// Phase 1 covers rupu's OWN egress — provider APIs, SCM connectors,
    /// MCP, webhooks. It does NOT cover the agent's `bash` subprocess
    /// traffic; `flow.fidelity` states what is actually known. See
    /// docs/superpowers/specs/2026-08-03-rupu-netflow-observability-design.md
    ///
    /// Boxed: `FlowRecord` dwarfs every other variant and clippy's
    /// `large_enum_variant` denies otherwise.
    NetFlow {
        flow: Box<rupu_netflow::FlowRecord>,
    },
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-transcript`
Expected: PASS.

- [ ] **Step 6: Confirm no HTTP stack leaked into the schema crate**

Run: `cargo tree -p rupu-transcript | grep -c reqwest`
Expected: `0`.

- [ ] **Step 7: Commit**

```bash
rustfmt --edition 2021 crates/rupu-transcript/src/event.rs
git add crates/rupu-transcript/
git commit -m "feat(transcript): Event::NetFlow carrying a FlowRecord"
```

---

### Task 2: `TranscriptSink` — flows into the run's JSONL

**Files:**
- Create: `crates/rupu-transcript/src/netflow_sink.rs`
- Modify: `crates/rupu-transcript/src/lib.rs`

**Interfaces:**
- Consumes: `FlowSink` trait and `FlowRecord` (Plan 1 Tasks 1–2), `Event::NetFlow` (Task 1), `JsonlWriter` (existing).
- Produces: `TranscriptSink::new(PathBuf) -> TranscriptSink`, implementing `FlowSink`. Task 8 composes it into the run's `FanoutSink`.

The sink opens, appends and drops per record, mirroring `crates/rupu-cli/src/cmd/dispatch.rs:391`. At flow volumes (tens per run) the syscall cost is irrelevant, and it avoids contending with the runner's own writer over one handle.

`complete()` is a deliberate no-op: the ledger is where a streamed body gets finalized. A transcript is an append-only narrative and cannot retroactively amend a line it already wrote — recording the header-time flow there is honest, and the finalized byte count is available from the ledger.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-transcript/src/netflow_sink.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowSink, Origin, Outcome};

    fn flow() -> FlowRecord {
        FlowRecord {
            id: FlowId::from_parts(1, 1),
            ts: chrono::Utc::now(),
            ctx: FlowCtx::system(Origin::Scm("github".into())),
            fidelity: Fidelity::Http,
            method: "GET".into(),
            scheme: "https".into(),
            host: "api.github.com".into(),
            port: 443,
            path: "/repos".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: None,
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: None,
            bytes_in: Some(64),
            body_complete: true,
            ttfb_ms: Some(3),
            duration_ms: Some(9),
        }
    }

    #[tokio::test]
    async fn writes_a_netflow_event_line_to_the_transcript() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("transcript.jsonl");
        JsonlWriter::create(&path).unwrap();

        let sink = TranscriptSink::new(path.clone());
        sink.record(flow()).await;

        let text = std::fs::read_to_string(&path).unwrap();
        let last = text.lines().last().unwrap();
        let event: Event = serde_json::from_str(last).unwrap();
        match event {
            Event::NetFlow { flow } => {
                assert_eq!(flow.host, "api.github.com");
                assert_eq!(flow.status, Some(200));
            }
            other => panic!("expected NetFlow, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unwritable_path_is_silently_tolerated() {
        // Capture must never break the caller. A bad path is a logged
        // no-op, not a panic and not an error the request path sees.
        let sink = TranscriptSink::new(std::path::PathBuf::from("/nonexistent/dir/t.jsonl"));
        sink.record(flow()).await;
        sink.complete(FlowId::from_parts(1, 1), 10, 10).await;
    }

    #[tokio::test]
    async fn complete_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("transcript.jsonl");
        JsonlWriter::create(&path).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        TranscriptSink::new(path.clone())
            .complete(FlowId::from_parts(2, 2), 99, 99)
            .await;

        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-transcript netflow_sink`
Expected: FAIL to compile — `cannot find struct TranscriptSink`.

- [ ] **Step 3: Implement above the tests**

```rust
//! Bridges netflow records into the run transcript as `Event::NetFlow`.
//!
//! This lives in `rupu-transcript`, not `rupu-netflow`, because the
//! dependency runs transcript → netflow. Putting the bridge in the
//! netflow crate would create a cycle.

use crate::event::Event;
use crate::writer::JsonlWriter;
use async_trait::async_trait;
use rupu_netflow::{FlowId, FlowRecord, FlowSink};
use std::path::PathBuf;

/// Appends each flow to a run's transcript JSONL.
///
/// Opens, appends and drops per record — the same pattern
/// `rupu-cli`'s dispatch path uses. At flow volumes the syscall cost is
/// irrelevant and it avoids contending with the runner's own writer.
pub struct TranscriptSink {
    path: PathBuf,
}

impl TranscriptSink {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl FlowSink for TranscriptSink {
    async fn record(&self, flow: FlowRecord) {
        let event = Event::NetFlow {
            flow: Box::new(flow),
        };
        match JsonlWriter::append(&self.path) {
            Ok(mut w) => {
                if let Err(e) = w.write(&event) {
                    tracing::debug!(error = %e, "netflow transcript write failed");
                }
                let _ = w.flush();
            }
            Err(e) => {
                tracing::debug!(error = %e, path = ?self.path, "netflow transcript unavailable");
            }
        }
    }

    /// No-op by design. A transcript is an append-only narrative and
    /// cannot amend a line it already wrote; the ledger owns finalized
    /// byte counts for streamed bodies.
    async fn complete(&self, _id: FlowId, _bytes_in: u64, _duration_ms: u64) {}
}
```

- [ ] **Step 4: Export from `lib.rs`**

```rust
pub mod netflow_sink;
pub use netflow_sink::TranscriptSink;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-transcript`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-transcript/src/netflow_sink.rs crates/rupu-transcript/src/lib.rs
git add crates/rupu-transcript/
git commit -m "feat(transcript): TranscriptSink bridging flows into run JSONL"
```

---

### Task 3: Migrate `rupu-auth`

**Files:**
- Modify: `crates/rupu-auth/Cargo.toml`
- Modify: `crates/rupu-auth/src/oauth/device.rs:54`
- Modify: `crates/rupu-auth/src/oauth/callback.rs:200`
- Modify: `crates/rupu-auth/src/resolver.rs:296`
- Modify: `crates/rupu-netflow/tests/choke_point.rs` (remove the three `rupu-auth` entries from `PENDING_PLAN_2`)

**Interfaces:**
- Consumes: `client`, `FlowCtx`, `Origin` (Plan 1 Task 8).
- Produces: OAuth token and discovery egress captured under `Origin::System`.

These three are token-exchange and discovery calls with no run context — `FlowCtx::system` is the accurate attribution, not a placeholder.

- [ ] **Step 1: Add the dependency**

`crates/rupu-auth/Cargo.toml`:

```toml
rupu-netflow = { workspace = true, features = ["http"] }
```

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-auth/tests/netflow_capture.rs`:

```rust
//! OAuth egress must be visible. These are the calls that carry
//! credentials, so their destinations are exactly what an operator
//! wants accounted for.

use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

#[tokio::test]
async fn oauth_client_records_flows_under_system_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/oauth/token");
            then.status(200).body("{}");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::System),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    client.post(server.url("/oauth/token")).send().await.unwrap();

    let records = sink.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].path, "/oauth/token");
    assert_eq!(records[0].ctx.origin, Origin::System);
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-auth --test netflow_capture`
Expected: FAIL to compile — `rupu_netflow` not a dependency, or `reqwest` not a dev-dependency of `rupu-auth`. Add `reqwest.workspace = true` under `[dev-dependencies]` if needed.

- [ ] **Step 4: Migrate the three sites**

At each of `oauth/device.rs:54`, `oauth/callback.rs:200`, `resolver.rs:296`, replace:

```rust
let client = reqwest::Client::new();
```

with:

```rust
let client = rupu_netflow::http::client(rupu_netflow::FlowCtx::system(
    rupu_netflow::Origin::System,
));
```

The local variable type changes from `reqwest::Client` to `reqwest_middleware::ClientWithMiddleware`. Downstream `.post(...)`, `.form(...)`, `.send()` calls are unchanged — the builder API is identical. Add `reqwest-middleware.workspace = true` to `[dependencies]` if any signature names the type explicitly.

- [ ] **Step 5: Shrink the debt register**

In `crates/rupu-netflow/tests/choke_point.rs`, delete these three entries from `PENDING_PLAN_2`:

```rust
    "crates/rupu-auth/src/oauth/device.rs",
    "crates/rupu-auth/src/oauth/callback.rs",
    "crates/rupu-auth/src/resolver.rs",
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p rupu-auth && cargo test -p rupu-netflow --test choke_point`
Expected: PASS both. The choke-point test now proves `rupu-auth` has no raw clients.

- [ ] **Step 7: Commit**

```bash
rustfmt --edition 2021 crates/rupu-auth/src/oauth/device.rs crates/rupu-auth/src/oauth/callback.rs crates/rupu-auth/src/resolver.rs crates/rupu-auth/tests/netflow_capture.rs
git add crates/rupu-auth/ crates/rupu-netflow/
git commit -m "feat(auth): route OAuth egress through the netflow client"
```

---

### Task 4: Migrate `rupu-update`

**Files:**
- Modify: `crates/rupu-update/Cargo.toml`
- Modify: `crates/rupu-update/src/github.rs:12-30` (the struct field and `new`)
- Modify: `crates/rupu-update/src/github.rs:71` (the download client builder)
- Modify: `crates/rupu-netflow/tests/choke_point.rs`

**Interfaces:**
- Consumes: `client`, `client_from`, `FlowCtx`, `Origin` (Plan 1 Task 8).
- Produces: release-check and binary-download egress under `Origin::Update`.

The binary download is the largest single flow rupu makes; its byte count is worth having.

- [ ] **Step 1: Add the dependency**

`crates/rupu-update/Cargo.toml`:

```toml
rupu-netflow = { workspace = true, features = ["http"] }
reqwest-middleware.workspace = true
```

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-update/tests/netflow_capture.rs`:

```rust
use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

#[tokio::test]
async fn release_check_is_recorded_under_update_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/releases/latest");
            then.status(200).body("[]");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Update),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    client
        .get(server.url("/releases/latest"))
        .send()
        .await
        .unwrap();

    let records = sink.records();
    assert_eq!(records[0].ctx.origin, Origin::Update);
    assert_eq!(records[0].ctx.run_id, None, "the updater has no run");
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-update --test netflow_capture`
Expected: FAIL to compile.

- [ ] **Step 4: Migrate the struct field**

At `crates/rupu-update/src/github.rs:12`, change:

```rust
    client: reqwest::Client,
```

to:

```rust
    client: reqwest_middleware::ClientWithMiddleware,
```

and in `new` (line 19), change `reqwest::Client::new()` to:

```rust
    client: rupu_netflow::http::client(rupu_netflow::FlowCtx::system(
        rupu_netflow::Origin::Update,
    )),
```

Update the `req` helper's signature at line 24 to take `&reqwest_middleware::ClientWithMiddleware` and return `reqwest_middleware::RequestBuilder`.

- [ ] **Step 5: Migrate the download client**

At line 71, the builder carries tuning that must survive. Change:

```rust
let client = reqwest::Client::builder()
    /* ...existing tuning... */
    .build()?;
```

to:

```rust
let client = rupu_netflow::http::client_from(
    rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Update),
    reqwest::Client::builder(), /* ...existing tuning... */
)?;
```

Keep every existing builder call in place — only the terminal `.build()?` is replaced by passing the builder to `client_from`.

- [ ] **Step 6: Shrink the debt register**

Remove `"crates/rupu-update/src/github.rs"` from `PENDING_PLAN_2`.

- [ ] **Step 7: Run tests**

Run: `cargo test -p rupu-update && cargo test -p rupu-netflow --test choke_point`
Expected: PASS both.

- [ ] **Step 8: Commit**

```bash
rustfmt --edition 2021 crates/rupu-update/src/github.rs crates/rupu-update/tests/netflow_capture.rs
git add crates/rupu-update/ crates/rupu-netflow/
git commit -m "feat(update): route release-check and download egress through netflow"
```

---

### Task 5: Migrate the hand-rolled `rupu-scm` connectors

**Files:**
- Modify: `crates/rupu-scm/Cargo.toml`
- Modify: `crates/rupu-scm/src/client_options.rs:94-96` (`http_client_builder`)
- Modify: `crates/rupu-scm/src/connectors/gitlab/events.rs:26-40`
- Modify: `crates/rupu-scm/src/connectors/linear/events.rs:33-46`
- Modify: `crates/rupu-scm/src/connectors/jira/events.rs:36-50`
- Modify: `crates/rupu-scm/src/connectors/jira/issues.rs:22-36`
- Modify: `crates/rupu-scm/src/connectors/github/client.rs:145` and `:191` (the two ad-hoc clients)
- Modify: `crates/rupu-netflow/tests/choke_point.rs`

**Interfaces:**
- Consumes: `client_from`, `FlowCtx`, `Origin` (Plan 1 Task 8).
- Produces: GitLab / Jira / Linear connector egress at `Fidelity::Http` under `Origin::Scm(platform)`.

GitLab, Jira and Linear are hand-rolled `reqwest` — they migrate cleanly. Only `octocrab` resists, and that is Task 6.

- [ ] **Step 1: Add the dependency**

`crates/rupu-scm/Cargo.toml`:

```toml
rupu-netflow = { workspace = true, features = ["http"] }
reqwest-middleware.workspace = true
```

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-scm/tests/netflow_capture.rs`:

```rust
use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

#[tokio::test]
async fn connector_egress_carries_the_platform_as_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/api/v4/projects");
            then.status(200).body("[]");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Scm("gitlab".into())),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    client
        .get(server.url("/api/v4/projects"))
        .send()
        .await
        .unwrap();

    let r = &sink.records()[0];
    assert_eq!(r.ctx.origin, Origin::Scm("gitlab".to_string()));
    assert_eq!(r.path, "/api/v4/projects");
    assert_eq!(r.fidelity, rupu_netflow::Fidelity::Http);
}

#[tokio::test]
async fn private_token_in_a_query_never_reaches_the_record() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/api/v4/user");
            then.status(200).body("{}");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Scm("gitlab".into())),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    let url = format!("{}?private_token=glpat-LEAKME", server.url("/api/v4/user"));
    client.get(url).send().await.unwrap();

    let json = serde_json::to_string(&sink.records()[0]).unwrap();
    assert!(!json.contains("glpat-LEAKME"));
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-scm --test netflow_capture`
Expected: FAIL to compile.

- [ ] **Step 4: Migrate `client_options.rs`**

At line 94, `http_client_builder` returns a tuned `reqwest::ClientBuilder`. Leave it returning a builder — it is the tuning source — and add a sibling that produces the instrumented client:

```rust
    /// Instrumented client carrying this connector's tuning.
    pub fn netflow_client(
        &self,
        platform: &str,
    ) -> reqwest::Result<reqwest_middleware::ClientWithMiddleware> {
        rupu_netflow::http::client_from(
            rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm(platform.to_string())),
            self.http_client_builder(),
        )
    }
```

- [ ] **Step 5: Migrate each connector**

For each of `gitlab/events.rs`, `linear/events.rs`, `jira/events.rs`, `jira/issues.rs`:

1. Change the struct field `http: reqwest::Client` to `http: reqwest_middleware::ClientWithMiddleware`.
2. Replace the constructor's `reqwest::Client::builder().<tuning>.build()` with:

```rust
    http: rupu_netflow::http::client_from(
        rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm("gitlab".into())),
        reqwest::Client::builder(), /* keep the existing tuning calls here */
    )
    .unwrap_or_else(|_| {
        rupu_netflow::http::client(rupu_netflow::FlowCtx::system(
            rupu_netflow::Origin::Scm("gitlab".into()),
        ))
    }),
```

substituting the correct platform string per file: `"gitlab"`, `"linear"`, `"jira"`, `"jira"`.

Preserve every existing tuning call (timeouts especially — ISSUES.md I-17 exists because they were missing once).

- [ ] **Step 6: Migrate the two ad-hoc GitHub clients**

`github/client.rs:145` (`fetch_token_scopes`) and `:191` (inside `graphql_json`) build raw clients. Both become:

```rust
let http = rupu_netflow::http::client_from(
    rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm("github".into())),
    reqwest::Client::builder().timeout(self.timeout),
)
.unwrap_or_else(|_| {
    rupu_netflow::http::client(rupu_netflow::FlowCtx::system(
        rupu_netflow::Origin::Scm("github".into()),
    ))
});
```

These two are genuinely `Fidelity::Http` — only the octocrab-routed calls are `Coarse`.

- [ ] **Step 7: Shrink the debt register**

Remove from `PENDING_PLAN_2`:

```rust
    "crates/rupu-scm/src/client_options.rs",
    "crates/rupu-scm/src/connectors/gitlab/events.rs",
    "crates/rupu-scm/src/connectors/linear/events.rs",
    "crates/rupu-scm/src/connectors/jira/events.rs",
    "crates/rupu-scm/src/connectors/jira/issues.rs",
```

- [ ] **Step 8: Run tests**

Run: `cargo test -p rupu-scm && cargo test -p rupu-netflow --test choke_point`
Expected: PASS both.

- [ ] **Step 9: Commit**

```bash
rustfmt --edition 2021 crates/rupu-scm/src/client_options.rs crates/rupu-scm/src/connectors/gitlab/events.rs crates/rupu-scm/src/connectors/linear/events.rs crates/rupu-scm/src/connectors/jira/events.rs crates/rupu-scm/src/connectors/jira/issues.rs crates/rupu-scm/src/connectors/github/client.rs crates/rupu-scm/tests/netflow_capture.rs
git add crates/rupu-scm/ crates/rupu-netflow/
git commit -m "feat(scm): route GitLab, Jira, Linear and ad-hoc GitHub egress through netflow"
```

---

### Task 6: `Coarse` fidelity for `octocrab`

**Files:**
- Modify: `crates/rupu-scm/src/connectors/github/client.rs:250` (`with_retry`)
- Test: `crates/rupu-scm/tests/netflow_coarse.rs` (create)

**Interfaces:**
- Consumes: `Fidelity::Coarse`, `FlowRecord`, `sink()` (Plan 1 Tasks 1, 8).
- Produces: one `Coarse` record per octocrab attempt. Plan 3's views render its fidelity badge.

`octocrab` owns its own `hyper`/`tower` stack, so the transport is not ours to instrument. `OctocrabBuilder::with_service` could lift this to `Http` later; it is a separate piece of work. Until then this records what is genuinely known — host, path, outcome, timing — and says so via the fidelity field rather than omitting GitHub entirely or inventing bytes.

`with_retry` is the single choke point every octocrab call passes through, and it retries. One record is emitted **per attempt**, because each attempt is a real connection.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-scm/tests/netflow_coarse.rs`:

```rust
//! `octocrab` calls are recorded at Coarse fidelity — host, outcome and
//! timing are real; bytes and peer IP are genuinely unknown and stay
//! `None` rather than being guessed.

use rupu_netflow::{Fidelity, MemorySink};
use std::sync::Arc;

#[tokio::test]
async fn a_coarse_record_states_what_it_does_not_know() {
    let sink = Arc::new(MemorySink::default());
    let record = rupu_scm::connectors::github::client::coarse_flow(
        "api.github.com",
        "/repos/foo/bar",
        true,
        42,
    );
    rupu_netflow::FlowSink::record(sink.as_ref(), record).await;

    let r = &sink.records()[0];
    assert_eq!(r.fidelity, Fidelity::Coarse);
    assert_eq!(r.host, "api.github.com");
    assert_eq!(r.path, "/repos/foo/bar");
    assert_eq!(r.duration_ms, Some(42));
    assert!(matches!(r.outcome, rupu_netflow::Outcome::Ok));
    assert_eq!(r.bytes_in, None, "Coarse cannot know body size");
    assert_eq!(r.bytes_out, None, "Coarse cannot know body size");
    assert_eq!(r.peer_ip, None, "Coarse cannot know the peer");
    assert_eq!(r.status, None, "octocrab does not surface the raw status here");
}

#[tokio::test]
async fn a_failed_attempt_records_a_transport_error() {
    let record = rupu_scm::connectors::github::client::coarse_flow(
        "api.github.com",
        "/repos/foo/bar",
        false,
        10,
    );
    assert!(matches!(
        record.outcome,
        rupu_netflow::Outcome::TransportError
    ));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-scm --test netflow_coarse`
Expected: FAIL to compile — `cannot find function coarse_flow`.

- [ ] **Step 3: Add `coarse_flow` to `github/client.rs`**

```rust
/// Build a `Coarse`-fidelity record for one `octocrab` attempt.
///
/// `octocrab` owns its own hyper/tower stack, so bytes, peer IP and raw
/// status are genuinely unavailable here. They stay `None` — the
/// `Fidelity::Coarse` marker is how a view knows the difference between
/// "zero bytes" and "we could not see the bytes".
pub fn coarse_flow(
    host: &str,
    path: &str,
    success: bool,
    duration_ms: u64,
) -> rupu_netflow::FlowRecord {
    rupu_netflow::FlowRecord {
        id: rupu_netflow::FlowId::new(),
        ts: chrono::Utc::now(),
        ctx: rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm("github".into())),
        fidelity: rupu_netflow::Fidelity::Coarse,
        method: "*".into(),
        scheme: "https".into(),
        host: host.to_string(),
        port: 443,
        path: path.to_string(),
        peer_ip: None,
        resolved_ips: Vec::new(),
        http_version: None,
        status: None,
        outcome: if success {
            rupu_netflow::Outcome::Ok
        } else {
            rupu_netflow::Outcome::TransportError
        },
        error: None,
        bytes_out: None,
        bytes_in: None,
        body_complete: true,
        ttfb_ms: None,
        duration_ms: Some(duration_ms),
    }
}
```

`method: "*"` is deliberate: `with_retry` is generic over the closure and does not know the verb. A literal `"*"` reads as "unknown" in a table; a fabricated `"GET"` would read as fact.

- [ ] **Step 4: Emit from `with_retry`**

Wrap each attempt inside the existing retry loop in `with_retry`:

```rust
let attempt_started = std::time::Instant::now();
let outcome = f().await;
rupu_netflow::http::sink()
    .record(coarse_flow(
        self.host_label(),
        "*",
        outcome.is_ok(),
        attempt_started.elapsed().as_millis() as u64,
    ))
    .await;
```

The host must be truthful rather than hardcoded — GitHub Enterprise installs are not `api.github.com`. Compute it **once at construction** and store it; do not derive it per call.

Add the field to `struct GithubClient`:

```rust
    /// Host this client talks to. Derived once from `graphql_url` so a
    /// GitHub Enterprise install is recorded as itself.
    host_label: String,
```

Populate it in `with_options`, immediately after `graphql_url` is computed:

```rust
        let host_label = url::Url::parse(&graphql_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "api.github.com".to_string());
```

and add `host_label` to the struct literal at the end of `with_options`, alongside `graphql_url`. Then the accessor:

```rust
    pub(crate) fn host_label(&self) -> &str {
        &self.host_label
    }
```

The path is `"*"` for the same reason as the method — `with_retry` does not know it.

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-scm`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-scm/src/connectors/github/client.rs crates/rupu-scm/tests/netflow_coarse.rs
git add crates/rupu-scm/
git commit -m "feat(scm): record octocrab attempts at Coarse fidelity"
```

---

### Task 7: Migrate `rupu-cp` and `rupu-cli`

**Files:**
- Modify: `crates/rupu-cp/Cargo.toml`, `crates/rupu-cli/Cargo.toml`
- Modify: `crates/rupu-cp/src/host/http.rs:51-56` and `:79`
- Modify: `crates/rupu-cli/src/cmd/cp.rs:2132`
- Modify: `crates/rupu-netflow/tests/choke_point.rs`

**Interfaces:**
- Consumes: `client`, `client_from`, `FlowCtx`, `Origin` (Plan 1 Task 8).
- Produces: fleet-host egress under `Origin::Cp`. Empties `PENDING_PLAN_2`.

`rupu-cp`'s host registry talks to remote fleet hosts. That is genuinely interesting egress — it is rupu reaching machines the operator configured.

- [ ] **Step 1: Add the dependencies**

Both `crates/rupu-cp/Cargo.toml` and `crates/rupu-cli/Cargo.toml`:

```toml
rupu-netflow = { workspace = true, features = ["http"] }
reqwest-middleware.workspace = true
```

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-cp/tests/netflow_capture.rs`:

```rust
use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

#[tokio::test]
async fn host_registry_egress_is_recorded_under_cp_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/api/host/info");
            then.status(200).body("{}");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Cp),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    client
        .get(server.url("/api/host/info"))
        .send()
        .await
        .unwrap();

    let r = &sink.records()[0];
    assert_eq!(r.ctx.origin, Origin::Cp);
    assert_eq!(r.path, "/api/host/info");
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-cp --test netflow_capture`
Expected: FAIL to compile.

- [ ] **Step 4: Migrate `rupu-cp/src/host/http.rs`**

At line 51 and line 79 the code builds a bounded client and falls back to an unbounded one. Preserve exactly that shape:

```rust
let ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Cp);
let client = rupu_netflow::http::client_from(
    ctx.clone(),
    reqwest::Client::builder(), /* keep existing timeout tuning */
)
.unwrap_or_else(|_| rupu_netflow::http::client(ctx));
```

Change the struct field at line 28 from `client: reqwest::Client` to `client: reqwest_middleware::ClientWithMiddleware`.

Note the comments at `registry.rs:148` and `run_resolve.rs:141` that reference the "unbounded `reqwest::Client::new()` root cause" — update their wording so they still describe reality, but do not change the behaviour they document.

- [ ] **Step 5: Migrate `rupu-cli/src/cmd/cp.rs:2132`**

```rust
let client = rupu_netflow::http::client(rupu_netflow::FlowCtx::system(
    rupu_netflow::Origin::Cp,
));
```

- [ ] **Step 6: Empty the debt register**

`PENDING_PLAN_2` should now be empty:

```rust
/// NOT yet migrated. Plan 2 empties this list; it must never grow.
/// Every entry here is a client whose egress is currently invisible.
const PENDING_PLAN_2: &[&str] = &[];
```

- [ ] **Step 7: Run the guard and the suites**

Run: `cargo test -p rupu-netflow --test choke_point`
Expected: PASS with an empty register — every rupu HTTP client now goes through one door.

Run: `cargo test -p rupu-cp -p rupu-cli`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/host/http.rs crates/rupu-cli/src/cmd/cp.rs crates/rupu-cp/tests/netflow_capture.rs
git add crates/rupu-cp/ crates/rupu-cli/ crates/rupu-netflow/
git commit -m "feat(cp): route fleet-host egress through the netflow client"
```

---

### Task 8: Wire capture into a run

**Files:**
- Modify: `crates/rupu-cli/src/cmd/dispatch.rs` (near the transcript writer setup around line 365-400)
- Test: `crates/rupu-cli/tests/netflow_run.rs` (create)

**Interfaces:**
- Consumes: `init`, `FanoutSink`, `NetflowWriterHandle`, `NetflowPaths` (Plan 1), `TranscriptSink` (Task 2).
- Produces: a run whose provider egress lands in both `.rupu/netflow/flows.jsonl` and the run transcript.

Until this task, every migrated client records into the default `NullSink` — capture exists but is not connected. This connects it.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/netflow_run.rs`:

```rust
//! A run's egress must reach BOTH destinations: the ledger (which
//! survives across runs and holds unattributed system egress) and the
//! run transcript (which streams live).

use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

#[tokio::test]
async fn a_flow_reaches_both_the_ledger_and_the_transcript() {
    use rupu_netflow::{FanoutSink, NetflowPaths, NetflowWriterHandle};
    use rupu_transcript::{JsonlWriter, TranscriptSink};

    let tmp = tempfile::TempDir::new().unwrap();
    let transcript = tmp.path().join("transcript.jsonl");
    JsonlWriter::create(&transcript).unwrap();

    let paths = NetflowPaths::new(tmp.path());
    let handle = NetflowWriterHandle::spawn(paths.clone()).unwrap();

    let observed = Arc::new(MemorySink::default());
    let fanout = FanoutSink::new(vec![
        handle.writer.clone(),
        Arc::new(TranscriptSink::new(transcript.clone())),
        observed.clone(),
    ]);

    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/ping");
            then.status(200).body("pong");
        })
        .await;

    let client = rupu_netflow::http::client_with(
        FlowCtx {
            run_id: Some("run-x".into()),
            step_id: None,
            agent: None,
            workspace_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        reqwest::Client::builder(),
        Arc::new(fanout),
    )
    .unwrap();

    client.get(server.url("/ping")).send().await.unwrap();
    handle.shutdown().await;

    // Ledger
    let flows = rupu_netflow::ledger::read_flows(&paths.flows).unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].ctx.run_id.as_deref(), Some("run-x"));

    // Transcript
    let text = std::fs::read_to_string(&transcript).unwrap();
    assert!(text.contains(r#""type":"net_flow""#));

    // And the observer saw it too — fanout reaches every child.
    assert_eq!(observed.records().len(), 1);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli --test netflow_run`
Expected: FAIL to compile — missing dev-dependencies. Add `rupu-netflow = { workspace = true, features = ["http"] }`, `rupu-transcript.workspace = true`, `httpmock.workspace = true`, `tempfile.workspace = true`, `reqwest.workspace = true` under `[dev-dependencies]` in `crates/rupu-cli/Cargo.toml` as needed.

- [ ] **Step 3: Install the sink at run start**

In `crates/rupu-cli/src/cmd/dispatch.rs`, at the point where the transcript path is known and `JsonlWriter::append(transcript_path)` is called (around line 391), install the process-wide sink before any provider client is built:

```rust
// Netflow capture. Two destinations: the ledger (persists across runs
// and holds system egress that has no run to attach to) and this run's
// transcript (streams live). Best-effort — a ledger that cannot be
// opened must not stop the run.
let netflow_paths = rupu_netflow::NetflowPaths::new(&workspace_path);
let mut sinks: Vec<std::sync::Arc<dyn rupu_netflow::FlowSink>> = vec![std::sync::Arc::new(
    rupu_transcript::TranscriptSink::new(transcript_path.to_path_buf()),
)];
match rupu_netflow::NetflowWriterHandle::spawn(netflow_paths) {
    Ok(handle) => sinks.push(handle.writer.clone()),
    Err(e) => tracing::debug!(error = %e, "netflow ledger unavailable for this run"),
}
rupu_netflow::http::init(std::sync::Arc::new(rupu_netflow::FanoutSink::new(sinks)));
```

`init` uses a `OnceLock`, so the first run in a process wins and a second call is ignored. That is correct for the CLI (one run per process) and safe for long-lived processes.

Substitute the actual local variable names for `workspace_path` and `transcript_path` — read the surrounding function before editing.

- [ ] **Step 4: Run the test**

Run: `cargo test -p rupu-cli --test netflow_run`
Expected: PASS.

- [ ] **Step 5: Verify end to end by hand**

Run a real workflow in this repo and confirm the ledger fills:

```bash
cargo run -p rupu-cli -- run --agent rupu-agent --prompt "say hi" 2>/dev/null
wc -l .rupu/netflow/flows.jsonl
head -1 .rupu/netflow/flows.jsonl | python3 -m json.tool
```

Expected: at least one line, whose `flow.host` is the configured provider's host and whose `ctx.origin` is `{"kind":"provider","name":"..."}`. If the file does not exist, `init` is being called after the provider client is constructed — move it earlier.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/dispatch.rs crates/rupu-cli/tests/netflow_run.rs
git add crates/rupu-cli/
git commit -m "feat(cli): install the netflow fanout sink at run start"
```

---

### Task 9: Automatic ASN refresh in `cp serve`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/cp.rs` (the existing gate-sweep / cron-tick loop)
- Test: inline `#[cfg(test)] mod tests` in `crates/rupu-cli/src/cmd/cp.rs`

**Interfaces:**
- Consumes: `asn_db_path`, `is_stale`, `refresh` (Plan 1 Task 7), `NetflowConfig` (Plan 1 Task 6).
- Produces: an ASN table that appears and stays fresh without the operator running anything.

The operator must never have to run a command for enrichment to work. This is the primary trigger; the lazy read-time trigger lands in Plan 3 alongside the API that reads the table.

- [ ] **Step 1: Locate the sweep loop**

Run: `grep -n "gate_sweep\|cron tick\|interval\|sweep" crates/rupu-cli/src/cmd/cp.rs | head -20`
Read how the existing gate sweep is scheduled and gated by `[cp].gate_sweep_enabled` / `gate_sweep_interval_secs`. Mirror that structure exactly.

- [ ] **Step 2: Write the failing test**

`should_refresh_asn` is `pub(crate)`, so this test goes **inline** in `crates/rupu-cli/src/cmd/cp.rs`'s existing `#[cfg(test)] mod tests`, not in `tests/`. Do not widen a module's visibility just to test it.

```rust
    #[test]
    fn asn_refresh_is_skipped_when_auto_refresh_is_off() {
        let cfg = rupu_config::NetflowConfig {
            asn_auto_refresh: false,
            ..Default::default()
        };
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(!should_refresh_asn(&cfg, &tmp.path().join("asn.db")));
    }

    #[test]
    fn asn_refresh_is_requested_when_the_table_is_missing() {
        let cfg = rupu_config::NetflowConfig::default();
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(should_refresh_asn(&cfg, &tmp.path().join("asn.db")));
    }

    #[test]
    fn asn_refresh_is_skipped_for_a_fresh_table() {
        let cfg = rupu_config::NetflowConfig::default();
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("asn.db");
        std::fs::write(&path, b"{}").unwrap();
        assert!(!should_refresh_asn(&cfg, &path));
    }
```

If `cp.rs` has no `#[cfg(test)] mod tests` yet, add one with `use super::*;`.

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-cli asn_refresh`
Expected: FAIL — `cannot find function should_refresh_asn`.

- [ ] **Step 4: Implement the decision function**

In `crates/rupu-cli/src/cmd/cp.rs`:

```rust
/// Whether the ASN table should be refreshed right now.
///
/// Pure so the policy is testable without a network or a timer.
pub(crate) fn should_refresh_asn(cfg: &rupu_config::NetflowConfig, path: &std::path::Path) -> bool {
    cfg.asn_auto_refresh && rupu_netflow::asn::is_stale(path, cfg.asn_refresh_interval_days)
}
```

- [ ] **Step 5: Add the tick to the sweep loop**

Inside the existing sweep loop body, alongside the gate sweep:

```rust
// ASN freshness. Best-effort and never blocking: a failed refresh
// leaves the existing table untouched and enrichment degrades to
// "not loaded" rather than to wrong answers.
if let Some(db) = rupu_netflow::asn::asn_db_path() {
    if should_refresh_asn(&cfg.netflow, &db) {
        let url = cfg.netflow.asn_source_url.clone();
        tokio::spawn(async move {
            let client = rupu_netflow::http::client(rupu_netflow::FlowCtx::system(
                rupu_netflow::Origin::System,
            ));
            match rupu_netflow::asn::refresh(&url, &db, &client).await {
                Ok(()) => tracing::info!(path = ?db, "netflow ASN table refreshed"),
                Err(e) => tracing::warn!(error = %e, "netflow ASN refresh failed; keeping existing table"),
            }
        });
    }
}
```

The refresh request goes through the instrumented client, so it appears in the ledger as an `Origin::System` flow — the subsystem accounts for its own egress.

- [ ] **Step 6: Run tests**

Run: `cargo test -p rupu-cli`
Expected: PASS.

- [ ] **Step 7: Verify by hand**

```bash
rm -f ~/.rupu/netflow/asn.db
cargo run -p rupu-cli -- cp serve &
sleep 90
ls -la ~/.rupu/netflow/asn.db
kill %1
```

Expected: the file exists and is non-trivial in size. If it does not appear, check the log for the warn line — a network failure here is expected behaviour, not a bug, but it should be visible.

- [ ] **Step 8: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/cp.rs
git add crates/rupu-cli/
git commit -m "feat(cp): refresh the netflow ASN table automatically on the sweep tick"
```

---

### Task 10: Raise the choke-point lint to `deny`

**Files:**
- Modify: `Cargo.toml` (`[workspace.lints.clippy]`)
- Modify: `docs/scm.md` or the nearest operator-facing doc (add a netflow note)

**Interfaces:**
- Consumes: an empty `PENDING_PLAN_2` (Task 7).
- Produces: a build that fails if anyone adds a raw `reqwest` client.

- [ ] **Step 1: Confirm the register is empty**

Run: `grep -A3 "PENDING_PLAN_2" crates/rupu-netflow/tests/choke_point.rs`
Expected: `const PENDING_PLAN_2: &[&str] = &[];`

If it is not empty, stop — an earlier task is incomplete. Do not raise the lint over a non-empty register.

- [ ] **Step 2: Raise the lint**

In the root `Cargo.toml` under `[workspace.lints.clippy]`, change:

```toml
disallowed_methods = "warn"
```

to:

```toml
disallowed_methods = "deny"
```

- [ ] **Step 3: Verify the whole workspace still builds**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no errors. Any failure names a file that still builds a raw client — migrate it and add it to neither list.

- [ ] **Step 4: Verify the lint actually bites**

Temporarily add to any crate's `src/lib.rs`:

```rust
fn _proves_the_lint_fires() {
    let _ = reqwest::Client::new();
}
```

Run: `cargo clippy -p <that-crate> 2>&1 | grep disallowed`
Expected: an error mentioning `use rupu_netflow::http::client(ctx)`.

Then **delete the temporary function** and re-run `cargo clippy --workspace --all-targets -- -D warnings` to confirm clean.

- [ ] **Step 5: Drop the unused `reqwest` dependency from `rupu-webhook`**

The migration audit found that `crates/rupu-webhook/Cargo.toml:36` declares `reqwest.workspace = true` but `crates/rupu-webhook/src` contains **zero** references to it. Confirm, then remove:

Run: `grep -rn "reqwest" crates/rupu-webhook/src | wc -l`
Expected: `0`. If it is not zero, stop and migrate those sites instead — do not delete a dependency that is in use.

Then delete the `reqwest.workspace = true` line from `crates/rupu-webhook/Cargo.toml`.

Run: `cargo build -p rupu-webhook`
Expected: success. A failure means the grep missed a use (a macro, or a re-export) — restore the line and migrate instead.

- [ ] **Step 6: Document the rule**

Add to the crate-level docs in `crates/rupu-netflow/src/lib.rs`:

```rust
//! # Adding an HTTP client
//!
//! Don't. Call [`http::client`] or [`http::client_from`] instead — a raw
//! `reqwest::Client` bypasses capture entirely and `clippy.toml` denies
//! it workspace-wide. If you need custom tuning, pass a tuned
//! `reqwest::ClientBuilder` to [`http::client_from`]; every builder
//! option survives.
```

- [ ] **Step 7: Run the full suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/lib.rs
git add Cargo.toml crates/rupu-netflow/ crates/rupu-webhook/
git commit -m "feat(netflow): deny raw reqwest clients workspace-wide"
```

---

## Done when

- `cargo test --workspace` passes and `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- `PENDING_PLAN_2` is empty and the choke-point test passes.
- A real run leaves flows in `.rupu/netflow/flows.jsonl` **and** `net_flow` lines in its transcript.
- `~/.rupu/netflow/asn.db` appears on its own within one sweep interval of starting `cp serve`.
- Every GitHub flow carries `fidelity: "coarse"`; every other flow carries `fidelity: "http"`.

Plan 3 builds the CP API and views on this data.
