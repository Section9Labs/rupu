# Non-HTTP Host Detail Endpoints Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every CP detail endpoint that today 500s with `invalid: proxy_get_json is not supported for ssh hosts` returns real data for SSH (and Tunnel/Bucket) hosts.

**Architecture:** `HostConnector` grew a generic `proxy_get_json` escape hatch that only the HTTP transport can implement. Seven CP routes call it directly, so every one of them hard-fails on a non-HTTP host. The codebase already established the correct pattern for this — a *named, structured* trait method per capability, defaulting to `Unsupported`, implemented per transport (`list_sessions`, `list_runs`, `get_run`, `dashboard_summary`). This plan finishes that migration for the remaining seven call sites, using two mechanisms:

1. **Run-scoped endpoints** (`graph`, `usage-timeline`) need no remote call at all — SSH/Tunnel runs are already mirrored into the coordinator's own `RunStore` by `NodeMirror`. A transport-capability predicate routes them to the local mirror instead of the wire.
2. **Session / usage / netflow endpoints** hold data that is genuinely only on the remote. Each gets a structured `HostConnector` method that the SSH connector serves by shelling `rupu <cmd> --format json` over ssh, reusing the existing `remote_json` / `remote_json_item` / `remote_json_rows` helpers.

**Tech Stack:** Rust 2021, axum, `async_trait`, tokio, serde_json; clap for the two new CLI subcommands.

**Spec:** none — this plan is the design record. The defect report and root-cause trace live in the "Background" section below.

## Global Constraints

- Workspace deps only; versions pinned in the root `Cargo.toml`, never in a crate `Cargo.toml`.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- `rupu-cli` stays thin: subcommands are arg parsing + delegation.
- Errors: `thiserror` in libraries, `anyhow` in the CLI binary.
- **Never run package-wide `cargo fmt`** — `main` is fmt-dirty under the pinned toolchain. Format only the files you touched (`rustfmt <file>`).
- New `HostConnector` trait methods **must** carry a default body returning `HostConnectorError::Unsupported`. Several test fakes implement the trait exhaustively; a default-less method breaks them all.
- The remote `rupu` on a host may predate a command this plan adds. Every new SSH method maps a remote CLI failure to `Unsupported` naming the missing command — never to `NotFound`, and never to a silent empty result (see `get_run`'s existing precedent at `crates/rupu-cp/src/host/ssh.rs:1465`).
- `cp serve` is a daemon: it must be restarted to pick up any change here. Reinstalling the binary is not enough.

## Background — the defect and its root cause

Reported symptom, verbatim:

```json
{"error":"invalid: proxy_get_json is not supported for ssh hosts"}
```

That string is `SshHostConnector::proxy_get_json` (`crates/rupu-cp/src/host/ssh.rs:1671-1678`), a hard `Err(Invalid(..))` stub, rendered by `ApiError::internal` through `ApiError::into_response` (`crates/rupu-cp/src/error.rs:38-42`) as a 500 with that exact body.

It is reachable because several routes accept `?host=<id>` and unconditionally proxy a raw HTTP GET at whatever connector that id resolves to. The web launcher navigates to `/runs/<id>?host=<ssh id>` immediately after a remote launch (`crates/rupu-cp/web/src/components/LauncherSheet.tsx:92-93`), so **every run launched on an SSH host lands on a broken run-detail page.**

Full blast radius — every one of these produces that identical 500 on an SSH, Tunnel, or Bucket host:

| # | Route | Call site | Fixed by |
|---|---|---|---|
| 1 | `GET /api/runs/:id/graph` | `crates/rupu-cp/src/api/graph.rs:40` | Task 1 |
| 2 | `GET /api/runs/:id/usage-timeline` | `crates/rupu-cp/src/api/runs.rs:1052` | Task 1 |
| 3 | `GET /api/sessions/:id` | `crates/rupu-cp/src/api/sessions.rs:347` | Task 2 |
| 4 | `GET /api/sessions/:id/runs` | `crates/rupu-cp/src/api/sessions.rs:562` | Task 2 |
| 5 | `GET /api/sessions/:id/usage-timeline` | `crates/rupu-cp/src/api/sessions.rs:420` | Task 3 |
| 6 | `GET /api/usage` per-host rollup | `crates/rupu-cp/src/api/usage.rs:364` | Task 4 |
| 7 | `GET /api/runs/:id/netflow`, `/api/netflow/explorer` | `crates/rupu-cp/src/api/netflow.rs:1917,1961` | Task 5 |

Case 6 is the one that does *not* 500 — `usage.rs:411-425` already matches `Invalid`/`Unsupported` and renders the host as `unavailable` with a reason. It is an honest gap, not a crash, but it still means an all-SSH fleet reports zero usage.

Evidence that Task 1's local-mirror route is sound, gathered on the reporter's machine: the SSH-placed run `run_01M1JE8MGETW5M8HY9VC372SX4` (`worker_id: host_01M1JCRX2TB077PYN1JGPRFBK1`, an ssh host) has `run.json`, `step_results.jsonl`, `workflow.yaml`, and `unit_checkpoints.jsonl` present in the coordinator's own `~/.rupu/runs/`, written by `NodeMirror::create_run` + the tail pump. `build_run_graph_json` needs exactly those. The same run has **no** local netflow ledger under `~/.rupu/netflow/` — which is why Task 5 needs a real remote fetch rather than a mirror read.

## File Structure

**Modified**
- `crates/rupu-cp/src/host/connector.rs` — the four new trait members (one predicate + three structured methods), each with a default body.
- `crates/rupu-cp/src/host/ssh.rs` — SSH overrides for all four.
- `crates/rupu-cp/src/host/tunnel.rs`, `crates/rupu-cp/src/host/bucket/connector.rs` — predicate override only (`false`).
- `crates/rupu-cp/src/api/graph.rs`, `crates/rupu-cp/src/api/runs.rs` — mirror fallback (Task 1).
- `crates/rupu-cp/src/api/sessions.rs` — three routes switch from proxy to structured methods (Tasks 2, 3).
- `crates/rupu-cp/src/api/usage.rs` — host rollup switches to the structured method (Task 4).
- `crates/rupu-cp/src/api/netflow.rs` — two proxy helpers switch to the structured method (Task 5).
- `crates/rupu-cli/src/cmd/session.rs` — new `usage-timeline` action (Task 3).
- `crates/rupu-cli/src/cmd/netflow.rs` — new `show` action (Task 5).

**Rationale for the split:** the connector trait and each API route change together, so they stay in one task each. The two CLI additions live with the connector method that consumes them — a reviewer cannot sensibly approve `rupu session usage-timeline` without the SSH method that calls it.

---

### Task 1: Transport predicate + run-scoped mirror fallback

Fixes routes 1 and 2 — the run-detail page, which is the reported symptom.

**Files:**
- Modify: `crates/rupu-cp/src/host/connector.rs` (add predicate near `proxy_get_json`, ~line 240)
- Modify: `crates/rupu-cp/src/host/ssh.rs:1671`, `crates/rupu-cp/src/host/tunnel.rs:277`, `crates/rupu-cp/src/host/bucket/connector.rs:264`
- Modify: `crates/rupu-cp/src/api/graph.rs:34-49`
- Modify: `crates/rupu-cp/src/api/runs.rs:1045-1060`
- Test: `crates/rupu-cp/src/api/graph.rs` (inline `mod tests`), `crates/rupu-cp/src/host/ssh.rs` (inline `mod tests`)

**Interfaces:**
- Produces: `HostConnector::serves_runs_from_local_mirror(&self) -> bool` — `false` by default (HTTP hosts own their own run artifacts); `true` for SSH/Tunnel/Bucket, whose runs are mirrored into the coordinator's `RunStore` by `NodeMirror`. Tasks 2–5 do not consume it.

Name it for the *guarantee* it makes ("this transport's runs are in our mirror"), not for the mechanism it lacks ("can't proxy") — the callers need the former to justify reading the mirror.

- [x] **Step 1: Write the failing test**

In `crates/rupu-cp/src/host/ssh.rs`, inside the existing `mod tests`:

```rust
#[test]
fn ssh_serves_runs_from_local_mirror() {
    // SSH runs are created in and tailed into the coordinator's own
    // RunStore by NodeMirror, so run-scoped detail endpoints must read
    // that mirror rather than attempting a generic GET the transport
    // structurally cannot serve.
    let conn = test_connector_offline();
    assert!(conn.serves_runs_from_local_mirror());
}
```

Reuse whichever constructor the neighbouring tests use to build an `SshHostConnector` without touching the network (`ssh_pause_run_offline_surfaces_unreachable` at `ssh.rs:3294` shows the established shape); if there is no shared helper, add `fn test_connector_offline() -> SshHostConnector` next to the first test that builds one and have both use it.

In `crates/rupu-cp/src/api/graph.rs`, add an inline `mod tests` (or extend it if present):

```rust
#[tokio::test]
async fn run_graph_reads_the_mirror_for_a_transport_that_cannot_proxy() {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = test_state_with_mirrored_run(&tmp, "run_mirrored", "host_ssh");

    let resp = run_graph(
        State(s),
        Path("run_mirrored".into()),
        Query(RunDetailQuery { host: Some("host_ssh".into()) }),
    )
    .await
    .expect("a mirrored run must render from the local store, not 500");

    assert_eq!(resp.0["run"]["id"], serde_json::json!("run_mirrored"));
}
```

`test_state_with_mirrored_run` builds an `AppState` whose `run_store` root is `tmp`, writes a minimal `run.json` + empty `workflow.yaml` for the run id, and registers `host_ssh` in `s.hosts` as a connector whose `serves_runs_from_local_mirror()` returns `true` and whose `proxy_get_json` **panics** — so the test fails loudly if the route still reaches for the wire. Model the fake on `FakeHostConnector` at `crates/rupu-cp/src/api/runs.rs:2543`.

- [x] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p rupu-cp serves_runs_from_local_mirror reads_the_mirror
```

Expected: FAIL — `no method named serves_runs_from_local_mirror`.

- [x] **Step 3: Add the trait predicate**

In `crates/rupu-cp/src/host/connector.rs`, directly beneath `proxy_get_json`:

```rust
/// Whether this transport's runs are mirrored into the coordinator's own
/// `RunStore` (by `NodeMirror`) rather than living only on the remote.
///
/// `true` means run-scoped detail endpoints (`graph`, `usage-timeline`)
/// must build from the local mirror: the artifacts are already here, and
/// these transports have no generic-GET surface to proxy to anyway.
/// `false` (the default, and the HTTP connector's answer) means the run's
/// artifacts live on the remote and must be fetched.
fn serves_runs_from_local_mirror(&self) -> bool {
    false
}
```

Override with `true` in `ssh.rs`, `tunnel.rs`, and `bucket/connector.rs`, each directly above that file's `proxy_get_json`:

```rust
fn serves_runs_from_local_mirror(&self) -> bool {
    true
}
```

- [x] **Step 4: Route the two endpoints to the mirror**

In `crates/rupu-cp/src/api/graph.rs`, replace the body of `run_graph_from_host` (currently lines 34-49) with:

```rust
async fn run_graph_from_host(
    s: &AppState,
    host_id: &str,
    id: &str,
) -> ApiResult<serde_json::Value> {
    let conn = resolve_host(s, host_id)?;
    // SSH/Tunnel/Bucket runs are mirrored into our own RunStore, so the
    // graph is built from local artifacts — these transports have no
    // generic-GET surface to proxy to (this is what used to 500 with
    // "proxy_get_json is not supported for ssh hosts").
    if conn.serves_runs_from_local_mirror() {
        return build_run_graph_json(&s.run_store, &s.pricing, id);
    }
    conn.proxy_get_json(&format!("/api/runs/{id}/graph"))
        .await
        .map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Unreachable(m) => {
                ApiError::internal(format!("host {host_id} unreachable: {m}"))
            }
            other => ApiError::internal(other.to_string()),
        })
}
```

Apply the identical shape to `usage_timeline_from_host` in `crates/rupu-cp/src/api/runs.rs:1045-1060`, calling `build_usage_timeline_json(&s.run_store, id)` in the mirror branch.

Note both mirror branches call `run_not_found_or_internal` internally via `store.load`, so a run id that is genuinely absent from the mirror still 404s honestly rather than reporting an empty graph.

- [x] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p rupu-cp serves_runs_from_local_mirror reads_the_mirror
cargo test -p rupu-cp
```

Expected: PASS, and no regression elsewhere in `rupu-cp`.

- [x] **Step 6: Commit**

```bash
rustfmt crates/rupu-cp/src/host/connector.rs crates/rupu-cp/src/host/ssh.rs \
  crates/rupu-cp/src/host/tunnel.rs crates/rupu-cp/src/host/bucket/connector.rs \
  crates/rupu-cp/src/api/graph.rs crates/rupu-cp/src/api/runs.rs
git add -A && git commit -m "fix(cp): build run graph + usage timeline from the mirror for non-HTTP hosts"
```

---

### Task 2: Session detail + session runs over SSH

Fixes routes 3 and 4. `rupu session show <id> --format json` already emits everything both routes need — including a `runs[]` array with `transcript_path` — via the `session_show` report at `crates/rupu-cli/src/cmd/session.rs:1261-1311`. No CLI change is required.

**Files:**
- Modify: `crates/rupu-cp/src/host/connector.rs` (new `get_session` method)
- Modify: `crates/rupu-cp/src/host/ssh.rs` (override)
- Modify: `crates/rupu-cp/src/api/sessions.rs:338-386` and `:553-570`
- Test: `crates/rupu-cp/src/host/ssh.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `HostConnector::get_session(&self, id: &str) -> Result<serde_json::Value, HostConnectorError>` — returns the `item` object of the remote's `session show` report. Task 3 consumes it to enumerate a session's runs.

- [x] **Step 1: Write the failing test**

In `crates/rupu-cp/src/host/ssh.rs`'s `mod tests`, modelled on `list_sessions_shells_rupu_session_list_and_parses_rows` (`ssh.rs:2449`):

```rust
#[tokio::test]
async fn get_session_shells_rupu_session_show_and_returns_the_item() {
    struct Exec;
    #[async_trait::async_trait]
    impl RemoteExec for Exec {
        async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
            assert!(remote.contains("session"), "expected session show: {remote}");
            assert!(remote.contains("show"), "expected session show: {remote}");
            assert!(remote.contains("sess-1"), "expected the session id: {remote}");
            Ok(RemoteOutput {
                success: true,
                stdout: r#"{"kind":"session_show","version":1,
                  "item":{"session_id":"sess-1","agent":"scout","scope":"active",
                          "status":"idle","provider":"anthropic","model":"opus",
                          "total_turns":3,"total_tokens_in":10,"total_tokens_out":20,
                          "created_at":"2026-09-01T00:00:00Z",
                          "updated_at":"2026-09-01T01:00:00Z",
                          "runs":[{"run_id":"run_a","transcript_path":"/t/run_a.jsonl"}]}}"#
                    .into(),
                stderr: String::new(),
            })
        }
        async fn run_bytes(
            &self,
            _remote: &str,
            _stdin: Option<Vec<u8>>,
        ) -> Result<Vec<u8>, RemoteExecError> {
            unimplemented!("not exercised by this test")
        }
    }

    let conn = test_connector_with_exec(Exec);
    let item = conn.get_session("sess-1").await.unwrap();
    assert_eq!(item["session_id"], serde_json::json!("sess-1"));
    assert_eq!(item["runs"][0]["run_id"], serde_json::json!("run_a"));
}
```

Match `RemoteExec`'s exact signature and the neighbouring tests' connector-construction helper; copy them from `ssh.rs:2449-2505` rather than inventing new ones.

- [x] **Step 2: Run it to verify it fails**

```bash
cargo test -p rupu-cp get_session_shells_rupu_session_show
```

Expected: FAIL — `no method named get_session`.

- [x] **Step 3: Add the trait method and the SSH override**

In `connector.rs`, beside `list_sessions`:

```rust
/// Fetch one session's detail record from this host. The structured
/// counterpart to `proxy_get_json("/api/sessions/<id>")`, so non-HTTP
/// transports can serve session detail too — the SSH connector shells
/// `rupu session show <id> --format json` and returns its report `item`
/// (which carries the session's `runs[]` array as well). The default
/// errors so transports without session enumeration compile unchanged.
async fn get_session(&self, _id: &str) -> Result<serde_json::Value, HostConnectorError> {
    Err(HostConnectorError::Unsupported("session detail".into()))
}
```

In `ssh.rs`, beside `list_sessions`:

```rust
async fn get_session(&self, id: &str) -> Result<serde_json::Value, HostConnectorError> {
    match self
        .remote_json_item(&["--format", "json", "session", "show", id])
        .await
    {
        Ok(v) => Ok(v),
        Err(e) => {
            let message = e.to_string();
            if message.to_lowercase().contains("not found") && message.contains(id) {
                return Err(HostConnectorError::NotFound(id.to_string()));
            }
            tracing::warn!(
                host_id = %self.host_id,
                session_id = %id,
                error = %e,
                "get_session: remote `rupu session show` failed; host may predate the command"
            );
            Err(HostConnectorError::Unsupported(format!(
                "remote host {} does not support `rupu session show --format json`: {e}",
                self.host_id
            )))
        }
    }
}
```

The `--format json` precedes the subcommand deliberately — `Cmd::Run` is `trailing_var_arg` and swallows trailing flags; `get_run` documents this at `ssh.rs:1466-1468`. Keep the order consistent even for non-`run` commands.

- [x] **Step 4: Wire both routes**

In `crates/rupu-cp/src/api/sessions.rs`, replace the remote-proxy block of `get_session` (lines 338-354) with:

```rust
    if let Some(host) = q.host.as_deref().filter(|h| *h != "local") {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        let item = conn.get_session(&id).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(session_detail_from_remote_item(&item, &s.pricing)?));
    }
```

Add the mapper below `load_session_file`. It exists because the CLI report names fields for a human table (`agent`, `provider`) while the API's `SessionDto` names them for the web client (`agent_name`, `provider_name`); the local branch's `scope` and computed `usage` must be preserved too, so a remote session renders identically to a local one.

```rust
/// Map a remote `session show` report `item` onto the same shape the local
/// `get_session` branch returns: a `SessionDto` body plus `scope` and a
/// computed `usage` block. Field renames are deliberate — the CLI report
/// labels for a table, the API for the web client.
fn session_detail_from_remote_item(
    item: &serde_json::Value,
    pricing: &rupu_config::PricingConfig,
) -> ApiResult<serde_json::Value> {
    let str_at = |k: &str| item.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let opt_str_at = |k: &str| {
        item.get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let u64_at = |k: &str| item.get(k).and_then(|v| v.as_u64()).unwrap_or(0);

    let dto = SessionDto {
        session_id: str_at("session_id"),
        agent_name: str_at("agent"),
        model: str_at("model"),
        provider_name: str_at("provider"),
        status: item.get("status").cloned().unwrap_or(serde_json::Value::Null),
        total_turns: u64_at("total_turns") as u32,
        total_tokens_in: u64_at("total_tokens_in"),
        total_tokens_out: u64_at("total_tokens_out"),
        // `session show` does not report cached tokens; 0 is the honest
        // value here, not a placeholder for something we dropped.
        total_tokens_cached: 0,
        created_at: str_at("created_at"),
        updated_at: str_at("updated_at"),
        active_run_id: opt_str_at("active_run_id"),
        last_error: opt_str_at("last_error"),
        target: opt_str_at("target"),
        workspace_id: String::new(),
    };

    let usage = session_usage(&dto, pricing);
    let mut val = serde_json::to_value(&dto).map_err(|e| ApiError::internal(e.to_string()))?;
    if let serde_json::Value::Object(ref mut map) = val {
        map.insert(
            "scope".to_string(),
            serde_json::Value::String(
                item.get("scope")
                    .and_then(|v| v.as_str())
                    .unwrap_or("active")
                    .to_string(),
            ),
        );
        if let Ok(u) = serde_json::to_value(&usage) {
            map.insert("usage".to_string(), u);
        }
    }
    Ok(val)
}
```

Then replace `get_session_runs`'s proxy block (lines 558-570) with:

```rust
    if let Some(host) = q.host.as_deref().filter(|h| *h != "local") {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        let item = conn.get_session(&id).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            other => ApiError::internal(other.to_string()),
        })?;
        let runs = item
            .get("runs")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
        return Ok(Json(runs));
    }
```

Check the local branch's response envelope before finalising this — if it returns `{ "runs": [...] }` rather than a bare array, wrap accordingly so the web client sees one shape from both branches.

- [x] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p rupu-cp get_session_shells_rupu_session_show
cargo test -p rupu-cp
```

Expected: PASS.

- [x] **Step 6: Commit**

```bash
rustfmt crates/rupu-cp/src/host/connector.rs crates/rupu-cp/src/host/ssh.rs \
  crates/rupu-cp/src/api/sessions.rs
git add -A && git commit -m "feat(cp): serve remote session detail + runs over ssh via session show"
```

---

### Task 3: Session usage-timeline over SSH

Fixes route 5. The local branch builds a per-*turn* token series by reading each of the session's run transcripts. Over SSH those transcripts are not mirrored (only *run* artifacts are), so this needs a remote computation.

**Two remote calls per session was rejected as the design.** Fetching each run's transcript individually would be N ssh round trips per page load, and bursting ssh connections at a host is a known hazard on this fleet. Instead the CLI grows a subcommand that does the whole computation remotely in **one** call, returning the same series the CP builds locally — exact parity, one round trip.

**Files:**
- Modify: `crates/rupu-cli/src/cmd/session.rs` (new `Action::UsageTimeline`, its dispatch arm, its report-kind arm, and the handler)
- Modify: `crates/rupu-cp/src/host/connector.rs` (new `session_usage_timeline` method)
- Modify: `crates/rupu-cp/src/host/ssh.rs` (override)
- Modify: `crates/rupu-cp/src/api/sessions.rs:411-427`
- Test: `crates/rupu-cli/src/cmd/session.rs` (inline `mod tests`), `crates/rupu-cp/src/host/ssh.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: Task 2's `get_session` is **not** used here — the new CLI command resolves the session's runs itself, remotely.
- Produces: `HostConnector::session_usage_timeline(&self, id: &str) -> Result<serde_json::Value, HostConnectorError>` — a JSON array of the same per-turn points the local branch emits.

- [x] **Step 1: Write the failing CLI test**

Before writing it, read `crates/rupu-cp/src/api/sessions.rs`'s local `get_session_usage_timeline` body in full and copy its point shape exactly — field names, ordering, and the run-id label. The test asserts that shape:

```rust
#[test]
fn session_usage_timeline_emits_one_point_per_turn_labeled_by_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Build a session with two runs, each with a two-turn transcript.
    let session_id = write_test_session_with_transcripts(&tmp, &[("run_a", 2), ("run_b", 2)]);

    let series = build_session_usage_timeline(tmp.path(), &session_id).unwrap();

    assert_eq!(series.len(), 4, "two runs x two turns");
    assert_eq!(series[0].run_id, "run_a");
    assert_eq!(series[3].run_id, "run_b");
}
```

`write_test_session_with_transcripts` writes a `sessions/<id>/session.json` naming each run plus its `transcript_path`, and a `.jsonl` transcript per run with the given number of assistant turns carrying token counts. Mirror whatever transcript fixture helper `cmd/session.rs`'s existing tests already use rather than inventing a second one.

- [x] **Step 2: Run it to verify it fails**

```bash
cargo test -p rupu-cli session_usage_timeline_emits_one_point_per_turn
```

Expected: FAIL — `cannot find function build_session_usage_timeline`.

- [x] **Step 3: Add the CLI subcommand**

In `crates/rupu-cli/src/cmd/session.rs`, add to `enum Action` (after `Show`):

```rust
    /// Per-turn token series across every run this session recorded.
    UsageTimeline {
        #[arg(add = ArgValueCompleter::new(session_ids))]
        session_id: String,
    },
```

Add the dispatch arm beside `Action::Show`'s (near line 1123):

```rust
        Action::UsageTimeline { session_id } => {
            usage_timeline(&session_id, global_format).await
        }
```

and the report-kind arm beside `Show`'s (near line 1145):

```rust
        Action::UsageTimeline { .. } => ("session usage-timeline", report::TABLE_JSON),
```

Implement `build_session_usage_timeline(global: &Path, session_id: &str) -> anyhow::Result<Vec<UsageTimelinePoint>>` by porting the local computation out of `crates/rupu-cp/src/api/sessions.rs`'s `get_session_usage_timeline`: read `session.json`, walk its `runs[]` in recorded order, parse each `transcript_path` as JSONL, and emit one point per assistant turn labelled with that run's id. Then `usage_timeline` wraps the result in the crate's collection-report envelope and calls `report::emit_collection(global_format, &output)`.

**Do not duplicate the aggregation logic between crates.** `rupu-cp` and `rupu-cli` both depend on the transcript reader already; if the per-turn fold is more than a few lines, lift it into the shared crate that owns transcript parsing and have both call sites use it. A silently-diverging second implementation of the same series is exactly the kind of drift that makes a remote session's chart disagree with a local one.

- [x] **Step 4: Run the CLI test to verify it passes**

```bash
cargo test -p rupu-cli session_usage_timeline_emits_one_point_per_turn
```

Expected: PASS.

- [x] **Step 5: Add the connector method, its test, and the route wiring**

Trait default in `connector.rs`:

```rust
/// Per-turn token series for one session on this host. The structured
/// counterpart to `proxy_get_json("/api/sessions/<id>/usage-timeline")`.
/// The SSH connector shells `rupu session usage-timeline <id> --format
/// json` — one round trip, computed remotely, rather than fetching each
/// run's transcript separately.
async fn session_usage_timeline(
    &self,
    _id: &str,
) -> Result<serde_json::Value, HostConnectorError> {
    Err(HostConnectorError::Unsupported("session usage timeline".into()))
}
```

SSH override, returning the report's `rows`:

```rust
async fn session_usage_timeline(
    &self,
    id: &str,
) -> Result<serde_json::Value, HostConnectorError> {
    let rows = self
        .remote_json_rows(&["--format", "json", "session", "usage-timeline", id])
        .await
        .map_err(|e| {
            tracing::warn!(
                host_id = %self.host_id,
                session_id = %id,
                error = %e,
                "session_usage_timeline: remote command failed; host may predate it"
            );
            HostConnectorError::Unsupported(format!(
                "remote host {} does not support `rupu session usage-timeline`: {e}",
                self.host_id
            ))
        })?;
    Ok(serde_json::Value::Array(rows))
}
```

Add a `mod tests` case mirroring Task 2's Step 1 shape — a fake `RemoteExec` asserting the argv contains `session`, `usage-timeline`, and the id, returning a two-row report — then assert the method returns a two-element array.

Finally replace the proxy block in `sessions.rs:415-427`:

```rust
    if let Some(host) = q.host.as_deref().filter(|h| *h != "local") {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        let v = conn.session_usage_timeline(&id).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(v));
    }
```

- [x] **Step 6: Run the full suites to verify**

```bash
cargo test -p rupu-cli && cargo test -p rupu-cp
```

Expected: PASS.

- [x] **Step 7: Commit**

```bash
rustfmt crates/rupu-cli/src/cmd/session.rs crates/rupu-cp/src/host/connector.rs \
  crates/rupu-cp/src/host/ssh.rs crates/rupu-cp/src/api/sessions.rs
git add -A && git commit -m "feat: add rupu session usage-timeline and serve it over ssh"
```

---

### Task 4: Usage rollup over SSH

Fixes route 6 — today an all-SSH fleet reports every host as `unavailable` and contributes zero usage.

**Files:**
- Modify: `crates/rupu-cp/src/host/connector.rs` (new `usage_rollup` method)
- Modify: `crates/rupu-cp/src/host/ssh.rs` (override)
- Modify: `crates/rupu-cp/src/api/usage.rs:357-430`
- Test: `crates/rupu-cp/src/host/ssh.rs`, `crates/rupu-cp/src/api/usage.rs` (inline `mod tests`)

**Interfaces:**
- Produces: `HostConnector::usage_rollup(&self, since: &str, until: &str, group_by: &str) -> Result<serde_json::Value, HostConnectorError>` — `since`/`until` are RFC-3339; `group_by` is the CP's `GroupBy::as_str()` value. Returns the remote `usage` report verbatim; the caller maps it.

- [x] **Step 1: Write the failing mapper test**

The CLI's breakdown report and the CP's `RemoteUsageBody` are near-identical but not the same shape: the CLI emits `cost_partial` where the CP wants `priced`, has no `total_tokens` (it is `input + output`), and carries no `host_id`/`workspace_id`. Pin the mapping first, in `crates/rupu-cp/src/api/usage.rs`'s `mod tests`:

```rust
#[test]
fn remote_usage_report_maps_onto_the_cp_body() {
    let report = serde_json::json!({
        "kind": "usage", "version": 1,
        "rows": [{
            "group": "anthropic/opus", "provider": "anthropic", "model": "opus",
            "agent": "scout", "workflow": "", "repo": "", "day": "",
            "input_tokens": 100, "output_tokens": 50, "cached_tokens": 10,
            "runs": 2, "cost_usd": 1.25, "cost_partial": false
        }]
    });

    let body = usage_body_from_remote_report(&report).expect("mappable");

    assert_eq!(body.breakdown.len(), 1);
    assert_eq!(body.breakdown[0].provider, "anthropic");
    assert_eq!(body.breakdown[0].total_tokens, 150, "input + output, not cached");
    assert!(body.breakdown[0].priced, "cost_partial:false means priced");
    assert_eq!(body.summary.input_tokens, 100, "summary folds the rows");
}
```

Before writing the implementation, run `rupu usage --group-by model --format json` locally and paste a real row into this test — the field names above are read from `UsageBreakdownOutput::csv_headers` (`crates/rupu-cli/src/cmd/usage.rs:523-537`) and must be confirmed against the actual JSON report, which may differ.

- [x] **Step 2: Run it to verify it fails**

```bash
cargo test -p rupu-cp remote_usage_report_maps_onto_the_cp_body
```

Expected: FAIL — `cannot find function usage_body_from_remote_report`.

- [x] **Step 3: Implement the mapper**

In `crates/rupu-cp/src/api/usage.rs`, write `fn usage_body_from_remote_report(report: &serde_json::Value) -> Result<RemoteUsageBody, String>` folding each `rows[]` entry into a `UsageBreakdownRow` (leaving `host_id`/`workspace_id` empty — the caller already overrides `host_id` for `group_by=host` at lines 371-381) and summing the rows into the `UsageSummary`. Compute `unpriced` from the rows carrying `cost_partial: true`, matching however `UnpricedGap` is populated on the local path.

- [x] **Step 4: Add the connector method and wire the route**

Trait default in `connector.rs`:

```rust
/// Token/cost rollup for a time window on this host. The structured
/// counterpart to `proxy_get_json("/api/usage?...")`. The SSH connector
/// shells `rupu usage --since <s> --until <u> --group-by <g> --format
/// json`; the caller maps the CLI report onto the API body.
async fn usage_rollup(
    &self,
    _since: &str,
    _until: &str,
    _group_by: &str,
) -> Result<serde_json::Value, HostConnectorError> {
    Err(HostConnectorError::Unsupported("usage rollup".into()))
}
```

SSH override:

```rust
async fn usage_rollup(
    &self,
    since: &str,
    until: &str,
    group_by: &str,
) -> Result<serde_json::Value, HostConnectorError> {
    self.remote_json(&[
        "--format", "json", "usage",
        "--since", since,
        "--until", until,
        "--group-by", group_by,
    ])
    .await
}
```

Confirm `rupu usage` actually accepts `--group-by` with the CP's group names before relying on it (`UsageGroupBy` at `crates/rupu-cli/src/cmd/usage.rs:23-31` lists `composite|provider|model|agent|workflow|repo|day`). The CP's `GroupBy` also has a `Host` variant with no CLI equivalent — for that case pass `composite` and let the existing caller-side `host_id` override collapse the remote's rows into that host's bucket.

Then in `usage.rs`'s per-host closure, try `usage_rollup` first and keep the existing `proxy_get_json` path as the HTTP branch. A host that supports neither still renders `unavailable` with a reason — that behaviour is correct and must not regress.

- [x] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p rupu-cp usage
cargo test -p rupu-cp
```

Expected: PASS.

- [x] **Step 6: Commit**

```bash
rustfmt crates/rupu-cp/src/host/connector.rs crates/rupu-cp/src/host/ssh.rs \
  crates/rupu-cp/src/api/usage.rs
git add -A && git commit -m "feat(cp): roll up remote usage over ssh via rupu usage --format json"
```

---

### Task 5: Netflow over SSH — new `rupu netflow show`

Fixes route 7. **This task expands the public CLI surface** — `rupu netflow` currently has exactly one subcommand (`prune`, `crates/rupu-cli/src/cmd/netflow.rs:30-33`) and no way to *read* a ledger. Verified on the reporter's machine: an SSH-placed run has no local netflow ledger, because its network activity is recorded by the runner on the remote. There is no mirror to fall back on, so serving this honestly requires a remote reader that does not exist yet.

**If this task is dropped**, do not leave route 7 as a 500. Replace the `other => ApiError::internal(..)` arm in both `explorer_from_host` and `run_netflow_from_host` with an honest empty response carrying a `reason` naming the host and the fact that its ledgers are not readable over this transport — the same posture `usage.rs` already takes.

**Files:**
- Modify: `crates/rupu-cli/src/cmd/netflow.rs` (new `Action::Show`, dispatch arm, report-kind arm, handler)
- Modify: `crates/rupu-cp/src/host/connector.rs`, `crates/rupu-cp/src/host/ssh.rs`
- Modify: `crates/rupu-cp/src/api/netflow.rs:1900-1985`
- Test: `crates/rupu-cli/src/cmd/netflow.rs`, `crates/rupu-cp/src/host/ssh.rs` (inline `mod tests`)

**Interfaces:**
- Produces: `HostConnector::run_netflow(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError>` — the run's raw flow records, which the CP then enriches (ASN table, spans) and aggregates locally.

Return **raw flows**, not an aggregated response: the CP's existing `enforce_range_on_proxied_response` / `enforce_filters_on_proxied_response` defensive re-filtering (`netflow.rs:1975-1977`) exists precisely because a remote's aggregation cannot be trusted to have applied this build's window and filters. Fetching raw records and aggregating locally removes that whole class of drift, and the local ASN table then applies uniformly across hosts.

- [x] **Step 1: Write the failing CLI test**

```rust
#[test]
fn netflow_show_emits_the_runs_ledger_records() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_test_ledger(&tmp, "run_a", 3); // 3 flow records

    let rows = collect_run_ledger(tmp.path(), "run_a").unwrap();

    assert_eq!(rows.len(), 3);
}

#[test]
fn netflow_show_is_empty_for_a_run_with_no_ledger() {
    let tmp = tempfile::TempDir::new().unwrap();
    let rows = collect_run_ledger(tmp.path(), "run_missing").unwrap();
    assert!(rows.is_empty(), "absent ledger is empty, never an error");
}
```

Reuse `rupu_netflow`'s own ledger fixture helpers if the crate exposes any; the on-disk shape is `<global>/netflow/<run_id>.jsonl`.

- [x] **Step 2: Run them to verify they fail**

```bash
cargo test -p rupu-cli netflow_show
```

Expected: FAIL — `cannot find function collect_run_ledger`.

- [x] **Step 3: Add the CLI subcommand**

Add to `crates/rupu-cli/src/cmd/netflow.rs`'s `enum Action`:

```rust
    /// Print one run's netflow ledger records.
    Show {
        #[arg(add = ArgValueCompleter::new(transcript_run_ids))]
        run_id: String,
    },
```

with its dispatch arm and `Action::Show { .. } => ("netflow show", report::TABLE_JSON_CSV)`. Implement `collect_run_ledger(global: &Path, run_id: &str) -> anyhow::Result<Vec<FlowRecord>>` reading `<global>/netflow/<run_id>.jsonl` through `rupu_netflow`'s existing reader, returning an empty vec when the file is absent. Emit via `report::emit_collection`.

- [x] **Step 4: Add the connector method and wire both netflow routes**

Trait default and SSH override follow Task 3's `session_usage_timeline` shape exactly, shelling `["--format", "json", "netflow", "show", run_id]` and returning `remote_json_rows`.

In `crates/rupu-cp/src/api/netflow.rs`, rewrite `run_netflow_from_host` and `explorer_from_host` so that when `conn.run_netflow(run_id)` succeeds they feed those raw records into the **same** local aggregation the `Global` branch uses (`to_explorer_flows` + `build_explorer_response`, `netflow.rs:1858-1877`), rather than relaying a remote aggregate. Keep the existing proxy path for HTTP hosts, including its re-enforcement passes.

- [x] **Step 5: Run the suites**

```bash
cargo test -p rupu-cli netflow && cargo test -p rupu-cp netflow && cargo test -p rupu-cp
```

Expected: PASS.

- [x] **Step 6: Commit**

```bash
rustfmt crates/rupu-cli/src/cmd/netflow.rs crates/rupu-cp/src/host/connector.rs \
  crates/rupu-cp/src/host/ssh.rs crates/rupu-cp/src/api/netflow.rs
git add -A && git commit -m "feat: add rupu netflow show and aggregate remote ledgers locally"
```

---

### Task 6: End-to-end verification against a live SSH host

Unit tests prove the wiring; they cannot prove the remote `rupu` answers as expected. This task is the honest verification gate and **must not be skipped** — every previous task's SSH branch is untested against a real host until it runs.

**Files:** none modified — this is a verification task.

- [x] **Step 1: Rebuild and restart the daemon**

`cp serve` is a long-lived daemon; a reinstalled binary does **not** update a running one.

```bash
make cp-web && make release
```

Then stop and restart the running `rupu cp serve` before testing anything below.

- [x] **Step 2: Confirm the reported symptom is gone**

Launch a run on an SSH host from the CP web launcher and open its detail page. Confirm the graph, transcript, and token chart render. Then check the raw endpoints directly (substitute a real run id and the ssh host id from `~/.rupu/hosts/*.toml`):

```bash
curl -s "http://127.0.0.1:7420/api/runs/<run_id>/graph?host=<ssh_host_id>" | head -c 400
```

Expected: a `{"run":...,"workflow":...}` body. **Not** `{"error":"invalid: proxy_get_json is not supported for ssh hosts"}`.

- [x] **Step 3: Exercise every other fixed route**

```bash
curl -s "http://127.0.0.1:7420/api/runs/<run_id>/usage-timeline?host=<ssh_host_id>" | head -c 200
curl -s "http://127.0.0.1:7420/api/sessions/<session_id>?host=<ssh_host_id>" | head -c 300
curl -s "http://127.0.0.1:7420/api/sessions/<session_id>/runs?host=<ssh_host_id>" | head -c 200
curl -s "http://127.0.0.1:7420/api/sessions/<session_id>/usage-timeline?host=<ssh_host_id>" | head -c 200
curl -s "http://127.0.0.1:7420/api/usage?since=2026-08-01T00:00:00Z&until=2026-09-30T00:00:00Z&group_by=model" | head -c 400
curl -s "http://127.0.0.1:7420/api/runs/<run_id>/netflow?host=<ssh_host_id>" | head -c 200
```

For the usage call, confirm the `hosts[]` array reports the ssh hosts as `ok` — not `unavailable`.

Record the actual output of each command in the PR description. A route that returns an honest empty result is a pass; a route that 500s is not.

- [x] **Step 4: Confirm no ssh connection burst**

These endpoints now shell out over ssh. Watch that a single page load does not fan out into many rapid connections to the same host — a burst previously earned a lasting block on one of this fleet's hosts. If a page load issues more than a couple of connections per host, batch them before merging.

- [x] **Step 5: Open the PR**

Per repo convention this work goes through a feature branch and a PR — never a direct commit to `main`.

---

## Self-Review

**Spec coverage.** All seven call sites from the Background table map to a task: routes 1–2 → Task 1, 3–4 → Task 2, 5 → Task 3, 6 → Task 4, 7 → Task 5. Task 6 verifies all of them against a live host.

**Placeholder scan.** Two steps deliberately instruct the implementer to confirm a shape against real output before finalising (Task 4 Step 1's `rupu usage` JSON row; Task 2 Step 4's `get_session_runs` envelope) rather than asserting a field list read only from CSV headers and route source. These are verification instructions with a named command and a named source file, not deferred design.

**Type consistency.** Four new trait members, each with a default and each used under one name throughout: `serves_runs_from_local_mirror` (Task 1), `get_session` (Task 2, consumed nowhere else), `session_usage_timeline` (Task 3), `usage_rollup` (Task 4), `run_netflow` (Task 5).

**Known risk carried by this plan.** Tasks 3 and 5 each add a public CLI subcommand, and both create a version dependency: a host running an older `rupu` cannot serve them, and will surface `Unsupported` naming the missing command. That is the intended failure mode — honest and actionable — but it means these two routes stay degraded against any host until its `rupu` is updated. Task 1 and Task 2 have no such dependency (Task 2 uses a command that already exists).

---

## As-built (2026-09-02)

All six tasks executed. Three deliberate deviations from the plan above,
plus two defects the plan did not anticipate — recorded here rather than
edited into the tasks, so the plan stays readable as the design it was.

**Deviation 1 — Task 2 became two trait methods, not one.** The plan had
`get_session` also answering `/runs` from its report body. That silently
regressed HTTP federation to an empty runs list: the API session DTO has no
`runs` field, so an HTTP host must proxy the dedicated `/runs` endpoint.
Caught by `federation_e2e`. Split into `get_session` + `session_runs`, each
with an HTTP override.

**Deviation 2 — pricing moved out of the mapper.** The plan had the CP map
the remote report into a `SessionDto` and price it. But `SshHostConnector`
deliberately carries no pricing config, and an HTTP remote already priced
its own session with its own config. The connector now returns API-shaped
fields and `ensure_usage_block` prices only a body that arrived unpriced —
an HTTP remote's own `usage` is left untouched rather than overwritten.

**Deviation 3 — Task 4's mapper was written against real output.** The plan
derived field names from `UsageBreakdownOutput::csv_headers`. The actual
`--format json` report differs: totals nest under `summary` with `total_`-
prefixed names, and rows populate only the identity fields their `group_by`
selected. Also handled explicitly: CP's `host` and `project` groupings have
no CLI counterpart — `host` rebuilds its single row from the summary,
`project` reports `unavailable` with that reason.

**Defect found mid-task — `session show` says "unknown session".** The
connector matched only "not found" (which is `run show`'s phrasing), so a
genuinely-missing remote session would have surfaced as a 500 "this host
cannot answer" instead of a 404. Found by running the real binary, not by
reading it. Fixed with a regression test.

**Defect found mid-task — "host unreachable" on a host that answered.**
`remote_json` maps a non-zero remote exit to `Unreachable`, so an
out-of-date remote's "unrecognized subcommand" reached the operator prefixed
with "host unreachable:" — pointing at the network instead of the binary.
Fixed with `remote_failure_detail`.

### Verification performed

Against an isolated `RUPU_HOME` (seeded with a real mirrored SSH run and the
real host registry) on port 7599 — never against the live daemon:

| Endpoint | Result |
|---|---|
| `/api/runs/:id/graph?host=<ssh>` | **200**, real graph — the reported symptom |
| `/api/runs/:id/usage-timeline?host=<ssh>` | 200, 44 turn points |
| `/api/usage` across three ssh hosts | 200, all three `ok`, 20.4M tokens / 55 runs that previously reported zero |
| `/api/sessions/:id?host=<ssh>` | 404 (live ssh round trip; "unknown session" mapped correctly) |
| `/api/sessions/:id/runs?host=<ssh>` | 404, same path |
| `/api/sessions/:id/usage-timeline?host=<ssh>` | 500 naming the missing remote command — correct, remote predates it |
| `/api/runs/:id/netflow` | see the open finding below |

Not verified live: session detail against a session that exists (no remote
host has any sessions), and every path that needs the two NEW CLI commands
on the remote (`session usage-timeline`, `netflow show`) — those require
deploying this build to the hosts.

### RESOLVED — placed-run netflow is silently empty

`GET /api/runs/:id/netflow` has **no `?host=` branch** — its `host` query
param is a flow *filter*, not a host selector — so it dispatches purely
through `resolve_run_location`. A run placed on an SSH host is mirrored into
the coordinator's own `RunStore`, so it resolves `Global`, and the endpoint
reads local ledgers that do not exist for it. The result is an empty flow
set returned as 200: a silently wrong answer rather than an error.

Task 5 fixed the `RunLocation::Host` path (an autoflow run whose history
recorded an SSH host), which is the one that produced the reported error.
The mirrored-run case is a distinct, pre-existing defect. The fix is to
merge the remote's ledger with the local one when a run's `worker_id` names
a registered non-local host — both sets are real flows for that run. **Fixed** (commit `a4917241`), after the open design question was settled:
a host that cannot answer never fails the read — that would throw away the
local half over an offline host — but it is NAMED. Both responses gained
`incomplete: [{host_id, reason}]`, and the web table renders it as a loud
banner on the populated path as well as the empty one, because a partial
list with no warning reads as a complete one and every number above it is
understated by an unknown amount.

The remote flows join the LEDGER side of the merge
(`run_scoped_flows_and_dropped_with`), not the result: a mirrored transcript
carries the snapshot each flow had when first recorded, while the ledger
carries the finalized record, so appending afterwards would let the degraded
local copy win an id collision.
