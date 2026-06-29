# Multi-host Slice 2.5 — finish the tunnel (approve/reject/resume) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route approval-gate approve/reject/resume of tunnel-node runs over the existing dial-home WebSocket, so a gated node run can be unblocked from the central CP — mirroring how `cancel` already works.

**Architecture:** Two new CP→node `Frame` variants (`Approve`, `Reject`) modeled line-for-line on `Frame::Cancel`. `TunnelHostConnector` sends them over the live connection (offline → `Unreachable`). The `rupu node` agent receives them and shells out to its existing `rupu workflow approve` / `rupu workflow reject` against the local run; the resumed run's output streams back through the artifact tail already running. The central resume worker never touches tunnel runs (they never carry the `resume_requested_at` marker; a defense-in-depth filter also skips runs attributed to a tunnel node).

**Tech Stack:** Rust 2021, tokio, axum (WS), tokio-tungstenite (node client), serde, thiserror (libs) / anyhow (CLI).

## Global Constraints

- Workspace deps only — versions pinned in the ROOT `Cargo.toml`, never in a crate `Cargo.toml`. (This slice adds NO new dependencies.)
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden (no `unsafe`).
- Library errors use `thiserror`; the CLI binary uses `anyhow`.
- Per-file `rustfmt` only — `main` is fmt-dirty; NEVER run workspace-wide `cargo fmt`.
- `rupu-cli` has a PRE-EXISTING red toolchain baseline under the local Homebrew toolchain. Verify only that NEW code compiles and its tests pass; do not try to fix unrelated baseline failures.
- New `Frame` variants are CP→node only; they ride the already-authenticated, token-gated tunnel — no new inbound surface, no new auth path.
- Scope is approval-gate approve/reject/resume ONLY. Sessions are deferred to Slice 5. Standalone `rupu workflow resume` of terminal runs is out of scope.
- Direct-frame semantics: the node must be online; offline → a clear `Unreachable` error (no durable/queued approvals).

---

## File Structure

- `crates/rupu-cp/src/node/protocol.rs` — add `Frame::Approve` + `Frame::Reject` variants (+ round-trip tests). (Task 1)
- `crates/rupu-cli/src/cmd/node.rs` — handle the two new inbound frames; spawn `rupu workflow approve`/`reject`; re-point the `active` child handle. Temporary pass-through arm added in Task 1, real handling in Task 3. (Tasks 1, 3)
- `crates/rupu-cp/src/host/tunnel.rs` — replace the `approve_run`/`reject_run` `Invalid` stubs with live-frame sends. (Task 2)
- `crates/rupu-cli/src/cmd/cp.rs` — defense-in-depth: resume worker skips runs attributed to a tunnel node. (Task 4)
- `crates/rupu-cp/tests/node_tunnel.rs` — connector unit tests (Task 2), invariant test (Task 4), and the e2e extension (Task 5).

---

## Task 1: Protocol — add `Approve` / `Reject` frames

**Files:**
- Modify: `crates/rupu-cp/src/node/protocol.rs` (the `Frame` enum + a `#[cfg(test)]` round-trip module)
- Modify: `crates/rupu-cli/src/cmd/node.rs` (keep the workspace compiling — add a temporary pass-through arm; real handling lands in Task 3)

**Interfaces:**
- Produces: `Frame::Approve { run_id: String, mode: String }` and `Frame::Reject { run_id: String, reason: Option<String> }` on the existing `#[serde(tag = "type", rename_all = "snake_case")]` `Frame` enum. JSON tags: `"approve"`, `"reject"`.

- [ ] **Step 1: Write the failing round-trip tests**

In the existing `#[cfg(test)] mod tests` in `crates/rupu-cp/src/node/protocol.rs` (follow the existing round-trip test style for `Frame::Cancel`), add:

```rust
#[test]
fn approve_frame_round_trips() {
    let f = Frame::Approve {
        run_id: "run_01ABC".to_string(),
        mode: "bypass".to_string(),
    };
    let json = serde_json::to_string(&f).unwrap();
    assert!(json.contains(r#""type":"approve""#));
    assert!(json.contains(r#""run_id":"run_01ABC""#));
    assert!(json.contains(r#""mode":"bypass""#));
    let back: Frame = serde_json::from_str(&json).unwrap();
    assert_eq!(back, f);
}

#[test]
fn reject_frame_round_trips_with_and_without_reason() {
    let with = Frame::Reject {
        run_id: "run_01ABC".to_string(),
        reason: Some("not now".to_string()),
    };
    let json = serde_json::to_string(&with).unwrap();
    assert!(json.contains(r#""type":"reject""#));
    assert!(json.contains(r#""reason":"not now""#));
    assert_eq!(serde_json::from_str::<Frame>(&json).unwrap(), with);

    let without = Frame::Reject {
        run_id: "run_01ABC".to_string(),
        reason: None,
    };
    let json2 = serde_json::to_string(&without).unwrap();
    assert_eq!(serde_json::from_str::<Frame>(&json2).unwrap(), without);
}
```

Note: `Frame` must `derive(PartialEq)` for `assert_eq!`. It already does (the existing `Cancel` round-trip test uses `assert_eq!`); if not, add `PartialEq` to its derive list.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp --lib node::protocol`
Expected: FAIL — `no variant named Approve`/`Reject`.

- [ ] **Step 3: Add the two variants**

In the `pub enum Frame { … }` in `crates/rupu-cp/src/node/protocol.rs`, add (after the `Cancel` variant, keeping the existing doc-comment style):

```rust
    /// CP→node: approve a run paused at an approval gate. `mode` is the
    /// resume mode (`"ask"` | `"bypass"` | `"readonly"`); empty means the
    /// node uses the run's stored mode / default.
    Approve {
        run_id: String,
        mode: String,
    },
    /// CP→node: reject a run paused at an approval gate.
    Reject {
        run_id: String,
        reason: Option<String>,
    },
```

- [ ] **Step 4: Keep the workspace compiling — temporary pass-through arm**

The node's inbound `match frame { … }` in `crates/rupu-cli/src/cmd/node.rs` is exhaustive (no catch-all). Add `Frame::Approve { .. }` and `Frame::Reject { .. }` to the existing ignore/warn arm so the workspace still compiles; real handling lands in Task 3. Change:

```rust
            Frame::Hello { .. }
            | Frame::Welcome {}
            | Frame::Pong {}
            | Frame::Artifact { .. }
            | Frame::RunFinished { .. } => {
```

to:

```rust
            // NOTE: Approve/Reject are handled for real in Slice 2.5 Task 3;
            // this temporary pass-through keeps the workspace compiling.
            Frame::Approve { .. }
            | Frame::Reject { .. }
            | Frame::Hello { .. }
            | Frame::Welcome {}
            | Frame::Pong {}
            | Frame::Artifact { .. }
            | Frame::RunFinished { .. } => {
```

(The CP-side read pump in `crates/rupu-cp/src/node/server.rs` already has a catch-all `other =>` arm, so it needs no change.)

- [ ] **Step 5: Run tests + compile checks to verify they pass**

Run: `cargo test -p rupu-cp --lib node::protocol`
Expected: PASS (both new tests).
Run: `cargo build -p rupu-cp -p rupu-cli`
Expected: compiles (the temporary arm keeps `rupu-cli` green).
Run: `cargo clippy -p rupu-cp`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/node/protocol.rs crates/rupu-cli/src/cmd/node.rs
git commit -m "feat(cp): Approve/Reject tunnel frames (protocol)"
```

---

## Task 2: `TunnelHostConnector` — send Approve/Reject frames

**Files:**
- Modify: `crates/rupu-cp/src/host/tunnel.rs:249-263` (replace the `approve_run` + `reject_run` `Invalid` stubs)
- Test: `crates/rupu-cp/tests/node_tunnel.rs` (extend)

**Interfaces:**
- Consumes: `Frame::Approve { run_id, mode }`, `Frame::Reject { run_id, reason }` (Task 1); `self.live_conn() -> Result<Arc<NodeConn>, HostConnectorError>` and `NodeConn::send(Frame) -> Result<(), _>` (existing).
- Produces: working `HostConnector::approve_run` / `reject_run` for tunnel hosts.

- [ ] **Step 1: Write the failing tests**

In `crates/rupu-cp/tests/node_tunnel.rs`, reuse the existing `TunnelHostConnector` test harness (the same one used by the cancel tests — it builds a connector with a fake `NodeConn` whose `mpsc::Receiver<Frame>` you hold, plus an offline variant). Add:

```rust
#[tokio::test]
async fn approve_run_sends_approve_frame() {
    let (conn, mut rx, _run_store) = online_tunnel_connector(); // existing helper
    conn.approve_run("run_01ABC", "bypass").await.unwrap();
    let frame = rx.recv().await.expect("a frame");
    match frame {
        Frame::Approve { run_id, mode } => {
            assert_eq!(run_id, "run_01ABC");
            assert_eq!(mode, "bypass");
        }
        other => panic!("expected Approve, got {other:?}"),
    }
}

#[tokio::test]
async fn reject_run_sends_reject_frame() {
    let (conn, mut rx, _run_store) = online_tunnel_connector();
    conn.reject_run("run_01ABC", Some("nope")).await.unwrap();
    match rx.recv().await.expect("a frame") {
        Frame::Reject { run_id, reason } => {
            assert_eq!(run_id, "run_01ABC");
            assert_eq!(reason.as_deref(), Some("nope"));
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[tokio::test]
async fn approve_reject_offline_node_returns_unreachable() {
    let conn = offline_tunnel_connector(); // existing helper (no registered conn)
    let a = conn.approve_run("run_01ABC", "").await;
    assert!(matches!(a, Err(HostConnectorError::Unreachable(_))));
    let r = conn.reject_run("run_01ABC", None).await;
    assert!(matches!(r, Err(HostConnectorError::Unreachable(_))));
}
```

Use the EXACT names of the existing helpers in the file (the cancel tests already construct an online connector + held `rx` and an offline connector — match those helper names; if the helpers return a different tuple shape, follow it). Import `Frame` and `HostConnectorError` as the cancel tests do.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp --test node_tunnel approve reject`
Expected: FAIL — `approve_run` returns `Invalid("approval over tunnel not supported (slice 2)")`, so the `Approve` match arm panics / the `Unreachable` assert fails.

- [ ] **Step 3: Replace the stubs**

In `crates/rupu-cp/src/host/tunnel.rs`, replace the `approve_run` and `reject_run` stub bodies (lines 249-263) with live-frame sends, mirroring `cancel_run` (lines 265-277):

```rust
    async fn approve_run(&self, run_id: &str, mode: &str) -> Result<(), HostConnectorError> {
        let conn = self.live_conn()?;
        conn.send(Frame::Approve {
            run_id: run_id.to_string(),
            mode: mode.to_string(),
        })
        .await
        .map_err(|_| {
            HostConnectorError::Unreachable(format!(
                "node {} disconnected before Approve frame could be sent",
                self.node_id
            ))
        })
    }

    async fn reject_run(
        &self,
        run_id: &str,
        reason: Option<&str>,
    ) -> Result<(), HostConnectorError> {
        let conn = self.live_conn()?;
        conn.send(Frame::Reject {
            run_id: run_id.to_string(),
            reason: reason.map(str::to_string),
        })
        .await
        .map_err(|_| {
            HostConnectorError::Unreachable(format!(
                "node {} disconnected before Reject frame could be sent",
                self.node_id
            ))
        })
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cp --test node_tunnel approve reject`
Expected: PASS (all three).
Run: `cargo clippy -p rupu-cp`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/host/tunnel.rs crates/rupu-cp/tests/node_tunnel.rs
git commit -m "feat(cp): TunnelHostConnector approve/reject over tunnel"
```

---

## Task 3: `rupu node` agent — handle Approve/Reject

**Files:**
- Modify: `crates/rupu-cli/src/cmd/node.rs` (move Approve/Reject out of the temporary arm into real handlers; add an argv builder + spawn helper; re-point `active[run_id].child`)
- Test: inline `#[cfg(test)]` in `crates/rupu-cli/src/cmd/node.rs` (the existing tail/argv tests live here)

**Interfaces:**
- Consumes: `Frame::Approve { run_id, mode }`, `Frame::Reject { run_id, reason }`; the existing `active: HashMap<String, RunState>` where `RunState { child: tokio::process::Child, offsets: FileOffsets }`; the existing `spawn_run`/`build_argv` pattern and the `exe: &Path`.
- Produces: a pure `fn build_control_argv(kind, run_id, mode, reason) -> Vec<String>` (unit-testable) and node behavior that spawns `rupu workflow approve`/`reject` and re-points the child handle.

- [ ] **Step 1: Write the failing argv test**

In the `#[cfg(test)]` module of `crates/rupu-cli/src/cmd/node.rs` (next to the existing `build_argv` tests), add:

```rust
#[test]
fn control_argv_approve_with_and_without_mode() {
    assert_eq!(
        build_control_argv(ControlKind::Approve, "run_1", "bypass", None),
        vec!["workflow", "approve", "run_1", "--mode", "bypass"]
    );
    assert_eq!(
        build_control_argv(ControlKind::Approve, "run_1", "", None),
        vec!["workflow", "approve", "run_1"]
    );
}

#[test]
fn control_argv_reject_with_and_without_reason() {
    assert_eq!(
        build_control_argv(ControlKind::Reject, "run_1", "", Some("nope")),
        vec!["workflow", "reject", "run_1", "--reason", "nope"]
    );
    assert_eq!(
        build_control_argv(ControlKind::Reject, "run_1", "", None),
        vec!["workflow", "reject", "run_1"]
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-cli --lib node::tests::control_argv`
Expected: FAIL — `build_control_argv` / `ControlKind` not defined.

- [ ] **Step 3: Add the argv builder + spawn helper**

In `crates/rupu-cli/src/cmd/node.rs` (near `build_argv`/`spawn_run`), add:

```rust
/// Which control subprocess to launch in response to an Approve/Reject frame.
#[derive(Debug, Clone, Copy)]
enum ControlKind {
    Approve,
    Reject,
}

/// Build the argv (after the executable) for the local approve/reject command
/// the node runs against a gated run.
///   Approve: `workflow approve <run_id> [--mode <mode>]`
///   Reject:  `workflow reject  <run_id> [--reason <reason>]`
fn build_control_argv(
    kind: ControlKind,
    run_id: &str,
    mode: &str,
    reason: Option<&str>,
) -> Vec<String> {
    let mut argv = vec!["workflow".to_string()];
    match kind {
        ControlKind::Approve => {
            argv.push("approve".to_string());
            argv.push(run_id.to_string());
            if !mode.is_empty() {
                argv.push("--mode".to_string());
                argv.push(mode.to_string());
            }
        }
        ControlKind::Reject => {
            argv.push("reject".to_string());
            argv.push(run_id.to_string());
            if let Some(r) = reason {
                argv.push("--reason".to_string());
                argv.push(r.to_string());
            }
        }
    }
    argv
}

/// Spawn a detached `rupu workflow approve|reject` child, same launch posture
/// as `spawn_run` (null stdio, own process group on Unix).
fn spawn_control(exe: &Path, argv: &[String]) -> anyhow::Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.spawn().context("spawn rupu control child")
}
```

- [ ] **Step 4: Run the argv test to verify it passes**

Run: `cargo test -p rupu-cli --lib node::tests::control_argv`
Expected: PASS.

- [ ] **Step 5: Wire the frames into the inbound match**

In `crates/rupu-cli/src/cmd/node.rs`, remove `Frame::Approve { .. }` and `Frame::Reject { .. }` from the temporary pass-through arm added in Task 1, and add real arms (place them next to the `Frame::Cancel` arm):

```rust
            Frame::Approve { run_id, mode } => {
                info!(run_id = %run_id, "node: Approve received");
                let argv = build_control_argv(ControlKind::Approve, &run_id, &mode, None);
                match spawn_control(exe, &argv) {
                    Ok(child) => {
                        // Re-point the active child so a later Cancel kills the
                        // resumed run, not the (already-exited) original.
                        if let Some(state) = active.get_mut(&run_id) {
                            state.child = child;
                        } else {
                            warn!(run_id = %run_id, "node: Approve for unknown run_id (spawned anyway)");
                        }
                    }
                    Err(e) => warn!(run_id = %run_id, error = %e, "node: approve spawn failed"),
                }
            }
            Frame::Reject { run_id, reason } => {
                info!(run_id = %run_id, "node: Reject received");
                let argv = build_control_argv(ControlKind::Reject, &run_id, "", reason.as_deref());
                match spawn_control(exe, &argv) {
                    Ok(child) => {
                        if let Some(state) = active.get_mut(&run_id) {
                            state.child = child;
                        } else {
                            warn!(run_id = %run_id, "node: Reject for unknown run_id (spawned anyway)");
                        }
                    }
                    Err(e) => warn!(run_id = %run_id, error = %e, "node: reject spawn failed"),
                }
            }
```

The temporary arm from Task 1 now reads (Approve/Reject removed):

```rust
            Frame::Hello { .. }
            | Frame::Welcome {}
            | Frame::Pong {}
            | Frame::Artifact { .. }
            | Frame::RunFinished { .. } => {
                warn!(/* unchanged */);
            }
```

Notes for the implementer:
- The resumed/rejected run keeps the same run dir; the existing 250 ms artifact tail streams its new lines + the eventual terminal `RunFinished` — do NOT add new streaming here.
- For Reject, the run flips to `Rejected` quickly; the tail's terminal-status check then removes it from `active` and sends `RunFinished{rejected}` on the next tick — no special handling needed.

- [ ] **Step 6: Verify build + tests**

Run: `cargo test -p rupu-cli --lib node`
Expected: PASS (argv tests + existing tail tests).
Run: `cargo build -p rupu-cli`
Expected: compiles. (Ignore the pre-existing unrelated rupu-cli baseline failures — verify only this file's tests pass and it builds.)

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cli/src/cmd/node.rs
git commit -m "feat(cli): rupu node handles Approve/Reject (spawn approve/reject, re-point child)"
```

---

## Task 4: Resume-worker defense-in-depth (skip tunnel runs)

**Files:**
- Modify: `crates/rupu-cli/src/cmd/cp.rs` (`run_resume_worker` signature + per-run skip; call site at ~line 56)
- Test: `crates/rupu-cp/tests/node_tunnel.rs` (invariant: a mirrored awaiting-approval run is not marked for resume)

**Interfaces:**
- Consumes: `RunStore::list_pending_resume(now) -> Vec<RunRecord>` (existing); `RunRecord.worker_id: Option<String>`; `rupu_workspace::HostStore::list() -> Result<Vec<Host>, _>` with `Host.transport == HostTransport::Tunnel { node_id }`.
- Produces: `run_resume_worker(store, worker_id, hosts, shutdown)` that skips runs whose `worker_id` belongs to a tunnel node.

- [ ] **Step 1: Write the failing invariant test**

The PRIMARY invariant is that a tunnel-mirrored awaiting-approval run never carries the `resume_requested_at` marker (the connector sends a frame instead of calling `request_resume_approval`), so `list_pending_resume` never returns it. Lock that in `crates/rupu-cp/tests/node_tunnel.rs`:

```rust
#[test]
fn mirrored_awaiting_run_is_not_pending_resume() {
    use rupu_orchestrator::RunStore;
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    // A mirrored node run that paused at a gate: AwaitingApproval, attributed
    // to a tunnel node, with NO resume_requested_at marker (the tunnel approve
    // path sends a frame; it never calls request_resume_approval).
    let mut rec = sample_awaiting_record("run_node_1"); // build a RunRecord in AwaitingApproval
    rec.worker_id = Some("node_abc".to_string());
    rec.resume_requested_at = None;
    store.create(rec, "").unwrap();

    let pending = store.list_pending_resume(chrono::Utc::now()).unwrap();
    assert!(
        pending.iter().all(|r| r.id != "run_node_1"),
        "mirrored awaiting-approval run must not be queued for the local resume worker"
    );
}
```

Build `sample_awaiting_record` using the real `RunRecord` constructor/fields the file's other tests use (status `AwaitingApproval`, `awaiting_step_id`/`approval_prompt` set, `resume_requested_at: None`). If the file already has a record-builder helper, reuse it.

- [ ] **Step 2: Run the test to verify it passes immediately (primary invariant)**

Run: `cargo test -p rupu-cp --test node_tunnel mirrored_awaiting_run_is_not_pending_resume`
Expected: PASS — confirms the natural invariant holds (no marker → not listed). This test guards against a future regression where someone makes the tunnel approve path set the marker.

- [ ] **Step 3: Add the defense-in-depth skip in the worker**

Even though the marker invariant already prevents it, add a belt-and-suspenders filter so a future change can't silently make the local worker resume a mirror dir. In `crates/rupu-cli/src/cmd/cp.rs`:

Change the `run_resume_worker` signature to accept a `HostStore`:

```rust
async fn run_resume_worker(
    store: Arc<RunStore>,
    worker_id: String,
    hosts: rupu_workspace::HostStore,
    mut shutdown: watch::Receiver<bool>,
) {
```

Inside the poll loop, after fetching `pending`, build the set of tunnel node ids and skip matching runs:

```rust
        // Defense-in-depth: never resume a run that belongs to a tunnel node;
        // its real run lives on the node and is resumed via a control frame,
        // not by this local worker. (Tunnel runs also never carry the
        // resume_requested_at marker, so this is belt-and-suspenders.)
        let tunnel_nodes: std::collections::HashSet<String> = hosts
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|h| match h.transport {
                rupu_workspace::HostTransport::Tunnel { node_id } => Some(node_id),
                _ => None,
            })
            .collect();

        for run in pending {
            if let Some(w) = run.worker_id.as_deref() {
                if tunnel_nodes.contains(w) {
                    tracing::debug!(run_id = %run.id, worker = %w,
                        "resume worker: skipping tunnel-node run");
                    continue;
                }
            }
            // ... existing claim_resume + spawn logic unchanged ...
        }
```

Update the call site (~`crates/rupu-cli/src/cmd/cp.rs:56`) to pass a `HostStore` built from the same `global_dir`:

```rust
            let worker_handle = tokio::spawn(run_resume_worker(
                Arc::clone(&store),
                worker_id.clone(),
                rupu_workspace::HostStore { root: global_dir.join("hosts") },
                shutdown_rx.clone(),
            ));
```

Match the EXACT existing argument names/order at the call site; only add the new `hosts` argument.

- [ ] **Step 4: Verify build + tests**

Run: `cargo build -p rupu-cli`
Expected: compiles.
Run: `cargo test -p rupu-cp --test node_tunnel mirrored_awaiting_run_is_not_pending_resume`
Expected: PASS.
Run: `cargo clippy -p rupu-cp`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/cmd/cp.rs crates/rupu-cp/tests/node_tunnel.rs
git commit -m "fix(cp): resume worker skips tunnel-node runs (defense-in-depth)"
```

---

## Task 5: e2e — approve & reject over the tunnel

**Files:**
- Modify: `crates/rupu-cp/tests/node_tunnel.rs` (extend the existing tunnel e2e)

**Interfaces:**
- Consumes: the existing e2e harness (spin CP router on a real listener with a launcher; enroll a node; connect a fake `tokio-tungstenite` client; `Hello`→`Welcome`); `Frame::Approve`/`Frame::Reject` (Task 1); the connector behavior (Task 2).

- [ ] **Step 1: Write the approve e2e test**

Extend `crates/rupu-cp/tests/node_tunnel.rs`, reusing the Slice-2 e2e harness (the one that dispatches a run and reads frames off the fake node). Add a test that:
1. spins the CP, connects a fake node, dispatches an agent run (as the existing e2e does);
2. the fake node streams a `run.json` artifact with `status: "awaiting_approval"` (+ `awaiting_step_id`, `approval_prompt`) so the central mirror shows the gate;
3. polls (bounded ~3s) `GET /api/runs/<id>?host=<node>` until the mirror shows `awaiting_approval`;
4. calls `POST /api/runs/<id>/approve?host=<node>` with body `{"mode":"bypass"}`;
5. asserts the fake node receives `Frame::Approve { run_id, mode: "bypass" }` on its WS;
6. the fake node then streams a `run.json` artifact `status: "completed"` + `Frame::RunFinished { status: "completed" }`;
7. polls the mirror until `GET /api/runs/<id>?host=<node>` shows `completed`.

```rust
#[tokio::test]
async fn e2e_approve_over_tunnel() {
    // ... reuse harness: start_cp_with_launcher(), enroll_node(), connect fake WS, Hello→Welcome ...
    // dispatch a run, read the Frame::Run, capture run_id
    // fake node -> Artifact(RunJson, awaiting_approval json) ; assert mirror shows awaiting_approval (bounded poll)
    // central POST /api/runs/{run_id}/approve?host={node_id}  body {"mode":"bypass"}
    // assert fake node receives Frame::Approve { run_id, mode == "bypass" }
    // fake node -> Artifact(RunJson, completed json) + RunFinished{completed}
    // assert mirror GET /api/runs/{run_id}?host={node_id} -> completed (bounded poll)
}
```

Use the EXACT helper names already present in the file (`recv_frame`, the bounded-poll helper, the `run.json` line builder that produces a valid `RunRecord` JSON — the Slice-2 e2e already constructs valid artifact lines; reuse it and just change `status`). The approve HTTP call mirrors the existing cancel HTTP call in the e2e (same `reqwest` + `?host=` pattern), changing the path to `/approve` and adding the JSON body.

- [ ] **Step 2: Write the reject e2e test**

Add a sibling test `e2e_reject_over_tunnel` that performs steps 1-3, then `POST /api/runs/<id>/reject?host=<node>` with body `{"reason":"nope"}`, asserts the fake node receives `Frame::Reject { run_id, reason: Some("nope") }`, then the fake node streams `RunFinished { status: "rejected" }` and the mirror shows `rejected`.

- [ ] **Step 3: Run the e2e tests to verify they fail, then (after Tasks 1-3 are in) pass**

Run: `cargo test -p rupu-cp --test node_tunnel e2e_approve_over_tunnel e2e_reject_over_tunnel`
Expected: PASS (Tasks 1-3 provide the behavior; this task only adds tests).

- [ ] **Step 4: Full suite + clippy**

Run: `cargo test -p rupu-cp -p rupu-workspace`
Expected: green.
Run: `cargo clippy -p rupu-cp -p rupu-workspace`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/tests/node_tunnel.rs
git commit -m "test(cp): node tunnel e2e — approve & reject over tunnel"
```

---

## Self-Review

**Spec coverage:**
- Wire protocol (`Frame::Approve`/`Reject`) → Task 1. ✅
- `TunnelHostConnector::approve_run`/`reject_run` → Task 2. ✅
- Node-side handling + child re-pointing → Task 3. ✅
- Resume-worker invariant + defense-in-depth → Task 4. ✅
- e2e approve + reject → Task 5. ✅
- Observation (already works via the mirror) — no task needed (verified in the spec). ✅
- Sessions deferred to Slice 5 — explicitly out of scope. ✅

**Type consistency:** `Frame::Approve { run_id: String, mode: String }` and `Frame::Reject { run_id: String, reason: Option<String> }` are used identically in Tasks 1, 2, 3, 5. `build_control_argv(ControlKind, &str, &str, Option<&str>)` defined and called consistently in Task 3. `run_resume_worker(store, worker_id, hosts, shutdown)` signature + call site updated together in Task 4.

**Placeholder scan:** every code step shows complete code; tests reference existing helper names with an instruction to match the file's real helpers (the harness already exists from Slice 2). No TBD/TODO.

---

## Process Note

Branch + single PR per repo convention. Working on branch `worktree-multi-host-slice-2.5` (based on `main` v0.28.1) in the existing worktree. Build subagent-driven (TDD). After all tasks: final whole-branch review, then push + open PR (no self-merge — GUI/agent-adjacent; matt validates before merge), then bundle into the next release.
