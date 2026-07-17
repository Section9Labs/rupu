// useDashboardData — live dashboard state.
//
// SSE is an INVALIDATION SIGNAL, not a data channel. Every number on the
// dashboard is a server-computed aggregate; the event stream carries step-level
// events. Applying step deltas to aggregates client-side would mean
// reimplementing the Rust aggregation in TypeScript and keeping the two in
// agreement forever — they would drift, and a dashboard quietly showing WRONG
// counts is worse than one that is 10s stale.
//
// So: event arrival marks dirty, and we refetch the aggregate. The server stays
// the single source of truth for every number.
//
// Note the stream is LOCAL-ONLY: /api/events/stream requires ?run= for any
// remote host, and there is no cross-host firehose. Remote hosts therefore
// refresh on the reconciling poll, which is why per-host freshness is rendered
// rather than one global "live" pill (spec §5.4).

import { useCallback, useEffect, useRef, useState } from 'react';
import { api, type DashboardRange, type DashboardResponse } from '../api';

/** Burst window. An autoflow cycle firing 12 runs must cost ONE refetch. */
const COALESCE_MS = 250;

/**
 * Reconciling poll. Runs regardless of SSE so a dropped connection degrades to
 * the old behavior instead of freezing.
 */
const RECONCILE_MS = 60_000;

/**
 * Collapse a burst of triggers into a single trailing call.
 *
 * Exported for testing — this is the piece that decides whether the page feels
 * fast or hammers the server.
 */
export function coalesce(fn: () => void, ms: number): { trigger: () => void; cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return {
    trigger() {
      if (timer !== null) return; // a call is already pending — fold into it
      timer = setTimeout(() => {
        timer = null;
        fn();
      }, ms);
    },
    cancel() {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
    },
  };
}

export function useDashboardData(range: DashboardRange) {
  const [data, setData] = useState<DashboardResponse | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [loading, setLoading] = useState(true);

  // Held in a ref so the SSE subscription and poll never re-subscribe just
  // because `range` changed identity.
  const rangeRef = useRef(range);
  rangeRef.current = range;

  const refresh = useCallback(async () => {
    try {
      const d = await api.getDashboard(rangeRef.current);
      setData(d);
      setError(null);
    } catch (e) {
      // Keep stale data on a transient error rather than flashing an error
      // state — a 10s-old number beats an empty page.
      setError(e instanceof Error ? e : new Error(String(e)));
    } finally {
      setLoading(false);
    }
  }, []);

  // Refetch immediately whenever the range changes.
  useEffect(() => {
    setLoading(true);
    void refresh();
  }, [range, refresh]);

  useEffect(() => {
    const { trigger, cancel } = coalesce(() => void refresh(), COALESCE_MS);

    // Payloads are deliberately ignored — arrival is the whole signal.
    const unsubscribe = api.subscribeEvents(() => trigger());

    const poll = setInterval(() => {
      // A dashboard in a background tab does no work.
      if (document.visibilityState === 'visible') void refresh();
    }, RECONCILE_MS);

    // Refetch on tab focus so returning to a backgrounded tab is never stale.
    const onVisible = () => {
      if (document.visibilityState === 'visible') trigger();
    };
    document.addEventListener('visibilitychange', onVisible);

    return () => {
      cancel();
      unsubscribe();
      clearInterval(poll);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [refresh]);

  return { data, error, loading, refresh };
}
