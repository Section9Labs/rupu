# rupu Multi-Host Slice 1 (Federated HTTP Control) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the central rupu CP register remote hosts that run `rupu cp serve`, and launch / observe / control their runs, agents, and sessions over HTTP — with `local` as host[0].

**Architecture:** One `HostConnector` async trait is the transport seam. `LocalHostConnector` delegates to today's in-process ports + `RunStore`; `HttpHostConnector` is a `reqwest` client of a remote CP's existing API. A `HostRegistry` (in `AppState`) resolves `host_id → Arc<dyn HostConnector>` from a file-backed `HostStore` (tokens in the OS keyring). Host-aware API handlers proxy to the resolved connector; run lists fan out across hosts and tolerate offline ones (proxy / live-query — no central mirror).

**Tech Stack:** Rust (axum, tokio, reqwest, async-trait, serde, thiserror, keyring), React + TypeScript + Tailwind (CP web), vitest, httpmock.

## Global Constraints

- Hexagonal separation: `HostConnector` is a port (trait) in `rupu-cp`; concrete connectors are adapters. `rupu-cli` stays thin (arg parse + delegate).
- Workspace deps only — pin versions in the root `Cargo.toml`, never in crate `Cargo.toml`s.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- Errors: `thiserror` in libraries, `anyhow` in the CLI. Async: `tokio`. Logging: `tracing`.
- `local` host is implicit and always present; it is host[0] and must behave exactly as today (parity).
- Federation (any remote host) requires `rupu cp serve` (full runtime). Read-only `rupu cp` exposes only `local`.
- Run addressing on the wire: a `?host=<host_id>` query param; default `local`. Do NOT change the run-id shape.
- Remote tokens live in the OS keyring (`keyring` crate), service `"rupu-host"`, account = `host_id`. The `HostStore` TOML never contains the token.
- Web build/test from `crates/rupu-cp/web`: `npm run build` (tsc + vite), `npx vitest run`. Rust: `cargo test -p <crate>`, `cargo clippy -p <crate>`. Per project memory, run web build/tsc once at the end of a batch (concurrent builds race); do not run package-wide `cargo fmt` (per-file only).

---

## File Structure

**rupu-workspace** (shared host model + store)
- Create `crates/rupu-workspace/src/host_store.rs` — `Host`, `HostTransport`, `HostStatus`, `HostStore` (file-backed at `<global>/hosts/<id>.toml`, mirrors `worker_store.rs`) + keyring token helpers.
- Modify `crates/rupu-workspace/src/lib.rs` — `pub mod host_store;` + re-exports.

**rupu-cp** (port + connectors + registry + API)
- Create `crates/rupu-cp/src/host/mod.rs` — module root + re-exports.
- Create `crates/rupu-cp/src/host/connector.rs` — `HostConnector` trait, `HostInfo`, `HostConnectorError`, `RunListRow` reuse.
- Create `crates/rupu-cp/src/host/http.rs` — `HttpHostConnector` (reqwest client of a remote CP).
- Create `crates/rupu-cp/src/host/local.rs` — `LocalHostConnector` (delegates to existing ports + `RunStore`).
- Create `crates/rupu-cp/src/host/registry.rs` — `HostRegistry` (resolve/add/remove, connector cache, hot-reload).
- Create `crates/rupu-cp/src/api/host_info.rs` — `GET /api/host/info`.
- Create `crates/rupu-cp/src/api/hosts.rs` — `GET/POST/DELETE /api/hosts`.
- Modify `crates/rupu-cp/src/state.rs` — add `hosts: Arc<HostRegistry>` + builder.
- Modify `crates/rupu-cp/src/server.rs` — mount `host_info` + `hosts` routes.
- Modify `crates/rupu-cp/src/api/{workflows,agents,sessions}.rs` — accept optional `host`, route via registry for non-local.
- Modify `crates/rupu-cp/src/api/runs.rs` + `events.rs` + `transcript.rs` — `?host=` routing (proxy for remote).
- Modify `crates/rupu-cp/src/api/run_streams.rs` (or wherever run lists are built) — fan out across hosts.
- Modify `crates/rupu-cp/Cargo.toml` — move `reqwest` to `[dependencies]`; add `keyring` if not present transitively.
- Tests: `crates/rupu-cp/tests/host_http.rs`, `host_registry.rs`, `hosts_api.rs`, extend `tests/sse.rs`.

**rupu-cli** (serve wiring + CLI)
- Modify `crates/rupu-cli/src/cmd/cp.rs` — build `HostRegistry` (local connector from existing adapters + http connectors from store) and pass to `serve`.
- Create `crates/rupu-cli/src/cmd/host.rs` — `rupu host add|list|remove`.
- Modify `crates/rupu-cli/src/main.rs` (or command enum) — register `host` subcommand.

**web** (`crates/rupu-cp/web/src`)
- Modify `lib/api.ts` — host types + `getHosts/addHost/removeHost`; `host` param on launch/list/run/events/control helpers.
- Create `pages/Hosts.tsx` + `pages/HostDetail.tsx`.
- Modify `lib/sidebarNav.ts`, `App.tsx` — Fleet group + routes.
- Modify run-list pages + `components/TargetPicker.tsx`/`LauncherSheet.tsx`/`RunDetail.tsx` — host column/filter, host selector, host on detail.
- Tests: `pages/Hosts.test.tsx`, `lib/api` host helpers, `components/HostSelect.test.tsx`.

**Integration**
- Create `crates/rupu-cp/tests/federation_e2e.rs` — real `cp serve` as remote, federate, launch→observe→cancel.

---

## Task 1: Host model + `HostStore` (rupu-workspace)

**Files:**
- Create: `crates/rupu-workspace/src/host_store.rs`
- Modify: `crates/rupu-workspace/src/lib.rs`
- Test: `crates/rupu-workspace/src/host_store.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces:
  - `struct Host { id: String, name: String, transport: HostTransport, created_at: String, last_seen_at: Option<String> }`
  - `enum HostTransport { Local, HttpCp { base_url: String } }` (token NOT stored here)
  - `struct HostStore { root: PathBuf }` with `save(&Host)`, `load(&str)->Option<Host>`, `list()->Vec<Host>`, `delete(&str)`, mirroring `WorkerStore`.
  - keyring helpers: `set_host_token(host_id, token) -> Result`, `get_host_token(host_id) -> Result<Option<String>>`, `delete_host_token(host_id)` using `keyring::Entry::new("rupu-host", host_id)`.
  - `Host::local() -> Host` (the implicit host[0], id `"local"`).

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn http_host(id: &str) -> Host {
        Host {
            id: id.into(),
            name: format!("host {id}"),
            transport: HostTransport::HttpCp { base_url: "https://h:8787".into() },
            created_at: "2026-06-28T00:00:00Z".into(),
            last_seen_at: None,
        }
    }

    #[test]
    fn save_load_list_delete_roundtrip() {
        let dir = tempdir().unwrap();
        let store = HostStore { root: dir.path().join("hosts") };
        assert!(store.list().unwrap().is_empty());
        store.save(&http_host("host_a")).unwrap();
        store.save(&http_host("host_b")).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);
        let a = store.load("host_a").unwrap().unwrap();
        assert!(matches!(a.transport, HostTransport::HttpCp { .. }));
        store.delete("host_a").unwrap();
        assert!(store.load("host_a").unwrap().is_none());
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn local_host_is_local_transport() {
        assert_eq!(Host::local().id, "local");
        assert!(matches!(Host::local().transport, HostTransport::Local));
    }
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p rupu-workspace host_store -- --nocapture`
Expected: FAIL (module/types not found).

- [ ] **Step 3: Implement `host_store.rs`**

Mirror `crates/rupu-workspace/src/worker_store.rs` exactly for the file layout (atomic write via `.tmp` + rename, `list()` skips non-`.toml`, TOML (de)serialize). Add `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]` to `Host`/`HostTransport`/`HostStatus`. `HostTransport` is `#[serde(tag = "kind", rename_all = "snake_case")]`. Keyring helpers wrap `keyring::Entry::new("rupu-host", host_id)?` `.set_password`/`.get_password` (map `keyring::Error::NoEntry` → `Ok(None)` in `get`). `HostStatus { Online, Offline, Stale }` is a runtime-derived enum (not persisted) used later by the API.

- [ ] **Step 4: Add to lib.rs**

```rust
pub mod host_store;
pub use host_store::{Host, HostStatus, HostStore, HostTransport};
```

- [ ] **Step 5: Run tests + clippy, verify pass**

Run: `cargo test -p rupu-workspace host_store && cargo clippy -p rupu-workspace`
Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-workspace/src/host_store.rs crates/rupu-workspace/src/lib.rs
git commit -m "feat(workspace): Host model + file-backed HostStore + keyring tokens"
```

---

## Task 2: `HostConnector` port + `LocalHostConnector` (rupu-cp)

Defines the trait and the local impl together (a trait alone isn't testable; the local impl gives parity coverage).

**Files:**
- Create: `crates/rupu-cp/src/host/mod.rs`, `connector.rs`, `local.rs`
- Modify: `crates/rupu-cp/src/lib.rs` (`pub mod host;`)
- Test: `crates/rupu-cp/tests/host_local.rs`

**Interfaces:**
- Consumes: existing ports `RunLauncher`/`AgentLauncher`/`SessionStarter`/`SessionSender` (from `crate::launcher` etc.), `rupu_orchestrator::runs::RunStore`, existing request types `LaunchRequest`/`AgentLaunchRequest`/`SessionStartRequest`/`SendMessageRequest`.
- Produces:
  ```rust
  #[async_trait::async_trait]
  pub trait HostConnector: Send + Sync {
      async fn info(&self) -> Result<HostInfo, HostConnectorError>;
      async fn launch_run(&self, req: LaunchRequest) -> Result<String, HostConnectorError>;
      async fn launch_agent(&self, req: AgentLaunchRequest) -> Result<String, HostConnectorError>;
      async fn start_session(&self, req: SessionStartRequest) -> Result<String, HostConnectorError>;
      async fn send_session_turn(&self, req: SendMessageRequest) -> Result<String, HostConnectorError>;
      async fn list_runs(&self, params: RunListQuery) -> Result<Vec<serde_json::Value>, HostConnectorError>;
      async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError>;
      async fn approve_run(&self, run_id: &str, mode: &str) -> Result<(), HostConnectorError>;
      async fn reject_run(&self, run_id: &str, reason: Option<&str>) -> Result<(), HostConnectorError>;
      async fn cancel_run(&self, run_id: &str) -> Result<(), HostConnectorError>;
      // stream_run_events returns a boxed byte stream of SSE frames; see Task 8.
      async fn stream_run_events(&self, run_id: &str) -> Result<EventByteStream, HostConnectorError>;
  }
  pub struct HostInfo { pub reachable: bool, pub version: Option<String>, pub capabilities: rupu_workspace::host_store::HostCapabilities }
  pub enum HostConnectorError { Unreachable(String), Unauthorized, NotFound(String), Remote(u16, String), Invalid(String) }
  pub struct RunListQuery { pub kind: RunKind, pub offset: usize, pub limit: usize, pub lifecycle: Option<String> }
  ```
  - `LocalHostConnector { launcher, agent_launcher, session_starter, session_sender, run_store, global_dir }` implementing `HostConnector` by delegating to the held ports + `RunStore`. `list_runs`/`get_run` return the SAME `serde_json::Value` shape the existing `api::runs` handlers produce (extract a shared builder fn if needed so local + remote agree).

- [ ] **Step 1: Write failing test** (`tests/host_local.rs`)

```rust
// LocalHostConnector.list_runs returns the same rows the runs API serves.
#[tokio::test]
async fn local_connector_lists_runs() {
    // seed a run via RunStore (reuse helper from tests/sse.rs seed_run)
    // build LocalHostConnector with no-op launchers + the seeded RunStore
    // assert list_runs(RunKind::Workflow, ..) returns 1 row whose id matches
}
```
(Write it concretely using the `seed_run`/`RunStore` pattern from `crates/rupu-cp/tests/sse.rs`.)

- [ ] **Step 2: Run, verify fail** — `cargo test -p rupu-cp --test host_local` → FAIL (types missing).

- [ ] **Step 3: Implement `connector.rs`** — the trait + types above. `HostConnectorError` is `thiserror`. Map nothing yet (pure definitions).

- [ ] **Step 4: Implement `local.rs`** — `LocalHostConnector` delegating control methods to the held port traits; `list_runs`/`get_run` call the existing run-listing/detail logic. **Refactor:** if run-row building is inline in `api/runs.rs`/`run_streams.rs`, extract a `pub(crate) fn build_run_rows(store, query) -> Vec<Value>` and `build_run_detail(store, id) -> Value` and call them from both the connector and the existing handlers (DRY). `info()` returns `{reachable:true, version: env!("CARGO_PKG_VERSION"), capabilities: <local caps>}`. `stream_run_events` opens the local `FileTailRunSource` (reuse `crate::sse`) boxed as `EventByteStream`.

- [ ] **Step 5: Run test + clippy** — `cargo test -p rupu-cp --test host_local && cargo clippy -p rupu-cp` → PASS.

- [ ] **Step 6: Commit** — `git commit -m "feat(cp): HostConnector port + LocalHostConnector (host[0] parity)"`

---

## Task 3: `HttpHostConnector` (rupu-cp)

**Files:**
- Modify: `crates/rupu-cp/Cargo.toml` (move `reqwest` to `[dependencies]`, features `["json","stream"]`; ensure `keyring` available via rupu-workspace re-export or add as dep)
- Create: `crates/rupu-cp/src/host/http.rs`
- Test: `crates/rupu-cp/tests/host_http.rs` (uses `httpmock` — add to root `Cargo.toml` dev-deps + rupu-cp `[dev-dependencies]`)

**Interfaces:**
- Consumes: `HostConnector` trait (Task 2), `Host`/`HostTransport` (Task 1).
- Produces: `HttpHostConnector { client: reqwest::Client, base_url: String, token: Option<String> }` impl `HostConnector`. Constructor `new(base_url, token) -> Self`.

Endpoint mapping (all with `Authorization: Bearer <token>` when token set):
- `info` → `GET {base}/api/host/info` (Task 7); on connect error → `HostInfo{reachable:false,..}` (do NOT error).
- `launch_run` → `POST {base}/api/workflows/{workflow}/run` body `{inputs,mode,target,working_dir}` → `{run_id}`.
- `launch_agent` → `POST {base}/api/agents/{agent}/run` body `{prompt,mode,target,working_dir}` → `{run_id}`.
- `start_session` → `POST {base}/api/agents/{agent}/session` → `{session_id}`.
- `send_session_turn` → `POST {base}/api/sessions/{id}/send` body `{prompt}` → `{run_id}`.
- `list_runs` → `GET {base}/api/runs/{workflows|agents|autoflows}?offset&limit&lifecycle` → `Vec<Value>`.
- `get_run` → `GET {base}/api/runs/{id}` → `Value`.
- `approve_run`/`reject_run`/`cancel_run` → `POST {base}/api/runs/{id}/{approve|reject|cancel}`.
- `stream_run_events` → `GET {base}/api/events/stream?run={id}` (Accept: text/event-stream), return `resp.bytes_stream()` boxed.
- Status mapping: 401→`Unauthorized`, 404→`NotFound`, connect/timeout→`Unreachable`, other ≥400→`Remote(status, body)`.

- [ ] **Step 1: Write failing tests** (`tests/host_http.rs`) using `httpmock::MockServer`:

```rust
#[tokio::test]
async fn launch_run_posts_with_bearer_and_returns_run_id() {
    let server = httpmock::MockServer::start_async().await;
    let m = server.mock(|when, then| {
        when.method("POST").path("/api/workflows/wf/run")
            .header("authorization", "Bearer tok");
        then.status(200).json_body(serde_json::json!({"run_id":"run_X"}));
    });
    let c = HttpHostConnector::new(server.base_url(), Some("tok".into()));
    let id = c.launch_run(LaunchRequest{ workflow:"wf".into(), ..Default::default() }).await.unwrap();
    assert_eq!(id, "run_X");
    m.assert();
}

#[tokio::test]
async fn info_unreachable_does_not_error() {
    let c = HttpHostConnector::new("http://127.0.0.1:9".into(), None); // closed port
    let info = c.info().await.unwrap();
    assert!(!info.reachable);
}

#[tokio::test]
async fn unauthorized_maps_to_error() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| { when.method("GET").path("/api/runs/run_x"); then.status(401); });
    let c = HttpHostConnector::new(server.base_url(), Some("bad".into()));
    assert!(matches!(c.get_run("run_x").await, Err(HostConnectorError::Unauthorized)));
}
```
(Add tests for list_runs, cancel, and an SSE pass-through smoke test.)

- [ ] **Step 2: Run, verify fail** — `cargo test -p rupu-cp --test host_http` → FAIL.
- [ ] **Step 3: Move reqwest to deps; add httpmock dev-dep** (root + crate). 
- [ ] **Step 4: Implement `http.rs`** per the mapping. Use one private `async fn send(req) -> Result<Response, HostConnectorError>` that attaches the bearer + maps transport/status errors, so every method is DRY.
- [ ] **Step 5: Run tests + clippy** → PASS.
- [ ] **Step 6: Commit** — `git commit -m "feat(cp): HttpHostConnector — client of a remote rupu cp serve"`

---

## Task 4: `HostRegistry` (rupu-cp)

**Files:** Create `crates/rupu-cp/src/host/registry.rs`; Test `crates/rupu-cp/tests/host_registry.rs`.

**Interfaces:**
- Consumes: `HostStore` (Task 1), `LocalHostConnector` (Task 2), `HttpHostConnector` (Task 3).
- Produces:
  ```rust
  pub struct HostRegistry { store: HostStore, local: Arc<dyn HostConnector>, cache: Mutex<HashMap<String, Arc<dyn HostConnector>>> }
  impl HostRegistry {
    pub fn new(store: HostStore, local: Arc<dyn HostConnector>) -> Self
    pub fn resolve(&self, host_id: &str) -> Result<Arc<dyn HostConnector>, HostConnectorError> // "local" → local; else build/reuse Http from store (token from keyring); unknown id → NotFound
    pub fn list_hosts(&self) -> Vec<Host> // local first, then store
    pub fn add_host(&self, name, base_url, token) -> Result<Host> // writes store + keyring; invalidates cache entry
    pub fn remove_host(&self, host_id) -> Result<()>          // deletes store + keyring + cache; refuses "local"
  }
  ```
  Hot-reload: `resolve` reads the store each call for non-cached ids; `add_host`/`remove_host` mutate store+cache so changes take effect without restarting `cp serve`.

- [ ] **Step 1: Failing tests** — resolve("local")→local; resolve(unknown)→NotFound; add_host then resolve returns an Http connector pointed at the right base_url; remove_host("local") errors; list_hosts puts local first.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** registry with `Mutex<HashMap>` cache; build `HttpHostConnector` from `Host.transport` + `get_host_token`.
- [ ] **Step 4: Run + clippy** → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp): HostRegistry — resolve host_id to a connector, hot-reload"`

---

## Task 5: `AppState.hosts` + serve wiring

**Files:** Modify `crates/rupu-cp/src/state.rs`, `crates/rupu-cp/src/lib.rs` (`ServeOpts`), `crates/rupu-cli/src/cmd/cp.rs`. Test: extend `crates/rupu-cp/tests/server.rs`.

**Interfaces:**
- Produces: `AppState.hosts: Arc<HostRegistry>`; `AppState::with_hosts(...)`. In `AppState::new`, default to a registry whose `local` connector is a `LocalHostConnector` built from the (initially `None`) ports + `run_store` — i.e. read-only `local`-only. `cp serve` (`cp.rs`) builds the `LocalHostConnector` from the real adapters and a `HostStore` rooted at `<global>/hosts`, then `with_hosts`.

- [ ] **Step 1: Failing test** — `AppState::new(..)` exposes `hosts.list_hosts()` containing exactly `local`; `router(state,..)` still builds.
- [ ] **Step 2–4:** wire it; `cargo test -p rupu-cp --test server` + clippy → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp): AppState.hosts registry + cp serve wiring"`

---

## Task 6: `GET /api/host/info` endpoint

**Files:** Create `crates/rupu-cp/src/api/host_info.rs`; Modify `server.rs`. Test: `crates/rupu-cp/tests/hosts_api.rs`.

**Interfaces:** Produces `GET /api/host/info` → `{ version: String, capabilities: { backends, scm_hosts, permission_modes } }`. Capabilities reuse the local worker capabilities (aggregate from `WorkerStore` or a static local set). This is what a remote `HttpHostConnector.info()` calls.

- [ ] **Step 1: Failing test** — GET returns 200 with `version == env!("CARGO_PKG_VERSION")`.
- [ ] **Step 2–4:** implement + mount; test + clippy → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp): GET /api/host/info (version + capabilities)"`

---

## Task 7: Hosts CRUD API (`/api/hosts`)

**Files:** Create `crates/rupu-cp/src/api/hosts.rs`; Modify `server.rs`. Test: `crates/rupu-cp/tests/hosts_api.rs`.

**Interfaces:**
- `GET /api/hosts` → `Vec<HostView>` where `HostView { id, name, transport_kind, base_url?, status, version?, capabilities?, active_run_count, last_seen_at }`. For each non-local host, call `connector.info()` concurrently (tolerate failure → `status: "offline"`); `active_run_count` derived from `list_runs` (best-effort; 0 on offline). `local` always present, `status: "online"`.
- `POST /api/hosts` body `{ name, base_url, token }` → `HostView` (calls `registry.add_host`). Requires `cp serve` (registry add) — read-only returns 501 via `ApiError::not_available`.
- `DELETE /api/hosts/:id` → 204 (refuse `local` → 400).

- [ ] **Step 1: Failing tests** — list returns `local`; POST adds a host (mock keyring or use a temp keyring service) and it appears in list; DELETE removes; DELETE local → 400.
- [ ] **Step 2–4:** implement; concurrent `info()` via `futures` join (mirror the events-firehose fan-out tolerance). test + clippy → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp): /api/hosts CRUD + health"`

---

## Task 8: Host-aware run observation — list fan-out, detail, events SSE, transcript

**Files:** Modify `crates/rupu-cp/src/api/{runs.rs,events.rs,transcript.rs}` and the run-list builder (`run_streams.rs`). Test: extend `crates/rupu-cp/tests/sse.rs` + `host_http.rs`.

**Interfaces:**
- All run-read endpoints accept optional `?host=<id>` (default `local`). For `local`, keep today's code path unchanged (parity). For a remote id, resolve the connector and **proxy**:
  - list: a new aggregating mode — when the client asks for "all hosts" (the run lists page), fan out `list_runs` across `registry.list_hosts()` concurrently, tag each row `{"host_id": id}`, merge; an offline host contributes nothing + a surfaced warning (do not fail). When `?host=` is a single id, just that host.
  - `GET /api/runs/:id?host=` → proxy `get_run`.
  - `GET /api/events/stream?run=&host=` → if remote, open `connector.stream_run_events(run)` and pipe its byte stream into the SSE response (reuse `crate::sse` plumbing; the remote already emits SSE frames, so pass through).
  - `GET /api/transcript?path=&host=` → proxy.
- Define `EventByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>` in `host/connector.rs`; `LocalHostConnector` adapts `FileTailRunSource`→bytes, `HttpHostConnector` returns `resp.bytes_stream()`.

- [ ] **Step 1: Failing tests** — (a) list fan-out merges local + a mock-remote host's runs, each tagged with `host_id`, and an offline host doesn't break it; (b) `/api/runs/:id?host=<remote>` proxies to the mock; (c) `/api/events/stream?run=&host=<remote>` yields the remote's frames.
- [ ] **Step 2–4:** implement; reuse the firehose fan-out + per-host `.catch`. test + clippy → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp): host-aware run list fan-out + detail/events/transcript proxy"`

---

## Task 9: Host-aware control + launch

**Files:** Modify `crates/rupu-cp/src/api/{workflows.rs,agents.rs,sessions.rs,runs.rs}`. Test: `hosts_api.rs` + `host_http.rs`.

**Interfaces:** launch endpoints (`/api/workflows/:name/run`, `/api/agents/:name/{run,session}`, `/api/sessions/:id/send`) and control endpoints (`/api/runs/:id/{approve,reject,cancel}`) accept optional `host` (body field for launch, `?host=` for control); default `local`. Resolve connector → call the matching method. Response includes `host_id`.

- [ ] **Step 1: Failing tests** — launch with `host:<remote>` proxies to the mock and returns `{host_id, run_id}`; cancel with `?host=<remote>` proxies.
- [ ] **Step 2–4:** implement (local path unchanged). test + clippy → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp): host-aware launch + run control (proxy to owning host)"`

---

## Task 10: CLI `rupu host add|list|remove`

**Files:** Create `crates/rupu-cli/src/cmd/host.rs`; Modify the command enum/dispatcher. Test: `crates/rupu-cli/tests/` (or inline) for arg parsing + store side-effects.

**Interfaces:** thin clap subcommand delegating to `HostStore`/keyring (same as the registry uses): `rupu host add <name> --url <base> [--token <t>|--token-stdin]`, `rupu host list`, `rupu host remove <id>`. No business logic beyond store calls.

- [ ] **Step 1: Failing test** — `add` writes a `HostStore` entry + keyring token; `list` prints it; `remove` deletes. (Use a temp `--global`-style root if the CLI supports it, else test the underlying helper.)
- [ ] **Step 2–4:** implement; `cargo test -p rupu-cli host` (note: per project memory the worktree toolchain may make rupu-cli baseline red — assert only the new test compiles/passes, ignore pre-existing unrelated failures) + clippy on the new file.
- [ ] **Step 5: Commit** — `git commit -m "feat(cli): rupu host add|list|remove"`

---

## Task 11: Web API client — host types + helpers

**Files:** Modify `crates/rupu-cp/web/src/lib/api.ts`. Test: `crates/rupu-cp/web/src/lib/api.host.test.ts`.

**Interfaces:** Produces TS types `HostView`, `HostTransportKind`; methods `getHosts()`, `addHost({name,base_url,token})`, `removeHost(id)`. Extend launch/list/run/events/control helpers with an optional `host` arg (appended as `?host=` or body field) — default omitted (local). `subscribeEvents`/`subscribeRunLog` gain an optional `host`.

- [ ] **Step 1: Failing test** — `getHosts` calls `/api/hosts`; launch helper appends `host` when provided, omits when not.
- [ ] **Step 2–4:** implement (mirror existing api.ts patterns); `npx vitest run src/lib/api.host.test.ts` → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp/web): host API types + helpers"`

---

## Task 12: Web — Hosts page + Fleet nav

**Files:** Create `pages/Hosts.tsx`, `pages/HostDetail.tsx`; Modify `lib/sidebarNav.ts`, `App.tsx`. Test: `pages/Hosts.test.tsx`.

**Interfaces:** Consumes `getHosts/addHost/removeHost`, `SortableTable`. A "Fleet" sidebar group → `/hosts`. Hosts table columns: Name · Transport · Status (status tokens) · Version · Active runs · Last seen, `local` pinned first. Add-Host form (name, base URL, token). HostDetail at `/hosts/:id` reuses the run list scoped to that host.

- [ ] **Step 1: Failing test** — renders rows from a mocked `getHosts`, shows `local` first, Add-Host form posts.
- [ ] **Step 2–4:** implement using `SortableTable` + existing form patterns; `npx vitest run src/pages/Hosts.test.tsx` → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp/web): Fleet → Hosts page + nav"`

---

## Task 13: Web — host attribution on runs + launcher host selector

**Files:** Modify the run-list pages (`runs/*`, `Dashboard` recent runs), `components/TargetPicker.tsx`/`LauncherSheet.tsx`, `RunDetail.tsx`. Create `components/HostSelect.tsx`. Test: `components/HostSelect.test.tsx`.

**Interfaces:** A `HostSelect` (defaults to `local`) added to the launcher; launch passes the chosen `host`. Run-list rows show a Host column + a host filter (default "this host"); rows carry `host_id` so detail/events deep-link with `?host=`. `RunDetail` reads `?host=` and threads it into `getRun`/`subscribeRunLog`/control calls; shows the owning host.

- [ ] **Step 1: Failing test** — `HostSelect` lists hosts (mocked) and emits the chosen id; defaults to `local`.
- [ ] **Step 2–4:** implement; thread `host` through detail/events/control; `npx vitest run` (HostSelect + affected pages) → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp/web): host selector + host-attributed run rows/detail"`

---

## Task 14: Integration test — federate two real instances

**Files:** Create `crates/rupu-cp/tests/federation_e2e.rs`.

**Interfaces:** Spin a real `rupu_cp::serve`-equivalent router as "remote" (in-process axum server with a seeded `RunStore` + a fake/no-op launcher), register it via `HostRegistry.add_host(base_url, token)` on a "central" `AppState`, and assert: central `/api/hosts` shows it online; central `/api/runs?host=<remote>` returns the remote's seeded run; central `/api/runs/:id/cancel?host=<remote>` reaches the remote. (Full subprocess `cp serve` launch is optional/manual; the in-process two-router test is the automated gate.)

- [ ] **Step 1: Write the test** as above.
- [ ] **Step 2: Run, verify fail** (before Tasks 7–9 wired) / PASS after.
- [ ] **Step 3: Commit** — `git commit -m "test(cp): federation e2e — central proxies a remote host"`

---

## Final verification (run once, end of batch)

- [ ] `cargo test -p rupu-workspace -p rupu-cp` → all pass.
- [ ] `cargo clippy -p rupu-workspace -p rupu-cp` → clean.
- [ ] `cd crates/rupu-cp/web && npx vitest run && npm run build` → all pass, tsc + vite clean.
- [ ] Manual smoke (matt): `rupu cp serve` on host A; on host B `rupu host add A --url https://A:PORT --token …`; confirm A's runs list, open one (live events), launch a run on A, cancel it — from B's CP.

## Self-review notes (coverage)

- Spec §"transport port" → Task 2; "two implementations" → Tasks 2 (local) + 3 (http); "registry/state wiring" → Tasks 4–5.
- "Host entity + registry" (store, keyring token, implicit local, CRUD, CLI) → Tasks 1, 7, 10.
- "Data flow (proxy)" launch/list/detail/events/control → Tasks 8–9.
- "Host-info endpoint" open question → resolved, Task 6.
- "CP UI" → Tasks 12–13.
- "Errors + security" (offline tolerance, bearer, no new inbound) → Tasks 3 (error mapping), 7–8 (fan-out tolerance), 1 (keyring).
- "Testing" → per-task tests + Task 14 integration.
- Open questions resolved: `?host=` param (Tasks 8–9), `/api/host/info` (Task 6), registry hot-reload (Task 4).
