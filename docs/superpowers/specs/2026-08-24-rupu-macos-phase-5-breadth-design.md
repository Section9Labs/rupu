# rupu.app macOS — Phase 5: Breadth (Projects · Library · Fleet · Security · Usage)

**Date:** 2026-08-24
**Status:** Executed under matt's standing "continue with all of the phases" authorization (2026-08-24); decisions recorded for the phase checkpoint
**Parent:** `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md` (umbrella §8, Phase 5)
**Visual contract:** `docs/macOS_design/V2-CONTRACT.md`; all five screens are born in the v2 language (rail already routes to them)

Replaces the five remaining placeholders. Split into two plans by net-new-API weight
(survey evidence: Library/Projects/Fleet ride mostly-existing client surface; Security/
Usage are ~100% new Swift API modules):

- **Plan A — wiring breadth:** Projects, Fleet, Library + the scope-aware launch fix.
  `docs/superpowers/plans/2026-08-24-rupu-macos-phase-5a-projects-fleet-library.md`
- **Plan B — data breadth:** Security (findings/coverage), Usage.
  `docs/superpowers/plans/2026-08-24-rupu-macos-phase-5b-security-usage.md`

## 1. Shared groundwork (Plan A, first)

- **Generic sortable list**: extract the SortableTable contract already proven on
  Activity into a reusable `RupuDesign`/`RupuStore` pair — a pure, tested
  column-sort helper (key + direction + nulls-last + stable, the `ActivitySort`
  semantics generalized) and a plain list-table view idiom the five screens share
  (header sort buttons with chevrons, ONE truncating subject column, fit-width
  metadata, right-aligned tabular numerals, 8pt rows). Server lists here are
  bare unpaginated arrays (web windows client-side) — no `PagedSnapshot`; a
  simple client-side windowing cap (render first N + "show all") only where a
  list can realistically grow large (runs lists), plain full render elsewhere.
- **Fixtures**: every new endpoint consumed gets a golden fixture emitted from
  the real serde type (`macos_fixtures.rs` pattern), drift-gated.

## 2. Plan A screens

**Projects** — list (`GET /api/projects`, local-only): sortable columns name /
runs / last-run / spend (usage summary), row → detail. Detail
(`GET /api/projects/:ws_id` + lazy per-tab): header (name, path, repo link,
created/last-run) + tabs **Overview** (detail counts + recent runs), **Runs**
(`/runs`, offset/limit — render windowed), **Sessions** (`/sessions`),
**Findings** (`GET /api/findings?ws_id=` — reuses the existing `APIFindings`
model), **Coverage** (`/coverage` summary rows — this one tab ships with Plan B, which owns the coverage models; the tab bar carries it from day one with an honest "arrives with Security" placeholder), **Definitions**
(`/agents|/workflows|/autoflows` scoped lists). Tab rows navigate to the
existing run/session detail routes. **Deferred (tracked): Code tab**
(tree/source viewers — umbrella Phase 6 `source`/`code` modules) and **Config
tab** (config write — Phase 6).

**Fleet** — host cards (auto-fill grid, per umbrella §HANDOFF item 6: faulted
card gets fail border + cause panel) from the existing `hosts()` (probe-backed);
workers table (`GET /api/workers` — new model+method+fixture); remove-host
action (`DELETE /api/hosts/:id`, confirm-first, pending-state). **Deferred
(tracked): add-host/enroll forms** (node/ssh/bucket — CLI-covered `rupu` flows;
Settings-adjacent, revisit Phase 6) and host detail page (web's is a thin
runs-by-host list — Activity already answers it).

**Library** — three definition tabs (agents / workflows / autoflows), never run
lists. Agents: existing `agentDefinitions()`; columns name/scope/model/runs/
last-run + permission-tone badges from the DTO (`tools` presence + mode per the
umbrella tone rule: read-only=done, ask=await, bypass=fail loud). Workflows:
existing `workflowDefinitions()` (+`autoflow_enabled` chip). Autoflows: new
`autoflowDefinitions()` (`GET /api/autoflows` — model+method+fixture) with
enabled chip + enable/disable toggle (`?scope_kind=&scope_id=` — the scoped
mutation endpoints exist). Detail views (read-only this phase): agent detail =
header meta chips + description + the raw `.md` (mono block); workflow detail =
existing `workflowDetail(name:)` (inputs + YAML, mono block) + autoflow toggle.
**Per-row and detail-page Launch** opens the existing LauncherSheet prefilled
with the definition — carrying the row's scope (see §3). **Deferred (tracked):
structured editors** (AgentBuilder/WorkflowEditor — authoring surface, post-
parity backlog), **"Used by" links** (net-new even on web: no server endpoint
computes reverse references; needs new API — post-parity backlog, recorded so
the umbrella's Library line has an explicit disposition).

## 3. Scope-aware launch (the umbrella's tracked Phase 3 gap — fixed here)

Server: the three launch bodies (`POST /api/agents/:name/run`, `/session`,
`/api/workflows/:name/run`) gain optional `scope_kind`/`scope_id` fields; when
present the handler resolves the definition through the same scoped resolution
the GET/PUT/DELETE handlers already use (`resolve_*_scoped`) instead of bare
path-walking, 404/409-ing honestly on a miss. Absent fields = today's behavior
(back-compatible; older clients unaffected). Request-body golden fixtures updated
(bidirectional rig). macOS: `DefinitionPicker` rows already know their
`scope`/`scopeKind`/`scopeID` — thread them through `LauncherStore.launch()`
into the bodies, so the picked definition is the launched definition. **Web
threading is a tracked follow-up** (same bug exists there; the server fix
enables it) — recorded, not silently skipped.

## 4. Plan B screens

**Security** — segmented tabs **Findings** / **Coverage**. Findings: global
`GET /api/findings` (new no-filter client method over the existing model) —
summary strip (severity figures, existing severity tokens) + findings table
(2px severity left edge per row, umbrella §4). Coverage: `GET /api/coverage`
(new module) — grouped-by-project summary rows (target, assertion lines,
has-catalog, findings) → coverage detail (`GET /api/coverage/:target?ws_id=`)
with **Overview** (assertions list + findings) and **Catalog** tabs.
**Deferred (tracked): audit / gap / runs / diff tabs** (web's heaviest coverage
machinery) and **coverage templates** — post-parity backlog with the umbrella's
existing template disposition.

**Usage** — range from the global toolbar (7d/30d/all; no custom drag-select
this phase — deferred, tracked); pivot picker (model/provider/agent/workflow/
host/project). Data: `GET /api/usage` (summary + breakdown + unpriced + host
freshness — fans out server-side; fetch `?host=local`-first per the Phase-2
lesson is NOT available here since the endpoint fans out internally — fetch it
once, accept the latency, render freshness strip honestly), `GET /api/usage/runs`
(flat rows, local-only) with a ported, pure, tested `buildTimeline`-equivalent
aggregation feeding the spend-over-time chart (web parity: the dormant
`/api/usage/timeline` endpoint stays unused, matching web), and
`GET /api/usage/outliers` (outlier panel: run, cost vs baseline, ratio).
Blocks: unpriced banner (named models), freshness strip (reuse Phase 4's),
spend chart, pivot breakdown table (aggregated from the same flat rows, web
parity), outlier panel. Null discipline throughout (`—`, unpriced flagged).

## 5. Verification & sequencing

- Per-task gates as established (Swift + build + cargo where Rust is touched;
  zero warnings; exit greps; @MainActor rule; condition-polling; stub isolation
  tokens for any new URLProtocol suites).
- Plan A executes first (groundwork + the launch fix unblock nothing in B; B is
  independent data work). Checkpoint screenshots per plan; matt's standing
  merge directive applies (merge on CI green, checkpoint packages delivered).
- Parity dispositions this phase closes or re-records: scope-aware launch
  (fixed, server+macOS), `usage-timeline` (consumed? no — dormant, matching
  web; recorded), `transcripts` plural + `run_resolve` (still no consumer —
  re-deferred, tracked), repos (`GET /api/repos` — no macOS consumer this
  phase; Fleet cards don't need it; deferred, tracked), `repo_scope` (internal
  helper, no HTTP surface — n/a, recorded).

## 6. Out of scope

Situation Room / menu bar / notifications / Settings depth (Phase 6); source/
code viewers + config write (Phase 6); structured definition editors,
"Used by", coverage audit/gap/diff, templates, custom usage windows, add-host
forms (post-parity backlog, all tracked above); web-side launcher threading
(tracked follow-up).
