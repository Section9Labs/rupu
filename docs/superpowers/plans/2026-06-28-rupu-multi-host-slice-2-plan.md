# Multi-Host Slice 2 (`rupu node` dial-home tunnel) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a NAT'd/firewalled host join the fleet via a `rupu node` agent that dials the CP over a WebSocket, executes dispatched workflow/agent runs locally, and streams results back — observable + cancellable from the central CP.

**Architecture:** A new inbound `GET /api/node/connect` WS + `NodeRegistry` of live connections; a typed `Frame` protocol (token-now, mTLS-ready); a `rupu node` agent (tokio-tungstenite client) that runs `rupu` locally and streams run artifacts back; a CP-side mirror that writes those artifacts into the central `RunStore` (host-attributed); and a `TunnelHostConnector` that satisfies the Slice-1 `HostConnector` port via **mirror-for-reads + frames-for-control** — so the Fleet/Hosts UI, run lists, and RunDetail work unchanged.

**Tech Stack:** Rust (axum WS [`ws` feature], tokio, `tokio-tungstenite` [new, node client], async-trait, serde, sha2, thiserror), the existing `rupu-orchestrator::RunStore`, React/TS web.

## Global Constraints

- Hexagonal: `TunnelHostConnector` implements the existing `HostConnector` port (in `rupu-cp/src/host/connector.rs`); the Frame protocol + registry + mirror are adapters. `rupu-cli` stays thin (the agent loop delegates to `rupu` subprocess + the protocol types). `#![deny(clippy::all)]`; `unsafe_code` forbidden; library errors `thiserror`, CLI `anyhow`; workspace deps only (pin `tokio-tungstenite`, `sha2` in the ROOT `Cargo.toml`).
- Frame types live in `rupu-cp::node::protocol` (rupu-cli depends on rupu-cp, so both share them — do NOT duplicate).
- Auth: enrollment **token**, stored **hashed** (sha256 hex in the host TOML `token_hash`), constant-time compare at handshake. The `Frame::Hello.auth` is an enum (`Token{token}` | `Mtls`) so mTLS slots in later (Slice 4) with no protocol change.
- `run_id` is **CP-allocated** and sent in the `Run` frame. Node runs are mirrored as first-class runs in the central `RunStore`, host-attributed (`host_id`/worker = the node).
- Observation for a tunnel host reads the **mirror** (central RunStore); control (`launch_*`/`cancel_run`) sends a **frame** over the live `NodeConn`; node offline → control returns a clear error, observation still serves last-known mirror state.
- Scope: workflow + agent RUNS, observe + cancel. NOT sessions, NOT approval/resume over the tunnel (deferred).
- `wss://` expected; warn on plaintext `ws://`. Bound the per-run artifact relay channel (backpressure).

---

## File Structure

**rupu-cp** (server side)
- `src/node/mod.rs` — module root + re-exports.
- `src/node/protocol.rs` — `Frame` enum, `Auth`, `RunSpec`, `ArtifactFile`, (de)serialize.
- `src/node/registry.rs` — `NodeRegistry`, `NodeConn` (send half + metadata), register/evict/get/mark_seen.
- `src/node/server.rs` — `GET /api/node/connect` WS upgrade + handshake + read/write pumps.
- `src/node/mirror.rs` — write streamed artifacts into `RunStore` (create/append/finish).
- `src/host/tunnel.rs` — `TunnelHostConnector` (impl `HostConnector`).
- Modify: `src/host/registry.rs` (resolve `Tunnel` → `TunnelHostConnector`), `src/state.rs` (`nodes: Arc<NodeRegistry>`), `src/server.rs` (mount node route), `src/host/connector.rs` (only if a control-frame signature is needed).
- `Cargo.toml`: ensure axum `ws` feature; add `sha2` (workspace).

**rupu-workspace** (shared host model)
- Modify `src/host_store.rs`: add `HostTransport::Tunnel { node_id }` + `token_hash: Option<String>` on `Host` (or a sibling enroll record) + `enroll_node(name) -> (Host, plaintext_token)` + `verify_node_token(node_id, token) -> bool` (sha256 + constant-time).

**rupu-cli** (the agent)
- `src/cmd/node.rs` — `rupu node` (agent loop) + `rupu node enroll <name>` (delegates to the CP enroll, or local host store if offline). 
- Modify the command enum/dispatch; `Cargo.toml` add `tokio-tungstenite` (workspace).

**web** (`crates/rupu-cp/web/src`)
- `pages/Hosts.tsx` / Add-host form: an "Add node (tunnel)" path that calls the enroll API and shows the one-time `rupu node …` command + token; transport chip "tunnel". (Node runs already render via the mirror-backed connector + the Slice-1.5 host-aware lists — no new run UI.)
- `lib/api.ts`: `enrollNode({name}) -> { host, command, token }`.

**Tests**: `crates/rupu-cp/tests/node_tunnel.rs` (registry, handshake, mirror, connector, e2e with a fake node WS client); rupu-workspace host_store unit tests; web vitest for the enroll UI.

---

## Task 1: Frame protocol (`rupu-cp::node::protocol`)

**Files:** Create `crates/rupu-cp/src/node/mod.rs`, `protocol.rs`; modify `src/lib.rs` (`pub mod node;`). Test: `#[cfg(test)]` in protocol.rs.

**Interfaces — Produces:**
```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
  Hello { node_id: String, auth: Auth, rupu_version: String, capabilities: Vec<String> },
  Welcome {},
  Run { run_id: String, spec: RunSpec },
  Cancel { run_id: String },
  Ping {},
  Pong {},
  Artifact { run_id: String, file: ArtifactFile, line: String },
  RunFinished { run_id: String, status: String },
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth { Token { token: String }, Mtls {} }
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RunSpec { pub kind: RunSpecKind, pub name: String, #[serde(default)] pub inputs: std::collections::BTreeMap<String,String>, pub prompt: Option<String>, pub mode: Option<String>, pub target: Option<String> }
#[serde(rename_all="snake_case")] pub enum RunSpecKind { Workflow, Agent }
#[serde(rename_all="snake_case")] pub enum ArtifactFile { Events, StepResults, UnitCheckpoints, RunJson }
```
`Frame` round-trips through serde_json (each variant a `{"type":…}` object).

- [ ] **Step 1: Failing test** — round-trip a `Frame::Run { run_id:"r1", spec: RunSpec{ kind: Workflow, name:"wf", inputs:{}, .. } }` and `Frame::Artifact{ file: Events, line:"{...}" }` through `serde_json::to_string` → `from_str`, assert equality; assert the `Hello` token variant serializes `auth.kind == "token"`.
- [ ] **Step 2: Run, verify fail** — `cargo test -p rupu-cp --lib node::protocol` FAIL.
- [ ] **Step 3: Implement** the enums above with derives.
- [ ] **Step 4: Run** → PASS; `cargo clippy -p rupu-cp` clean.
- [ ] **Step 5: Commit** — `feat(cp): node tunnel Frame protocol (token-now, mTLS-ready)`

---

## Task 2: Host model `Tunnel` transport + enrollment (rupu-workspace)

**Files:** Modify `crates/rupu-workspace/src/host_store.rs` (+ lib re-exports). Test: `#[cfg(test)]`.

**Interfaces — Produces:** add `HostTransport::Tunnel { node_id: String }`; add `token_hash: Option<String>` to `Host`; `fn enroll_node(store: &HostStore, name: &str) -> Result<(Host, String), HostStoreError>` (generates `node_id = node_<ULID>`, a random high-entropy token, stores `Host{ transport: Tunnel{node_id}, token_hash: Some(sha256_hex(token)) }`, returns the plaintext token ONCE); `fn verify_node_token(host: &Host, token: &str) -> bool` (sha256 + `subtle`/manual constant-time compare). Add `sha2` workspace dep.

- [ ] **Step 1: Failing tests** — `enroll_node` returns a token + a saved `Tunnel` host whose `token_hash` is `sha256_hex(token)` and NOT the plaintext; `verify_node_token(host, token)` true, wrong token false; the TOML never contains the plaintext token.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** (mirror the existing `Host`/`HostStore` serde; `sha256_hex` via `sha2::Sha256`; constant-time compare).
- [ ] **Step 4: Run** `cargo test -p rupu-workspace` + clippy → PASS.
- [ ] **Step 5: Commit** — `feat(workspace): Tunnel host transport + token-hashed node enrollment`

---

## Task 3: `NodeRegistry` (rupu-cp)

**Files:** Create `crates/rupu-cp/src/node/registry.rs`. Test: `#[cfg(test)]` + `tests/node_tunnel.rs`.

**Interfaces — Produces:**
```rust
pub struct NodeConn { tx: tokio::sync::mpsc::Sender<Frame>, pub connected_at: DateTime<Utc>, pub last_seen: Mutex<DateTime<Utc>> }
impl NodeConn { pub async fn send(&self, f: Frame) -> Result<(), NodeError>; }
pub struct NodeRegistry { conns: Mutex<HashMap<String, Arc<NodeConn>>> }
impl NodeRegistry {
  pub fn new() -> Self;
  pub fn register(&self, node_id: &str, tx: Sender<Frame>) -> Arc<NodeConn>; // evicts+drops prior
  pub fn get(&self, node_id: &str) -> Option<Arc<NodeConn>>;
  pub fn remove(&self, node_id: &str, only_if: &Arc<NodeConn>); // remove only if still the current conn
  pub fn is_online(&self, node_id: &str) -> bool;
  pub fn mark_seen(&self, node_id: &str);
}
```
`send` maps a closed channel → `NodeError::Offline`.

- [ ] **Step 1: Failing tests** — register then `get` returns a conn; re-register same id evicts the old (old `send` errors); `remove(only_if)` no-ops if a newer conn replaced it; `is_online` reflects presence.
- [ ] **Step 2–4:** implement (`Mutex<HashMap>`, mpsc); `cargo test -p rupu-cp --test node_tunnel` + clippy → PASS.
- [ ] **Step 5: Commit** — `feat(cp): NodeRegistry (live tunnel connections)`

---

## Task 4: Mirror writer (rupu-cp)

**Files:** Create `crates/rupu-cp/src/node/mirror.rs`. Test: `tests/node_tunnel.rs`.

**Interfaces — Consumes:** `RunStore` (rupu-orchestrator), `ArtifactFile`. **Produces:**
```rust
pub struct NodeMirror { run_store: Arc<RunStore> }
impl NodeMirror {
  pub fn create_run(&self, run_id: &str, node_id: &str, spec: &RunSpec) -> Result<(), MirrorError>; // RunRecord status Running, worker_id=node, workflow_name=spec.name
  pub fn append(&self, run_id: &str, file: ArtifactFile, line: &str) -> Result<(), MirrorError>; // append to events.jsonl/step_results.jsonl/unit_checkpoints.jsonl; RunJson → overwrite run.json
  pub fn finish(&self, run_id: &str, status: &str) -> Result<(), MirrorError>; // set RunStatus + finished_at
}
```
Uses `RunStore::create`/`events_path`/`update`. `append` opens the target file in append mode (mirror `JsonlSink`). The mirrored run dir lives in the SAME `RunStore` as local runs (so the existing read endpoints + events firehose see it); host attribution is via the run record's `worker_id`/a `host_id` marker the `TunnelHostConnector` filters on.

- [ ] **Step 1: Failing test** — `create_run` then `append(Events, "{json}")` ×2 then `finish("completed")` produces a run readable via `RunStore::load` (status Completed) with a 2-line events.jsonl at `events_path`.
- [ ] **Step 2–4:** implement; test + clippy → PASS.
- [ ] **Step 5: Commit** — `feat(cp): node run mirror (stream artifacts into RunStore)`

---

## Task 5: Tunnel WS server endpoint (rupu-cp)

**Files:** Create `crates/rupu-cp/src/node/server.rs`; modify `src/server.rs` (mount), `Cargo.toml` (axum `ws` feature). Test: `tests/node_tunnel.rs` (fake WS client via `tokio-tungstenite` dev-dep, or axum's test client).

**Interfaces — Consumes:** `NodeRegistry`, `NodeMirror`, `HostStore` (verify token), `Frame`. **Produces:** `pub fn routes() -> Router<AppState>` with `GET /api/node/connect` (axum `WebSocketUpgrade`). On upgrade: read first msg → `Frame::Hello`; look up the host by `node_id`, `verify_node_token`; on fail → close. On success: create an mpsc `Sender<Frame>`, `registry.register(node_id, tx)`, send `Welcome`, then run two pumps — **write pump** (mpsc rx → ws send), **read pump** (ws recv → match: `Artifact`→`mirror.append`, `RunFinished`→`mirror.finish`, `Pong`→`mark_seen`). A keepalive task sends `Ping` periodically; on socket close, `registry.remove(node_id, only_if)`.

- [ ] **Step 1: Failing test** — connect a fake WS client to a spawned CP router (need a real listener — mirror `tests/sse.rs` server spawn + a `tokio-tungstenite` client). Send `Hello` with a valid enrolled token → receive `Welcome`; the node shows online in the registry. Send `Hello` with a bad token → connection closed, not registered.
- [ ] **Step 2–4:** add axum `ws` feature + `tokio-tungstenite` dev-dep; implement handshake + pumps; test + clippy → PASS.
- [ ] **Step 5: Commit** — `feat(cp): /api/node/connect tunnel WS (handshake + pumps + keepalive)`

---

## Task 6: `TunnelHostConnector` (rupu-cp)

**Files:** Create `crates/rupu-cp/src/host/tunnel.rs`; modify `src/host/registry.rs` (resolve `Tunnel` → it). Test: `tests/node_tunnel.rs`.

**Interfaces — Consumes:** `HostConnector` trait, `NodeRegistry`, `NodeMirror`/`RunStore`, `Frame`. **Produces:** `TunnelHostConnector { node_id, registry: Arc<NodeRegistry>, mirror: Arc<NodeMirror>, run_store: Arc<RunStore> }` impl `HostConnector`:
- `launch_run(req)`: allocate `run_id`, `mirror.create_run(run_id, node_id, spec)`, `registry.get(node_id).ok_or(Offline)?.send(Frame::Run{run_id, spec})`; return run_id. `launch_agent` similar (RunSpecKind::Agent). `start_session`/`send_session_turn` → `Err(Invalid("sessions not supported over tunnel (slice 2)"))`.
- `cancel_run(id)`: `registry.get(node_id)?.send(Frame::Cancel{run_id:id})`; (also mark mirror cancelled if you choose).
- `list_runs/get_run/stream_run_events/proxy_get_json`: read the **mirror** (`run_store`) filtered to this node's runs (reuse the local read builders from Slice 1's `LocalHostConnector` — extract/share if needed). `info()`: `reachable = registry.is_online(node_id)`.
- `approve_run`/`reject_run` → `Err(Invalid("approval over tunnel not supported (slice 2)"))`.

- [ ] **Step 1: Failing tests** — `launch_run` on an online fake node creates a mirror run + the node receives a `Run` frame with that run_id; `cancel_run` sends `Cancel`; `launch_run` on an OFFLINE node → `Offline` error; `list_runs` returns the node's mirrored runs.
- [ ] **Step 2–4:** implement; register `Tunnel` resolution in `host/registry.rs`; test + clippy → PASS.
- [ ] **Step 5: Commit** — `feat(cp): TunnelHostConnector (mirror reads + frames for control)`

---

## Task 7: AppState + serve wiring

**Files:** Modify `src/state.rs` (`nodes: Arc<NodeRegistry>` + `node_mirror`), `src/server.rs` (merge `node::routes()`), `src/host/registry.rs` (build `TunnelHostConnector` with the shared `NodeRegistry`+mirror), `crates/rupu-cli/src/cmd/cp.rs` if needed. Test: `tests/server.rs` + `node_tunnel.rs`.

**Interfaces:** `AppState.nodes: Arc<NodeRegistry>`; the `HostRegistry` gets the `NodeRegistry`+`NodeMirror` so its `resolve()` can build a `TunnelHostConnector` for `Tunnel` hosts. `AppState::new` creates an empty `NodeRegistry`; `/api/node/connect` uses `s.nodes` + the mirror.

- [ ] **Step 1: Failing test** — router builds with the node route; a `Tunnel` host in the store resolves (via registry) to a connector whose `info().reachable` is false when no node is connected.
- [ ] **Step 2–4:** wire; `cargo test -p rupu-cp` + clippy → PASS.
- [ ] **Step 5: Commit** — `feat(cp): wire NodeRegistry + tunnel resolution into AppState/serve`

---

## Task 8: `rupu node` agent + enroll CLI (rupu-cli)

**Files:** Create `crates/rupu-cli/src/cmd/node.rs`; modify command enum/dispatch; `Cargo.toml` (`tokio-tungstenite` workspace). Test: a focused unit test for the artifact-tailing/dispatch helper + a manual/integration note.

**Interfaces:** `rupu node --cp-url <wss-url> --token <t>|--token-stdin` (the agent): connect via `tokio-tungstenite`, send `Hello{node_id from a local config or --node-id, auth: Token}`, await `Welcome`; loop: `Run{run_id, spec}` → spawn `rupu workflow run <name> --run-id <run_id> --plain [--input k=v]… [--mode m]` (or `rupu run <agent> --run-id … [--prompt]`) DETACHED (reuse the argv builders from `cp_launcher`/`cp_agent_launcher`), then tail `<global>/runs/<run_id>/{events.jsonl,step_results.jsonl,unit_checkpoints.jsonl}` + read `run.json` via a `FileTailRunSource`-style poller, sending `Artifact` frames per new line, then `RunFinished` on terminal status; `Cancel{run_id}` → kill the child's process group; `Ping`→`Pong`. Reconnect with exponential backoff (cap 60 s), re-`Hello`. `rupu node enroll <name> [--cp-url …]` → call the CP enroll endpoint (or local store) and print the `rupu node …` command + token.

- [ ] **Step 1: Failing test** — a unit test for the run-dir tail→Frame helper: given a temp run dir with N events.jsonl lines, the helper yields N `Artifact{file:Events}` frames in order. (Full dial loop is covered by the Task-10 integration test.)
- [ ] **Step 2–4:** implement (thin: dispatch + subprocess + tail + WS client). `cargo test -p rupu-cli node` + clippy on the new file (note: per project memory, ignore PRE-EXISTING unrelated rupu-cli toolchain clippy/test baseline issues — assert only the new code compiles + its test passes). Commit.
- [ ] **Step 5: Commit** — `feat(cli): rupu node dial-home agent + enroll`

---

## Task 9: Web — Add Node (enrollment) UI

**Files:** Modify `crates/rupu-cp/web/src/pages/Hosts.tsx` (Add-host form gains a "tunnel node" path), `src/lib/api.ts` (`enrollNode`). Backend: a `POST /api/hosts/node` (or extend `POST /api/hosts`) that calls `enroll_node` + returns `{ host, command, token }`. Test: vitest.

**Interfaces:** `api.enrollNode({ name }): Promise<{ host: HostView; command: string; token: string }>`. The Hosts page Add form offers transport "Tunnel node"; on submit it calls `enrollNode`, then shows a one-time panel with the `rupu node --cp-url … --token …` command (copyable) + a "token shown once" warning. The node then appears in the list (offline until it connects). Use the existing themed components (Button/Chip/HostStatusBadge).

- [ ] **Step 1: Failing test** — submitting the Add form in "tunnel" mode calls `api.enrollNode` and renders the returned command + token-once warning.
- [ ] **Step 2–4:** implement backend `enroll` route + web; `npx vitest run src/pages/Hosts.test.tsx` + `tsc -b`; `cargo test -p rupu-cp` for the route + clippy → PASS.
- [ ] **Step 5: Commit** — `feat(cp): enroll-node API + Add-node UI (one-time command/token)`

---

## Task 10: Integration e2e — fake node, dispatch, mirror, cancel

**Files:** `crates/rupu-cp/tests/node_tunnel.rs` (extend).

**Interfaces:** spin the CP router on a real listener; enroll a node (`enroll_node`); connect a **fake node WS client** (`tokio-tungstenite`) that sends `Hello` (valid token) → gets `Welcome`. Then drive via the CENTRAL: `POST /api/agents/<a>/run` (or workflows) with `host=<node_id>` → assert the fake node receives a `Run` frame; the fake node replies with a few `Artifact{Events}` frames + `RunFinished{completed}`; assert the central `GET /api/runs?host=<node_id>` (mirror) shows the run + its events via `GET /api/runs/<id>/log?host=<node_id>`; then `POST /api/runs/<id>/cancel?host=<node_id>` → assert the fake node receives a `Cancel` frame.

- [ ] **Step 1: Write the test** as above (reuse Slice-1.5 federation_e2e harness patterns + the Task-5 fake WS client).
- [ ] **Step 2: Run** `cargo test -p rupu-cp --test node_tunnel` → PASS; full `cargo test -p rupu-cp` + clippy green.
- [ ] **Step 3: Commit** — `test(cp): node tunnel e2e — dispatch, mirror, observe, cancel`

---

## Final verification (lead, end of batch)

- [ ] `cargo test -p rupu-cp -p rupu-workspace` + `cargo clippy -p rupu-cp -p rupu-workspace` clean.
- [ ] `cd crates/rupu-cp/web && npx vitest run && npm run build` clean.
- [ ] Manual (matt): `rupu cp serve` on host A; enroll a node from the CP; on host B run the printed `rupu node --cp-url wss://A:7878 --token …`; confirm B shows online in Fleet/Hosts; launch a workflow + an agent run targeting B; watch live events stream into the central RunDetail; cancel one. Toggle dark mode on the Add-node panel.

## Self-review (coverage)

- Spec §"tunnel server + NodeRegistry" → Tasks 3,5,7. §"wire protocol" → Task 1. §"node agent" → Task 8. §"mirror" → Task 4. §"TunnelHostConnector" → Task 6. §"enrollment + Host model" → Tasks 2,9. §"CP surface" → Tasks 5,7,9. §"errors/security" (token hash, offline, keepalive, evict) → Tasks 2,3,5,6. §"testing" → per-task + Task 10.
- Open questions resolved: token hash in host TOML (Task 2); CP-allocated run_id in `Run` (Tasks 1,6); per-line `Artifact` frames + bounded channel (Tasks 1,3,5).
- Out of scope honored: sessions/approval over tunnel return `Invalid` (Task 6); pull/SSH not present.
