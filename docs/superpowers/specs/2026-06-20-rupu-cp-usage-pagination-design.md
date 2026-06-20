# rupu Control Plane — Per-transaction usage + lazy-loaded lists (Slice A.4) — Design

**Date:** 2026-06-20
**Author:** matt + Claude
**Status:** Design draft (approved in brainstorm)
**Builds on:** Slice A.3 (cost & tokens, PR #325, merged). Branches from `main`.

## Summary

Two fast follow-ups on A.3, deliberately bundled because they're coupled:

1. **Per-transaction usage on every list row.** A.3 added the token+cost `UsageChip` to Workflow/Project runs and Sessions, but the **Agent Runs** page, the **Dashboard recent-runs** list, and the **autoflow cycle/event** rows still show no cost/tokens. Fill those gaps using the existing A.3 machinery (`summarize_paths` / `summarize_run` / `rollup`).

2. **Lazy-loaded lists.** CP list endpoints return *all* rows today; long lists render everything at once. Adopt Okesu's infinite-scroll pattern — fetch a small page, append more as the user scrolls — across the list views. Page size **20**.

**Why coupled:** A.3 computes `usage` for *every* run on each list fetch (reading each run's transcript). On a large store that's expensive. Pagination is the fix: compute usage only for the **page** being shown. So the lists must paginate for per-row usage to stay cheap at scale.

This is mostly frontend + thin backend (query-param slicing); no new crates, no `rupu-cli` dependency in `rupu-cp`.

## Design decisions (locked with matt)

- **Page size = 20** ("do not show more than 20"). Default initial render is 20 rows; more load on scroll.
- **Everywhere by default** — every list view renders at most 20 and lazily loads/renders the rest ("load fast, only render more when necessary").
- **Usage on all remaining run/agent rows** — Agent Runs, Dashboard recent, autoflow cycles + events (row-level; no per-step breakdown).
- **Okesu's pattern, verbatim where it fits** — a portable `useInfiniteScroll` hook (scroll-listener + bottom sentinel + 240px pre-load threshold + double-fetch lock; *not* IntersectionObserver), offset/limit pagination, raw-array responses, `hasMore` inferred from `page.length < limit`, periodic page-0 refresh preserved.

## Two list categories (decides backend vs client-side)

| Category | Examples | Treatment |
|---|---|---|
| **Growable** (scale with runs/sessions/time) | Workflow Runs, Agent Runs, Autoflow cycles + events, Sessions, Project Runs, Project Sessions | **Backend pagination** (`?offset&limit`) → fetch 20, lazy-fetch more. Fast load + fast render. |
| **Bounded** (config-like, a handful) | Agents, Workflows, Workers, Projects, Autoflow defs, Coverage targets | Backend returns all (already small/fast); frontend renders 20 + **widens the client window** on scroll. Fast render; no backend change. |

Both categories use the **same** `useInfiniteScroll` hook — the only difference is whether `loadMore` fetches the next page (growable) or widens an in-memory slice (bounded).

## Part 1 — Per-transaction usage (backend + frontend)

### Backend (`crates/rupu-cp/src/api/`)

- **`run_streams.rs` — `AgentRunRow`** gains `usage: UsageSummary`. Each row already carries a `transcript_path`; compute `crate::usage::summarize_paths(&[path], &pricing)` per row (both standalone and session-derived rows have a transcript path). Rows with no transcript path get `UsageSummary::default()`.
- **`run_streams.rs` — `AutoflowEventRow`** gains `usage: UsageSummary` — when the event has a `run_id`, compute `summarize_run(store, run_id, pricing)`; otherwise default.
- **`run_streams.rs` — `AutoflowCycleRow`** gains `usage: UsageSummary` — `rollup` over the cycle's `run_ids.iter().map(|id| summarize_run(store, id, pricing))`.
- **`dashboard.rs` — `RecentRun`** gains `usage: UsageSummary` via `summarize_run(store, id, pricing)` for each of the (≤10) recent runs.

All four reuse A.3's `crate::usage` functions; `AppState` already carries `run_store` + `pricing`.

### Frontend (`crates/rupu-cp/web/src/`)

- `api.ts`: add `usage: UsageSummary` to `AgentRunRow`, `AutoflowEventRow`, `AutoflowCycleRow`, and the `DashboardResponse.recent_runs` element.
- Render `<UsageChip usage={...} />` in: `AgentRuns.tsx` (`AgentRunEntry` — replaces the A.3 placeholder comment), `Dashboard.tsx` (`RecentRunRow`), and both `AutoflowRuns.tsx` rows (`AutoflowEventItem`, `AutoflowCycleItem`). Guard with `&&` where the field is conceptually optional (events without a run).

## Part 2 — Lazy-loaded lists (backend + frontend)

### Backend — pagination on growable list endpoints

Add optional `offset` (default 0) + `limit` (default **20**, capped e.g. 200) query params to the growable list handlers:
- `/api/runs`, `/api/runs/workflows`, `/api/runs/agents`
- `/api/runs/autoflows`, `/api/runs/autoflows/events`
- `/api/sessions`
- `/api/projects/:ws_id/runs`, `/api/projects/:ws_id/sessions`

Each handler: build the full sorted (newest-first) collection it already builds, **slice `[offset, offset+limit)`, THEN compute per-row usage on the slice only**, and return the raw array (unchanged response shape — just fewer elements). Absent params → the existing default page (offset 0, limit 20) so an un-paginated caller still gets a bounded, fast response. A shared helper `fn paginate<T>(items: Vec<T>, offset, limit) -> Vec<T>` keeps it DRY; a small `PageQuery { offset: Option<usize>, limit: Option<usize> }` extractor parses the params.

**Ordering guarantee:** each endpoint must sort deterministically (newest-first by `started_at`/`at`) *before* slicing, so successive pages don't overlap or skip. Most already sort; confirm/centralize.

### Frontend — the infinite-scroll hook + wiring

- **`web/src/lib/useInfiniteScroll.ts`** (new) — port Okesu's hook: `useInfiniteScroll({ hasMore, loadMore, threshold = 240 }) → { sentinelRef, loading }`. Scroll listener on the nearest `overflow-auto` ancestor (walk up via `getBoundingClientRect`), fires `loadMore()` when the sentinel is within `threshold` px of the viewport bottom; a synchronous `loadingRef` lock prevents double-fetch; a `requestAnimationFrame` re-check after load handles short pages. Dependency-free, framework-agnostic. Unit-tested where feasible (the geometry math is DOM-bound; test the lock/`hasMore` guard logic).
- **`api.ts`** — the growable list methods accept `{ offset?, limit? }` and append them as query params (mirroring A.3's `getUsage` param-builder). Bounded-list methods are unchanged.
- **List pages** — each adopts the pattern:
  ```
  const [items, setItems] = useState<Row[]>([]);
  const [hasMore, setHasMore] = useState(true);
  // page-0 fetch (also the 15s refresh): api.list({ limit: 20 }) → setItems(page); setHasMore(page.length >= 20)
  const loadMore = async () => {
    const next = await api.list({ offset: items.length, limit: 20 });
    setItems(prev => [...prev, ...next]);
    if (next.length < 20) setHasMore(false);
  };
  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });
  // render rows, then a <div ref={sentinelRef}> sentinel: "loading…" / "scroll for more" / "— end of N —"
  ```
  - **Growable** pages (Workflow/Agent/Autoflow runs, Sessions, Project runs/sessions): `loadMore` fetches the next page.
  - **Bounded** pages (Agents, Workflows, Workers, Projects, Autoflow defs, Coverage): one fetch returns all; `loadMore` just widens an in-memory `visibleCount` (e.g. `setVisible(v => v + 20)`), rendering `items.slice(0, visible)`. Same hook, `hasMore = visible < items.length`.
- Each list lives inside an `overflow-auto` scroll container (most already do — verify) with the sentinel as the last child.

## Components & boundaries

| Unit | Responsibility | Notes |
|---|---|---|
| `PageQuery` extractor + `paginate` helper (rupu-cp) | parse offset/limit, slice | DRY across handlers |
| growable list handlers | sort → slice → usage-on-slice → array | bounded usage cost |
| `crate::usage::*` (A.3) | tokens→cost | reused unchanged |
| `useInfiniteScroll` hook (web) | scroll→loadMore | one shared hook, both categories |
| list pages | accumulate/append or window | thin wiring |
| `UsageChip` (A.3) | render tokens+cost | reused unchanged |

## Error handling

- **Pagination params:** a non-numeric `offset`/`limit` → ignore + use defaults (lenient; never 500 a list over a bad query string). `limit` clamped to `[1, 200]`.
- **loadMore failure:** the hook swallows the rejection (logs), keeps `hasMore` so the user can retry by scrolling; the page's error banner is reserved for the initial fetch (matches Okesu).
- **Usage on a missing/partial transcript:** `summarize_*` already tolerate it (zero-token, `priced:true`) — unchanged.
- **Empty list / end reached:** sentinel shows `— end of N —`; no further fetches (`hasMore=false`).

## Testing

- **Backend:** `paginate` helper unit tests (offset/limit slicing, out-of-range offset → empty, limit clamp, default 20); a handler test that `/api/runs?offset=0&limit=2` returns 2 rows and `offset=2` returns the next slice (no overlap); usage is present on the sliced rows; `AgentRunRow`/autoflow/recent-run carry `usage`. `cargo test -p rupu-cp` + `clippy --all-targets` clean.
- **Frontend:** `useInfiniteScroll` lock/`hasMore` guard logic (the double-fetch lock; that `loadMore` isn't called when `hasMore=false`); the param-builder in `api.ts` (`{offset,limit}` → `?offset=..&limit=..`); a render test that a list shows ≤20 rows initially and the sentinel renders. `npm run build` strict + `npm test -- --run`. Main chunk stays ~48 KB. Rendering validated by matt.

## Scope & non-goals

- **In:** usage on Agent Runs / Dashboard recent / autoflow rows; offset/limit on the growable list endpoints; the `useInfiniteScroll` hook; wiring all list pages (growable = fetch-more, bounded = window-more) at page size 20.
- **Out (YAGNI):** per-step usage inside a run; cursor-based pagination (offset/limit suffices, matches Okesu); virtualized rendering (windowing 20-at-a-time is enough); total-count headers (`hasMore` from page length is sufficient); pagination of the `/api/usage` breakdown (it's already a bounded top-N).

## Decomposition

One spec, **two plans**:
1. **Plan 4a — backend:** `PageQuery`/`paginate`, offset/limit on the growable handlers (sort→slice→usage-on-slice), and `usage` on `AgentRunRow`/`AutoflowEventRow`/`AutoflowCycleRow`/`RecentRun`. Tests. Ships independently (curl-verifiable).
2. **Plan 4b — frontend:** `useInfiniteScroll` hook, `api.ts` param wiring, the new `UsageChip` placements, and infinite-scroll/windowing on all list pages.
