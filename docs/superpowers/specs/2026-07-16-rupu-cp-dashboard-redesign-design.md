# rupu-cp Dashboard Redesign — Ops-First, Multi-Host, Real-Time

**Date:** 2026-07-16
**Status:** Design
**Scope:** `crates/rupu-cp` (Rust API + `web/` SPA), `crates/rupu-cp/src/host/` (connector trait + SSH impl), `crates/rupu-cli` (new `run list --format json` — see §4.3)

## 1. Problem

The dashboard at `127.0.0.1:7878/dashboard` (`crates/rupu-cp/web/src/pages/Dashboard.tsx`) is
spend-forward: its largest, most prominent element is cost and tokens. Three concrete failures
follow from that framing plus its age.

**Autoflow floods the run feed.** `DashboardResponse::RecentRun` (`src/api/dashboard.rs:29`) is
hard-capped at 10 rows server-side and omits `trigger`, `workspace_id`, and `host_id` — unlike
`RunListRow`, which carries all three. A chatty autoflow cycle emitting twelve runs consumes the
entire list, and the table cannot distinguish those runs from ones the operator launched. The
discriminator already exists (`trigger_of`, `src/api/runs.rs:345`); the dashboard response simply
never learned about it.

**Run status answers the wrong question.** The status donut renders `runs.by_status`, seeded with
all eight `RunStatus` variants at zero so it never has holes. The result is an eight-slice pie that
is ~95% `completed`. It answers "what fraction of all runs ever succeeded" — a question no operator
asks. The two questions they *do* ask are *is anything stuck right now* (a live count) and *is
failure trending up* (needs a time axis). A ratio can express neither.

**It is not real-time, and it is local-only.** The page polls on a fixed 15s timer
(`POLL_MS`, `Dashboard.tsx:206`) while rupu-cp already serves SSE at `/api/events/stream`. Worse,
`get_dashboard(State(s))` never touches `s.hosts` — it reads `s.run_store.list()` directly. It is
the one list-ish view in CP that never learned about hosts, so every number on it silently means
"local only" while the app has five transports.

## 2. Goals / Non-Goals

**Goals**

- Re-frame the dashboard as an operations surface: what is running, what is stuck, what needs me.
- Group the run feed by autoflow cycle so a cycle reads as one event, not twelve.
- Replace the status donut with a split that separates live state from outcome trend.
- Make it live where liveness is actually available, and *visibly honest* where it is not.
- Span local and remote hosts across all five transports, including SSH.
- Give spend a dedicated page with room to answer attribution and anomaly questions.

**Non-Goals**

- No new frontend dependencies. `recharts`, `@xyflow/react`, and `@dagrejs/dagre` are already present.
- No cross-host event firehose. It does not exist and this design does not invent one (see §5.3).
- No SSH connection pooling / `ControlMaster` work. Called out as a constraint, deferred (§8).
- No changes to `RunStatus` variants or `is_terminal()` semantics.
- rupu-cp stays read-only. Any write path remains in `cp serve`.

## 3. Architecture

Three layers, corresponding to the three implementation plans:

```
Plan 2 (foundation)     HostConnector::dashboard_summary()  ── new structured method
                        SshHostConnector::list_runs()        ── mirror-read → remote CLI
                                    │
                                    ▼
Plan 1 (ops page)       GET /api/dashboard?range=  ── fan_out_via() across hosts
                                    │
                                    ▼
                        Dashboard.tsx  ── swimlane · split status · cycle feed
                                          + SSE invalidation (local) / TTL poll (remote)

Plan 3 (spend page)     GET /api/usage?group_by={model|workflow|host|project}
                                    │
                                    ▼
                        Usage.tsx  ── pivot-over-time · outliers · unpriced gap
```

### 3.1 Why structured methods, not `proxy_get_json`

`proxy_get_json` is a contract to speak HTTP — its doc comment (`host/connector.rs:177`) defines it
as "issue `GET {base_url}{path_and_query}` (bearer token attached)." Only `HttpHostConnector`
implements it. `SshHostConnector` returns an error, and this is **structural, not a TODO**: SSH has
no `base_url` and no HTTP listener on the far side. Its entire vocabulary is `RemoteExec`
(`host/ssh.rs:311-330`): `run(cmd)`, `spawn_lines(cmd)`, `run_bytes(cmd, stdin)` — process exec, not
URLs. There is no general mapping from an arbitrary `/api/...?query` string to a CLI invocation.
Tunnel and Bucket also decline it.

Anything built on `proxy_get_json` therefore silently excludes 3 of 5 transports. The codebase
already knows this; `host_fanout.rs:17` states that fan-out uses structured methods "so SSH hosts —
which can't serve `proxy_get_json` — still contribute rows."

**This design follows the established structured-method pattern.** It is the same shape as
`list_sessions`, `list_agent_runs`, and `list_autoflow_runs`.

## 4. Plan 2 — Data foundation (Rust)

### 4.1 `HostConnector::dashboard_summary()`

New trait method on `host/connector.rs`, with an `Unsupported` default so impls opt in:

```rust
/// Aggregate dashboard state for this host, in ONE round-trip.
///
/// Deliberately coarse: SSH hosts pay a full ssh handshake per call
/// (no ControlMaster multiplexing — see ssh.rs RemoteExec::run), so this
/// must not decompose into per-panel calls.
async fn dashboard_summary(
    &self,
    range: DashboardRange,
) -> Result<DashboardSummary, HostConnectorError> {
    Err(HostConnectorError::Unsupported)
}
```

`DashboardSummary` carries everything one host contributes:

| Field | Type | Notes |
|---|---|---|
| `active` | `ActiveCounts` | `running`, `awaiting_approval`, `paused`, `pending` |
| `terminal_buckets` | `Vec<TerminalBucket>` | `{ ts, completed, failed, rejected, cancelled }` |
| `active_runs` | `Vec<ActiveRunBar>` | swimlane input: `{ run_id, workflow_name, status, started_at, trigger, cycle_id }` |
| `cycles` | `Vec<CycleRollup>` | `{ cycle_id, worker_name, started_at, finished_at, ran, skipped, failed, runs: Vec<CycleRun> }` |
| `recent_manual` | `Vec<RecentRun>` | manual-trigger runs, individual |
| `findings_open` | `u64` | |
| `captured_at` | `DateTime<Utc>` | **freshness — see §5.4** |

Per-transport implementation:

- **Local** (`host/local.rs`) — in-process, reads `run_store` / autoflow history directly.
- **HTTP** (`host/http.rs`) — `proxy_get_json("/api/dashboard?host=local&range=…")`.
- **SSH** (`host/ssh.rs`) — shells `rupu … --format json` on the far side, mirroring the existing
  `list_sessions` / `list_autoflow_runs` implementations.
- **Tunnel / Bucket** — default `Unsupported` initially. They contribute nothing and are rendered
  as such (§5.4), rather than being counted as zero.

**`Unsupported` must not read as zero.** A host that cannot report is not a host with no runs.
`DashboardSummary` is `Option`-shaped per host at the aggregation layer; `None` renders as
"unavailable," never as `0`.

### 4.2 Aggregation

`GET /api/dashboard` gains `?range=` and `?host=` and routes through the existing
`fan_out_via(hosts, local_values, what, f)` helper in `api/host_fanout.rs`, which already tags rows
with `host_id` and degrades per-host failure to `warn!` + empty contribution rather than failing the
whole request. `DashboardResponse` gains `host_id` on every row plus a `hosts: Vec<HostFreshness>`
breakdown.

`?host=` absent → fan out across all registered hosts. `?host=<id>` → that host only. This matches
the `/api/runs` idiom (`api/runs.rs:574-600`).

### 4.3 SSH `list_runs` — behavior fix (separate PR)

**This is a change to an existing shipped path and lands as its own PR inside plan 2.**

Today `SshHostConnector::list_runs` reads a *local mirror* (`mirror_list_runs`,
`host/connector.rs:433`). The mirror is populated by `spawn_tail_pump` (`host/ssh.rs:616`), an
in-memory `tokio::spawn` created inside `launch_run` (`ssh.rs:897`) and `launch_agent` (`ssh.rs:977`)
— **and nowhere else**.

Consequences, all permanent for the affected runs:

- A run started directly on the remote box is invisible.
- A run launched by a *previous* `cp serve` process is invisible — the pump is in-memory and is not
  restarted on boot.
- `mirror_get_run` returns `NotFound` for both.

A "runs on host X" panel built on this under-reports and cannot know it. This is the silently-degraded
shape the project explicitly rejects.

**Fix:** `SshHostConnector::list_runs` shells the remote CLI as the source of truth, consistent with
how `list_sessions`, `list_autoflow_runs`, and `list_agent_runs` already work — all three shell
`rupu … --format json`. The mirror remains the backing store for `stream_run_events`: tailing a known
path on a live run is a genuinely different problem from enumerating the store, and the pump stays
as-is for CP-launched runs.

**Blocker: the CLI surface does not exist.** `rupu run list --format json` is not a command.
`Cmd::Run` is not a clap subcommand — it is `trailing_var_arg` capturing tokens verbatim
(`rupu-cli/src/lib.rs:79-83`) — and `--format json` is explicitly rejected for `rupu run`
(`lib.rs:318-322`, `Table` only). The only run-listing surface that speaks JSON is
`rupu workflow runs --format json`, and it is lossy in ways that are *wrong*, not merely blank:

- **No `trigger`.** The row carries no `event` / `source_wake_id`, so `trigger_of` is not computable.
  Hardcoding `"manual"` would mis-classify every autoflow run on an SSH host as manual, so it would
  escape cycle grouping (§5.5) — the exact bug this redesign exists to fix.
- **`started_at` is `"%Y-%m-%d %H:%M:%S"`** (`workflow.rs:1859`) — space-separated, no timezone, not
  RFC-3339 — while every merge in the fan-out path does a **lexicographic string compare** on that
  field. `' '` (0x20) sorts before `'T'` (0x54), so every SSH row would sort after every local row at
  the same instant, silently.
- No `finished_at` (only `duration_seconds`); `usage` is flat `total_tokens` + `cost_usd` rather than
  a `UsageSummary`; `--limit` is applied *before* sorting (`workflow.rs:1827`), so a small limit
  returns an arbitrary subset rather than the newest N.

**Therefore plan 2 adds a real CLI surface first:** `rupu run list --format json`, emitting the full
`RunRecord` fields CP needs — `trigger`, `finished_at`, RFC-3339 timestamps — versioned
`kind: "run_list", version: 1` per the existing convention. The impedance mismatches are then fixed
at the source rather than patched in a mapper.

**Rejected: `cat`-ing remote `run.json` over `RemoteExec`.** It yields the complete `RunRecord` with
no CLI change and has surface precedent (the pump `cat`s `run.json` to poll terminal status). But it
makes `list_runs` the one method in `ssh.rs` not going through the CLI, and — decisively — it
promotes `run.json`'s on-disk shape from an internal detail to a **remote wire format**, so any
future store-layout change breaks remote listing silently on untouched hosts. The CLI's JSON contract
is versioned precisely so it can evolve deliberately. Remains available as a later fallback if old
hosts prove common; not built on speculation.

**Version skew is a visible state, not a fallback path.** A remote host whose `rupu` predates the
command renders **unavailable with a reason** — `builder-01 · needs rupu ≥ 0.49` — gated on
`info().version`, reusing the freshness strip (§5.4) and the §4.1 rule that `Unsupported` never
renders as `0`. No second listing path is carried.

**Risk:** this changes what an SSH host reports and touches the launch-path pump's neighborhood.
Requires a test against a real SSH host, not just a mock — a mock would happily fake the mirror the
fix exists to stop trusting. Separable and revertable on its own.

## 5. Plan 1 — Ops dashboard (page)

### 5.1 Composition

Top to bottom:

1. **Header** — range control (`7d`/`30d`/`all`, retained) + **per-host freshness strip** (§5.4).
2. **Attention row** — the triage ribbon, weighted rather than four equal chips. `AwaitingApproval`
   and `Paused` are the only states where the system is blocked *on the operator*; they carry real
   visual weight. Failed-in-window sits beside them. Open findings demotes — it is a backlog, not an
   interrupt. Chips remain links to the filtered list pages.
3. **Live swimlane** (hero) — §5.2.
4. **Split status** — §5.3.
5. **Activity feed** — §5.5.

### 5.2 Live swimlane (hero)

Each active run is a horizontal bar. X-axis is time; lanes group by host or by workflow (toggle).
Bars are amber on `awaiting_approval`, red on failure, neutral on `running`.

Why this earns the hero slot: the split status tells you *how many*; the swimlane tells you *what is
happening*. A run executing 40× longer than its median is visually obvious in a way no table makes
it, and duration-outlier detection is not available from any other element on the page.

**Hand-rolled SVG.** Recharts has no Gantt. Okesu has precedent — its war-room case timeline is
hand-rolled SVG, "no extra deps" (`web/src/components/investigations/CaseTimeline.tsx`).

**Percentile auto-fit.** Range is fit to the 5th/95th percentile of bar durations, not min/max, so a
single 6-hour run does not crush the rest into 2% of the width. Lifted from Okesu's
`timeline/scale.ts:autoFitRange`.

**Bars do not animate.** They redraw on data arrival. Liveness is per-transport (§5.4): local bars
update sub-second via SSE, SSH-host bars step forward on the poll tick. A smoothly-animating bar
beside one that jumps in 10s increments reads as broken. Redraw-on-data implies no smoothness we do
not have.

### 5.3 Split status

The eight `RunStatus` variants split along a seam already present in the Rust: `is_terminal()` is
`Completed | Failed | Rejected | Cancelled`, and it deliberately excludes `Paused` because a paused
run expects a resume (`rupu-orchestrator/src/runs.rs:58-97`). That exclusion is the design saying
there are two populations here.

- **Active** — `Running`, `AwaitingApproval`, `Paused`, `Pending` as live counts with weight, each
  with an inline segmented bar (Okesu's `Dashboard.tsx:597-613` idiom: a ~1.5px proportional bar
  under the headline number).
- **Terminal** — `Completed`, `Failed`, `Rejected`, `Cancelled` as a stacked area over the range, so
  failure trend is a slope.

Segmented-bar colors are **locked to the stacked-area palette** so the eye ties the live count to the
history without a legend. Both consume `colors.status.*` from the existing
`web/src/lib/useThemeColors.ts` — no new color values are introduced.

### 5.4 Real-time — per-transport, and honest about it

**There is no cross-host event stream.** `/api/events/stream` requires `?run=` whenever `?host=`
names a remote host (`api/events.rs:61-95`); the merged firehose is local-only. "Subscribe to the
firehose and invalidate" is therefore a *local-only* capability. This design does not pretend
otherwise.

| Transport | Mechanism | Liveness |
|---|---|---|
| Local | SSE invalidation | Sub-second |
| HTTP | SSE, proxied end-to-end (`http.rs:327`) | Sub-second |
| SSH | TTL poll, one `dashboard_summary()` per tick | Tick-bounded |
| Tunnel | none — `dashboard_summary()` is `Unsupported` (§4.1) | Renders unavailable |
| Bucket | none — `dashboard_summary()` is `Unsupported` (§4.1) | Renders unavailable |

Tunnel and Bucket are not polled, because a host that cannot report has nothing to poll *for*. They
render as unavailable (§5.4, freshness strip) until they implement the method. Adding either later is
purely additive: implement `dashboard_summary()`, and the host starts contributing on the SSH-style
TTL cadence with no dashboard change.

**SSE is an invalidation signal, not a data channel.** Every number on this page is an aggregate;
the stream carries step-level events. Applying step deltas to aggregates client-side means
reimplementing the Rust aggregation in TypeScript and keeping the two in agreement forever. They
will drift, and the failure mode — a dashboard quietly showing wrong counts — is worse than one that
is 10s stale.

So: subscribe, ignore payloads for arithmetic, use *arrival* to mark the affected slice dirty, and
refetch that aggregate from the server. The server stays the single source of truth for every number.

**Debounce is load-bearing, not an implementation detail.** An autoflow cycle firing twelve runs
produces a burst; naive invalidation means twelve refetches. Dirty marks coalesce on a ~250ms timer,
issuing one refetch per burst. This is the piece that decides whether the page feels fast or hammers
the server.

**Reconciling poll.** A ~60s poll runs regardless, so a dropped SSE connection degrades to today's
behavior rather than freezing. Gated on `document.visibilityState === 'visible'` — a dashboard in a
background tab does no work.

**SSH poll cadence is cost-driven.** Every `RemoteExec::run` spawns a fresh
`tokio::process::Command::new("ssh")` (`ssh.rs:367`) — new process, new TCP connect, new handshake,
per call. There is no `ControlMaster` multiplexing anywhere in the file. `info()` alone is two
sequential round-trips. So: **one `dashboard_summary()` call per host per tick**, TTL-cached, on a
cadence matched to transport cost — never a uniform interval across transports.

**Per-host freshness, shown.** One global "live" pill would lie about the SSH host. Each host carries
its own truth, from `DashboardSummary::captured_at`:

```
local · live      builder-01 · 14s      bucket-west · offline      tunnel-a · unavailable
```

`GET /api/hosts` already returns `{ id, name, transport_kind, status, version, active_run_count,
last_seen_at }` and nothing consumes it. This is also the host-status view rupu lacks entirely today.

### 5.5 Activity feed — cycle grouping

**One row per autoflow cycle**, collapsed by default:

```
▸ autoflow nightly-review · 12 runs · 10 ok, 2 failed · 3m ago       [cron] [builder-01]
```

Expandable to individual runs. **Manual runs always render individually** — they are never grouped.
Inside an expanded cycle, clean runs fold behind a clickable `+N clean` pill (Okesu's
`isInterestingTick` idiom, `EventTimeline.tsx:1113-1128`): hidden, never lost. Failures inside a
cycle stay visible on their own.

Rows carry `trigger` and `host_id` chips. `TriggerChip` already exists
(`web/src/components/TriggerChip.tsx`; manual=neutral, cron=violet, event=sky).

**Grouping is by cycle, not by outcome.** A cycle failing *as a cycle* is a real event;
outcome-grouping scatters that across rows.

**Join direction: cycle → runs, not run → cycle.** There is no `autoflow_id` on `RunRecord`. The
linkage is a reverse index — `api/run_resolve.rs` reads autoflow *history* to map a run back to its
cycle, so grouping N runs run-first would mean N history lookups. `AutoflowCycleRow`
(`api/run_streams.rs`) already carries `cycle_id`, `worker_name`, `ran_cycles`, `skipped_cycles`,
`failed_cycles`, and a `run_ids` array. Pull from the cycle store and join outward.

**`CycleRollup` carries `runs: Vec<CycleRun { run_id, status }>`, not a bare `run_ids: Vec<String>`.**
The `+N clean` pill needs each run's status to decide what folds, and `AutoflowCycleRow` supplies
only IDs. The status join happens server-side in `build_summary`, which already holds every run —
making the client fetch a run per ID to learn its status would turn one expanded cycle into N
requests. This is why the DTO diverges from `AutoflowCycleRow`'s shape rather than mirroring it.

**The 10-row server cap is removed.** It exists only because the feed was ungrouped.

## 6. Plan 3 — Spend page

Spend demotes on the dashboard to a compact tile linking here. This page answers attribution and
anomaly; trend is what those two *look like* on a time axis, not a third thing to build.

**Attribution (spine).** Pivot by model / workflow / host / project. Today `group_by` supports
`model` only (`api/usage.rs`). Runs already carry `workspace_id`, `workflow_name`, and now `host_id`,
and `AutoflowCycleRow` carries `usage` directly — so workflow, project, and host pivots are joins
over data that already exists. "This autoflow costs $40/night" is currently unanswerable and is the
actionable question.

**Anomaly (panel).** Outlier runs by cost against a per-workflow baseline. Nearly free once
per-dimension cost over time exists.

**Trend** falls out: a pivot with a time axis *is* the trend view.

**Agent dimension is deferred to plan-3 scoping.** Usage is summarized per-run via `summarize_run`;
whether it decomposes per-agent depends on how the transcript attributes tokens. Not committed here.

**The unpriced-model gap becomes a visible number.** Today a `*` footnote marks partial spend when
some models lack a price. On a dedicated page that becomes explicit — `$12.40 known · 3 models
unpriced` — because an attribution page that silently under-counts is worse than no page. Same
reasoning as §4.3.

**Spend fans out.** It routes through `dashboard_summary()` / `fan_out_via` like everything else, or
it is local-only and wrong.

## 7. Testing

**Rust (plan 2).**
- `dashboard_summary()` per transport against fixtures; `Unsupported` default verified to surface as
  `None`, never `0`.
- Fan-out: per-host failure degrades to a warning and an absent contribution, never a failed request.
- Range bucketing, including empty buckets (fill the grid so charts do not lie about gaps).
- **SSH `list_runs` against a real SSH host.** A mock cannot catch this — the bug being fixed is that
  the mirror is populated only on the launch path, which a mock would happily fake.

**TypeScript (plans 1, 3).**
- Aggregation and layout helpers are pure functions with co-located tests, following Okesu's
  `cluster.ts` / `scale.ts` / `filter.ts` precedent: cycle grouping, `+N clean` folding, percentile
  auto-fit, swimlane lane assignment.
- Debounce coalescing: a 12-event burst issues exactly one refetch.
- Freshness rendering: `Unsupported` / offline hosts render as unavailable, not zero.

**Runtime validation.** Per CLAUDE.md, `cargo build` + `cargo test` cleanliness is not rendering
cleanliness. The page is validated in a browser before merge.

## 8. Constraints & Deferred

- **No SSH connection reuse.** Every call is a fresh handshake. Designed around (one aggregate call
  per host per tick, TTL-cached) rather than fixed. `ControlMaster` multiplexing would materially
  improve remote-host cadence and is the highest-value follow-up.
- **No cross-host firehose.** Remote liveness is poll-bounded except HTTP. A per-host SSE multiplexer
  over `stream_run_events` is possible but out of scope.
- **Tunnel / Bucket `dashboard_summary()`** ships as `Unsupported` and renders as unavailable.
- **`HttpHostConnector` timeout.** `reqwest::Client::new()` has an effectively unbounded timeout on
  the normal `?host=` path; only the probe path bounds it via `resolve_for_probe`. An unreachable
  HTTP host can stall on the OS TCP connect timeout. Pre-existing; flagged because fan-out makes it
  more reachable. Should be bounded in plan 2 if cheap.
- **`proxy_get_json` returns `Invalid`, not `Unsupported`,** on SSH/Tunnel/Bucket — a wart, since
  `Unsupported` exists for exactly this. Not fixed here; noted.
- **`make cp-web` before `make release`.** The SPA is embedded via `RustEmbed` from `web/dist/`
  (`src/embed.rs`); `build.rs` writes a placeholder if absent. Building without it ships a stale or
  placeholder UI.

## 9. Plan Sequence

| Plan | Scope | Depends on |
|---|---|---|
| 2 | `rupu run list --format json` in `rupu-cli` (§4.3); `dashboard_summary()` across transports; SSH `list_runs` fix (own PR); fan-out in `/api/dashboard` | — |
| 1 | Ops page: swimlane, split status, cycle feed, attention row, freshness strip, SSE invalidation | 2 |
| 3 | Spend page: pivots, outliers, unpriced gap | 2 |

Plan 2 is the foundation and carries the risk. Plans 1 and 3 are independent of each other.

**Scope note.** Plan 2 grew a `rupu-cli` task once it emerged that the CLI surface the SSH fix
assumed does not exist (§4.3). It is the first task in plan 2 and blocks the SSH fix. Note that this
lands in `rupu-cli`, which CLAUDE.md requires stay thin — a `run list` subcommand is arg parsing plus
a `RunStore::list()` call and a serializer, so it stays within that rule.
