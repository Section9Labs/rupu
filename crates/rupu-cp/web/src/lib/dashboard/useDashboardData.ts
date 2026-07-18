// useDashboardData — live dashboard state, loaded PER HOST and async.
//
// The load-time fix this hook exists for: the dashboard used to issue ONE
// `/api/dashboard` call that fanned out server-side and AWAITED every host —
// including an unreachable SSH host — so the whole page blocked on the
// slowest host (~10s) when `?host=local` alone answers in ~0.26s. Operator
// directive: "load things as you get them, do not lock on waiting for remote
// things."
//
// The fix: `getRegisteredHosts()` seeds a per-host slice (state 'loading'),
// so the freshness strip can render immediately. Then `getDashboard(range,
// hostId)` fires INDEPENDENTLY for every host — no `Promise.all`, no shared
// await — and each call's own `.then`/`.catch` flips ONLY that host's slice.
// A hung remote promise therefore never delays local's paint, nor any other
// host's.
//
// The merged `data` view is `mergeSummaries` (mergeSummaries.ts) recomputed
// over whichever hosts are currently `state: 'ok'`. Combining
// already-correct per-host summaries is safe; deriving numbers from raw
// events here would mean reimplementing the Rust aggregation in TypeScript
// and keeping the two in agreement forever.
//
// SSE remains an INVALIDATION SIGNAL, not a data channel — arrival marks
// local dirty and triggers a refetch of `?host=local` ONLY. There is no
// cross-host firehose (`/api/events/stream` requires `?run=` for a remote
// host), so remote hosts refresh only on the visibility-gated reconciling
// poll, which is why per-host freshness is rendered instead of one global
// "live" pill.
//
// `findings_partial` / `cycles_partial` are response-level flags, not part
// of `DashboardSummary` (mergeSummaries.ts's contract). This hook computes
// them directly from the same set of `state: 'ok'` summaries it feeds to
// `mergeSummaries`: true when at least one of those hosts contributed `null`
// for that field. That is the exact "not reported ≠ 0" rule the server
// applies — this hook just applies it at the same seam where it already has
// the per-host summaries in hand, rather than threading extra return values
// through the pure merge function.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, type DashboardRange, type DashboardSummary } from '../api';
import { mergeSummaries } from './mergeSummaries';

/** Burst window. An autoflow cycle firing 12 runs must cost ONE refetch. */
const COALESCE_MS = 250;

/**
 * Reconciling poll. Runs regardless of SSE so a dropped connection degrades
 * to polling instead of freezing. Refetches EVERY host (not just local) —
 * this is how remote hosts ever refresh, since there is no remote SSE.
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

/**
 * One host's loading/reporting state, as tracked by this hook.
 *
 * Deliberately NOT the wire `HostFreshness` type (`api.ts`): that type is
 * three-valued (`ok` | `offline` | `unavailable`) because the SERVER only
 * ever reports a host AFTER it has resolved. This hook needs a fourth state
 * — `loading` — for the window between "we know this host exists" (from
 * `getRegisteredHosts()`) and "this host's `getDashboard` call resolved".
 * The other three values are carried through VERBATIM from the matching
 * entry in that resolved response's `hosts` array (see `fetchOneHost`) —
 * a 200 response is not proof of health, `resp.hosts[].state` is.
 */
export interface DashboardHostState {
  hostId: string;
  name: string;
  transportKind: string;
  state: 'loading' | 'ok' | 'offline' | 'unavailable';
  summary?: DashboardSummary;
  /** Cause when `state !== 'ok'`; also set (without changing state) when a
   *  previously-`ok` host's refresh errors — see the stale-on-error note in
   *  `fetchOneHost`. */
  reason?: string | null;
}

/** The merged fleet-wide view: `mergeSummaries`'s output plus the two
 *  response-level partial flags (see the module doc comment for the split). */
export type MergedDashboard = DashboardSummary & {
  findings_partial: boolean;
  cycles_partial: boolean;
};

export function useDashboardData(range: DashboardRange) {
  const [hosts, setHosts] = useState<DashboardHostState[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  // Held in refs so callbacks never need to be recreated (and never race)
  // just because `range` changed identity or a render happened.
  const rangeRef = useRef(range);
  rangeRef.current = range;
  const hostIdsRef = useRef<string[]>([]);
  // Bumped on every bootstrap (mount + range change) so a fetch started by a
  // PREVIOUS bootstrap that resolves late can recognize it is stale and
  // no-op instead of clobbering a slice the current bootstrap owns.
  const genRef = useRef(0);

  const fetchOneHost = useCallback((hostId: string, gen: number) => {
    api.getDashboard(rangeRef.current, hostId).then(
      (resp) => {
        if (genRef.current !== gen) return; // superseded by a newer bootstrap
        // A resolved promise is NOT proof of health: the server answers a
        // down remote host with HTTP 200 and a zeroed summary whose own
        // `hosts[]` entry says `offline`/`unavailable`. That per-host entry
        // is authoritative — never assume `ok` just because the call
        // resolved. Only store `resp` as this host's summary when its own
        // wire entry confirms `ok`; otherwise drop the summary entirely so
        // its zeros/nulls (a fresh-looking `captured_at`, `findings_open:
        // null`, ...) never reach the merge, the freshness strip, or the
        // `findings_partial`/`cycles_partial` derivation below.
        const wireHost = resp.hosts.find((h) => h.host_id === hostId);
        setHosts((prev) =>
          prev.map((h) => {
            if (h.hostId !== hostId) return h;
            if (!wireHost) {
              // Should never happen — the host we just asked about is
              // missing from its own response. Fail closed rather than
              // trust a summary with no corroborating per-host entry.
              return { ...h, state: 'unavailable', summary: undefined, reason: 'host missing from response' };
            }
            if (wireHost.state !== 'ok') {
              // Authoritative and immediate: the server confirmed this host
              // is down, so this flips even a previously-`ok` host — unlike
              // the `.catch` stale-on-error path below, which keeps
              // last-good data on a mere failure to get an answer. Here we
              // HAVE the answer, and it's "down". Never paint a fresh
              // `captured_at` for it.
              return { ...h, state: wireHost.state, summary: undefined, reason: wireHost.reason };
            }
            return { ...h, state: 'ok', summary: resp, reason: null };
          }),
        );
      },
      (e: unknown) => {
        if (genRef.current !== gen) return;
        const err = e instanceof Error ? e : new Error(String(e));
        setHosts((prev) =>
          prev.map((h) => {
            if (h.hostId !== hostId) return h;
            // Stale-on-error: a host that already has data keeps showing it
            // rather than flipping to `unavailable` — a 10s-old number beats
            // an empty tile. Only a host that never had data flips state.
            if (h.state === 'ok') return { ...h, reason: err.message };
            return { ...h, state: 'unavailable', reason: err.message };
          }),
        );
        setError(err);
      },
    );
  }, []);

  const refreshAllHosts = useCallback(() => {
    const gen = genRef.current;
    for (const id of hostIdsRef.current) fetchOneHost(id, gen);
  }, [fetchOneHost]);

  const refreshLocalOnly = useCallback(() => {
    fetchOneHost('local', genRef.current);
  }, [fetchOneHost]);

  // Bootstrap: list registered hosts (a cheap, probe-free store read — see
  // `getRegisteredHosts`'s doc comment), seed a `loading` slice per host so
  // the freshness strip can render immediately, THEN fire every host's
  // `getDashboard` independently. Re-runs when `range` changes.
  useEffect(() => {
    genRef.current += 1;
    const gen = genRef.current;
    setLoading(true);
    setError(null);
    setHosts([]);
    hostIdsRef.current = [];

    api.getRegisteredHosts().then(
      (registered) => {
        if (genRef.current !== gen) return;
        const seeded: DashboardHostState[] = registered.map((h) => ({
          hostId: h.id,
          name: h.name,
          transportKind: h.transport_kind,
          state: 'loading',
        }));
        hostIdsRef.current = seeded.map((h) => h.hostId);
        setHosts(seeded);
        // The host list is known and the strip can render NOW — do not wait
        // on any individual host's getDashboard to resolve.
        setLoading(false);
        // Independent fire-and-forget per host. No Promise.all: a hung
        // promise for one host must never delay another's `.then` from
        // running, and none of them delay this loop from completing.
        for (const h of seeded) fetchOneHost(h.hostId, gen);
      },
      (e: unknown) => {
        if (genRef.current !== gen) return;
        setError(e instanceof Error ? e : new Error(String(e)));
        setLoading(false);
      },
    );
  }, [range, fetchOneHost]);

  // SSE invalidation (local-only) + visibility-gated reconciling poll (every
  // host). Stable across renders — `fetchOneHost`/`refreshLocalOnly`/
  // `refreshAllHosts` never change identity, and `range` is read via ref.
  useEffect(() => {
    const { trigger, cancel } = coalesce(refreshLocalOnly, COALESCE_MS);

    // Payloads are deliberately ignored — arrival is the whole signal.
    const unsubscribe = api.subscribeEvents(() => trigger());

    const poll = setInterval(() => {
      // A dashboard in a background tab does no work.
      if (document.visibilityState === 'visible') refreshAllHosts();
    }, RECONCILE_MS);

    // Refetch everything on tab focus so returning to a backgrounded tab is
    // never stale — remote hosts included, since they have no SSE channel.
    const onVisible = () => {
      if (document.visibilityState === 'visible') refreshAllHosts();
    };
    document.addEventListener('visibilitychange', onVisible);

    return () => {
      cancel();
      unsubscribe();
      clearInterval(poll);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [refreshLocalOnly, refreshAllHosts]);

  const okSummaries = useMemo(
    () =>
      hosts
        .filter((h): h is DashboardHostState & { summary: DashboardSummary } => h.state === 'ok' && !!h.summary)
        .map((h) => h.summary),
    [hosts],
  );

  const data: MergedDashboard | null = useMemo(() => {
    if (okSummaries.length === 0) return null;
    const merged = mergeSummaries(okSummaries);
    // The "not reported ≠ 0" rule, applied at the same seam where the
    // per-host summaries are already in hand (see module doc comment).
    const findings_partial = okSummaries.some((s) => s.findings_open === null);
    const cycles_partial = okSummaries.some((s) => s.cycles.clean === null || s.cycles.with_failures === null);
    return { ...merged, findings_partial, cycles_partial };
  }, [okSummaries]);

  return { data, hosts, loading, error, refresh: refreshAllHosts };
}
