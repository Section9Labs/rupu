# rupu CP — progressive per-host loading for Activity, Usage and ⌘K

- **Date:** 2026-10-01
- **Status:** design approved in brainstorm; awaiting spec review; implemented on branch `claude/cp-progressive-per-host-loading`
- **Branch:** `claude/cp-progressive-per-host-loading`
- **Prior art:** the dashboard's per-host load (`web/src/lib/dashboard/useDashboardData.ts`,
  spec `2026-07-30-rupu-dashboard-fleet-strip-design.md`); the `/api/hosts` probe cache
  (`host/probe_cache.rs`, PR #701)

## 1. Problem

Three CP list surfaces wait for the slowest registered host before painting anything.
On a CP with 4 SSH hosts, the local work takes ~20–50 ms but the requests take 1.5–4 s.
The endpoints are:

- `GET /api/runs` and `GET /api/runs/workflows` (`fan_out_list_runs`, `api/runs.rs`)
- `GET /api/runs/autoflows` (`list_autoflow_runs`, `api/run_streams.rs`)
- `GET /api/usage` (`get_usage`, `api/usage.rs`; SSH hosts shell `rupu usage --format json`)

All three fan out server-side and `join_all` every host before responding. The dashboard
already solved this pattern: the web lists hosts via the probe-free `/api/hosts/registered`,
fires one `?host=<id>` request per host independently, and merges client-side behind a
per-host freshness strip. This spec applies the same pattern to every Activity table, the
Usage page and the ⌘K palette.

## 2. What the code does today

Facts established while designing. Some of them correct the original framing.

1. **The Activity tables already default to "This host".** WorkflowRuns, AgentRuns,
   AutoflowRuns and Sessions start at `hostFilter = 'local'`, so their default load never
   touches SSH. They block only after the operator picks "All hosts". The workflow table
   calls `/api/runs/workflows`, which shares `fan_out_list_runs` with `/api/runs`.
2. **Bare `/api/runs` has exactly one web caller:** the ⌘K palette, on open
   (`getRuns({ limit: 200 })`).
3. **The Usage page does block.** `getUsage(window, pivot)` is called with no host, so it
   fans out. It does so again every 30 s while a preset window is active, and again on
   every pivot click.
4. **The page never reads `/api/usage`'s `breakdown`.** The breakdown table and the graph
   are built from local-only `/api/usage/runs`. Only `summary`, `unpriced` and `hosts` are
   read. Under `group_by=project`, SSH hosts report `unavailable` (the CLI cannot group by
   project), so the fleet headline silently drops their spend on that pivot.
5. **SSH listings are all-or-nothing.** Every `RemoteExec::run` is a fresh `ssh` process
   with a full handshake (no ControlMaster). SSH `list_runs` always runs
   `rupu run list --limit 10000` remotely. That command computes per-run usage from
   transcripts for every run, and the CP then slices `offset/limit` locally. Every
   infinite-scroll page and every 5 s poll on an All-hosts Running tab therefore re-pulls
   each SSH host's entire history. `autoflow history` is likewise unpaged.
6. **The server merge orders by raw string** (`sort_values_newest_first`,
   `fan_out_list_runs`). That mis-orders `…Z` against `…+00:00` across hosts that
   serialize differently.
7. **`usePagedList`'s poll splice is buggy.** `[...page0, ...rows.slice(page0.length)]`
   drops a row when one new run arrives at the top. When a run leaves the list (e.g.
   finishes and leaves the Running tab), it duplicates a row instead. Its only `poll: true`
   callers are the Activity tables this spec moves to a new hook.
8. **`/api/hosts` probes are now cached** (PR #701). Each remote's last probe is served,
   and a stale one triggers a background refresh. `HostSelect`'s All-hosts dropdown only
   needs ids and names, which the probe-free `/api/hosts/registered` already returns.

## 3. Goals and non-goals

**Goals**

- Local rows render as soon as local answers. Each remote host's rows merge in as that
  host answers. No request waits on another host.
- Every host's state is visible and honest: loading, ok (with age), stale (with reason),
  offline (with reason) or unavailable (with reason).
- Infinite scroll over the merged list stays in correct newest-first order.
- SSH load on remote hosts goes down, not up, even though All hosts becomes the default.

**Non-goals**

- Remote-host live push (SSE). There is no cross-host firehose, and remotes stay
  poll-bounded.
- Fanning out the local-only usage endpoints (`/api/usage/runs`, `/api/usage/outliers`)
  or the graph.
- Removing the server-side fan-out paths. Bare `?host`-absent requests keep working
  unchanged for scripts and curl; the web simply stops depending on them.
- ControlMaster or SSH connection pooling.

## 4. Decisions

| Question | Decision |
|---|---|
| Default scope of the Activity tables | **All hosts**. Local paints first; remotes merge in. |
| Remote refresh cadence | **60 s while the tab is visible, plus on tab focus, plus manual Refresh.** Local keeps its current cadence (5 s on polling tabs; Usage's 30 s window tick). |
| Scope | **Every Activity tab** (Workflow runs, Agent runs, Autoflow cycles + events, Sessions), **plus Usage and the ⌘K run list.** Autoflow Claims stays local (its endpoint is local-only). |
| Approach | **Client-side per-host fan-out** (the dashboard pattern), with three small server changes (§7). Rejected: one streaming request per table (new plumbing, holds browser connections, same pagination problem); a server-side background snapshot (polls SSH hosts when nobody is looking, and needs a breaking response envelope). |
| Pagination | **Per-host offset cursors with a watermark merge** (§6.2). |

## 5. Architecture

```
               ┌──────────────────────── browser ────────────────────────┐
 page mounts → │ GET /api/hosts/registered   (store read, no SSH)        │
               │   seed one slice per host: state = loading               │
               │   ├─ GET /api/runs/workflows?host=local&offset=0&limit=20 ─┐ independent
               │   ├─ GET …?host=mini&…                                     │ promises, no
               │   └─ GET …?host=kuki&…                                     │ Promise.all
               │ watermarkMerge(slices) → visible rows · floor · end       │
               │ HostFreshnessStrip(slices)                                │
               └───────────────────────────────────────────────────────────┘
                            │ ?host=<id> (single-host path, already exists)
               ┌──────────── rupu cp serve ───────────┐
               │ single-host list handlers            │
               │   connector error → 502 / 501        │  (§7.1; 404 = unknown host id only)
               │ SshHostConnector                     │
               │   ListingCache: 5 s TTL, single-flight, cleared on mutation │  (§7.2)
               └──────────────────────────────────────┘
```

The client owns ordering, pagination and freshness. The server only answers one host at a
time, as it already can.

## 6. Web

### 6.1 Slice model

One slice per host, kept by the hook:

```ts
interface HostSlice<T> {
  hostId: string; name: string; transportKind: string;
  state: 'loading' | 'ok' | 'offline' | 'unavailable';
  rows: T[];              // newest-first, deduped by id
  hasMore: boolean;
  catchingUp: boolean;    // §6.3
  pagingFailed: boolean;  // a next-page / catch-up request for this host failed
  reason: string | null;  // cause when not ok, or why a refresh failed (stale)
  receivedAt: number | null;  // when this host last answered successfully
}
```

`t(row)` is `Date.parse(row[timeField])`; an unparseable or missing value counts as
`-Infinity` (oldest). `timeField` is set per table and matches the field each host's own
server sorts by:

| Table | timeField |
|---|---|
| Workflow runs, Agent runs, Autoflow cycles, ⌘K runs | `started_at` |
| Autoflow events | `at` |
| Sessions | `updated_at` |

**Assumption:** within one host, rows arrive already ordered by `timeField`. Each host
serializes its own timestamps consistently, so its string order agrees with parsed order.
Across hosts the client compares parsed instants only.

### 6.2 `lib/perHost/watermarkMerge.ts` (pure)

Definitions:

- `covered(h)` = `t(last row of h)` if `h.hasMore`, else `-Infinity` (fully listed).
- **Gating set** `G` = hosts with `state = 'ok' ∧ hasMore ∧ ¬catchingUp ∧ ¬pagingFailed`.
- **Floor** `F` = `max over G of covered(h)`, or `-Infinity` when `G` is empty.
- **Visible** = every loaded row of every slice with `t(row) ≥ F`. Rows are sorted by
  `t` descending, ties broken by `(hostId, id)`, and de-duplicated by `(hostId, id)`.
  Rows below `F` are held back: a row from a gating host could still belong above them.
- **`hasMore`** (drives the scroll sentinel) = `G` is non-empty.
- **`ended`** = `G` is empty, and no slice is `loading` or `catchingUp`.
- **`gatingHosts`** = the members of `G` whose `covered(h) = F`. The next scroll page is
  fetched from these hosts only.

It returns `{ visible, floor, hasMore, ended, gatingHosts, excludedHosts }`.
`excludedHosts` lists offline and unavailable hosts, for the footer and empty state.

### 6.3 `lib/perHost/usePerHostPagedList.ts`

**Signature:**

```ts
usePerHostPagedList<T>({
  host: string | null,                   // one host id, or null = every registered host
  fetch: (p: { host: string; offset: number; limit: number; signal?: AbortSignal }) => Promise<T[]>,
  timeField: keyof T & string,
  idField: keyof T & string,
  deps: unknown[],
  poll?: boolean,
}) → { rows, slices, loading, error, hasMore, ended, sentinelRef,
       refresh, refreshHost, retryPaging, removeRow }
```

`rows` is `watermarkMerge(...).visible`.

- `loading` is true only until the host list is known and at least one slice has left
  `loading`. After that, per-host loading lives in `slices`, and the page shows "Waiting
  on …" (§8) rather than a spinner.
- `error` is set when the host list could not be read, or when every slice has failed.

Per-host state lives in `lib/perHost/engine.ts` (`PerHostListEngine`), outside React. The
hook owns one engine per filter generation, disposes it on change, and owns the cadence
timers. A disposed engine ignores every late answer.

The deps/generation handling is the same as `usePagedList`: deps are compared index by
index into a stable `gen`, and `fetch` is read through a ref. `host: string` (one id)
produces a single slice, so every page has one code path whatever its filter. Its first
fetch never waits on the registered-hosts read; for a remote id that read only supplies the
slice's display name once it answers (the raw id until then, or if it fails).

**Bootstrap** (on mount, or when `gen` changes):

1. Read `GET /api/hosts/registered`. On failure, fall back to `['local']` and surface the
   error.
2. Seed one `loading` slice per host.
3. Fire page 0 (`offset 0, limit 20`) for every host independently. Each promise's
   handler updates only its own slice. An answer from an older `gen` is dropped.

**Late arrival and catch-up.** A host can answer while other hosts already have visible
rows. This covers a remote's first page and a host recovering from offline. In that case:

- Let `displaced` = the number of currently visible rows with `t < covered(h)`. These are
  the rows that would drop below the floor if `h` started gating now.
- If `displaced = 0` or `¬h.hasMore`: `h` gates immediately.
- Otherwise `h.catchingUp = true`, with a budget of `displaced` rows. The hook fetches
  batches of `limit = clamp(remaining budget, 20, 195) + 5` (the 5-row overlap; 200 is the
  server's `MAX_LIMIT`) until one of these holds:
  - `covered(h) ≤ F` (computed without `h`)
  - `¬hasMore`
  - the budget is spent
  
  Then `catchingUp = false` and `h` gates.
- **Property (tested, with a real catch-up batch):** the visible list is never shorter than
  it was before the late host arrived. The visible count can peak during catch-up and then
  settle. If catch-up reaches `F`, nothing is displaced. If the budget runs out first, the
  late host has added at least `displaced` rows, all above the new floor, and the rows it
  now displaces are a subset of the original `displaced` set.
- A late host can still insert rows above the row you're reading, because that is what
  merging means. The browser's default scroll anchoring keeps the row being read in place.
  The live check (§10) verifies this.

**Scrolling.** The sentinel calls `loadMore`, which fetches the next page for each host
in `gatingHosts`. The request uses `offset = max(0, rows.length − 5)` and `limit = 25`,
and the result is de-duplicated. The 5-row overlap absorbs up to 5 rows leaving that
host's list between pages without skipping any. A page is full when
`returned.length === limit`.

**Offset drift.** A full page that adds no new rows means rows landed on top of the host's
list and shifted every offset, so the cursor now points at rows already held. The host
re-anchors (a page-0 splice for that host) and retries the page once. If it still adds
nothing, the host gets `pagingFailed` with the reason "list shifted while paging", rather
than silently ending its list. If the re-anchor itself fails, the host gets `pagingFailed`
with that failure's reason, and the page is not retried. A short page that adds nothing is
the real end of the list.

**At most one request in flight per host.** A refresh that comes due while that host
has a page or catch-up request in flight runs after it, coalesced. This bounds
concurrency per host and table to 1. Together with the server's single-flight (§7.2),
that bounds the SSH commands an open table can cause.

**Bounded fetches.** Every engine fetch is bounded at 45 s (`FETCH_TIMEOUT_MS`): a host
that has not answered by then fails with "no answer after 45s" (offline for a first load,
`pagingFailed` for a page), so one hung host cannot hold its own queue, or `loadMore`'s
scroll lock, forever. `PerHostFetchParams.signal` is aborted on that timeout and when the
engine is disposed, and the list and usage api calls forward it to `fetch`. Without that,
abandoned requests to a hung host would pile up and exhaust the browser's 6-per-origin
connection pool.

**Refresh merge rule** (pure, `lib/perHost/spliceHead.ts`). Given a host's old rows `O`
and a fresh page 0 `P`, requested with `limit L`:

- If `|P| < L`, the host is fully listed: `rows = P`, `hasMore = false`.
- Otherwise, if `P` does not overlap the old rows (`O` is empty, or the newest old row is
  older than `cutoff = t(last of P)`), the union would leave a gap of rows between the two
  ranges. `P` **replaces** the slice and `hasMore = true`.
- Otherwise `rows = P ∪ { o ∈ O : t(o) ≤ cutoff ∧ o.id ∉ P }`, re-sorted, with `hasMore`
  unchanged.
  - Old rows newer than `cutoff` that are missing from `P` are dropped. They left the
    list: finished, archived, or filtered out.
  - Rows exactly at `cutoff` are kept, so a timestamp tie is never lost.
  - `P` wins for rows present in both, so status updates land.

This replaces `usePagedList`'s splice (§2.7).

**Cadence:**

| | Local | Remote |
|---|---|---|
| Polling tables (`poll: true`) | page-0 refresh every 5 s, as today | page-0 refresh every 60 s while `document.visibilityState === 'visible'` |
| Tab regains focus (polling tables) | refresh | refresh |
| Non-polling tables | load + manual Refresh only (unchanged) | load + manual Refresh only |
| `offline` host | — | retried on the 60 s remote cadence (the recovery path) |
| `unavailable` host (e.g. 501, old remote rupu) | — | retried on manual Refresh only (an old binary doesn't upgrade itself in a minute) |

Polling tables are WorkflowRuns and AgentRuns on Running, Sessions on Active, and
Autoflow cycles and events.

**Retry.** `retryPaging(id)` is a no-op unless that host is `pagingFailed`. It clears the
flag and re-anchors on a fresh page 0 before rejoining the merge (catch-up as for a late
host). Throughout the re-anchor the host stays out of gating (`catchingUp`): gating again at
its old, shallower coverage would lift the floor and hide rows right where Retry was clicked.

**Manual Refresh** re-reads `/api/hosts/registered`, adding slices for new hosts and
dropping removed ones. It then applies the refresh merge rule to every host. It does not
reset the list, so nothing flashes. **`refreshHost(id)`** refreshes one slice. Row
actions (archive/restore/delete) use it for the host that owns the row, after
`removeRow(hostId, id)` drops the acted-on row from that slice.

**`usePagedList` cleanup.** Once the Activity tables move off it, `usePagedList` has no
`poll: true` caller left. Its `poll` option and the buggy splice are removed. The
project tabs and Claims keep using it, without polling.

### 6.4 Pages

- **WorkflowRuns, AgentRuns, AutoflowRuns (cycles + events), Sessions:**
  - Default `hostFilter = ALL_HOSTS`.
  - Switch to `usePerHostPagedList`.
  - Show `HostFreshnessStrip` in All-hosts mode when more than one host is registered.
  - Autoflow Claims is unchanged (local-only endpoint).
- **`HostSelect` (`allowAll` branch only):** read `/api/hosts/registered`. The launcher
  branch keeps `/api/hosts`, because its "(offline)" suffix is useful when choosing where
  to launch.
- **Footer:** "— end of N —" when `ended`. It is followed by "· not included: mini
  (offline), kuki (needs a newer rupu)" when `excludedHosts` is non-empty.

### 6.5 Usage: `lib/usage/mergeUsage.ts` (pure) and `lib/usage/useUsageData.ts`

- **`useUsageData(window)`** follows `useDashboardData`'s shape: read
  `/api/hosts/registered`, seed one `loading` entry per host, then fire
  `getUsage({ since, until }, host)` per host independently.
- **`group_by` is pinned to `model`** (the server default, which every transport, SSH
  included, can answer), not omitted. The page never reads `breakdown`, so a pivot click no
  longer refetches, and SSH hosts stay in the headline under every pivot (§2.4).
- **Each response's own `hosts[0]` entry is authoritative** (the dashboard rule). A 200
  whose entry says `offline`/`unavailable` is stored as that state, without its zeroed
  summary.
- **`mergeUsage`** runs over the hosts that are `ok` for the current window. It ports
  `usage::rollup` (sum tokens and runs; `cost_usd` is `None` unless some host priced;
  `priced` ANDs; `partial` ORs) and `merge_unpriced` (union of models, sum of rows).
- **Headline honesty.** When any host is not current for the window, the headline's
  sub-label adds "· excludes mini (offline)". Loading hosts are named the same way, and a
  stale one reads "(stale)" (below).
- **Cadence.**
  - Local refetches whenever the window changes: the existing 30 s preset tick, and any
    user range change.
  - Remotes refetch every 60 s while visible, on focus, and immediately on a
    user-initiated window change (range click or drag-select). They do not refetch on the
    30 s tick.
  - The 60 s poll and the focus refetch skip any host that already has a request in
    flight: restarting it would mean a host slower than the poll never answers. A user
    window change and the tick supersede the in-flight request on purpose.
  - A remote's figure can therefore lag local by up to 60 s; the strip shows each host's
    age.
- **Bounded and abortable requests.** Each usage request is bounded at 45 s
  (`FETCH_TIMEOUT_MS`, shared with the list engine): a host that has not answered is
  aborted and recorded as failed ("no answer after 45s"), so the in-flight skip above can
  never leave a host loading forever. A newer request for a host aborts its previous one,
  and unmount aborts them all, so a hung remote does not hold a browser connection for as
  long as it lives. An aborted or superseded request never marks its host offline.
- **A 404 drops a non-local host** (removed from the registry), as the list engine does.
  `local` is never dropped; it shows as offline.
- **Stale-on-error:** a host that answered for window A and then fails its request for
  window B keeps its old answer, but is excluded from B's headline. The failure is
  recorded against the window it failed for (`failedKey`). A host whose `failedKey` is the
  current window counts as **failed** for it, whatever stale data it still holds:
  - it shows `offline` in the strip and `(stale)` in the headline's excluded list;
  - it counts toward `error`, so a page whose every host ended up that way reads as an
    error rather than loading forever;
  - it **stays** failed while a retry for that same window (tick, poll, focus) is in
    flight, until the retry answers, so the error does not flicker back to a spinner each
    poll cycle.

  A failure for some other window says nothing about this one: a window change reads
  `loading`, not failed.
- **Window changes keep the last good headline.** `useUsageData` returns no headline from a
  user window change until the first host answers for the new window (it never mixes an old
  window's figures into a new one). The page keeps the last good headline on screen
  meanwhile, with its "updating" cue and the strip's `loading` entries, and does not
  collapse to the full-page spinner (which would unmount the timeline and delay its run-rows
  fetch).

### 6.6 ⌘K palette

The runs and sessions sources become per host: `getRuns({ host, limit: 200, signal })` and
`getSessions({ host, limit: 200, signal })` for each registered host, with that host's items
appended as it answers. The palette's loading spinner tracks the other sources and local runs
and sessions only, so a hung remote never holds the palette in a loading state. A failed host
contributes no rows and no error.

**One `AbortController` per open.** Closing the palette (or unmounting it) aborts every
per-host run and session request, so a hung remote cannot pin a connection per open and
exhaust the browser's 6-per-origin pool.

A remote run's entry links to `/runs/<id>?host=<host_id>` and a remote session's to
`/sessions/<id>?host=<host_id>`; both are keyed `<host_id>:<id>`. Without the host, the CP
would have to probe every host to find the row.

## 7. Server

### 7.1 Honest error statuses on single-host list paths

The `?host=<remote-id>` branches of these handlers map connector errors with one shared
helper:

- `list_runs`, `list_workflow_runs`
- `list_agent_runs`, `list_autoflow_runs`, `list_autoflow_events`
- the sessions list

The mapping:

| `HostConnectorError` | Status | Client state |
|---|---|---|
| `Unsupported`, `Invalid` | 501 (`ApiError::not_available`) | `unavailable` |
| everything else (`Unreachable`, `Unauthorized`, `Remote`, `NotJson`, **`NotFound`**, …) | 502 (`ApiError::bad_gateway`) | `offline` |
| *(not a connector error)* `resolve_host`'s unknown host id | 404 | slice dropped as "host removed" |

The helper is `api::runs::host_list_error`. A connector `NotFound` on a list path is
**502**, never 404: a reachable HTTP remote that answers a list route with 404 would
otherwise vanish silently. **404 on these paths comes only from `resolve_host`'s unknown
host id**, i.e. the host was removed from the registry, and the client drops that slice.
(The client drops a 404 slice only in All-hosts mode, and never `local`.)

The body stays `{ "error": "<reason>" }`, and the client shows the reason. Today all of
these return 500, which can't tell "down" from "too old". For SSH, which side of that line
a failure is on is decided in §7.2.

### 7.2 SSH listing cache

`SshHostConnector` gains a `ListingCache`. Connectors are long-lived: `HostRegistry`
caches one per host.

- **Key:** the remote argv. Exactly these listing commands are cached:

  | Remote argv | Used by |
  |---|---|
  | `--format json run list --limit 10000` | `list_runs`, `dashboard_summary` |
  | `autoflow history --format json` | `list_autoflow_runs`, `list_autoflow_events`, `dashboard_summary` |
  | `transcript list --format json` | `list_agent_runs` |
  | `session list …` (active and `--archived` forms, keyed separately) | `list_sessions` |

- **TTL 5 s.** An `Ok` result younger than 5 s is served from the cache. Older ones wait
  for a fresh fetch. Stale data is never served.
  - **This refines the brainstorm's 15 s.** Lists return bare arrays with no
    `captured_at`, so the client's per-host age is measured from receipt. 5 s matches the
    strip's "live" threshold (`LIVE_THRESHOLD_MS`), so the cache can never make a host
    look fresher than one live-threshold of slack.
  - 5 s still covers the main wins: concurrent identical requests (dashboard + Activity +
    ⌘K on one page load, two browser tabs) and back-to-back catch-up and scroll pages.
- **Single-flight:** concurrent callers for the same key share one in-flight `ssh`,
  including its error. **Errors are never stored** beyond that one call.
- **Runs to completion.** The fetch runs in its own `tokio::spawn`ed task, whether or
  not anyone still waits on it. Waiters only await its shared result, and a caller
  arriving while it runs joins it, even when every earlier waiter went away. So client
  churn (rapid tab switches, aborted page requests) never starts a second listing for one
  key on one host. A spawned task is always polled, so its answer is fresh when it
  completes; the task itself retires its in-flight entry and, on `Ok`, stores the rows.
- **Bounded: `LISTING_MAX_INFLIGHT` = 60 s.** A fetch still running after 60 s is
  abandoned: its future is dropped, every waiter gets
  `Unreachable("listing took longer than 60s")` (offline, 502; the down-vs-old classifier
  below counts it as a transport failure), and nothing is stored. The cached listing path
  runs its remote command through `RemoteExec::run_cancellable`, which `SshExec`
  implements as `run` plus `kill_on_drop(true)`, so dropping the fetch **kills the `ssh`
  child**. Every other ssh call, mutations included, keeps `run`, so a dropped caller never
  cuts one short.
- **Panic.** A fetch that panics fails **every** waiter, including ones parked on it, with
  an `Unreachable("listing fetch panicked")` error. Nothing is stored. The panic is caught
  inside the fetch task, so the task completes normally and every waiter is woken.
- **Cleared on mutation.** Every mutating `SshHostConnector` method clears the whole
  cache (a `clear_on_drop` guard), so a refresh straight after an action never shows the
  pre-action row:
  - runs: `approve_run`, `reject_run`, `cancel_run`, `pause_run`, `resume_run`,
    `archive_run`, `restore_run`, `delete_run`
  - launches: `launch_run`, `launch_agent`
  - sessions: `start_session`, `send_session_turn`, `archive_session`, `restore_session`,
    `delete_session`
  - transcripts: `archive_transcript`, `delete_transcript` (both through
    `remote_transcript`, which changes what the cached `transcript list` returns)
  
  The cache is cleared after the remote command returns, whether it succeeded or failed:
  a failed mutation may still have partly applied. A clear does not abort an in-flight
  fetch (its waiters asked before the mutation and still get their answer), but it
  forgets it: a fetch that started before the clear never stores its (pre-mutation)
  answer, and a caller arriving after the clear never joins it.
- **Down vs old (classification of `list_runs` / `dashboard_summary` failures).** A host
  that is down is not a host that predates `rupu run list`. An ssh **transport** failure
  (`is_ssh_transport_failure`) passes through as `Unreachable` (**offline**, 502). Only a
  failure of the remote rupu itself maps to `Unsupported` (**unavailable**, "needs a
  newer rupu", 501). `RemoteOutput` carries no exit code, so the classifier is stderr
  markers: `ssh: `, `ssh spawn failed`, `connection refused`, `connection reset by`,
  `connection timed out`, `connection closed by`, `operation timed out`, `no route to host`,
  `permission denied (publickey`, `host key verification failed`,
  `kex_exchange_identification`, the mid-session drops `closed by remote host`,
  `not responding`, `broken pipe`, `received disconnect`, and the cache's own
  `listing took longer than` (a hung host, not an old rupu, which fails fast). The
  other cached listings (`autoflow history`, `transcript list`, `session list`) pass the
  `Unreachable` through unchanged, so they are offline on any failure.
- **Shared use:** `list_runs` and `dashboard_summary` share the `run list` entry.
  `list_autoflow_runs`, `list_autoflow_events` and `dashboard_summary` share the
  `autoflow history` entry.
- **Implementation:** `host/listing_cache.rs`, a sibling of `HostProbeCache`. Per key it
  keeps one in-flight entry (an id plus a `futures::Shared` over the spawned task's
  `JoinHandle`) behind an `Arc<std::sync::Mutex>` (never held across an await) that the
  task shares to retire and store its own entry. Unlike `HostProbeCache`, its fetch is
  fallible, errors are never stored, there is no serve-stale window, and it has a
  `clear()`.

**SSH budget this buys:** an open All-hosts polling table on a CP with 4 SSH hosts costs
about **4 remote commands a minute**. Today's All-hosts Running tab costs 48 (4 hosts ×
12 polls). Scrolling and catch-up add commands only for the host at the floor, and
back-to-back pages share the 5 s entry.

### 7.3 Unchanged

The bare, `host`-absent fan-out paths of every endpoint above stay as they are. The
single-host paths' row shapes, `host_id` tagging and codename injection don't change. No
new endpoints are added.

## 8. Errors and honest state

- **Per host:** see the §7.1 table.
- **Stale-on-error:** a host that already has rows keeps them when a refresh fails. Its
  `reason` is set, and the strip shows it as stale with its age. Only a host that never
  answered becomes `offline`/`unavailable`.
- **Paging or catch-up failure:** `pagingFailed = true`. The host stops gating, so the
  list never gets stuck behind it. This covers a failed or timed-out page, a failed
  re-anchor, and a list that kept shifting ("list shifted while paging", §6.3). The footer
  reads "older rows from `<host>` couldn't load · Retry", and Retry re-anchors the host on
  a fresh page 0 and resumes (§6.3).
- **Never claim emptiness we can't vouch for:**
  - Hosts still loading and no rows yet → "Waiting on mini, kuki…", not the empty state.
  - Every host answered with zero rows → today's empty state. If any host is
    offline/unavailable it reads "No runs on the hosts that answered · mini offline".
- **The error banner** appears only when every host failed. Single-host failures,
  including local, live in the strip.
- **Single-host mode** keeps today's behaviour: one slice, a banner on failure, no strip.
- **Usage:** the per-host `hosts[0]` entry is authoritative; the headline names excluded
  hosts (§6.5).

## 9. Testing

**Pure (vitest)**

- **`watermarkMerge`:**
  - order by parsed instant across `Z` / `+00:00` / fractional-second stamps
  - an unparseable timestamp sorts last
  - floor set by the right host; `gatingHosts` correct
  - catching-up, offline and paging-failed hosts excluded from gating
  - `(hostId, id)` dedupe
  - `ended` and `hasMore` truth table
- **Catch-up property:** a table of late-arrival scenarios asserting the visible count
  never decreases, and that rows stay globally ordered.
- **`spliceHead`:**
  - a new run arrives → no row lost (regression for §2.7)
  - a run leaves the list → removed, not duplicated (regression)
  - a status change → updated from `P`
  - short `P` → replaces the slice; timestamp tie at `cutoff` → kept
- **`mergeUsage`:** mirrors the Rust `rollup` / `merge_unpriced` cases (`cost_usd` `None`
  vs `Some`, priced AND, partial OR, union of unpriced models).

**Hooks and pages (vitest, a fake `api` with deferred promises, fake timers)**

- `usePerHostPagedList`:
  - local paints while a remote promise hangs; the remote resolving merges in
  - 502 → offline, 501 → unavailable, 404 → slice dropped
  - a stale `gen` answer is dropped
  - scrolling fetches only from `gatingHosts`; catch-up batches stop at the floor or the
    budget
  - one request in flight per host; a refresh is coalesced behind it
  - cadence: local 5 s, remote 60 s only while visible, focus refetch, `unavailable`
    retried on manual Refresh only
  - manual Refresh picks up added and removed hosts
- `useUsageData`: the same structure as `useDashboardData.test.ts`, plus "a pivot change
  does not refetch".
- **Pages:** existing tests updated for the All-hosts default and the `getRegisteredHosts`
  mock. The strip renders per host; the footer names excluded hosts; the empty state is
  honest.
- **⌘K:** a hung remote doesn't block other sources or the spinner.

**Rust (`cargo test -p rupu-cp`)**

- Error mapping for each listed handler, with a stub connector: `Unreachable` → 502,
  `Unsupported` → 501, `NotFound` → 502 (404 only from `resolve_host`'s unknown host id).
- `ListingCache` + `SshHostConnector`, using `FakeExec` to count exec calls:
  - two concurrent `list_runs` → 1 exec
  - a repeat within 5 s → 0 execs; after 5 s → 1
  - an error is not stored
  - a fetch whose waiters all left still completes and serves the next caller; a caller
    arriving meanwhile joins it; a fetch past 60 s is abandoned (future dropped, nothing
    stored); a panicking fetch fails every waiter
  - the cached listings exec through `run_cancellable`, everything else through `run`
  - each mutating method clears the cache
  - an ssh transport failure is `Unreachable` (offline); a remote-rupu failure is
    `Unsupported` (unavailable)
  - `list_runs` and `dashboard_summary` share one `run list` exec
  
  These call-count assertions are the guarantee against SSH bursts.

**Gates:** `cargo test -p rupu-cp`, `cargo clippy`, web `vitest`, and web `tsc`, which is
run locally because `ci.yml` skips it.

## 10. Live verification: zero SSH traffic to real hosts

- **Setup:** a throwaway `rupu cp serve` built from the branch, on a spare port, with a
  temp `RUPU_HOME` and seeded local runs. Its registered "remote" hosts are HTTP hosts
  pointed at tiny local stub servers: one answers after ~3 s, one returns 502, one hangs.
- **In the browser pane, check:**
  - each Activity tab, Usage and ⌘K
  - local paints first, slow-host rows merge in, and the dead and hung hosts are labelled
    honestly
  - scroll order holds across hosts, and the reading row stays put when rows insert above
    it
  - Refresh and row actions refresh the right slice
- **Not done:** no live check is pointed at the real fleet (`mini` or any other SSH host).
- **Owed to matt after merge and release:** one look at Activity and Usage with the real
  4 SSH hosts, confirming local is instant and the chips fill in.

## 11. Out of scope and follow-ups

- **A server-stamped `captured_at` for list responses.** It would need a `HostConnector`
  return-type change across five transports and every list method. It is the
  prerequisite for raising the SSH cache TTL above the strip's live threshold.
- **Paged remote listing for SSH:** `rupu run list --offset`, plus `--trigger` and
  lifecycle-group filters. Remote cost would then scale with the page instead of the
  history. It needs an old-remote fallback.
- **The v2 shell footer** still polls `/api/hosts` (now cached by #701). No change.

## 12. Risks

- **Offset churn on remote lists between pages.** Mitigated by the 5-row overlap plus
  dedupe. More than 5 rows leaving one host's list between two pages can still skip a
  row until the next refresh; this is the same class of limitation as today.
- **Clock skew between hosts** affects cross-host interleaving, as it already does in the
  server merge. Ordering within a host is unaffected.
- **Default flip to All hosts on a CP with many slow hosts:** each table load costs one
  request per host. Bounded by single-flight, the one-in-flight-per-host rule and the
  60 s remote cadence.

## 13. Execution amendments (2026-10-01)

Rulings made while implementing; each is folded into the section named.

- **§7.1: a connector `NotFound` on a list path is 502, not 404.** 404 comes only from
  `resolve_host`'s unknown host id. A reachable HTTP remote that answers a list route with
  404 would otherwise vanish silently as "host removed".
- **§7.2: the mutation list gains transcript archive/delete (`remote_transcript`),
  `start_session` and `delete_session`.** They change what a cached listing returns (or, for
  `start_session`, would), so they must clear it too.
- **§7.2: a listing fetch runs in a spawned task to completion, even if every waiter
  leaves, bounded at `LISTING_MAX_INFLIGHT` = 60 s; on that bound its future is dropped and
  the ssh child killed (`run_cancellable` = `run` + `kill_on_drop`, on the cached listing
  path only).** This replaces an earlier "a fetch whose every waiter left is dropped, never
  resumed" rule, which left the ssh child running and let rapid tab switches start a new
  `run list --limit 10000` per click on each SSH host. That rule existed because an
  un-polled `Shared` resumed later served an old snapshot as fresh; a spawned task is always
  polled, so its answer is fresh when it completes. Later callers join it, so there is at
  most one listing per key per host whatever the client does.
- **§7.2: a panicking fetch fails every waiter, parked ones included, and stores nothing.**
  The panic is caught inside the fetch task; `Shared` does not wake parked waiters on a
  panic of its own inner future, so they would hang.
- **§7.2: a fetch that started before a clear never stores, and a later caller never joins
  it.** A pre-mutation answer must not outlive the mutation.
- **§7.2: an ssh transport failure is `Unreachable` (offline); only a remote-rupu failure is
  `Unsupported`.** A down host would otherwise read as "needs a newer rupu". The classifier is
  stderr markers because `RemoteOutput` carries no exit code.
- **§7.2: implementation is a spawned task per fetch, shared through `Shared` behind a
  `std` mutex**, not `HostProbeCache`'s `tokio::Mutex` slot. The task retires and stores
  its own entry; `clear()` forgets in-flight fetches without aborting them.
- **§7.2: a listing timeout reads as offline.** `listing took longer than` is one of the
  down-vs-old markers, so a hung host is not reported as "needs a newer rupu".
- **§6.3: a full page 0 that does not overlap the old rows replaces the slice** and sets
  `hasMore = true`. The union would otherwise lose the rows between the two ranges.
- **§6.3: a full page that adds no rows re-anchors and retries once, then `pagingFailed`
  ("list shifted while paging").** Rows landing on top shift every offset; silently ending the
  list hid older rows. A failed re-anchor reports its own reason.
- **§6.3: `retryPaging` is a no-op unless paging-failed, re-anchors before rejoining, and
  stays out of gating while doing so.** Gating at the old, shallower coverage would lift the
  floor and hide rows where Retry was clicked.
- **§6.3: every engine fetch is bounded at 45 s, and its `signal` is aborted on timeout and
  dispose.** Abandoned requests to a hung host would exhaust the browser's 6-per-origin
  connection pool.
- **§6.3: the catch-up property is "never shorter than the pre-arrival list", tested with a
  real catch-up batch.** The count can peak during catch-up and then settle. Batches are
  `clamp(budget, 20, 195) + 5` so they never exceed the server's `MAX_LIMIT`.
- **§6.3: the hook takes `host: string | null` (not `hosts`), a required `idField`, and
  returns `retryPaging` and `removeRow`.** They are what the pages' Retry and row actions use.
- **§6.3: a replaced page 0 stays out of gating and pages back down at once** (`join()` with a
  minimum budget of the rows the slice lost), showing its old rows below the re-fetched span
  meanwhile. Gating at page 0's coverage lifted the floor and collapsed the list under the
  reader. A single picked remote takes its registered display name without delaying its first
  fetch.
- **§6.5: `group_by` is pinned to `model`, not omitted.** It is the server default and every
  transport can answer it; the page never reads `breakdown`.
- **§6.5: a host that is `ok` for an older window and failed for the current one
  (`failedKey === windowKey`) counts as failed.** Otherwise a page whose every host ended up
  that way read `loading` forever. It stays failed while a same-window retry is in flight, so
  the error does not flicker back to a spinner each poll.
- **§6.5: usage requests are bounded at 45 s, a newer request aborts the previous one, and
  unmount aborts all.** The same connection-pool starvation as the list engine.
- **§6.5: the 60 s poll and focus refetch skip hosts with a request in flight.** Restarting
  it would mean a host slower than the poll never answers.
- **§6.5: a 404 drops a non-local host.** It was removed from the registry.
- **§6.5: the page keeps the last good headline, with its "updating" cue, while a
  user-changed window loads.** The hook never mixes an old window's figures into a new
  one, and collapsing to the full-page spinner would unmount the timeline.
- **§6.6: the palette aborts its per-host run requests when it closes, and a remote run
  links with `?host=`.** A hung remote would otherwise pin a connection per open, and a
  host-less link makes the CP probe every host to find the run.
- **§6.6: the palette's sessions source is per host too, and a remote session links with
  `?host=`.** The host-less `getSessions` fanned out on the server, so the palette's spinner
  waited on a hung host (found in the live check).
