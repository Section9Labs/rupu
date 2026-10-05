# Subprocess netflow capture — Plan 2: the `rupu-netwatch` crate + pure tracker

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the `rupu-netwatch` crate with a pure, OS-agnostic attribution state machine (`tracker.rs`) and an always-unavailable fallback backend, plus the `rupu-runtime::net_capture::shared` entry point — so later plans can drop in OS backends (Plans 3/4) and the bash wiring (Plan 5) against a stable seam. No OS code, no bash wiring, no behavior change to any run.

**Architecture:** `rupu-netwatch` implements the `SubprocessCapture` port shipped in Plan 1 (`rupu_netflow::capture`). Its heart is `tracker.rs`: a synchronous, IO-free state machine that both OS backends will feed with socket observations keyed by an opaque `OwnerId`, and that emits `FlowRecord`/`SocketCompletion` values attributed to the right bash call's sink. This plan ships the tracker, its exhaustive unit tests, the `unsupported` backend (reports capture unavailable, hands back inert calls), and `rupu-runtime::net_capture::shared`, which picks a backend once per process.

**Tech Stack:** Rust 2021, `rupu-netflow` (the `capture` port + record model from Plan 1), `chrono`, `serde`. No OS crates, no `tokio` in the tracker.

**Spec:** `docs/superpowers/specs/2026-10-04-rupu-subprocess-netflow-capture-design.md` (this plan is §17 item 2; the tracker realizes §4, §9.4, §10.2; the never-established rule is §7.6; config is §14).

## Global Constraints

- **No `unsafe`.** Workspace `unsafe_code = "forbid"`. This plan needs none.
- **`#![deny(clippy::all)]`**, workspace dependency versions only (declare `rupu-netwatch` in root `[workspace.dependencies]` and as a path member; never pin versions in a crate manifest).
- **The tracker is pure:** no OS calls, no filesystem, no network, no `tokio`, no `async`. Every method is synchronous and takes the current wall-clock time (`chrono::DateTime<Utc>`) as a parameter, so tests are deterministic. The only IO in the crate is in a backend module, and this plan's only backend (`unsupported`) does none either.
- **No behavior change.** Nothing in this plan is reachable from a running agent yet: `net_capture::shared` returns a backend, but no caller invokes it until Plan 5. Do not touch the bash tool, the runner, or any wiring site.
- **Attribution is by `OwnerId`, never by OS concept.** The tracker knows nothing of cgroups or pids; a backend resolves its OS handle to an `OwnerId` (Plan 3: cgroup id; Plan 4: the owning call's shell pid) before calling the tracker.

---
## File structure

| File | Responsibility |
|---|---|
| `crates/rupu-netwatch/Cargo.toml` | new crate manifest (deps: `rupu-netflow` http, `chrono`, `tracing`; dev: none OS) |
| `crates/rupu-netwatch/src/lib.rs` | module decls + re-exports + crate doc |
| `crates/rupu-netwatch/src/types.rs` | `Transport`, `SocketSnapshot`, `OwnerId`/`CallId`/`SocketId` aliases, `Emit`, `Emission` |
| `crates/rupu-netwatch/src/tracker.rs` | the pure state machine + its unit tests (`#[cfg(test)] mod tests`) |
| `crates/rupu-netwatch/src/unsupported.rs` | `UnsupportedCapture`: records one `Capture{Unavailable}` per run, inert calls |
| `crates/rupu-netwatch/tests/it/main.rs` + `tracker_behavior.rs` | integration tests over the tracker through realistic event sequences |
| `crates/rupu-runtime/src/net_capture.rs` | `shared(&NetflowConfig) -> Arc<dyn SubprocessCapture>` (process-wide, lazy) |
| `crates/rupu-config/src/netflow_config.rs` | add `subprocess_capture`/`subprocess_poll_ms`/`subprocess_linger_ms` (§14) |
| root `Cargo.toml` | add `crates/rupu-netwatch` to members + `[workspace.dependencies]` |

**Key interface (defined here; Plans 3–6 build on it). Flagged for matt — this is the one load-bearing design decision in Plan 2:**

```rust
// types.rs
pub type CallId = u64;    // backend assigns one per begin()
pub type OwnerId = u64;   // backend's attribution handle: cgroup id (Linux) / shell pid (macOS)
pub type SocketId = u64;  // cookie (Linux) / srcref (macOS)

pub enum Transport { Tcp, Udp }

pub struct SocketSnapshot {
    pub transport: Transport,
    pub local: Option<std::net::SocketAddr>,
    pub remote: Option<std::net::SocketAddr>,
    pub established: bool,          // TCP reached ESTABLISHED (or a later state) at least once
    pub bytes_in: Option<u64>,
    pub bytes_out: Option<u64>,
    pub process: Option<rupu_netflow::FlowProcess>,
}

pub enum Emit { Flow(Box<rupu_netflow::FlowRecord>), Complete(rupu_netflow::SocketCompletion) }
pub struct Emission { pub sink: std::sync::Arc<dyn rupu_netflow::FlowSink>, pub emit: Emit }
```

```rust
// tracker.rs
pub struct CallInfo { pub owner: OwnerId, pub attribution: rupu_netflow::CallAttribution }

pub struct Tracker { /* linger: Duration; maps described in tasks */ }

impl Tracker {
    pub fn new(linger: chrono::Duration) -> Self;
    pub fn register_call(&mut self, id: CallId, info: CallInfo, now: DateTime<Utc>);
    pub fn observe(&mut self, sock: SocketId, owner: OwnerId, snap: SocketSnapshot, now: DateTime<Utc>) -> Vec<Emission>;
    pub fn close(&mut self, sock: SocketId, snap: SocketSnapshot, now: DateTime<Utc>) -> Vec<Emission>;
    pub fn finish_call(&mut self, id: CallId, now: DateTime<Utc>);          // begin linger
    pub fn tick(&mut self, now: DateTime<Utc>) -> Vec<Emission>;            // expire lingers
    pub fn finish_run(&mut self, run_id: &str, now: DateTime<Utc>) -> Vec<Emission>; // flush still-open
}
```

Rationale for matt: the tracker stays OS-free by taking an opaque `OwnerId`. Backends resolve their native handle to it (Plan 3 maps a call's cgroup directory inode = cgroup id; Plan 4 walks a socket's process ancestry to the registered shell pid). The tracker *returns* emissions rather than pushing to the async sink itself, so it has no `tokio` dependency and unit tests assert on returned values with no runtime. If matt prefers the tracker to own the sink writes (push model), that is the one thing to flip before execution.

---
### Task 1: Scaffold `rupu-netwatch` + the shared types

**Files:**
- Create: `crates/rupu-netwatch/Cargo.toml`, `crates/rupu-netwatch/src/lib.rs`, `crates/rupu-netwatch/src/types.rs`
- Modify: root `Cargo.toml` (members list + `[workspace.dependencies]`)

**Interfaces:**
- Produces: the crate `rupu-netwatch`; `types` module with `CallId`/`OwnerId`/`SocketId` aliases, `Transport`, `SocketSnapshot`, `Emit`, `Emission` exactly as in the key-interface block above. Re-export all from `lib.rs`.

- [ ] **Step 1: Add the crate to the workspace.** In root `Cargo.toml`, add `"crates/rupu-netwatch",` to `members` (next to `crates/rupu-netflow`), and under `[workspace.dependencies]` add `rupu-netwatch = { path = "crates/rupu-netwatch" }`.

- [ ] **Step 2: Write `Cargo.toml`.**

```toml
[package]
name = "rupu-netwatch"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
rupu-netflow = { workspace = true, features = ["http"] }
chrono.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true

[lints]
workspace = true
```

(`rupu-netflow` needs `http` only because `CallAttribution` carries `Arc<dyn FlowSink>` and the http feature pulls the full sink surface used by tests; if a `cargo build -p rupu-netwatch` shows it is unneeded, drop to default features.)

- [ ] **Step 3: Write `types.rs`** exactly as the key-interface block specifies. Derive `Debug, Clone` on `SocketSnapshot` and `Transport` (and `Copy, PartialEq, Eq` on `Transport`). `Emission`/`Emit` need no derives (they carry an `Arc<dyn FlowSink>`). Document each field with one line.

- [ ] **Step 4: Write `lib.rs`** with a crate doc comment (what the crate is; that the tracker is pure and OS-free), `pub mod types; pub mod tracker; pub mod unsupported;` (create empty `tracker.rs`/`unsupported.rs` with a `//!` line so the build passes — later tasks fill them), and `pub use types::*;`.

- [ ] **Step 5: Write the failing test** in `types.rs` under `#[cfg(test)]`:

```rust
#[test]
fn socket_snapshot_carries_transport_and_addrs() {
    let s = SocketSnapshot {
        transport: Transport::Tcp,
        local: Some("10.0.0.2:54321".parse().unwrap()),
        remote: Some("140.82.116.3:443".parse().unwrap()),
        established: true,
        bytes_in: Some(48_000),
        bytes_out: Some(1_200),
        process: Some(rupu_netflow::FlowProcess { pid: 4412, name: "curl".into() }),
    };
    assert_eq!(s.transport, Transport::Tcp);
    assert_eq!(s.remote.unwrap().port(), 443);
}
```

- [ ] **Step 6: Build + test.** Run `cargo build -p rupu-netwatch`, then `cargo test -p rupu-netwatch`. Expected: PASS. Run `cargo clippy -p rupu-netwatch`.

- [ ] **Step 7: Commit.**

```bash
git add crates/rupu-netwatch Cargo.toml
git commit -m "netwatch: scaffold crate and shared capture types"
```

---

### Task 2: `Tracker` — registration and the first-Flow emission on observe

**Files:**
- Modify: `crates/rupu-netwatch/src/tracker.rs`
- Test: inline `#[cfg(test)] mod tests` in `tracker.rs`

**Interfaces:**
- Consumes: `types::*`, `rupu_netflow::{CallAttribution, FlowRecord, Fidelity, Origin, FlowCtx, Outcome, FlowId}`.
- Produces: `CallInfo`, `Tracker::new`, `register_call`, `observe`. `observe` emits exactly one `Emit::Flow` the first time a socket is both attributable (its `owner` maps to a registered call) and reportable (TCP with `established == true`, or any UDP). Later observations of the same socket emit nothing.

- [ ] **Step 1: Write failing tests** (in `tracker.rs`):

```rust
// helper: a registered call with a MemorySink we can assert on
// (use rupu_netflow::MemorySink; Arc it; put it in CallAttribution.sink)
```
Write three tests:
  1. `observe_before_established_emits_nothing_then_flow_on_established`: register call (owner 100), observe a TCP socket owned by 100 with `established:false` → `vec![]`; observe again with `established:true` → one `Emit::Flow` whose `FlowRecord` has `fidelity == Fidelity::Socket`, `ctx.origin == Origin::Subprocess("curl")`, `ctx.tool_call_id == Some("toolu_1")`, `ctx.run_id == Some("run-1")`, `host == "140.82.116.3"`, `port == 443`, `scheme == "tcp"`, `body_complete == false`, and `process == Some(..pid 4412..)`. A third observe → `vec![]` (already emitted).
  2. `udp_emits_flow_on_first_observation`: a UDP socket owned by a registered call emits a Flow on first observe (no established needed); unconnected UDP (`remote: None`) emits a Flow with `host == ""` and `port == 0`.
  3. `observation_for_unregistered_owner_is_ignored`: observe a socket whose owner maps to no call → `vec![]`, and a later `close` for it also emits nothing.

- [ ] **Step 2: Run them, confirm fail.** `cargo test -p rupu-netwatch --lib tracker`

- [ ] **Step 3: Implement.** `Tracker` holds: `linger: chrono::Duration`; `calls: HashMap<CallId, CallState>` where `CallState { owner: OwnerId, attribution: CallAttribution, run_id: String, closing_at: Option<DateTime<Utc>> }`; `owner_to_call: HashMap<OwnerId, CallId>`; `sockets: HashMap<SocketId, SockState>` where `SockState { call_id: CallId, id: FlowId, transport: Transport, flow_emitted: bool, ever_established: bool, first_seen: DateTime<Utc>, last: SocketSnapshot }`. `register_call` inserts into `calls` + `owner_to_call`. `observe` resolves `owner → call_id` (ignore if none), creates/updates `SockState`, and when `!flow_emitted` and reportable, mints a `FlowId`, builds the `FlowRecord` (via a private `flow_record_from(call, sock, snap, now)` that sets fidelity Socket, origin `Subprocess(process name or "unknown")`, the `FlowCtx` from the attribution incl. `tool_call_id`, `scheme` from transport, `host`/`port`/`peer_ip` from remote, `local_addr` from local, `process`, `outcome: Ok`, `body_complete:false`, `ts: first_seen`), sets `flow_emitted = true`, and returns one `Emission { sink: call.attribution.sink.clone(), emit: Flow(..) }`. Track `ever_established |= snap.established`.

- [ ] **Step 4: Run tests, confirm pass; clippy.**

- [ ] **Step 5: Commit.** `git commit -m "netwatch: tracker registration and first-flow emission on observe"`

---
### Task 3: `Tracker::close` — finalize a socket

**Files:**
- Modify: `crates/rupu-netwatch/src/tracker.rs`
- Test: inline tests

**Interfaces:**
- Produces: `Tracker::close(sock, snap, now)`. Behavior:
  - Socket unknown (never observed / unattributed) → `vec![]`.
  - A socket that already emitted a `Flow` → emit one `Emit::Complete(SocketCompletion)` with `id` = the flow's `FlowId`, `duration_ms` = `(now - first_seen)` in ms, `bytes_in`/`bytes_out` from the final snapshot (`None` stays `None`), `outcome`: `Some(Ok)` if `ever_established` (or UDP, or bytes_in>0), else `Some(TransportError)`, `error`: `Some("connection never established")` only when `outcome` is `TransportError`.
  - A TCP socket that was attributed but **never reportable** (never established, so no Flow emitted) → emit a single `Emit::Flow` with `outcome: TransportError`, `error: Some("connection never established")`, `body_complete: true`, `bytes_*` as known (§7.6 "never established" case). Do **not** also emit a Complete.
  - Remove the socket from the map either way.

- [ ] **Step 1: Write failing tests:**
  1. `established_socket_closes_with_complete_ok`: register, observe established (captures the Flow), close with `bytes_in: Some(50000), bytes_out: Some(1300)` → one `Emit::Complete` with those bytes, `outcome: Some(Ok)`, `error: None`, `duration_ms` > 0, and its `id` equals the Flow's id.
  2. `tcp_never_established_closes_with_single_transport_error_flow`: register, observe `established:false` (no Flow yet), close → exactly one `Emit::Flow`, `outcome == TransportError`, `error == Some("connection never established")`, `body_complete == true`; NO Complete.
  3. `close_of_unknown_socket_is_noop`: close a socket id never observed → `vec![]`.
  4. `udp_closes_with_complete_ok`: UDP socket (Flow already emitted on observe), close → one `Emit::Complete`, `outcome: Some(Ok)`.

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement** per the behavior above. Factor the outcome decision into a private `fn settle_outcome(transport, ever_established, bytes_in) -> (Outcome, Option<String>)`.

- [ ] **Step 4: Run tests, confirm pass; clippy.**

- [ ] **Step 5: Commit.** `git commit -m "netwatch: tracker close emits Complete or the never-established Flow"`

---

### Task 4: linger, `tick`, and `finish_run`

**Files:**
- Modify: `crates/rupu-netwatch/src/tracker.rs`
- Test: inline tests

**Interfaces:**
- Produces:
  - `finish_call(id, now)` — marks the call's `closing_at = now`; does not drop it yet (sockets may still close within the linger window and must stay attributable).
  - `tick(now)` — for every call whose `closing_at` is set and `now - closing_at >= linger`: flush its still-open sockets as `Emit::Complete` with `error: Some("observation ended with the run; socket still open")` and `outcome` per `settle_outcome`, then drop the call (and its `owner_to_call` entry and its sockets). Returns all emissions. A call with no `closing_at` is untouched.
  - `finish_run(run_id, now)` — immediately flush+drop every call whose `run_id` matches (same still-open flush as `tick`), regardless of linger. Returns emissions.

- [ ] **Step 1: Write failing tests:**
  1. `socket_closing_within_linger_is_still_attributed`: register, observe established, `finish_call`, then `close` the socket 1s later (linger 3s) → the `close` still emits a `Complete` attributed to the call (the call is still present).
  2. `tick_after_linger_flushes_still_open_sockets_and_drops_call`: register, observe established (socket stays open), `finish_call` at T, `tick` at T+2s (linger 3s) → `vec![]` (not expired); `tick` at T+4s → one `Emit::Complete` with `error` containing "still open", and a subsequent `observe` for that owner is ignored (call dropped).
  3. `finish_run_flushes_all_calls_for_that_run_immediately`: register two calls on run-1 (owners 100, 101) each with an open socket, one call on run-2 → `finish_run("run-1", now)` emits two Completes (run-1 only), leaves run-2's call live.

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement.** Keep a helper `fn flush_call(&mut self, call_id) -> Vec<Emission>` that drains the call's open sockets to Completes (reusing `settle_outcome`, `error` = still-open), removes the sockets, the call, and its owner mapping; `tick` and `finish_run` both call it.

- [ ] **Step 4: Run tests, confirm pass; clippy.**

- [ ] **Step 5: Commit.** `git commit -m "netwatch: tracker linger, tick, and finish_run flush"`

---
### Task 5: the `unsupported` fallback backend

**Files:**
- Modify: `crates/rupu-netwatch/src/unsupported.rs`
- Create: `crates/rupu-netwatch/tests/it/main.rs`, `crates/rupu-netwatch/tests/it/unsupported_backend.rs`

**Interfaces:**
- Produces: `UnsupportedCapture { reason: String }` implementing `rupu_netflow::SubprocessCapture`. `begin` returns an inert `CaptureCall` (no `shell_prefix`, `spawned`/`finished` no-ops) BUT, the first time a given run id is seen, writes one `LedgerLine::Capture { state: Unavailable { reason }, .. }` to that call's sink so the CP can say capture was unavailable (spec §13). `run_finished` drops the per-run "already announced" marker.

Note the sink API: `FlowSink` has `record`/`complete`/`complete_socket` but no raw `LedgerLine` method. Writing a `Capture` line needs a sink entry point. Add to `rupu_netflow::FlowSink` a provided method `async fn capture_state(&self, line: CaptureStateLine) {}` (default no-op), implemented by `NetflowWriter` (offer `LedgerLine::Capture`) and `MemorySink` (push to a vec with accessor). Define `CaptureStateLine { state: CaptureState, tool_call_id: Option<String>, note: Option<String> }` in `rupu-netflow` (the `ts` is stamped by the writer at `Utc::now`). **This is a small additive change to `rupu-netflow` carried by this task** — mirror the `complete_socket` pattern Plan 1 shipped.

- [ ] **Step 1: Write failing tests** (`tests/it/unsupported_backend.rs`, with `tests/it/main.rs` doing `mod unsupported_backend;`):
  1. `announces_unavailable_once_per_run`: build `UnsupportedCapture { reason: "cgroup v2 not mounted".into() }`; call `begin` twice for run-1 (same `MemorySink`) and once for run-2 (its own sink). Assert run-1's sink received exactly one capture-state line with `state == Unavailable{reason}` after draining, run-2's sink exactly one; the inert calls' `shell_prefix()` are `None`.
  2. `run_finished_allows_reannounce`: begin for run-1 (one line), `run_finished("run-1")`, begin again for run-1 → a second line (a fresh run with a recycled id re-announces).
  Because `begin` is sync but the sink write is async, the backend spawns the one-shot write on a `tokio` task (add `tokio` to `rupu-netwatch` deps with `rt` feature) OR exposes the pending line for the caller to flush. Choose the spawn approach; the test runs under `#[tokio::test]` and awaits a short yield (or uses `MemorySink` whose write is synchronous under the hood — verify). If spawning proves awkward in a sync method, instead have `begin` return the line to write through a tiny channel the backend's own thread drains; decide during implementation and record which in the report.

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement** the `rupu-netflow` additive `capture_state`/`CaptureStateLine` first (with its own round-trip test in `rupu-netflow`'s `record_model.rs`: a `NetflowWriter` `capture_state` call produces a foldable-skipped `Capture` line), then `UnsupportedCapture`.

- [ ] **Step 4: Run tests (both crates), confirm pass; clippy both.**

- [ ] **Step 5: Commit.** `git commit -m "netwatch: unsupported backend announces capture unavailable once per run"`

---

### Task 6: config fields + `rupu-runtime::net_capture::shared`

**Files:**
- Modify: `crates/rupu-config/src/netflow_config.rs` (add fields §14)
- Create: `crates/rupu-runtime/src/net_capture.rs`; Modify: `crates/rupu-runtime/src/lib.rs`, `crates/rupu-runtime/Cargo.toml`
- Test: inline in `net_capture.rs`

**Interfaces:**
- Consumes: `rupu_config::NetflowConfig`, `rupu_netflow::{SubprocessCapture, NoopCapture}`, `rupu_netwatch::unsupported::UnsupportedCapture`.
- Produces: `rupu_runtime::net_capture::shared(cfg: &NetflowConfig) -> std::sync::Arc<dyn SubprocessCapture>`. Rules: if `cfg.subprocess_capture` is false (or env `RUPU_NETFLOW_SUBPROCESS=0`), return `Arc::new(NoopCapture)`. Otherwise return the OS backend — which in this plan does not exist yet for any OS, so return `Arc::new(UnsupportedCapture { reason: "subprocess capture backend not built for this platform yet".into() })`. The function memoizes a single process-wide instance in a `OnceLock<Arc<dyn SubprocessCapture>>` so every run shares one backend. Add `rupu-netwatch` and `rupu-config` (already present) to `rupu-runtime`'s deps.

- [ ] **Step 1: Add config fields.** In `NetflowConfig`: `subprocess_capture: bool` (default true), `subprocess_poll_ms: u64` (default 50), `subprocess_linger_ms: u64` (default 3000), each with a `#[serde(default = "…")]` default fn, mirroring the existing fields' style. Add a round-trip test that an empty `[netflow]` table yields the defaults and that an override parses.

- [ ] **Step 2: Write failing test** (`net_capture.rs`):
  1. `disabled_config_returns_noop`: `shared(&cfg_with_subprocess_false)` — assert it begins a call that returns `shell_prefix() == None` and writes NO capture-state line to the sink (NoopCapture is silent). (Distinguish from UnsupportedCapture, which DOES write one.)
  2. `enabled_config_returns_a_backend_that_announces_unavailable`: with the env var unset and `subprocess_capture: true`, `shared(&cfg)` begins a call whose sink receives one `Unavailable` capture-state line (the unsupported backend, until Plans 3/4).
  3. `env_override_forces_noop`: set `RUPU_NETFLOW_SUBPROCESS=0` (use a `#[serial]`-style guard or a test-only param to avoid global-env races — prefer reading the env inside `shared` via a small injectable, or gate this assertion behind a serialized test; record the choice).

- [ ] **Step 3: Implement** `shared` + the `OnceLock`. Because memoization makes the three tests interfere (first call wins), make the `OnceLock` wrap only the real default path and have `shared` take the decision before the lock, OR expose a test-only `fn choose(cfg, env_off) -> Arc<dyn SubprocessCapture>` that the tests call directly and have `shared` delegate to `choose` + memoize. Use the `choose` split so tests are deterministic and the public `shared` still memoizes. Document it.

- [ ] **Step 4: Run tests, confirm pass; clippy.**

- [ ] **Step 5: Commit.** `git commit -m "runtime: net_capture::shared backend selection + netflow subprocess config"`

---

## Final verification

- [ ] `cargo test -p rupu-netwatch -p rupu-netflow -p rupu-config -p rupu-runtime` — all green.
- [ ] `cargo clippy -p rupu-netwatch -p rupu-runtime --all-targets` — no findings.
- [ ] `cargo build -p rupu-cli -p rupu-cp` — the new config fields and runtime module break no downstream build.
- [ ] No behavior change: `git grep -n "net_capture::shared\|SubprocessCapture\|UnsupportedCapture" crates/rupu-tools crates/rupu-agent crates/rupu-cli/src crates/rupu-cp/src` returns nothing (no caller wires it yet — that is Plan 5).

## Notes for later plans
- **Plan 3 (Linux)** adds `linux/` to `rupu-netwatch`, builds cgroups + netlink, and resolves cgroup id → `OwnerId`; it feeds the SAME `Tracker` via `observe`/`close`/`tick`. `net_capture::shared` returns it under `#[cfg(target_os="linux")]` when a runtime check passes, else `UnsupportedCapture`.
- **Plan 4 (macOS)** adds `macos/`, resolves socket → owning call's shell pid → `OwnerId`, feeds the same `Tracker`.
- **Plan 5** wires `ToolContext.net_capture = net_capture::shared(&cfg)` and drives `begin`/`spawned`/`finished` from the bash tool; it owns the end-to-end `complete_socket`/`capture_state` test deferred from Plan 1.
- `CaptureStateLine`/`capture_state` added here is the mechanism Plan 5's "capture unavailable" and visible-loss notes use.
