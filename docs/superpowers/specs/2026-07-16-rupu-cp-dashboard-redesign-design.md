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

- Re-frame the dashboard as an operations surface answering glance-questions: what is running, what
  is stuck, what needs me — as **key points and aggregate graphs, never per-item lists** (revised
  2026-07-17). The list-and-drill-in job belongs to `/runs`.
- Surface autoflow-vs-manual volume as a **throughput-by-trigger graph** — the day-one "autoflow
  taking over the runs" complaint answered as a shape, not by grouping or filtering a feed.
- Replace the status donut with a split that separates live state from outcome trend.
- **Load async, never lock** (revised 2026-07-17): every host and every panel loads independently; a
  slow or dead host holds up nothing.
- Be *visibly honest* about what cannot report — unavailable-with-reason, never a fabricated `0`.
- Span local and remote hosts across all five transports, including SSH.
- Give spend a dedicated page with room to answer attribution and anomaly questions.

**Non-Goals**

- No new frontend dependencies. `recharts` covers every graph on the page; `@xyflow/react` /
  `@dagrejs/dagre` remain for run-detail views. No hand-rolled SVG / Gantt (the swimlane is removed).
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
                        Dashboard.tsx  ── key-point tiles · split status · throughput-by-trigger
                                          (async per-host loading; no per-item lists)
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

`DashboardSummary` carries everything one host contributes. **All aggregate — no per-item row
arrays** (revised 2026-07-17, §5): the first build's `active_runs` / `cycles` / `recent_manual`
arrays fed a swimlane and a feed that are now removed. They were most of the payload and the entire
"detailed table" problem; they are gone from the wire.

| Field | Type | Notes |
|---|---|---|
| `active` | `ActiveCounts` | `running`, `awaiting_approval`, `paused`, `pending` |
| `active_longest_ms` | `Option<u64>` | longest currently-running run's age, + its workflow name / run id, for the "Active now" key point (§5.2). `None` when nothing is running. |
| `terminal_buckets` | `Vec<TerminalBucket>` | `{ ts, completed, failed, rejected, cancelled }` — the outcome trend (§5.3). `ts` day-key-aligned (C1). |
| `throughput_buckets` | `Vec<ThroughputBucket>` | `{ ts, manual, cron, event }` — runs started per bucket by trigger, for §5.5. Same day-key alignment. |
| `cycles` | `CycleCounts` | scalar `{ total: u64, clean: Option<u64>, with_failures: Option<u64> }` — the one line of cycle numbers (§5.5). `Option` where a host cannot report the breakdown (SSH). NOT a row array. |
| `findings_open` | `Option<u64>` | `None` = this host does not report findings (SSH). Never `0`. |
| `captured_at` | `DateTime<Utc>` | **freshness — see §5.4** |

**Removed DTOs.** `ActiveRunBar`, `CycleRollup`, `CycleRun`, `RecentRun` — and the `host_id` tagging,
the cycle→run status join, and the `recent_manual` cycle-exclusion guard that supported them — are
deleted. They were correct for a feed that no longer exists.

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

> **REVISED 2026-07-17 after operator review of the first build.** The first build shipped a
> row-per-cycle **activity feed** and a per-run **swimlane**. Verdict: *"key points, not lists — this
> is a detailed table, not a dashboard."* And it took ~10s to load because the fan-out blocked on an
> unreachable SSH host. Two principles now govern §5, and they supersede the swimlane (old §5.2) and
> the feed (old §5.5):
>
> 1. **Key points, not lists.** The dashboard shows *aggregate* signal — counts, rates, trends — and
>    nothing per-item. The list-and-drill-in job already belongs to `/runs`; duplicating it on the
>    dashboard costs load and attention and answers no glance-question. A per-run swimlane and a
>    per-cycle feed are both *lists in disguise* and are removed.
> 2. **Nothing locks on anything.** Every host and every panel loads independently and async. The
>    page paints its frame immediately and fills in as data lands; one slow or dead host can never
>    hold up another, nor the rest of the page. See the rewritten §5.4.

### 5.1 Composition

Every element is a number or an aggregate graph — **no per-item rows, bars, or feeds anywhere.**

1. **Header** — range control (`7d`/`30d`/`all`, retained) + **per-host freshness strip** (§5.4).
2. **Key-point tiles** (§5.2) — the operator's glance-questions, each a number, most with a
   sparkline. "Awaiting you" / "Paused" (blocked on the operator, loud when nonzero); "Failed"
   (in-window, with trend); "Success rate"; "Active now" (count + longest-running — the one fact that
   answers *is anything stuck*); "Open findings" (backlog).
3. **Split status** (§5.3) — active counts + a terminal-outcome trend graph.
4. **Throughput by trigger** (§5.5) — runs over time, stacked manual / cron / event. The graph that
   answers the day-one complaint (*"autoflow taking over the runs"*) as a *shape*, not a filtered
   list, plus a line of aggregate cycle numbers.

Each of the four blocks is an independent async unit (§5.4). None awaits another.

### 5.2 Key-point tiles

The tiles replace the swimlane. The swimlane's *only* irreplaceable signal was duration-outlier
detection — "is a run stuck". That is a **key point**, not a chart: surface it directly as
`Active now: 3 · longest 2h14m — nightly-review →`, linking to `/runs` for the bars. Everything else
the swimlane showed (which runs, on which host) is a list, and lists live on `/runs`.

Each tile is a headline number, optionally over a **sparkline** of the same metric across the range,
so the trend reads without a second chart. Tiles that mean *the system is blocked on you*
(`AwaitingApproval`, `Paused`) take visual weight when nonzero — they are the only states where
nothing moves until the operator acts. "Failed" carries a trend. "Open findings" stays quiet — a
backlog, not an interrupt.

Colors come from `colors.status.*` in `web/src/lib/useThemeColors.ts` — no new values. Sparklines use
Recharts (already a dep); there is **no** hand-rolled SVG and no Gantt on this page anymore.

**The `Option`/`null` discipline is load-bearing here.** `findings_open` and the per-host cycle
counts are `Option<u64>` on the wire (§4.1). A tile renders `null` as an em-dash and a
`findings_partial` total with a "(partial)" marker — never as `0`. A fabricated `0` on a key-point
tile is exactly the silent-wrong-number this whole design rejects (§8).

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

### 5.4 Loading — async per host, nothing locks

**This is the load-time fix, and it is a hard requirement, not a nicety.** The first build issued
ONE `GET /api/dashboard` that fanned out server-side and awaited every host. An unreachable SSH host
made the whole page hang ~10s (`?host=local` alone was 0.26s). A dashboard must never block on a
remote host.

**Per-host, client-orchestrated loading.** The page does NOT issue one blocking fan-out. Instead:

1. **Enumerate hosts cheaply, no probe.** A fast endpoint returns the *registered* hosts from the
   store (`HostRegistry::list_hosts()` — store read, no `info()`) — id, name, transport_kind. This is
   instant even when a host is down. **`GET /api/hosts` must NOT be used for this** — it probes every
   host via `info()` and takes the same ~10s (measured). The freshness strip renders immediately from
   this list, every host in a `loading…` state.
2. **Paint local first.** `GET /api/dashboard?host=local` (~0.26s) fills the tiles and graphs.
3. **Each remote host in parallel, independently.** `GET /api/dashboard?host=<id>` per remote,
   merged into the aggregate as each answers. A host that is slow or down holds up nothing — its
   freshness flips `loading… → unavailable` on its own timeout (§8: SSH gets a bounded connect
   timeout so this resolves in ~3s, not ~10s), while every other panel is already interactive.

There is **no shared `await`** across hosts or across panels. One slow thing cannot lock another.

**Client-side merge of per-host summaries — and why this is NOT the anti-pattern below.** With
per-host loading, the client combines the arrived summaries: sum the active counts, concat the
throughput/terminal buckets by day, take the oldest `captured_at`, OR the `findings_partial` flags.
This is a small pure function over **already-correct, server-computed per-host summaries** — combining
N right answers, not deriving aggregates from raw events. It is unit-tested with the same
bucket-day-key discipline that the server merge uses (the C1 seam test). Keep the per-host
`DashboardSummary` the single source of each host's numbers; the client only *combines*, never
*computes*.

**SSE stays an invalidation signal — for local only.** `/api/events/stream` requires `?run=` for any
remote host (`api/events.rs:61-95`); there is no cross-host firehose. So SSE arrival marks the
*local* slice dirty and re-fetches `?host=local`; remote hosts refresh on the reconciling poll. This
is exactly why freshness is per-host (below) rather than one global "live" pill that would lie about
the SSH host.

| Transport | Mechanism | Liveness |
|---|---|---|
| Local | SSE invalidation, re-fetch `?host=local` | Sub-second |
| HTTP | independent `?host=<id>` fetch; SSE end-to-end where wired | Fetch-bounded |
| SSH | independent `?host=<id>` fetch, bounded connect timeout (§8) | ≤ timeout |
| Tunnel | none — `dashboard_summary()` is `Unsupported` (§4.1) | Renders unavailable |
| Bucket | none — `dashboard_summary()` is `Unsupported` (§4.1) | Renders unavailable |

Tunnel and Bucket are not fetched, because a host that cannot report has nothing to fetch *for*.
They render as unavailable (freshness strip, below) until they implement `dashboard_summary()`;
adding either later is purely additive.

**Debounce is load-bearing, not an implementation detail.** An autoflow cycle firing twelve runs
produces a burst of local SSE events; naive invalidation means twelve `?host=local` refetches. Dirty
marks coalesce on a ~250ms timer,
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

### 5.5 Throughput by trigger — the graph that replaces the feed

The first build answered *"autoflow is taking over the runs"* with a **cycle-grouped feed** — a list
that fought the noise by folding it. The revised answer is a **graph that makes the noise legible**:
runs started per time bucket, stacked by `trigger` (manual / cron / event). Autoflow-vs-manual
proportion becomes a *shape* — "cron is 90% of today's volume" reads at a glance, with no rows to
scan and nothing to expand. The thing that was drowning the list becomes one band in a chart.

Recharts stacked area/bar, colors from the existing `TriggerChip` palette (manual=neutral,
cron=violet, event=sky) so the graph and any trigger chip elsewhere agree without a legend.

Beneath it, **one line of aggregate cycle numbers** — `N cycles · X clean · Y with failures` — the
only cycle information a dashboard needs. The per-cycle detail, the per-run drill-in, the `+N clean`
expansion: all of that is what `/runs` is for. A "see all →" link goes there.

**Data shape: buckets, not rows.** This needs a `throughput` series (`{ ts, manual, cron, event }`
per bucket) and three scalar cycle counts — NOT the `cycles[]` / `recent_manual[]` row arrays the
first build shipped. Those arrays are removed from the wire (§4.1); shedding them is most of the
payload and the entire "detailed table" problem at once. The trigger classification is
`RunRecord::trigger_str()` (already shipped), bucketed the same way terminal outcomes already are.

<!-- Superseded design retained below for provenance: the cycle→run status join that fed the
     `+N clean` pill. The pill is gone with the feed; the join is no longer built. -->
**(Removed — was: cycle→run status join for the `+N clean` pill.)**
The `runs: Vec<CycleRun { run_id, status }>` join fed the expandable feed. The feed is gone, so the
join is no longer built and `CycleRun` / `CycleRollup`'s row shape is no longer on the dashboard
wire. `AutoflowHistoryStore` still supplies the three aggregate cycle counts (§5.5) directly.

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
- The client-side per-host **merge** is a pure function with a co-located test — combining N
  server-computed summaries: sum counts, group buckets by day-key (the C1 seam test — a local-shaped
  and an SSH-shaped bucket for the same day merge into one), oldest `captured_at`, OR
  `findings_partial`. It combines correct inputs; it never derives an aggregate from raw events.
- Async loading: a dead/slow remote host never blocks local or another panel from rendering (mock a
  host whose fetch hangs; assert local paints and the strip flips that host to `unavailable`).
- Debounce coalescing: a 12-event local SSE burst issues exactly one `?host=local` refetch.
- Key-point / freshness rendering: `Option`/`null` counts render as an em-dash and a partial total is
  marked "(partial)" — `Unsupported` / offline hosts render as unavailable, never `0`.

**Runtime validation.** Per CLAUDE.md, `cargo build` + `cargo test` cleanliness is not rendering
cleanliness. The page is validated in a browser before merge.

## 8. Constraints & Deferred

- **SSH connect timeout must be bounded (revised 2026-07-17).** The first build left the SSH
  `dashboard_summary` path unbounded, so an unreachable host stalled the dashboard ~10s on the OS
  default connect timeout. Bound it (~3s connect) so a dead host's freshness resolves `loading… →
  unavailable` quickly. This is the SSH analogue of the `HttpHostConnector` bound already shipped in
  plan 2 (5s/30s). Combined with async per-host loading (§5.4), no host — bounded or not — blocks the
  page; the bound only caps how long a `loading…` lingers.
- **Cheap host enumeration (revised 2026-07-17).** The page needs the registered host list without
  probing. `HostRegistry::list_hosts()` is a store read (no `info()`); expose it as a fast endpoint
  (e.g. `GET /api/hosts?probe=false` or a dedicated route). `GET /api/hosts` as-is probes every host
  and is itself ~10s against a dead host — it must not be on the dashboard's load path.
- **No SSH connection reuse.** Every call is still a fresh handshake. `ControlMaster` multiplexing
  would materially improve remote-host cadence and is the highest-value follow-up.
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
| 2 | `rupu run list --format json`; `dashboard_summary()` across transports; SSH `list_runs`/`get_run` fix (own PR); fan-out in `/api/dashboard` — **DONE (8/8), merged on branch** | — |
| 2b | **Revision (2026-07-17):** reshape `DashboardSummary` to aggregate-only (drop `active_runs`/`cycles`/`recent_manual`; add `throughput_buckets`, `active_longest_ms`, scalar `CycleCounts`); bounded SSH connect timeout; cheap `list_hosts()` endpoint | 2 |
| 1 | Ops page: **key-point tiles, split status, throughput-by-trigger graph, freshness strip, async per-host loading** (revised — no swimlane, no feed) | 2b |
| 3 | Spend page: pivots, outliers, unpriced gap | 2 |

Plan 2 is done and merged on the branch. The 2026-07-17 operator review ("key points, not lists" +
"nothing locks") adds **Plan 2b** (data-shape + load-path revision) ahead of a reworked Plan 1. Parts
of the first Plan 1 build survive (freshness strip, attention/active tiles, terminal trend); the
swimlane and activity feed — and the row arrays that fed them — are removed.

**Scope note.** Plan 2 grew a `rupu-cli` task once it emerged that the CLI surface the SSH fix
assumed does not exist (§4.3). It is the first task in plan 2 and blocks the SSH fix. Note that this
lands in `rupu-cli`, which CLAUDE.md requires stay thin — a `run list` subcommand is arg parsing plus
a `RunStore::list()` call and a serializer, so it stays within that rule.
