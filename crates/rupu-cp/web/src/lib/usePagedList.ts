// usePagedList — the shared fetch/paginate/poll state machine, replacing the
// ~60 lines of it duplicated per list page (WorkflowRuns, AgentRuns,
// AutoflowRuns, Sessions, ProjectRunsTab, ProjectSessionsTab). Page size 20,
// infinite-scroll sentinel via the existing `useInfiniteScroll`, an `ended`
// flag for the "— end of N —" footer, and an opt-in 5s poll that refreshes
// ONLY the first page (so an already-scrolled-down list doesn't jump).

import { useCallback, useEffect, useRef, useState } from 'react';
import { useInfiniteScroll } from './useInfiniteScroll';

const PAGE = 20;

export interface UsePagedListOptions<T> {
  fetch: (p: { offset: number; limit: number }) => Promise<T[]>;
  /** Reactive trigger for "start over" (filters, host, tab, …). Compared by
   *  INDEX below, not by array identity — see the note on `gen` below. */
  deps: unknown[];
  /** When true, a 5s interval silently re-fetches page 0 and splices it back
   *  in over the existing head of `rows`. Off by default. */
  poll?: boolean;
}

export interface UsePagedListResult<T> {
  rows: T[];
  loading: boolean;
  error: string | null;
  hasMore: boolean;
  sentinelRef: (el: HTMLDivElement | null) => void;
  refresh: () => void;
  ended: boolean;
}

export function usePagedList<T>({
  fetch,
  deps,
  poll = false,
}: UsePagedListOptions<T>): UsePagedListResult<T> {
  const [rows, setRows] = useState<T[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const [ended, setEnded] = useState(false);

  // `fetch` closes over the caller's live filter state, so it's a fresh
  // function identity almost every render. Depending on it directly in an
  // effect (or even in a useCallback dep array feeding an effect) reintroduces
  // the fresh-object dep loop that has bitten this codebase twice before —
  // so it is READ via a ref at call time, never listed as a reactive
  // dependency. The only reactive trigger is `deps`, and even THAT is not
  // used by array identity (callers pass a fresh `[a, b, c]` literal every
  // render) — it's compared below index-by-index into a stable `gen` number.
  const fetchRef = useRef(fetch);
  fetchRef.current = fetch;

  const offsetRef = useRef(0);
  const rowsRef = useRef<T[]>([]);

  const prevDepsRef = useRef<unknown[]>([]);
  const genRef = useRef(0);
  const depsChanged =
    deps.length !== prevDepsRef.current.length ||
    deps.some((d, i) => !Object.is(d, prevDepsRef.current[i]));
  if (depsChanged) {
    prevDepsRef.current = deps;
    genRef.current += 1;
  }
  const gen = genRef.current;

  const loadPage = useCallback(async (offset: number, replace: boolean): Promise<T[]> => {
    const page = await fetchRef.current({ offset, limit: PAGE });
    rowsRef.current = replace ? page : [...rowsRef.current, ...page];
    setRows(rowsRef.current);
    offsetRef.current = offset + page.length;
    const full = page.length === PAGE;
    setHasMore(full);
    setEnded(!full);
    return page;
  }, []);

  const loadFirstPage = useCallback(() => {
    setLoading(true);
    setError(null);
    return loadPage(0, true)
      .catch((e: unknown) => {
        setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => setLoading(false));
  }, [loadPage]);

  // Reset + reload from scratch whenever `deps` changes (by-index compare
  // above, surfaced as the stable primitive `gen`).
  useEffect(() => {
    offsetRef.current = 0;
    rowsRef.current = [];
    setRows([]);
    setHasMore(true);
    setEnded(false);
    void loadFirstPage();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `gen` IS the
    // deps-changed signal; `loadFirstPage` is stable-enough (see above) and
    // must not be listed or this fires every render.
  }, [gen]);

  const loadMore = useCallback(() => {
    return loadPage(offsetRef.current, false).catch((e: unknown) => {
      setError(e instanceof Error ? e.message : String(e));
    });
  }, [loadPage]);

  // `useInfiniteScroll` fires its rAF-driven `checkAndLoad()` as soon as the
  // sentinel mounts, gated only on ITS OWN internal loading ref — it has no
  // idea an initial/reset fetch (`loadFirstPage`, driven by `loading` here)
  // is already in flight. Passing raw `hasMore` in means a consumer that
  // mounts the sentinel while page 0 is still loading (any real network
  // latency) fires a SECOND concurrent offset-0 fetch, which appends
  // (replace:false) on top of the first and permanently duplicates page 0 —
  // it never self-corrects. Gating on `!loading` too closes that window;
  // consumers MAY mount the sentinel unconditionally (even before the first
  // page resolves) — the hook is safe either way.
  const { sentinelRef } = useInfiniteScroll({ hasMore: hasMore && !loading, loadMore });

  // Opt-in 5s poll of page 0 only. Splices the fresh head back over
  // `rowsRef.current` without touching anything the user has already
  // scrolled to load beyond it. Poll failures are swallowed — a background
  // refresh going stale for one tick shouldn't blank the list or throw up an
  // error banner over data the operator is actively looking at.
  useEffect(() => {
    if (!poll) return;
    const id = setInterval(() => {
      fetchRef
        .current({ offset: 0, limit: PAGE })
        .then((page0) => {
          rowsRef.current = [...page0, ...rowsRef.current.slice(page0.length)];
          setRows(rowsRef.current);
        })
        .catch(() => {
          /* swallow — keep showing the last good page */
        });
    }, 5000);
    return () => clearInterval(id);
    // `gen` restarts the poll cadence in sync with a filter reset (harmless
    // either way, but avoids a poll tick racing the reset fetch above).
  }, [poll, gen]);

  const refresh = useCallback(() => {
    void loadFirstPage();
  }, [loadFirstPage]);

  return { rows, loading, error, hasMore, sentinelRef, refresh, ended };
}

export default usePagedList;
