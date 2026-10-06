# Netflow progressive loading — design

**Status:** approved in brainstorm 2026-10-06; spec awaiting review.
**Scope rule (operator, verbatim intent):** make the existing Network pages
load better and faster. *What is rendered stays the same.* How it is
rendered and how the data gets there may change; no new UI elements, states,
controls, paging affordances, or changed control semantics.

## 1. Problem

The Network surface (`NetflowExplorer` — global page, project Network tab,
run Network tab) is slow on large histories and shows nothing until every
byte has arrived. Measured on matt's Mac (2026-10-06), `~/.rupu/netflow`:
6,940 per-run ledgers, 40,261 flows, 17 MB of flow JSONL.

| Request | Time | Body |
|---|---|---|
| `GET /api/netflow` (cold) | 3.03 s | 20.9 MB |
| `GET /api/netflow` (warm) | 0.41 s | 20.9 MB |
| `GET /api/netflow/explorer` | 0.47 s | 20 KB |
| `GET /api/netflow?from=<24h ago>` | 0.36 s | 1.6 KB |

Root causes:

1. **Every request re-opens every ledger.** `read_all_run_ledgers_in_dir` /
   `project_fallback_flows_and_dropped` / `resolve_ledger_paths` callers
   parse each file in full on every request; nothing is cached. Opens cost
   ~0.3–0.5 ms on this Mac vs ~4 µs for a `stat`, so 6,940 opens ≈ 2.8 s —
   the cold number. `read_flows_in_range` parses the whole file and filters
   afterwards, so a narrow window saves no read work; the explorer always
   reads unbounded (its histogram and topology node universe are
   whole-history by contract).
2. **The page waits for everything.** `NetflowExplorer` gates the whole
   surface on `Promise.all([explorer, flows])`; the 20 KB aggregates sit
   behind the 21 MB flows list.
3. **The table mounts every row.** `NetflowTable` → `SortableTable` renders
   all 40k `<tr>`s into the DOM.
4. **The default window is `All`.**

The per-run ledger layout is NOT the problem and is not changed: one file
per run keeps concurrent writers (fan-out units, parallel steps, subprocess
capture) lock-free, and a coarser partition would not help the global page.

## 2. Design overview

Four changes, in order of impact:

1. **`NetflowIndex`** — a two-tier, stat-validated, incrementally-tailed
   index of every local ledger, owned by `cp serve`, with a memory budget.
   Every local netflow read goes through it. Same endpoints, same response
   shapes, same values.
2. **Independent section loading** in `NetflowExplorer`: each existing
   section renders as soon as its own data arrives.
3. **`24h` default window** at global and project scope (existing picker
   preset).
4. **Virtualized table rendering** above a row threshold — visually and
   behaviourally identical, fewer DOM nodes.

Out of scope: remote host / worker paths (`host_raw_flows`,
`worker_host_flows`, the `Host` run-location proxy) — they keep their
current reads; the `rupu netflow` CLI; ledger layout or migration; any API
shape change; any visual change.

## 3. `NetflowIndex` (server)

### 3.1 Shared fold — `rupu-netflow`

The line-folding logic inside `ledger::views::read_flows_and_dropped`
(flow lines, `complete`, `socket_complete` — which can change a prior
flow's `bytes_*`, `outcome`, `error` — `dropped`, `capture`) moves into a
resumable `ledger::fold::LedgerFold`:

- `LedgerFold::default()`, `feed_line(&str)`, and accessors for flows,
  dropped total, capture entries.
- `read_flows_and_dropped` and `read_capture_states` become thin wrappers
  over it, so a file read in one pass and a file read in appended chunks
  are folded by the same code.
- Malformed lines are skipped exactly as today.

### 3.2 Per-file entry

Keyed by canonical ledger path, across the same directory union the global
view reads today (each registered workspace's `.rupu/netflow/` plus
`<global>/netflow/`, canonicalized and deduped; `is_per_run_ledger_path`
decides membership, so the legacy `flows.jsonl` stays excluded).

**Tier 1 — always resident (exact answers for whole-history views):**

- `FileStamp` (`rupu_runtime::file_cache::FileStamp`: len, mtime, ctime,
  inode) and the byte offset consumed so far (always at a line boundary).
- flow count, `dropped` total, capture entries.
- `min_ts` / `max_ts`.
- per-flow `(ts, is_error)` in file order — the exact input
  `histogram_view` needs (bucket edges depend on the scope-wide min/max, so
  per-hour pre-bucketing would change the numbers).
- distinct origin keys and distinct `peer_ip`s (including "none") — the
  inputs to the topology node universe. Org keys/labels are derived at
  query time through the current ASN table, as today. Workflow
  attribution stays per ledger file via `RunMetaIndex`.

Tier 1 is not budgeted (it is the minimum needed for exact whole-history
answers; ~16 B/flow + small per-file sets) but is counted and reported.

**Tier 2 — budgeted rows:** the file's flows as compact rows.

- Each file has its own string table; `CompactFlow` stores `u32` symbols
  for `method`, `scheme`, `host`, `path`, `http_version`, `error`,
  `local_addr`, and the `FlowCtx` / `FlowProcess` strings, plus the scalar
  fields inline. Per-file tables mean evicting a file frees all of its
  memory.
- `CompactFlow::from_record` destructures `FlowRecord` (and nested structs)
  WITHOUT `..`, so adding a field to `FlowRecord` fails to compile until
  the compact form handles it; `to_record` rebuilds an identical
  `FlowRecord`. A round-trip test with every field populated pins it.
- While a file's tier 2 is resident, the fold's `FlowId → row` map is kept
  with it so later `complete` / `socket_complete` lines apply in place.

### 3.3 Refresh

The index is a drop-in for the per-file read: callers keep listing ledger
files exactly as today (same directories, same iteration order) and ask the
index for each file instead of opening it. That keeps every caller's
iteration order — and so every order-dependent detail of the responses —
unchanged. Each per-file call refreshes that file's entry under the
entry's own lock (concurrent requests for one file collapse into one read;
all callers are already on the blocking pool via `run_blocking`):

- **stamp unchanged** → nothing (one `stat`).
- **stamp changed** → open the file and check that its first bytes still
  match the prefix recorded at the last read (ledgers are append-only; a
  prefix mismatch means the file was rewritten in place):
  - prefix matches, same inode, len ≥ offset, tier 2 resident → read from
    the stored offset; feed complete lines only (a trailing partial line is
    left for the next refresh); advance offset; update tier 1 and tier 2.
  - anything else (tier 2 evicted, inode changed, shrank, prefix differs,
    new file) → full re-read.
- **file gone** → drop the entry (covers `rupu netflow prune`).
- **non-UTF-8 content** → the entry reads as empty, exactly as today's
  readers do (`lines()` fails, callers `unwrap_or_default`).

The prefix check replaces `FileCache`'s settle window for this file type:
ledgers are never rewritten by rupu, so the only in-place rewrite to catch
is an external one, which the prefix check detects without the settle
rule's cost of re-reading every live ledger on every request.

Documented deviation: a final line with no trailing newline is not read
until its newline arrives (today's `lines()` would parse it). rupu's writer
always terminates lines, so this only matters for a writer killed mid-line,
whose partial line is malformed and skipped today anyway.

Entries for files that no longer appear in a global-scope listing are
dropped at the end of that listing (`retain_only`), so pruned or
unregistered ledgers do not linger in tier 1.

`serve_on` prewarms the index on the blocking pool at startup, beside
`usage::prewarm`, logging elapsed time.

### 3.4 Budget and eviction

- Config: `[netflow].cp_index_budget_mb` (`rupu_config::NetflowConfig`),
  default `256`. Accounted bytes = sum over resident tier-2 row sets
  (fixed row size × rows + string-table bytes + id map).
- Over budget: evict tier 2 of the files with the oldest `max_ts` first
  until under budget. Tier 1 is never evicted.
- Row sets are `Arc`-shared; a request holds its own `Arc`s, so eviction
  never disturbs an in-flight response.
- A query that needs an evicted file's rows re-reads that file (through
  `LedgerFold`) for the request; the result is re-admitted only if it fits
  the budget after normal eviction order is applied. **Exceeding the
  budget costs speed, never correctness.**
- A `debug` log line per eviction pass (files evicted, bytes freed).

### 3.5 Query paths

All three scopes read local ledgers through the index; the response
builders are unchanged in contract.

- **File selection.** Global: every indexed file. Project: the workspace's
  local directory files plus the global-root files named by the project's
  `RunMetaIndex::ledger_ids()` (exactly today's union, including the
  `$HOME`/`RUPU_HOME` same-directory guard). Run: `resolve_ledger_paths`
  for `run_and_unit_ids`, as today; transcript merge
  (`merge_with_transcript`) and remote/worker sources unchanged.
- **Flows list** (`/api/netflow`, `/api/projects/:id/netflow`,
  `/api/runs/:id/netflow`): rows from files whose `[min_ts, max_ts]`
  overlaps the window (tier 1), window-filtered per flow, then
  `build_filtered_response` as today. `dropped_total` sums tier 1 over ALL
  scope files (today's semantics: whole-ledger, never window-scoped).
  Run-scope `capture` comes from tier 1 capture entries.
- **Explorer** (`/api/netflow/explorer`): `histogram` from tier 1
  `(ts, is_error)` over all scope files; topology node universe from tier 1
  distinct origins / peer IPs / ledger attribution over all scope files;
  every windowed aggregate (node aggregates, links, KPIs, timeline lanes)
  from tier 2 rows of window-overlapping files (an unbounded side resolves
  to the histogram bounds, as today). `rupu_netflow::ledger::explorer`
  gains entry points that take the universe and histogram inputs
  separately from the windowed flow set; the existing whole-slice
  functions remain and delegate, so both paths share one implementation.
- **Status:** `GET /api/netflow/index` → `{ files, flows, tier1_bytes,
  tier2_bytes, budget_bytes, resident_files, evictions_total }`.
  API only; no UI.
- **Budget source:** read from config on every netflow request (cheap), so
  editing `[netflow].cp_index_budget_mb` takes effect without a restart.

### 3.6 Equivalence guarantee

The index must not change any number on screen. Tests build fixtures,
compute every response the old way (full `read_flows_and_dropped` per
file + existing builders) and the new way (index, with budget both ample
and forced tiny so evicted files are re-read), across windows (bounded,
half-open, unbounded, crossed, empty) and filter combinations, and assert
the serialized JSON is identical.

## 4. Web

All changes in `crates/rupu-cp/web/src/components/netflow/`; no new
components, styles, copy, or controls.

### 4.1 Independent section loading (`NetflowExplorer.tsx`)

- Replace the single `Promise.all` effect with two effects sharing the
  same dependencies (`scope`, `projectId`, `runId`, `range`, `filters`):
  one for the explorer aggregates, one for the flows list, each with its
  own cancellation, error, and refreshing state.
- Before the explorer response first arrives, the surface renders the
  `TimeRangePicker` (as the error branch already does) and the existing
  "Loading network flows…" text in place of the aggregate sections. When
  it arrives, the existing aggregate sections render (`ActivityStrip`,
  `KpiStrip`, topology/timeline panel, `OrgCards`, `FilterChips`).
- The table area shows the existing "Loading network flows…" text until
  the flows response first arrives, then `NetflowTable` as today.
- Errors are per section using the existing error text style; the picker
  stays reachable in every state (today's retry affordance).
- The existing "Updating…" status shows while either request refetches
  over existing data; `CoveragePopover` renders once both responses it
  reads (`explorer.dropped_total`, `flows.capture`) have arrived.

### 4.2 Default window

- Global and project scope initialize `range` to the existing `24h`
  preset, resolved against `now` exactly as a user's click on that preset
  would be (`TimeRangePicker` exports the preset-resolution helper it
  already uses internally).
- Run scope is unchanged (the run's own span via `initialWindow`).
- `clearAll` and the window chip's clear are unchanged (→ `All`).

### 4.3 Virtualized table rendering (`SortableTable.tsx`)

- New opt-in prop `virtualize?: { threshold: number }`; only
  `NetflowTable` passes it (`threshold: 500`). At or below the threshold —
  and in every other table — rendering is exactly today's.
- Above it: sorting is unchanged (client-side over the full `rows`); only
  the rows intersecting the window viewport (page scroll — the table has no
  inner scroll container) plus an overscan are mounted, with spacer `<tr>`s
  above and below sized from measured row heights (`ResizeObserver`;
  estimated until measured), so page height, scrollbar, scroll position,
  expanded rows, and row clicks behave as today.
- Known difference, accepted: browser find-in-page only matches mounted
  rows when the table is above the threshold.

## 5. Testing

- **`rupu-netflow`:** `LedgerFold` chunked-vs-whole equivalence (random
  split points, including mid-line); `socket_complete` / `complete`
  applied after a chunk boundary; `CompactFlow` round-trip with every
  field populated.
- **`rupu-cp` (`tests/it/` modules):** index refresh — unchanged, appended
  (with trailing partial line), replaced (new inode), truncated, deleted,
  unsettled; budget eviction order and on-demand re-read; the §3.6
  old-vs-new JSON equivalence across all three scopes and both endpoints;
  `GET /api/netflow/index`.
- **Web (vitest):** explorer section renders while the flows request is
  pending (and vice versa); per-section errors; `24h` default at global and
  project scope, run scope unchanged; `SortableTable` virtualization —
  below-threshold output identical to today, above-threshold mounts a
  bounded row count with correct spacers and sort order.
- **Live verification on matt's Mac** (restart `cp serve` — it is a
  daemon): before/after for the §1 table (cold and warm), time to first
  section paint on the global page, and `GET /api/netflow/index` memory
  figures; recorded in the PR.

## 6. Files touched

- `crates/rupu-netflow/src/ledger/fold.rs` (new), `ledger/views.rs`,
  `ledger/explorer.rs`, `ledger/compact.rs` (new), `ledger/mod.rs`.
- `crates/rupu-cp/src/netflow_index.rs` (new), `api/netflow.rs`,
  `state.rs`, `lib.rs` (prewarm).
- `crates/rupu-config/src/netflow_config.rs` (`cp_index_budget_mb`).
- `crates/rupu-cp/web/src/components/netflow/explorer/NetflowExplorer.tsx`,
  `components/netflow/TimeRangePicker.tsx`,
  `components/netflow/NetflowTable.tsx`,
  `components/lists/SortableTable.tsx`, plus their tests.
- Docs: `docs/configuration.md` documents `[netflow].cp_index_budget_mb`
  and the `GET /api/netflow/index` status endpoint (no `[netflow]` key is
  documented there today; this adds the first); `CLAUDE.md`'s `rupu-cp`
  entry gains one line on the index.
