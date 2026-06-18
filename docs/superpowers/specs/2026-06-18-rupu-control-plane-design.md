# rupu Control Plane (single-host) — Design

**Date:** 2026-06-18
**Status:** Draft for review
**Author:** matt + Claude

## Goal

A local, web-based **Control Plane** for rupu — `rupu cp serve` opens a browser
UI to observe and (later) drive everything rupu is doing on this host: agents,
workflows, sessions, runs, workers, coverage, and a live event stream. The UI is
a faithful port of **Okesu's** control-plane front end (same Tailwind design
system, colors, layout shell, component library, and interaction patterns —
matt likes it and there's no reason to reinvent it), with Okesu's
security-domain resources remapped to rupu's.

**This spec covers single-host, observability-first (Phase 1).** Control,
authoring, and multi-host are staged (Phases 2–4) but the API/data shapes are
chosen so they extend without rework.

## Why this is mostly assembly, not invention

rupu already ships the expensive backend pieces, explicitly designed for a
future control plane (CLAUDE.md: the portable records exist so "future
rupu.cloud services consume these shapes rather than inventing a second
protocol"):

- **Data model:** `RunEnvelope`, `WorkerRecord`, `ArtifactManifest`,
  `WakeRecord` (`crates/rupu-runtime`) + `RunRecord`/`StepResultRecord`
  (`crates/rupu-orchestrator/src/runs.rs`) — version-stable, serde-complete.
- **Run store:** `RunStore::{list, load, read_step_results, read_run_envelope,
  read_artifact_manifest, events_path}` over `~/.rupu/runs/<id>/`.
- **Live events:** the executor `EventSink` — `InMemorySink` (tokio broadcast),
  `JsonlSink` (durable `events.jsonl`), `FileTailRunSource` (tail any run).
- **HTTP substrate:** `crates/rupu-webhook` is a working axum server (routing,
  HMAC, a `WorkflowDispatcher`) to model the CP backend on.
- **Listing/dispatch:** `load_agents`, workflow/session/worker listing,
  `run_workflow(OrchestratorRunOpts)`, the session `_worker` daemon.

The net-new work is an **HTTP API layer + a web frontend**, not a new runtime.

## Architecture

```
crates/rupu-cp/                       new crate
  src/
    main.rs / lib.rs   → `rupu cp serve [--bind 127.0.0.1:7878] [--token …]`
    server.rs          → axum Router (mirrors rupu-webhook's shape)
    api/
      runs.rs          → list/get runs, SSE run-event stream, (P2) approve/reject/cancel
      events.rs        → SSE global event stream
      agents.rs        → list/get agent specs (P3: write)
      workflows.rs     → list/get workflows (P3: write + validate)
      sessions.rs      → list/get sessions + transcript stream (P2: send)
      workers.rs       → list workers/instances (WorkerStore)
      coverage.rs      → list coverage targets + findings (from the ledger)
      dashboard.rs     → aggregated counts for the landing page
    sse.rs             → axum SSE helper over InMemorySink / FileTailRunSource
    embed.rs           → rust-embed of web/dist (compiled-in), SPA fallback
  web/                 → Vite + React app (ported from Okesu/web)
```

- **Backend:** a new `rupu-cp` crate. axum + tokio. Reuses the orchestrator
  crates directly (in-process) — it reads `RunStore`/`WorkerStore`, taps the
  `EventSink`/`FileTailRunSource` for live events, and (Phase 2) calls
  `run_workflow` / the existing approve/reject/cancel paths. **No business logic
  is reimplemented** — the CP is a thin web adapter over existing functions
  (consistent with rupu's "thin CLI" rule).
- **Source of truth:** the existing **file-based stores** (`~/.rupu/runs`,
  `~/.rupu/agents`, `~/.rupu/workflows`, `~/.rupu/sessions`,
  `~/.rupu/autoflows/workers`, the coverage ledger). **No database in Phase 1.**
  A SQLite read-index is a later option only if list performance demands it;
  the files stay authoritative (matches rupu's file-first design and avoids a
  second source of truth).
- **Frontend:** a Vite/React app under `crates/rupu-cp/web`, **built and
  embedded into the binary** (`rust-embed`), served by axum on one port — same
  single-binary model as Okesu (`embed.FS`). `vite dev` proxies `/api/*` to the
  axum server during development.
- **Live updates:** **SSE** (`text/event-stream`), exactly as Okesu does
  (`EventSource` on the client). Global stream (`GET /api/events/stream`) +
  per-run stream (`GET /api/runs/{id}/log`). Backend bridges the `EventSink`
  broadcast (for runs the CP started) and `FileTailRunSource` (for runs started
  by the CLI/cron/autoflow) into the SSE response.
- **Auth (Phase 1):** binds to `127.0.0.1` by default; an optional shared
  `--token` (Bearer) guards the API. Full cookie-session + RBAC (Okesu's model)
  is Phase 4 (needed when the CP is exposed beyond localhost / multi-user).

## Frontend — porting Okesu's UI

The whole front end is copied from `Okesu/web` and rebranded/remapped. **Stack
(identical):** React 18.3, Vite 5, react-router 6, **Tailwind 3.4**,
@xyflow/react 12 (graph editor), recharts 3 (charts), lucide-react (icons),
js-yaml + CodeMirror 6 (spec editing), clsx + tailwind-merge.

### Design system — copy verbatim (this is the "down to the colors" ask)
Port `Okesu/web/tailwind.config.ts` + `src/styles.css` as-is:
- **Colors:** `bg #fafafa`, `panel #ffffff`, `border #e5e7eb`; ink
  `#0f172a / #64748b / #94a3b8`; **brand purple** `50 #f5f3ff · 100 #ede9fe ·
  500 #7c3aed · 600 #6d28d9 · 700 #5b21b6`; status palette
  `critical #9333ea · high #dc2626 · medium #ea580c · low #ca8a04 · info
  #64748b`.
- **Type:** `-apple-system, Inter, system-ui` sans; `ui-monospace, SFMono` mono.
- Card shadow, `rounded-md/xl/full`, light-only, and the **live-event
  animations** (`timeline-enter`, `timeline-row-glow`, `dot-pulse`,
  `ring-expand`, `fresh-stripe`).
- Severity tokens are reused as a generic **status palette** (run states /
  coverage states) — same hexes, relabeled.

### Layout shell — copy, remap nav
Port `Layout.tsx` + `sidebarNav.ts` + `SidebarGroup.tsx` + `CommandPalette.tsx`
(Cmd-K) + `EntityDrawerHost`. The 240px sidebar; remap nav to rupu:

```
Dashboard
── Observe ──
  Runs            (Activity)      ← Okesu Orchestration-runs / Runs
  Live Events     (Radio)         ← Okesu Live Events
  Coverage        (ShieldCheck)   ← Okesu Findings
── Build ──
  Workflows       (Workflow)      ← Okesu Orchestrations (DAG editor)
  Agents          (Sparkles)      ← Okesu Agents/Daimons
── Run ──
  Sessions        (MessageSquare) ← rupu-specific (loosely Daimons)
  Workers         (Server)        ← Okesu Nodes (running instances)
── ──
  Settings        (Settings)
```

### Portable components — copy as-is (domain-agnostic)
`Tooltip`, `TabBar/TabButton`, `lists/ListCard`, `lists/SectionHeader`,
`StatusPill/StatusDot` (remap status enum), `BulkActionBar`,
`dashboard/DashboardCharts` (recharts), `CommandPalette`, `EventTimeline` (the
SSE timeline — reuse directly for the Live Events + per-run stream),
`OrchestrationCanvas` (SVG step DAG — reuse for the run view; aligns with the
live TUI git-graph we already ship), `OrchestrationEditorCanvas`
(@xyflow/react — reuse for the workflow editor, Phase 3), `MarkdownEditor`
(CodeMirror — reuse for agent `.md` / workflow `.yaml` editing, Phase 3),
`ErrorBoundary`, the hooks (`useInfiniteScroll`, `useSelection`, `useHotkey`,
`preferences`), the `api.ts` typed-fetch + `subscribeEvents`/`subscribeRunLog`
SSE pattern, `cn()`.

### Domain remap: Okesu resource → rupu resource
| Okesu | rupu | CP view |
|---|---|---|
| Orchestration | **Workflow** (`.yaml`) | Workflows list + DAG editor (P3); reuse OrchestrationEditorCanvas → rupu YAML |
| Orchestration-run | **Run** (RunRecord + events) | Runs list + live run view (DAG + step log) — the core observe page |
| Agent / Daimon | **Agent** (`.md`) | Agents list + CodeMirror editor (P3) |
| Node | **Worker / instance** (WorkerRecord) | Workers list (running instances, last_seen) |
| Finding | **Coverage** cell/finding (ledger) | Coverage page (status grid + findings); reuse StatusPill/Kanban look |
| Event | **Event** (executor `Event`) | Live Events (SSE) — reuse EventTimeline as-is |
| (—) | **Session** (`_worker` daemon) | Sessions list + transcript stream |
| Investigation / Catalog / Federation | — | not in scope (no rupu analog yet) |

The `@xyflow/react` workflow editor serializes to **the same workflow YAML the
orchestrator already parses** (and the same node/status vocabulary the live TUI
view renders) — so authoring round-trips through the existing format.

## Backend API (Phase 1 = read + SSE; later phases noted)

All JSON, `/api/...`, mirroring Okesu's `api.ts` shapes where sensible.

| Method | Path | Phase | Backed by |
|---|---|---|---|
| GET | `/api/dashboard` | 1 | aggregate RunStore + WorkerStore + coverage counts |
| GET | `/api/runs` | 1 | `RunStore::list` |
| GET | `/api/runs/{id}` | 1 | `RunStore::load` + `read_step_results` + envelope/manifest |
| GET | `/api/runs/{id}/log` (SSE) | 1 | `events.jsonl` tail (`FileTailRunSource`) + broadcast |
| GET | `/api/events/stream` (SSE) | 1 | merged executor event broadcast / tail |
| GET | `/api/agents`, `/api/agents/{name}` | 1 | `load_agents` |
| GET | `/api/workflows`, `/api/workflows/{name}` | 1 | workflow listing + YAML |
| GET | `/api/sessions`, `/api/sessions/{id}` | 1 | SessionStore |
| GET | `/api/sessions/{id}/transcript` (SSE) | 1 | transcript tail |
| GET | `/api/workers` | 1 | `WorkerStore::list` |
| GET | `/api/coverage`, `/api/coverage/{target}` | 1 | coverage ledger |
| POST | `/api/workflows/{name}/run` | 2 | `run_workflow` |
| POST | `/api/runs/{id}/{approve,reject,cancel}` | 2 | existing approve/reject/cancel |
| POST | `/api/sessions/{id}/send` | 2 | session enqueue |
| PUT | `/api/workflows/{name}`, `/api/agents/{name}` | 3 | validate + write YAML/`.md` |

DTOs reuse the existing serde types (`RunRecord`, `Event`, `WorkerRecord`,
`RunEnvelope`) directly where possible.

## Phasing

- **Phase 1 — Observe (this spec's focus).** `rupu-cp` crate + embedded React;
  the design-system + shell port; Dashboard, Runs (list + live run view + SSE
  log), Live Events, and read-only list views for Agents / Workflows / Sessions
  / Workers / Coverage. Outcome: "watch everything rupu is doing on this host,
  live, in a browser that looks like Okesu."
- **Phase 2 — Control.** Dispatch a workflow, approve/reject/cancel a run, send
  a session prompt. (Engine already exists; this is buttons + POST handlers.)
- **Phase 3 — Author.** The @xyflow workflow graph editor + the CodeMirror agent
  editor, round-tripping to rupu YAML/`.md`.
- **Phase 4 — Remote / multi-instance + RBAC.** Drive rupu instances on other
  hosts (the `WorkerRecord` registry + an Okesu-style tunnel/relay), cookie
  sessions + roles. The single-host data shapes were chosen to extend here.

## Non-goals / open decisions
- **DB:** none in Phase 1 (file stores are authoritative). Revisit only for
  list-scale.
- **Auth:** localhost + optional token in Phase 1; full RBAC in Phase 4.
- **Coverage UI depth:** Phase 1 lists targets + findings; the richer
  grid/Kanban (Okesu Findings parity) can deepen later.
- **Okesu code reuse mechanics:** copy `Okesu/web` into `crates/rupu-cp/web`,
  strip the domain-specific pages/components (Investigations, Catalog,
  Federation, Daimon/Finding internals), keep the design system + shell +
  generic components, and remap the rest. Decision to confirm: vendor a trimmed
  copy (clean, diverges over time) vs. a shared design package (more coupling).
  Recommendation: **vendor a trimmed copy** — the two products will diverge.

## Testing
- Backend: handler unit tests over a temp `RunStore`/`WorkerStore` (mirror the
  orchestrator test harness); an SSE smoke test (events.jsonl → stream).
- Frontend: port Okesu's vitest setup; component tests for the remapped
  list/run views; the live rendering itself validated by running it.
- No CI assertion of the browser UI (same rule as the TUI) — matt runs it.
