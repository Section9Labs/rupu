# rupu — Autoflow runs as first-class CP runs

Status: approved (design), pending implementation plan
Date: 2026-07-07

## Context

Autoflow runs execute in per-repo/per-issue worktrees under
`~/.rupu/autoflows/worktrees/<repo>/<issue>/…`, but their `run.json` +
transcript currently land in the **global** store (`~/.rupu/runs/`,
`~/.rupu/transcripts/`), which the CP reads — so *local* autoflow runs already
resolve. Three gaps surfaced (verified on the live `~/.rupu`):

1. **Failed/unpersisted linked runs 404.** A run that fails at dispatch (e.g. a
   provider `401 invalid x-api-key`) never writes a `run.json`, but the autoflow
   history logs a `run_launched` event (status `blocked`) referencing the
   run_id. The CP links to that run_id → `GET /api/runs/:id` → `{"error":"run
   … not found"}`. (Confirmed: `run_01KWYZ2QY4…` — no record anywhere except
   the autoflow history/claim; the underlying failure is the invalid Anthropic
   key.)
2. **No view parity.** The CP has a separate `AutoflowRuns.tsx` (list + claims)
   distinct from the rich workflow `RunDetail.tsx` (graph, transcript
   drill-down, usage, approve/reject/pause). matt wants autoflow runs to use the
   **same** RunDetail view + autoflow-specific additions.
3. **No cross-store/host run resolution.** `AppState.run_store` reads only
   `global_dir/runs` (state.rs:65). A run whose artifacts live on a **remote
   host** or a **project-local store** can't be resolved. PR #463 just made the
   Build *definition* lists project-aware (`api/repo_scope.rs::
   distinct_repo_workspaces`); the *runs* side needs the analogous awareness.

## Spine decisions (approved)

1. **On-demand resolve + proxy/read** (not mirror): keep artifacts where they
   are; the CP locates and reads/proxies them at view time. Extends the existing
   `?host=` proxy (`proxy_get_json`).
2. **Autoflow history is the location + status resolver** (+ host-probe
   fallback for non-autoflow remote runs). The history (run_id →
   repo/workspace/host/status) both locates a run's artifacts and, for a
   failed/unpersisted run, supplies the failure to show instead of a 404.
3. **View parity:** autoflow runs open in the **same `RunDetail`** as workflow
   runs, plus an **Autoflow panel**. `AutoflowRuns` stays as the list; its
   divergent detail rendering is retired.

## Goals

- `GET /api/runs/:id` (+ `/graph`, transcript, log, usage) resolves a run's
  artifacts across **global → project-local → remote-host**, on demand.
- A failed/unpersisted run linked from the autoflow history renders as a
  **failed run with its cause** (never a bare 404).
- Autoflow runs render in the **shared RunDetail** view + an Autoflow panel
  (entity, claim, cycle, prior cycles, project/host).
- No artifact duplication; resolver is fail-closed; hexagonal boundaries kept.

## Non-goals

- Mirroring/copying remote or project-local artifacts into the global store
  (on-demand only).
- Fixing the invalid Anthropic key (operator action: `rupu auth login
  --provider anthropic`) — this spec makes the *failure visible*, not the run
  succeed.
- Changing where autoflow runs persist (they keep writing to the global store
  for local runs).

## Architecture

### 1. `resolve_run_location` (rupu-cp)

New resolver (e.g. `crates/rupu-cp/src/api/run_resolve.rs`):

```
enum RunLocation {
    Global,                         // in AppState.run_store
    ProjectLocal { path: PathBuf }, // a registered project's run store
    Host { host_id: String },       // proxy to that host's CP
    Unpersisted { cycle: AutoflowCycleRef, failure: String, status: RunStatus },
    NotFound,
}
fn resolve_run_location(s: &AppState, run_id: &str) -> RunLocation
```

Order: (a) global store hit → `Global`; (b) **autoflow-history** lookup
(run_id → repo/workspace_path/host_id/status via an `AutoflowHistoryStore`
reader) → `Host` if placed on a remote host, `ProjectLocal` if a project-local
store, or `Unpersisted{failure}` if the history has the run_id but no artifacts
exist; (c) **host-probe fallback** for non-autoflow runs (bounded iteration over
registered hosts' `/api/runs/:id`, cached per request); (d) else `NotFound`.
Reuse `distinct_repo_workspaces` (PR #463) to enumerate project stores.

### 2. Location-aware run endpoints (rupu-cp)

`get_run`, `run_graph`, `get_run_log`, `get_run_usage_timeline`, and the
transcript stream consult the resolver and dispatch:
- **Global** → unchanged.
- **ProjectLocal{path}** → load from a `RunStore` rooted at that project's
  `.rupu/runs` (or the resolved run dir); same DTOs.
- **Host{host_id}** → `proxy_get_json` to that host's CP for the same route
  (already done for `?host=`; generalize so it triggers on resolution, not only
  an explicit `?host=` param).
- **Unpersisted** → synthesize the run DTO from the autoflow cycle/event: a
  `RunRecord`-shaped response with status (failed/blocked), `workflow_name`,
  entity, `error_message` = the cycle failure (e.g. the 401), timestamps, and
  the cycle/event ids. `run_graph` returns a minimal single-node/paused-state
  graph so RunDetail renders (mirrors PR #460's empty-snapshot handling).
- **NotFound** → 404 (only when global + history + hosts all miss).

Fail-closed: an unreachable resolved host returns a clear `host_unreachable`
error state, not a 500/panic.

### 3. Autoflow-history reader (rupu-cp)

A thin reader over `~/.rupu/autoflows/history/{cycles,events}` +
`~/.rupu/autoflows/claims/<repo--issue>/claim.toml` exposing: run_id →
`{ repo_ref, workspace_path, host_id?, status, cycle_id, latest_event, failure? }`
and, for a given entity, the list of cycles. Reuses the `AutoflowHistoryStore`
/ `rupu_runtime::autoflow_history` types where possible (read-only in the CP).
New endpoint `GET /api/runs/:id/autoflow` → the autoflow context for a run
(entity, claim, cycle, prior cycles, project/host), consumed by the panel.

### 4. Web — view parity (crates/rupu-cp/web)

- Autoflow run rows (in `AutoflowRuns.tsx` and anywhere autoflow runs link)
  point to the shared run route `/runs/:id` → `RunDetail.tsx`.
- `RunDetail` gains an **Autoflow panel** (rendered when `GET /api/runs/:id/
  autoflow` returns data / the run is autoflow-sourced): entity (issue/PR +
  link), claim (status/lease), cycle id + link, prior cycles for the entity,
  and the resolved **project/host** the run ran on.
- A failed/unpersisted run renders with its status + `error_message` (the
  synthesized detail) — the graph shows the failed node; the panel shows the
  cycle failure.
- `AutoflowRuns.tsx` keeps the list + claims tabs; its separate per-run detail
  rendering is removed in favor of RunDetail.

## Errors & security

- Resolver never panics; unreachable host / unreadable project dir → a clear
  typed error surfaced in the detail, not a crash.
- 404 only when truly unresolvable (global + history + all hosts miss).
- Host proxy respects the existing auth/gating; project-local reads are confined
  to registered project paths (reuse the fs-safety confinement).
- Hexagonal: the resolver + history reader live in `rupu-cp`, composing the
  `RunStore` port, `HostConnector`, and the read-only history types; no
  `rupu-orchestrator`/`rupu-runtime` → `rupu-cp` dependency added.
- No new secrets; no artifact duplication.
- `#![deny(clippy::all)]`; no `unsafe`; `thiserror`/`ApiError`; workspace deps
  only.

## Testing

- **resolve_run_location:** global hit; project-local hit; host hit (fake
  connector); unpersisted (history has run_id, no artifacts) → cycle-failure;
  truly-missing → NotFound.
- **Endpoints:** `get_run`/`run_graph`/log/usage through each location; the
  unpersisted synthesis (the `run_01KWYZ2QY4…` shape) returns a failed run with
  the 401 message, not a 404; project-local + host-proxy round-trips.
- **Autoflow-history reader:** run_id → context; prior-cycles list; missing
  history → empty/None.
- **web:** an autoflow run opens RunDetail + the Autoflow panel; a
  failed/unpersisted run shows status + reason; the list rows link to
  `/runs/:id`; a workflow (non-autoflow) run shows no autoflow panel.

## Decomposition (plan)

- **Slice 1 — backend:** autoflow-history reader + `resolve_run_location` +
  location-aware `get_run`/`graph`/log/usage/transcript + unpersisted synthesis
  + `GET /api/runs/:id/autoflow`.
- **Slice 2 — web:** route autoflow runs to `RunDetail`; add the Autoflow
  panel + failed-run rendering; retire `AutoflowRuns`'s separate detail.
Slice 2 depends on Slice 1's DTOs/endpoints.

## Open questions (resolve in the plan)

- **ProjectLocal reality:** confirm whether any autoflow/workflow run actually
  writes to a project-local `.rupu/runs` today (vs always global) — if none do,
  the ProjectLocal arm is future-proofing; keep it but note it's exercised only
  by tests until a project-local store exists.
- **Host-probe bound/cache:** cap the fallback probe (e.g. only hosts, short
  timeout, cache the run→host mapping for the session) to avoid a slow miss.
- **Autoflow-detection signal on a run:** `workflow_name` matching an
  autoflow-enabled workflow, a `source_wake_id`/backend marker, or the history
  lookup succeeding — pick the cheapest reliable signal for "show the panel".
