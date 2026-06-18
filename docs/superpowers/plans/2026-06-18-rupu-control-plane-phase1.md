# rupu Control Plane — Phase 1 (Observe) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `rupu cp serve` command that opens a localhost web UI (Okesu's design system + shell, ported) to **observe** everything rupu is doing on this host — dashboard, runs (live), live events, coverage, and read-only lists of agents/workflows/sessions/workers — over a thin axum API on top of rupu's existing file stores + event sink.

**Architecture:** New `crates/rupu-cp` crate = axum backend (read-only API + SSE) wrapping the existing `RunStore`/`WorkerStore`/listing/`FileTailRunSource`; a Vite/React frontend under `crates/rupu-cp/web` vendored+trimmed from `Okesu/web` and remapped to rupu resources, **embedded** into the binary via `rust-embed` and served on one localhost port.

**Tech Stack:** Rust (axum, tokio, serde, tower-http, rust-embed); React 18 + Vite 5 + TypeScript + Tailwind 3.4 + react-router 6 + @xyflow/react 12 + recharts 3 + lucide-react + CodeMirror 6.

**Spec:** `docs/superpowers/specs/2026-06-18-rupu-control-plane-design.md`

---

## File structure

```
crates/rupu-cp/
  Cargo.toml
  build.rs                      # fail build if web/dist missing (after frontend lands)
  src/
    lib.rs                      # pub fn serve(opts) ; AppState
    main_cmd.rs                 # arg struct for `rupu cp serve` (wired from rupu-cli)
    state.rs                    # AppState { global_dir, run_store, worker_store, pricing }
    server.rs                   # axum Router assembly + bind + run
    error.rs                    # ApiError -> IntoResponse (JSON)
    sse.rs                      # events.jsonl tail -> axum Sse stream
    api/
      mod.rs
      dashboard.rs              # GET /api/dashboard
      runs.rs                   # GET /api/runs, /api/runs/{id}, /api/runs/{id}/log (SSE)
      events.rs                 # GET /api/events/stream (SSE)
      agents.rs                 # GET /api/agents, /api/agents/{name}
      workflows.rs              # GET /api/workflows, /api/workflows/{name}
      sessions.rs               # GET /api/sessions, /api/sessions/{id}, .../transcript (SSE)
      workers.rs                # GET /api/workers
      coverage.rs               # GET /api/coverage, /api/coverage/{target}
    embed.rs                    # rust-embed of web/dist + SPA fallback handler
  web/                          # vendored from Okesu/web (trimmed)
    package.json, vite.config.ts, tailwind.config.ts, tsconfig.json
    src/
      main.tsx, App.tsx, styles.css
      lib/{cn.ts, sidebarNav.ts, api.ts, hooks…}
      components/{Layout, SidebarGroup, CommandPalette, Tooltip, TabBar,
                 StatusPill, lists/*, EventTimeline, OrchestrationCanvas,
                 dashboard/DashboardCharts, …}
      pages/{Dashboard, Runs, RunDetail, Events, Coverage, Agents,
             Workflows, Sessions, Workers}
```

Wiring into the existing CLI: add a `Cp { … }` subcommand in `crates/rupu-cli/src/lib.rs` (the clap dispatcher) that calls `rupu_cp::serve(...)`. `rupu-cp` is a workspace member; versions pinned in the root `Cargo.toml` per the workspace-deps rule.

---

## PART A — Backend (Rust, TDD)

### Task 1: Crate scaffold + `rupu cp serve` + healthz

**Files:**
- Create: `crates/rupu-cp/Cargo.toml`, `crates/rupu-cp/src/lib.rs`, `crates/rupu-cp/src/server.rs`, `crates/rupu-cp/src/state.rs`, `crates/rupu-cp/src/error.rs`
- Modify: root `Cargo.toml` (add `crates/rupu-cp` to `members`, add `axum`, `tower-http`, `rust-embed` to `[workspace.dependencies]` if absent), `crates/rupu-cli/src/lib.rs` (+`Cp` subcommand)

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "rupu-cp"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
axum = { workspace = true }
tokio = { workspace = true, features = ["full"] }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
tower-http = { workspace = true, features = ["cors", "trace"] }
tracing = { workspace = true }
anyhow = { workspace = true }
rust-embed = { workspace = true }
rupu-orchestrator = { path = "../rupu-orchestrator" }
rupu-runtime = { path = "../rupu-runtime" }
rupu-workspace = { path = "../rupu-workspace" }
rupu-config = { path = "../rupu-config" }

[dev-dependencies]
tempfile = { workspace = true }
reqwest = { workspace = true, features = ["json", "stream"] }

[lints]
workspace = true
```

(If `axum`/`tower-http`/`rust-embed` aren't yet workspace deps, add them to the root `[workspace.dependencies]`; `rupu-webhook` already depends on axum — copy its pinned versions.)

- [ ] **Step 2: state.rs — AppState**

```rust
use std::path::PathBuf;
use std::sync::Arc;
use rupu_orchestrator::runs::RunStore;

#[derive(Clone)]
pub struct AppState {
    pub global_dir: PathBuf,                 // ~/.rupu
    pub run_store: Arc<RunStore>,
    pub pricing: rupu_config::PricingConfig,
}

impl AppState {
    pub fn new(global_dir: PathBuf, pricing: rupu_config::PricingConfig) -> Self {
        let run_store = Arc::new(RunStore::new(global_dir.join("runs")));
        Self { global_dir, run_store, pricing }
    }
}
```

(Check `RunStore`'s real constructor signature in `crates/rupu-orchestrator/src/runs.rs` and match it; adjust the `runs` dir join if the store owns it.)

- [ ] **Step 3: error.rs — JSON error type**

```rust
use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

pub struct ApiError(pub StatusCode, pub String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}
impl ApiError {
    pub fn not_found(m: impl Into<String>) -> Self { Self(StatusCode::NOT_FOUND, m.into()) }
    pub fn internal(m: impl Into<String>) -> Self { Self(StatusCode::INTERNAL_SERVER_ERROR, m.into()) }
}
pub type ApiResult<T> = Result<T, ApiError>;
```

- [ ] **Step 4: server.rs + lib.rs — Router + serve + healthz**

```rust
// server.rs
use axum::{routing::get, Router};
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
}

// lib.rs
pub mod state; pub mod server; pub mod error;
use std::net::SocketAddr;

pub struct ServeOpts { pub bind: SocketAddr, pub token: Option<String>, pub global_dir: std::path::PathBuf }

pub async fn serve(opts: ServeOpts) -> anyhow::Result<()> {
    let pricing = rupu_config::PricingConfig::default(); // or load from config
    let state = state::AppState::new(opts.global_dir, pricing);
    let app = server::router(state);
    let listener = tokio::net::TcpListener::bind(opts.bind).await?;
    tracing::info!(%opts.bind, "rupu cp serving");
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 5: Wire `rupu cp serve` into the CLI**

In `crates/rupu-cli/src/lib.rs`, add a `Cp { #[command(subcommand)] action: CpAction }` to the top-level `Cmd` enum with `CpAction::Serve { #[arg(long, default_value="127.0.0.1:7878")] bind: SocketAddr, #[arg(long)] token: Option<String> }`, and dispatch it to `rupu_cp::serve(ServeOpts { bind, token, global_dir: paths::global_dir()? }).await`. Follow the existing thin-dispatcher pattern (no logic in the CLI).

- [ ] **Step 6: Test — healthz responds**

`crates/rupu-cp/tests/server.rs`:
```rust
#[tokio::test]
async fn healthz_ok() {
    let dir = tempfile::tempdir().unwrap();
    let state = rupu_cp::state::AppState::new(dir.path().into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let body = reqwest::get(format!("http://{addr}/healthz")).await.unwrap().text().await.unwrap();
    assert_eq!(body, "ok");
}
```

- [ ] **Step 7: Run + commit**
`cargo test -p rupu-cp` → PASS. `cargo build -p rupu-cli`. Commit `feat(cp): rupu-cp crate scaffold + cp serve + healthz`.

---

### Task 2: GET /api/runs + /api/runs/{id}

**Files:** Create `crates/rupu-cp/src/api/mod.rs`, `crates/rupu-cp/src/api/runs.rs`; modify `server.rs` (mount routes).

- [ ] **Step 1: Test (fixture run-store)** — seed a `RunStore` with one `RunRecord` (use the orchestrator's record constructors / write a `run.json`), then `GET /api/runs` returns a JSON array with that run, and `GET /api/runs/{id}` returns its record + steps.

```rust
#[tokio::test]
async fn lists_runs_and_one_run() {
    let dir = tempfile::tempdir().unwrap();
    // seed: create a RunStore at dir/runs and write one RunRecord via store.create(...)
    // (mirror crates/rupu-orchestrator/tests for how RunRecord is built)
    // GET /api/runs -> 200, array len 1, the run id present
    // GET /api/runs/{id} -> 200, has fields: id, status, steps[]
}
```

- [ ] **Step 2: Handler**

```rust
// api/runs.rs
use axum::{extract::{State, Path}, Json, routing::get, Router};
use crate::{state::AppState, error::{ApiError, ApiResult}};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs", get(list_runs))
        .route("/api/runs/:id", get(get_run))
}

async fn list_runs(State(s): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let runs = s.run_store.list().map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::to_value(runs).unwrap()))
}

async fn get_run(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let record = s.run_store.load(&id).map_err(|_| ApiError::not_found("run"))?;
    let steps = s.run_store.read_step_results(&id).unwrap_or_default();
    Ok(Json(serde_json::json!({ "run": record, "steps": steps })))
}
```

Mount in `server.rs`: `.merge(crate::api::runs::routes())`. (Confirm `RunRecord`/`StepResultRecord` derive `Serialize` — they do per the run-store; if a field needs hiding, add a thin DTO instead of serializing the record directly.)

- [ ] **Step 3-5:** Run test → PASS; `cargo clippy -p rupu-cp`; commit `feat(cp): GET /api/runs + /api/runs/{id}`.

---

### Task 3: Read-only list endpoints — agents, workflows, sessions, workers, coverage

**Files:** Create `api/agents.rs`, `api/workflows.rs`, `api/sessions.rs`, `api/workers.rs`, `api/coverage.rs`; mount in `server.rs`.

For each, reuse the existing listing function rather than re-scanning:
- **agents:** `rupu_agent::loader::load_agents(global_dir, project_dir)` → name/description/provider/model/effort/path. (`GET /api/agents`, `/api/agents/{name}` returns the parsed `AgentSpec` + raw `.md`.)
- **workflows:** the workflow listing used by `rupu workflow list` (global + project YAML names + scope); `/api/workflows/{name}` returns the parsed `Workflow` + raw YAML.
- **sessions:** `SessionStore::list` (TOML) → id/agent/status/turns/updated_at; `/api/sessions/{id}` returns the record (NOT the huge `message_history`).
- **workers:** `rupu_workspace::worker_store::WorkerStore::list` → the `WorkerRecord`s.
- **coverage:** scan `<target>/.rupu/coverage/<target_id>/` ledgers (or the CWD's); `/api/coverage` lists targets with summary counts (assessed/total, findings); `/api/coverage/{target}` returns the per-target status (reuse `rupu-coverage`'s status/remaining readers).

- [ ] For EACH endpoint: write a fixture test (seed the relevant store dir, assert the JSON), implement the handler (≤25 lines each, delegating to the existing reader), run, commit. One commit per endpoint: `feat(cp): GET /api/{agents,workflows,sessions,workers,coverage}`.

(Coverage may need a small reader if `rupu-coverage` doesn't expose a "list targets" function — add it to `rupu-coverage` if so, with its own test, rather than re-implementing the ledger parse in the CP.)

---

### Task 4: GET /api/dashboard (aggregate)

**Files:** Create `api/dashboard.rs`; mount.

- [ ] **Step 1: Test** — seed 2 runs (1 running, 1 completed) + 1 worker; assert `GET /api/dashboard` returns `{ runs: { running, completed, failed }, workers, sessions, coverage_targets, recent_runs[] }`.
- [ ] **Step 2: Handler** — aggregate from `RunStore::list` (bucket by status), `WorkerStore::list`, `SessionStore::list`, coverage targets; include the 10 most-recent runs. Pure counting, no new storage.
- [ ] **Step 3-5:** run, clippy, commit `feat(cp): GET /api/dashboard`.

---

### Task 5: SSE — /api/events/stream + /api/runs/{id}/log

**Files:** Create `src/sse.rs`, `api/events.rs`; add `/api/runs/:id/log` to `api/runs.rs`.

The bridge: an axum `Sse<impl Stream<Item = Event>>` fed by tailing `events.jsonl`. For Phase 1 (observe runs the CP didn't start), use the **`FileTailRunSource`** pattern (already in `crates/rupu-orchestrator/src/executor/file_tail.rs`) — it yields `Event`s from a run's `events.jsonl` via notify + poll fallback. The global stream tails all active runs' event files (or a merged source if the executor exposes one).

- [ ] **Step 1: sse.rs helper**

```rust
use axum::response::sse::{Event as SseEvent, Sse};
use futures::Stream;

pub fn run_event_stream(events_path: std::path::PathBuf)
    -> Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
    // Wrap FileTailRunSource(events_path) -> map each rupu Event to
    // SseEvent::default().json_data(&event). Keep-alive every 15s.
    // (Mirror how rupu-app / `rupu watch` consume FileTailRunSource.)
    todo!("implement using FileTailRunSource from rupu-orchestrator::executor")
}
```

Implement it concretely against `FileTailRunSource`'s real API (check `file_tail.rs` for its constructor + the stream it yields). `Sse` keep-alive: `.keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15)))`.

- [ ] **Step 2: Handlers** — `GET /api/runs/:id/log` → `run_event_stream(run_store.events_path(&id))`; `GET /api/events/stream` → a stream merging the newest active runs' event files (Phase 1: tail the most-recent run, or the run passed as `?run=<id>`; a true global multiplex can come with Phase 2's in-process executor).
- [ ] **Step 3: Test** — write a few lines to a temp `events.jsonl`, connect to `/api/runs/{id}/log`, assert the first SSE event decodes to the expected `Event`. (Use `reqwest` streaming + read the first `data:` line; bound with a timeout.)
- [ ] **Step 4-5:** run, clippy, commit `feat(cp): SSE event streams for runs + global`.

---

### Task 6: Embed web/dist + SPA fallback + serve

**Files:** Create `src/embed.rs`, `build.rs`; modify `server.rs`.

(Do this LAST in Part A, after the frontend builds to `web/dist`. Until then, gate it behind a feature or a "dist missing → serve a placeholder" so the backend builds.)

- [ ] **Step 1: embed.rs**

```rust
use rust_embed::RustEmbed;
use axum::{response::{IntoResponse, Response}, http::{header, StatusCode, Uri}};

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        // SPA fallback: serve index.html for unknown non-API routes
        None => match Assets::get("index.html") {
            Some(c) => ([(header::CONTENT_TYPE, "text/html")], c.data).into_response(),
            None => (StatusCode::NOT_FOUND, "build the frontend: web/dist missing").into_response(),
        },
    }
}
```

- [ ] **Step 2:** In `server.rs`, add `.fallback(crate::embed::static_handler)` (API routes match first; everything else → static/SPA).
- [ ] **Step 3: build.rs** — emit a `cargo:warning` if `web/dist/index.html` is missing (don't hard-fail dev backend builds).
- [ ] **Step 4-5:** `rupu cp serve`, curl `/healthz` + `/` ; commit `feat(cp): embed + serve the web UI`.

---

## PART B — Frontend (vendor `Okesu/web`, trim, remap)

> These tasks are **port + adapt**, not TDD. Each references exact Okesu source files. Work under `crates/rupu-cp/web`. After each task: `npm run build` (or `vite build`) succeeds and the page renders against a running `rupu cp serve`.

### Task 7: Scaffold the web app + vendor the design system

- [ ] Copy `Okesu/web/{package.json, vite.config.ts, tailwind.config.ts, postcss.config.js, tsconfig.json, index.html}` into `crates/rupu-cp/web/`. In `vite.config.ts` set `build.outDir = 'dist'` and add a dev proxy: `server.proxy = { '/api': 'http://127.0.0.1:7878' }`.
- [ ] Copy the design system **verbatim**: `Okesu/web/tailwind.config.ts` (the full `theme.extend.colors` palette — `bg/panel/border/ink/brand/sev`), `Okesu/web/src/styles.css` (base layer + the timeline animations), `Okesu/web/src/lib/cn.ts`.
- [ ] Trim `package.json` to the UI-relevant deps (React 18.3, react-dom, react-router-dom 6, tailwindcss 3.4 + postcss + autoprefixer, recharts 3, @xyflow/react 12, lucide-react, clsx, tailwind-merge, js-yaml, @codemirror/*; dev: vite 5, @vitejs/plugin-react, typescript, vitest + testing-library). Drop yjs/y-protocols and any Okesu-only deps.
- [ ] `npm install && npm run build` → succeeds (empty app). Commit `feat(cp/web): scaffold + vendored Okesu design system`.

### Task 8: Layout shell + nav + router

- [ ] Port `Okesu/web/src/components/{Layout.tsx, SidebarGroup.tsx, CommandPalette.tsx, Tooltip.tsx, ErrorBoundary.tsx}` and `src/lib/sidebarNav.ts`. Rebrand the sidebar header to "rupu Control Plane".
- [ ] Remap `sidebarNav.ts` to the rupu nav (per the spec): Dashboard; Observe → Runs / Live Events / Coverage; Build → Workflows / Agents; Run → Sessions / Workers; Settings. Use lucide icons (`Activity, Radio, ShieldCheck, Workflow, Sparkles, MessageSquare, Server, Settings`).
- [ ] Port `App.tsx` router; define routes for the pages built below. Strip Okesu-only routes (findings/investigations/catalog/federation/daimons/nodes).
- [ ] Build + render the shell with empty pages. Commit `feat(cp/web): layout shell + rupu nav`.

### Task 9: API client + SSE helpers (typed for rupu)

- [ ] Port the `Okesu/web/src/api.ts` **pattern** (the `request<T>` typed-fetch wrapper, `ApiError`, the `subscribeEvents`/`subscribeRunLog` `EventSource` helpers) into `crates/rupu-cp/web/src/lib/api.ts`. Replace Okesu's resource methods + types with rupu's: `runs()`, `run(id)`, `subscribeRunLog(id)`, `events` SSE, `agents()`, `workflows()`, `sessions()`, `workers()`, `coverage()`, `dashboard()`. Type the responses to match the backend JSON (RunRecord, Event, WorkerRecord, etc.).
- [ ] Commit `feat(cp/web): rupu api client + SSE helpers`.

### Task 10: Runs (list + live run view) — the core observe page

- [ ] Port `Okesu/web/src/components/{StatusPill.tsx, TabBar.tsx, lists/ListCard.tsx, lists/SectionHeader.tsx, EventTimeline.tsx, OrchestrationCanvas.tsx}` (remap `OrchestrationStepView` → rupu's step/unit shape; remap the status enum to run states `pending/running/completed/failed/awaiting_approval`).
- [ ] `pages/Runs.tsx`: list runs from `api.runs()`, bucketed by status (ListCard + SectionHeader), each row → run id, workflow, status, started, duration, tokens.
- [ ] `pages/RunDetail.tsx`: the live run view — `OrchestrationCanvas` painting the run's steps from `api.run(id)`, plus the **live log** via `subscribeRunLog(id)` SSE feeding the `EventTimeline`. This is the browser twin of the live TUI view.
- [ ] Build + verify against a real run (`rupu workflow run …` then open the CP). Commit `feat(cp/web): Runs list + live run view`.

### Task 11: Live Events page

- [ ] `pages/Events.tsx`: reuse `EventTimeline` driven by `api.subscribeEvents()` (global SSE). Connection status indicator (connecting/live/reconnecting). Commit `feat(cp/web): Live Events page`.

### Task 12: Coverage page (first-class — progress signal)

- [ ] `pages/Coverage.tsx`: list coverage targets from `api.coverage()` with per-target **progress** (assessed/total cells, % complete, finding count) using `SectionHeader`/`ListCard` + a progress bar (reuse the dashboard chart palette). `pages/CoverageDetail.tsx` (or a drawer): per-target status grid + findings list (StatusPill-style states). This is the "how far is each project getting" view. Commit `feat(cp/web): Coverage page`.

### Task 13: Dashboard

- [ ] Port `Okesu/web/src/components/dashboard/DashboardCharts.tsx` (recharts) — keep the chart components, swap data. `pages/Dashboard.tsx`: stat tiles (running runs, sessions, workers, coverage %), a runs-over-time chart + a tokens/cost chart from `api.dashboard()`. 15s poll (Okesu's pattern). Commit `feat(cp/web): Dashboard`.

### Task 14: Agents / Workflows / Sessions / Workers list views

- [ ] Four list pages using `ListCard`/`SectionHeader`/`StatusPill`: `Agents.tsx` (name/provider/model/effort + a read-only `.md` view via the existing `MarkdownEditor` in read mode), `Workflows.tsx` (name/scope/steps + the static graph from `OrchestrationCanvas` read-only), `Sessions.tsx` (id/agent/status/turns + a transcript stream link), `Workers.tsx` (the running instances + last_seen). Read-only in Phase 1. Commit `feat(cp/web): agents/workflows/sessions/workers list views`.

### Task 15: Build pipeline + embed wiring + smoke test

- [ ] Add an npm `build` script producing `crates/rupu-cp/web/dist`. Document `cd crates/rupu-cp/web && npm ci && npm run build` as a prereq before `cargo build -p rupu-cp` (the `rust-embed` folder). Optionally a `make cp` target that builds the web then the binary.
- [ ] Complete Task 6 (embed) now that `dist` exists; `cargo build -p rupu-cli`.
- [ ] **End-to-end smoke:** start a real workflow (`rupu workflow run oracle-assessor-a`), run `rupu cp serve`, open `http://127.0.0.1:7878` — confirm: the run appears under Runs, the live run view streams steps/events, Live Events ticks, Coverage shows the target's progress, and the lists populate. Commit `feat(cp): build pipeline + embedded UI end-to-end`.

---

## Self-review

**Spec coverage:** dashboard ✓ (T4/T13), runs+live ✓ (T2/T5/T10), events SSE ✓ (T5/T11), agents/workflows/sessions/workers ✓ (T3/T14), coverage first-class ✓ (T3/T12), design-system+shell port ✓ (T7/T8), api+SSE client ✓ (T9), embed+serve ✓ (T6/T15), `cp serve` + localhost/token ✓ (T1). Auth token enforcement is minimal in P1 (bind localhost) — add a tiny middleware check of `--token` as a Bearer in T1 Step 5 if a token is set. Control/author/remote are explicitly P2–P4 (out of this plan).

**Placeholder scan:** the two `todo!()`s (SSE helper, run-store seeding in tests) are flagged with the exact existing APIs to implement against (`FileTailRunSource`, the orchestrator test harness) — resolve them against the real signatures during the task, not as shipped code. The frontend port tasks cite exact Okesu source files rather than re-deriving code.

**Type consistency:** `AppState` (global_dir, run_store, pricing) is used consistently; `ApiError`/`ApiResult` shared; the frontend `api.ts` resource methods match the backend route names 1:1.

---

## Notes for the executor
- rupu rules: workspace deps pinned in root `Cargo.toml`; `rupu-cli` stays a thin dispatcher; `#![deny(clippy::all)]`; never run package-wide `cargo fmt` (main is fmt-dirty) — only `rustfmt --edition 2021` per file.
- The web UI can't be CI-asserted (same rule as the TUI) — matt runs `rupu cp serve` to validate rendering before merge.
- Keep PRs per-task or per-part; each task ends green + committed.
