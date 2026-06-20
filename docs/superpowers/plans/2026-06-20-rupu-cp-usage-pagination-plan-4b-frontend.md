# CP Usage + Pagination — Plan 4b (Frontend) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render usage chips on the remaining run/agent rows and make every list view load/render at most 20 rows, lazily loading more on scroll.

**Architecture:** Port Okesu's `useInfiniteScroll` hook. *Growable* list pages fetch page-0 (`limit=20`) then append pages on scroll (`offset = items.length`); *bounded* list pages fetch all once and widen an in-memory `visible` window on scroll. Usage chips reuse A.3's `UsageChip`.

**Tech Stack:** React 18 + TS strict + Vite + vitest. The backend (Plan 4a) now serves `?offset&limit` on the growable endpoints and `usage` on the new rows.

**Prerequisite:** Plan 4a is implemented (or stacked beneath). The API accepts `?offset&limit` and returns `usage` on `AgentRunRow`, `AutoflowCycleRow`, `AutoflowEventRow`, and `recent_runs`.

**Conventions (enforced — READ before starting):**
- Branch `feat-cp-usage-pagination`. NEVER touch `main`. All work under `crates/rupu-cp/web/`; run npm from there.
- NO `any` (TS strict). STATIC Tailwind only. Stage ONLY files you change (never `-A`).
- GUI rendering is validated by matt; your automatable gate is `npm run build` (strict, main chunk ~48 KB) + `npm test -- --run`.
- End commits with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

---

## File structure

| File | Responsibility | Task |
|---|---|---|
| `src/lib/useInfiniteScroll.ts` (new) + test | scroll→loadMore hook | 1 |
| `src/lib/api.ts` | `{offset,limit}` params on growable methods + `usage` on 4 DTOs | 2 |
| `src/pages/runs/AgentRuns.tsx`, `src/pages/Dashboard.tsx`, `src/pages/runs/AutoflowRuns.tsx` | usage chips on the new rows | 3 |
| `src/pages/runs/WorkflowRuns.tsx`, `runs/AgentRuns.tsx`, `runs/AutoflowRuns.tsx` | infinite-scroll (growable) | 4 |
| `src/pages/Sessions.tsx`, `ProjectRuns.tsx`, `ProjectSessions.tsx` | infinite-scroll (growable) | 5 |
| `src/pages/Agents.tsx`, `Workflows.tsx`, `Workers.tsx`, `Projects.tsx`, `AutoflowsDefs.tsx`, `Coverage.tsx` | client-window (bounded) | 6 |

---

## Task 1: `useInfiniteScroll` hook

**Files:**
- Create: `src/lib/useInfiniteScroll.ts`, `src/lib/useInfiniteScroll.test.tsx`

Port Okesu's proven hook verbatim (scroll-listener + bottom sentinel + 240px pre-load threshold + synchronous double-fetch lock; *not* IntersectionObserver). It's framework-agnostic.

- [ ] **Step 1: Create the hook**

Create `src/lib/useInfiniteScroll.ts`:

```ts
import { useCallback, useEffect, useRef, useState } from 'react';

// useInfiniteScroll wires the consumer's bottom-of-list sentinel to a
// scroll-event listener on the nearest scrollable ancestor. When the sentinel
// is within `threshold` pixels of the visible bottom of that ancestor (or of
// the window if the page itself scrolls), and `hasMore` is true, and we aren't
// already loading, it calls `loadMore`.
//
// Scroll listener (not IntersectionObserver): IO is unreliable for inner
// overflow:auto containers; getBoundingClientRect + a scroll listener behaves
// identically in every browser. Overhead is negligible for the few mounted
// sentinels.

function findScrollAncestor(el: Element | null): HTMLElement | null {
  let p: Element | null = el?.parentElement ?? null;
  while (p) {
    const style = getComputedStyle(p);
    const ovr = style.overflowY;
    if (ovr === 'auto' || ovr === 'scroll' || ovr === 'overlay') {
      if (p.scrollHeight > p.clientHeight) {
        return p as HTMLElement;
      }
    }
    p = p.parentElement;
  }
  return null;
}

export function useInfiniteScroll({
  hasMore,
  loadMore,
  threshold = 240,
}: {
  hasMore: boolean;
  loadMore: () => Promise<unknown> | unknown;
  /** Pixels above the scroll-viewport bottom at which the sentinel triggers.
   *  Default 240 so the next page starts loading before the user hits the end. */
  threshold?: number;
}) {
  const [loading, setLoading] = useState(false);

  const sentinelElRef = useRef<HTMLElement | null>(null);
  const scrollerElRef = useRef<HTMLElement | null>(null);

  const loadMoreRef = useRef(loadMore);
  const hasMoreRef = useRef(hasMore);
  const loadingRef = useRef(loading);
  loadMoreRef.current = loadMore;
  hasMoreRef.current = hasMore;
  loadingRef.current = loading;

  const checkAndLoad = useCallback(() => {
    if (!hasMoreRef.current || loadingRef.current) return;
    const sentinel = sentinelElRef.current;
    if (!sentinel) return;

    const sRect = sentinel.getBoundingClientRect();
    const scroller = scrollerElRef.current;

    let bottomEdge: number;
    if (scroller) {
      bottomEdge = scroller.getBoundingClientRect().bottom;
    } else {
      bottomEdge = window.innerHeight || document.documentElement.clientHeight;
    }

    if (sRect.top - bottomEdge < threshold) {
      // Lock synchronously to prevent re-entry from a burst of scroll events
      // before setState propagates back to loadingRef.
      loadingRef.current = true;
      setLoading(true);
      Promise.resolve(loadMoreRef.current())
        .catch(() => {
          /* swallow — the user can scroll again to retry */
        })
        .finally(() => {
          loadingRef.current = false;
          setLoading(false);
          // New content may have pushed the sentinel back into view; re-check.
          requestAnimationFrame(() => checkAndLoad());
        });
    }
  }, [threshold]);

  const sentinelRef = useCallback(
    (el: HTMLDivElement | null) => {
      const oldScroller = scrollerElRef.current;
      if (oldScroller) {
        oldScroller.removeEventListener('scroll', checkAndLoad);
      } else if (sentinelElRef.current) {
        window.removeEventListener('scroll', checkAndLoad);
      }

      sentinelElRef.current = el;
      scrollerElRef.current = null;
      if (!el) return;

      const newScroller = findScrollAncestor(el);
      scrollerElRef.current = newScroller;
      if (newScroller) {
        newScroller.addEventListener('scroll', checkAndLoad, { passive: true });
      } else {
        window.addEventListener('scroll', checkAndLoad, { passive: true });
      }
      requestAnimationFrame(() => checkAndLoad());
    },
    [checkAndLoad],
  );

  useEffect(() => {
    if (hasMore && sentinelElRef.current) {
      requestAnimationFrame(() => checkAndLoad());
    }
  }, [hasMore, checkAndLoad]);

  useEffect(() => {
    return () => {
      const scroller = scrollerElRef.current;
      if (scroller) {
        scroller.removeEventListener('scroll', checkAndLoad);
      } else if (sentinelElRef.current) {
        window.removeEventListener('scroll', checkAndLoad);
      }
      sentinelElRef.current = null;
      scrollerElRef.current = null;
    };
  }, [checkAndLoad]);

  return { sentinelRef, loading };
}
```

- [ ] **Step 2: Write a smoke test**

The geometry is DOM-bound (jsdom has no layout), so test that the hook mounts, returns the API, and does NOT call `loadMore` when `hasMore` is false. Create `src/lib/useInfiniteScroll.test.tsx`:

```tsx
// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { useInfiniteScroll } from './useInfiniteScroll';

function Harness({ hasMore, loadMore }: { hasMore: boolean; loadMore: () => void }) {
  const { sentinelRef } = useInfiniteScroll({ hasMore, loadMore });
  return <div ref={sentinelRef}>sentinel</div>;
}

describe('useInfiniteScroll', () => {
  it('mounts and renders the sentinel without crashing', () => {
    const loadMore = vi.fn();
    const { getByText } = render(<Harness hasMore={false} loadMore={loadMore} />);
    expect(getByText('sentinel')).toBeInTheDocument();
  });

  it('does not call loadMore when hasMore is false', async () => {
    const loadMore = vi.fn();
    render(<Harness hasMore={false} loadMore={loadMore} />);
    // Give the rAF check a tick.
    await new Promise((r) => setTimeout(r, 20));
    expect(loadMore).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 3: Verify**

Run: `npm test -- --run useInfiniteScroll` → 2 pass. `npm run build` → strict exit 0.

- [ ] **Step 4: Commit**

```bash
git add src/lib/useInfiniteScroll.ts src/lib/useInfiniteScroll.test.tsx
git commit -m "feat(cp/web): useInfiniteScroll hook (ported from Okesu)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: API params + usage types

**Files:**
- Modify: `src/lib/api.ts`

Add `{ offset?, limit? }` to the growable list methods and `usage` to the four new-row types.

- [ ] **Step 1: A shared list-params type + query builder**

Near the top of the `api` object (or just above it), add a helper type and use `URLSearchParams` (mirroring `getUsage` from A.3):

```ts
export interface ListParams {
  offset?: number;
  limit?: number;
}

function listQuery(params?: ListParams): string {
  const q = new URLSearchParams();
  if (params?.offset != null) q.set('offset', String(params.offset));
  if (params?.limit != null) q.set('limit', String(params.limit));
  const qs = q.toString();
  return qs ? `?${qs}` : '';
}
```

- [ ] **Step 2: Thread params into the growable list methods**

Update these methods to accept `params?: ListParams` and append `listQuery(params)`:

```ts
  getRuns(params?: ListParams): Promise<RunListRow[]> {
    return request<RunListRow[]>(`/api/runs${listQuery(params)}`);
  },
  getWorkflowRuns(params?: ListParams): Promise<RunListRow[]> {
    return request<RunListRow[]>(`/api/runs/workflows${listQuery(params)}`);
  },
  getAutoflowRuns(params?: ListParams): Promise<AutoflowCycleRow[]> {
    return request<AutoflowCycleRow[]>(`/api/runs/autoflows${listQuery(params)}`);
  },
  getAutoflowEvents(params?: ListParams): Promise<AutoflowEventRow[]> {
    return request<AutoflowEventRow[]>(`/api/runs/autoflows/events${listQuery(params)}`);
  },
  getAgentRuns(params?: ListParams): Promise<AgentRunRow[]> {
    return request<AgentRunRow[]>(`/api/runs/agents${listQuery(params)}`);
  },
  getSessions(params?: ListParams): Promise<SessionSummary[]> {
    return request<SessionSummary[]>(`/api/sessions${listQuery(params)}`);
  },
  getProjectRuns(wsId: string, params?: ListParams): Promise<RunListRow[]> {
    return request<RunListRow[]>(`/api/projects/${encodeURIComponent(wsId)}/runs${listQuery(params)}`);
  },
  getProjectSessions(wsId: string, params?: ListParams): Promise<SessionSummary[]> {
    return request<SessionSummary[]>(`/api/projects/${encodeURIComponent(wsId)}/sessions${listQuery(params)}`);
  },
```

(Match the exact existing method bodies — only add the param + query suffix. Keep the other methods unchanged.)

- [ ] **Step 3: Add `usage` to the four DTOs**

- `AgentRunRow` — add `usage: UsageSummary;`
- `AutoflowEventRow` — add `usage: UsageSummary;`
- `AutoflowCycleRow` — add `usage: UsageSummary;`
- `DashboardResponse.recent_runs` element — add `usage: UsageSummary;` (the array's inline object type)

(`UsageSummary` is already imported in `api.ts` from A.3.)

- [ ] **Step 4: Test + build**

Add to `src/lib/api.test.ts` (mirror the existing fetch-mock pattern) one test that `getRuns({ offset: 20, limit: 20 })` requests `/api/runs?offset=20&limit=20`.

Run: `npm test -- --run api` → green. `npm run build` → strict exit 0.

- [ ] **Step 5: Commit**

```bash
git add src/lib/api.ts src/lib/api.test.ts
git commit -m "feat(cp/web): list pagination params + usage on agent/autoflow/recent rows

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Usage chips on the new rows

**Files:**
- Modify: `src/pages/runs/AgentRuns.tsx`, `src/pages/Dashboard.tsx`, `src/pages/runs/AutoflowRuns.tsx`

Render `UsageChip` on the rows that now carry `usage`.

- [ ] **Step 1: AgentRuns**

In `src/pages/runs/AgentRuns.tsx`, add the import `import UsageChip from '../../components/UsageChip';`. Find the A.3 placeholder comment (`{/* Per-run token/cost: AgentRunRow has no usage field … */}`) in `AgentRunEntry` and replace it with the chip, using the row's variable (e.g. `run`):

```tsx
<UsageChip usage={run.usage} className="ml-2" />
```

- [ ] **Step 2: Dashboard recent runs**

In `src/pages/Dashboard.tsx` `RecentRunRow`, add `import UsageChip from '../components/UsageChip';` and render the chip next to the run meta:

```tsx
<UsageChip usage={run.usage} className="ml-2" />
```

(The `run` here is `DashboardResponse['recent_runs'][number]`, which now has `usage`.)

- [ ] **Step 3: AutoflowRuns (events + cycles)**

In `src/pages/runs/AutoflowRuns.tsx`, add `import UsageChip from '../../components/UsageChip';`. In `AutoflowEventItem`, render the chip in the row meta (the event's `usage` is `UsageSummary::default()` / zeros when it has no run — that's fine, it renders `0 tok · $0.00`):

```tsx
<UsageChip usage={event.usage} className="ml-2" />
```

In `AutoflowCycleItem`, render the rolled-up cycle usage in the cycle's stats cluster:

```tsx
<UsageChip usage={cycle.usage} className="ml-2" />
```

(Use the actual loop/prop variable names for each item.)

- [ ] **Step 4: Verify**

Run: `npm run build` → strict exit 0. `npm test -- --run` → green.

- [ ] **Step 5: Commit**

```bash
git add src/pages/runs/AgentRuns.tsx src/pages/Dashboard.tsx src/pages/runs/AutoflowRuns.tsx
git commit -m "feat(cp/web): usage chips on agent / autoflow / dashboard-recent rows

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## The growable list pattern (reference for Tasks 4 & 5)

Each growable page accumulates pages into an array and appends on scroll. The canonical shape (adapt variable names + the fetch call per page):

```tsx
import { useInfiniteScroll } from '../lib/useInfiniteScroll'; // depth: ../ or ../../

const PAGE = 20;
// state
const [items, setItems] = useState<Row[] | null>(null);
const [hasMore, setHasMore] = useState(true);
const [error, setError] = useState<string | null>(null);

// page-0 fetch — used on mount AND as the periodic refresh (resets pagination)
const refresh = useCallback(async () => {
  try {
    const page = await api.list({ limit: PAGE });   // the page's fetch
    setItems(page);
    setHasMore(page.length >= PAGE);
    setError(null);
  } catch (e) {
    setError(e instanceof Error ? e.message : 'Failed to load');
  }
}, [/* filter deps */]);

useEffect(() => {
  void refresh();
  const t = window.setInterval(() => void refresh(), 15_000);
  return () => window.clearInterval(t);
}, [refresh]);

const loadMore = async () => {
  const current = items ?? [];
  const next = await api.list({ offset: current.length, limit: PAGE });
  if (next.length === 0) { setHasMore(false); return; }
  setItems([...current, ...next]);
  if (next.length < PAGE) setHasMore(false);
};

const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

// render: the existing list/grouping over `items`, then the sentinel as the LAST child
{items && items.length > 0 && (
  <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
    {loading ? 'loading more…' : hasMore ? 'scroll for more' : `— end of ${items.length} —`}
  </div>
)}
```

Notes:
- If a page **groups** rows (e.g. WorkflowRuns Active/Completed/Failed), group the accumulated `items` for render and put the single sentinel after the last group.
- Keep each page's existing filter UI; filters belong in `refresh`'s deps (changing a filter re-fetches page 0).
- The list must sit inside an `overflow-auto` ancestor (most pages already do; the hook finds it).

---

## Task 4: Infinite-scroll on the run-stream pages

**Files:**
- Modify: `src/pages/runs/WorkflowRuns.tsx`, `src/pages/runs/AgentRuns.tsx`, `src/pages/runs/AutoflowRuns.tsx`

Apply the growable pattern (import depth `../../lib/useInfiniteScroll`).

- [ ] **Step 1: WorkflowRuns**

READ `src/pages/runs/WorkflowRuns.tsx`. It fetches `api.getWorkflowRuns()` into a `runs` state and groups by lifecycle. Convert to the growable pattern: page-0 via `getWorkflowRuns({ limit: 20 })`, `loadMore` appends `getWorkflowRuns({ offset: runs.length, limit: 20 })`, group the accumulated `runs` for render, sentinel after the last group. Preserve the trigger filter (it's a client-side filter over the fetched set — keep it, but note the filter now applies to the loaded pages; that's acceptable).

- [ ] **Step 2: AgentRuns**

READ `src/pages/runs/AgentRuns.tsx`. Convert its `getAgentRuns()` fetch to the growable pattern (`offset = rows.length`). Sentinel after the list.

- [ ] **Step 3: AutoflowRuns (two lists)**

READ `src/pages/runs/AutoflowRuns.tsx`. It renders TWO lists: cycles (`getAutoflowRuns`) and events (`getAutoflowEvents`). Apply the growable pattern to the **primary** list shown (the events feed it leads with). For the secondary list, either apply the same pattern with its own state+sentinel OR (simpler) keep it at the first page of 20 — pick based on the page's layout; if both are full lists, give each its own `useInfiniteScroll` instance + sentinel. Note in your report which lists you wired.

- [ ] **Step 4: Verify**

Run: `npm run build` → strict exit 0. `npm test -- --run` → green.

- [ ] **Step 5: Commit**

```bash
git add src/pages/runs/WorkflowRuns.tsx src/pages/runs/AgentRuns.tsx src/pages/runs/AutoflowRuns.tsx
git commit -m "feat(cp/web): infinite-scroll on the run-stream pages

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Infinite-scroll on Sessions + project lists

**Files:**
- Modify: `src/pages/Sessions.tsx`, `src/pages/ProjectRuns.tsx`, `src/pages/ProjectSessions.tsx`

Apply the growable pattern (import depth `../lib/useInfiniteScroll`).

- [ ] **Step 1: Sessions**

READ `src/pages/Sessions.tsx`. Convert `getSessions()` to the growable pattern (`getSessions({ offset, limit: 20 })`). Sentinel after the list.

- [ ] **Step 2: ProjectRuns**

READ `src/pages/ProjectRuns.tsx`. Convert `getProjectRuns(wsId)` to `getProjectRuns(wsId, { offset, limit: 20 })` with the growable pattern.

- [ ] **Step 3: ProjectSessions**

READ `src/pages/ProjectSessions.tsx`. Convert `getProjectSessions(wsId)` to `getProjectSessions(wsId, { offset, limit: 20 })` with the growable pattern.

- [ ] **Step 4: Verify**

Run: `npm run build` → strict exit 0. `npm test -- --run` → green.

- [ ] **Step 5: Commit**

```bash
git add src/pages/Sessions.tsx src/pages/ProjectRuns.tsx src/pages/ProjectSessions.tsx
git commit -m "feat(cp/web): infinite-scroll on sessions + project lists

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## The bounded list pattern (reference for Task 6)

Bounded lists already fetch a small full set; just render 20 and widen on scroll (no extra fetch):

```tsx
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;
const [visible, setVisible] = useState(STEP);
// `items` is the already-fetched full array
const shown = (items ?? []).slice(0, visible);
const { sentinelRef, loading } = useInfiniteScroll({
  hasMore: visible < (items?.length ?? 0),
  loadMore: () => setVisible((v) => v + STEP),
});

// render `shown` instead of `items`, then the sentinel as the last child:
{items && items.length > visible && (
  <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
    {loading ? '…' : 'scroll for more'}
  </div>
)}
```

(Reset `visible` to `STEP` when the underlying list is re-fetched/filtered, e.g. inside the load effect.)

---

## Task 6: Client-window the bounded list pages

**Files:**
- Modify: `src/pages/Agents.tsx`, `src/pages/Workflows.tsx`, `src/pages/Workers.tsx`, `src/pages/Projects.tsx`, `src/pages/AutoflowsDefs.tsx`, `src/pages/Coverage.tsx`

Apply the bounded pattern (import depth `../lib/useInfiniteScroll`). Each page already fetches its full list; render `slice(0, visible)` and widen on scroll.

- [ ] **Step 1: Apply to each page**

For EACH of the six pages: READ it, find where it renders the fetched array, introduce `const [visible, setVisible] = useState(20)`, render `items.slice(0, visible)`, add the `useInfiniteScroll` widening hook + a sentinel after the list. Reset `visible` to 20 wherever the list is re-fetched/refreshed. If a page is trivially short (e.g. Workers is usually a handful), still apply the pattern for consistency — it's harmless (the sentinel just never appears when `length <= 20`).

(If any of these pages don't render a flat list — e.g. Coverage groups targets — window the primary list and note it.)

- [ ] **Step 2: Verify**

Run: `npm run build` → strict exit 0. `npm test -- --run` → green.

- [ ] **Step 3: Commit**

```bash
git add src/pages/Agents.tsx src/pages/Workflows.tsx src/pages/Workers.tsx src/pages/Projects.tsx src/pages/AutoflowsDefs.tsx src/pages/Coverage.tsx
git commit -m "feat(cp/web): client-window bounded list pages (render 20, widen on scroll)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Whole-slice build gate + visual handoff

**Files:** none (verification)

- [ ] **Step 1: Strict build + full suite**

Run: `npm run build && npm test -- --run`. Paste the test counts + the main `index-*.js` chunk size (must stay ~48 KB; recharts/markdown stay lazy — `grep -c recharts dist/assets/index-*.js` → 0).

- [ ] **Step 2: No `any` / static Tailwind audit**

Run: `grep -rn ": any\|as any\| bg-\${\| text-\${" src/lib/useInfiniteScroll.ts src/pages/runs/ src/pages/Sessions.tsx src/pages/Projects.tsx` → none.

- [ ] **Step 3: Visual validation checklist (matt runs the app)**

Hand off with this checklist:
- Every list initially shows ≤20 rows; scrolling to the bottom loads/renders more (smooth, no jump, no double-load).
- Agent Runs rows show `· N tok · $X`; Dashboard recent rows + autoflow cycle/event rows show usage.
- An end-of-list sentinel reads `— end of N —` when fully loaded; "loading more…" while fetching.
- Filters/refresh reset to the first 20.

---

## Done criteria (whole plan)

- `npm run build` strict-clean; main chunk ~48 KB.
- `npm test -- --run` green (hook smoke + api param test).
- No `any`; static Tailwind only.
- Every list view renders ≤20 then lazily loads (growable) or widens (bounded) on scroll, page size 20.
- Usage chips on Agent Runs, Dashboard recent, autoflow cycles + events.
