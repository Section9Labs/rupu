// useUsageData — the /usage headline, loaded PER HOST (spec 2026-10-01 §6.5).
//
// Same shape as useDashboardData: `/api/hosts/registered` seeds a
// `loading` entry per host, then each host's `/api/usage?host=<id>` fires
// independently. The headline is the merge (mergeUsage) over hosts whose
// last good answer is for the CURRENT window (`windowKey`), so a slow
// host's figure for an old window never mixes into a new one. Each
// response's own `hosts[0]` entry is authoritative: a 200 for a down host is
// not proof of health.
//
// `group_by` is pinned to `model` (the server default; every transport,
// SSH included, can answer it). The page never reads `breakdown`, so a pivot
// change no longer refetches, and SSH hosts stay in the headline under
// every pivot.
//
// Cadence: local follows the page's 30 s preset tick; remotes refetch
// every 60 s while visible, on focus, and immediately on a user window
// change. The 60 s poll and the focus refetch leave a host whose request is
// still in flight alone (restarting it would mean a host slower than the poll
// never answers); a user window change and the tick supersede it on purpose.
//
// A host that answered for window A and then fails its refetch for window B
// keeps its old answer (stale-on-error) but is EXCLUDED from B's headline. With
// no request left in flight it counts as failed for B: shown `offline` in the
// strip, labelled `(stale)` in `excluded`, and counted toward `error` -
// otherwise a page whose every host ended up that way would read `loading`
// forever. While its refetch IS in flight it is `loading`.
//
// Abandoned requests are aborted: a host's newer request aborts its previous
// one, and unmount aborts everything still in flight. A hung remote would
// otherwise hold one of the browser's six connections per origin for as long as
// it lives, and enough of them starve every other request the page makes.

import { useEffect, useMemo, useReducer, useRef, useState } from 'react';
import { api, apiErrorMessage, type UsageResponse, type UsageWindow } from '../api';
import type { HostFreshnessEntry } from '../../components/dashboard/HostFreshnessStrip';
import type { HostSeed } from '../perHost/types';
import { classifyFailure } from '../perHost/status';
import { mergeUsage, type MergedUsage } from './mergeUsage';

export const USAGE_REMOTE_POLL_MS = 60_000;

interface HostUsage {
  hostId: string;
  name: string;
  transportKind: string;
  state: 'loading' | 'ok' | 'offline' | 'unavailable';
  response: UsageResponse | null;
  /** The window key `response` was fetched for. */
  windowKey: string | null;
  reason: string | null;
  receivedAt: number | null;
}

export interface UseUsageDataResult {
  data: (MergedUsage & { excluded: string[] }) | null;
  hosts: HostFreshnessEntry[];
  error: Error | null;
}

const seedOf = (h: HostSeed): HostUsage => ({
  hostId: h.id,
  name: h.name,
  transportKind: h.transport_kind,
  state: 'loading',
  response: null,
  windowKey: null,
  reason: null,
  receivedAt: null,
});

export function useUsageData(usageWindow: UsageWindow, windowKey: string, windowSource: 'user' | 'tick'): UseUsageDataResult {
  const [hosts, setHosts] = useState<HostUsage[]>([]);
  const [listError, setListError] = useState<Error | null>(null);
  const windowRef = useRef(usageWindow);
  windowRef.current = usageWindow;
  const keyRef = useRef(windowKey);
  keyRef.current = windowKey;
  const statesRef = useRef<HostUsage[]>([]);
  statesRef.current = hosts;
  /** Latest request id per host: an older, slower answer never overwrites a newer one. */
  const seqRef = useRef(new Map<string, number>());
  /** The in-flight request's controller per host; the next request for the host aborts it. */
  const controllersRef = useRef(new Map<string, AbortController>());
  /** Set on unmount: a handler that runs after it must not touch state. */
  const disposedRef = useRef(false);
  /**
   * Whether a request is in flight is read from `controllersRef` while rendering, so a
   * request STARTING (which changes no host state) bumps this to re-render. Settling
   * needs no bump: the handlers' `setHosts` already re-renders.
   */
  const [inflightRev, bumpInflight] = useReducer((n: number) => n + 1, 0);

  const fetchHost = useRef((hostId: string) => {
    if (disposedRef.current) return;
    const seq = (seqRef.current.get(hostId) ?? 0) + 1;
    // Bump the seq BEFORE aborting: the aborted request's rejection then sees itself superseded.
    seqRef.current.set(hostId, seq);
    controllersRef.current.get(hostId)?.abort();
    const controller = new AbortController();
    controllersRef.current.set(hostId, controller);
    bumpInflight();
    const key = keyRef.current;
    const update = (f: (h: HostUsage) => HostUsage) =>
      setHosts((prev) => prev.map((h) => (h.hostId === hostId ? f(h) : h)));
    const settled = () => {
      if (controllersRef.current.get(hostId) === controller) controllersRef.current.delete(hostId);
    };
    api.getUsage(windowRef.current, 'model', hostId, controller.signal).then(
      (resp) => {
        if (disposedRef.current || seqRef.current.get(hostId) !== seq) return;
        settled();
        const wire = resp.hosts.find((h) => h.host_id === hostId);
        update((h) => {
          if (!wire) return { ...h, state: 'unavailable', response: null, reason: 'host missing from response' };
          if (wire.state !== 'ok') return { ...h, state: wire.state, response: null, reason: wire.reason };
          return { ...h, state: 'ok', response: resp, windowKey: key, reason: null, receivedAt: Date.now() };
        });
      },
      (e: unknown) => {
        // First check, always: an aborted or superseded request must never mark its host offline.
        if (disposedRef.current || seqRef.current.get(hostId) !== seq) return;
        settled();
        const f = classifyFailure(e);
        if (f.kind === 'gone' && hostId !== 'local') {
          // The host is no longer registered: drop it, as the list engine does. Local is never
          // dropped; it falls through and shows as offline.
          setHosts((prev) => prev.filter((h) => h.hostId !== hostId));
          return;
        }
        update((h) =>
          h.response
            ? { ...h, reason: f.reason } // stale-on-error: keep last good
            : { ...h, state: f.kind === 'unavailable' ? 'unavailable' : 'offline', reason: f.reason },
        );
      },
    );
  }).current;

  const fetchWhere = (pred: (h: HostUsage) => boolean) => {
    for (const h of statesRef.current) if (pred(h)) fetchHost(h.hostId);
  };

  // Unmount: stop waiting on everything still in flight. Declared before the effects that
  // fetch, so a StrictMode remount re-arms `disposed` before they run again.
  useEffect(() => {
    disposedRef.current = false;
    const controllers = controllersRef.current;
    return () => {
      disposedRef.current = true;
      for (const c of controllers.values()) c.abort();
      controllers.clear();
    };
  }, []);

  // Bootstrap once: the host list, then every host.
  useEffect(() => {
    let cancelled = false;
    api.getRegisteredHosts().then(
      (hs) => {
        if (cancelled) return;
        setHosts(hs.map(seedOf));
        for (const h of hs) fetchHost(h.id);
      },
      (e: unknown) => {
        if (cancelled) return;
        setListError(new Error(`Could not list hosts (${apiErrorMessage(e)}); showing this host only.`));
        setHosts([seedOf({ id: 'local', name: 'Local', transport_kind: 'local' })]);
        fetchHost('local');
      },
    );
    return () => {
      cancelled = true;
    };
  }, [fetchHost]);

  // A user window change (preset button, drag-select, clear): every host, now.
  const firstKey = useRef(true);
  useEffect(() => {
    if (firstKey.current) {
      firstKey.current = false;
      return;
    }
    fetchWhere(() => true);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed on the window identity only
  }, [windowKey]);

  // The 30 s preset tick: local only.
  useEffect(() => {
    if (windowSource === 'tick') fetchWhere((h) => h.hostId === 'local');
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed on the tick-moved `until`
  }, [usageWindow.until, windowSource]);

  // Remote cadence: 60 s while visible, plus on focus (unavailable hosts wait for a reload).
  useEffect(() => {
    // A request still in flight is left alone: restarting it would mean a host slower than
    // the poll never answers.
    const idle = (h: HostUsage) => !controllersRef.current.has(h.hostId);
    const remote = (h: HostUsage) => h.hostId !== 'local' && h.state !== 'unavailable' && idle(h);
    const t = setInterval(() => {
      if (document.visibilityState === 'visible') fetchWhere(remote);
    }, USAGE_REMOTE_POLL_MS);
    const onVisible = () => {
      if (document.visibilityState === 'visible') fetchWhere((h) => h.state !== 'unavailable' && idle(h));
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      clearInterval(t);
      document.removeEventListener('visibilitychange', onVisible);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- fetchWhere reads refs
  }, []);

  const current = (h: HostUsage) => h.state === 'ok' && h.response !== null && h.windowKey === windowKey;
  /**
   * Answered for another window, then failed its refetch for this one, with nothing left in
   * flight: it has failed for the current window, whatever stale data it still holds.
   */
  const staleFailed = (h: HostUsage) =>
    h.state === 'ok' && h.windowKey !== windowKey && h.reason != null && !controllersRef.current.has(h.hostId);

  const data = useMemo(() => {
    const ok = hosts.filter(current);
    if (ok.length === 0) return null;
    const excluded = hosts
      .filter((h) => !current(h))
      .map((h) => `${h.name} (${h.state === 'ok' ? (staleFailed(h) ? 'stale' : 'loading') : h.state})`);
    return { ...mergeUsage(ok.map((h) => h.response as UsageResponse)), excluded };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `current`/`staleFailed` close over windowKey and the in-flight map (inflightRev)
  }, [hosts, windowKey, inflightRev]);

  const entries: HostFreshnessEntry[] = hosts.map((h) => ({
    host_id: h.hostId,
    name: h.name,
    transport_kind: h.transportKind,
    state: staleFailed(h) ? 'offline' : h.state === 'ok' && h.windowKey !== windowKey ? 'loading' : h.state,
    captured_at: current(h) && h.receivedAt != null ? new Date(h.receivedAt).toISOString() : null,
    reason: h.reason,
  }));

  const allFailed =
    hosts.length > 0 && hosts.every((h) => h.state === 'offline' || h.state === 'unavailable' || staleFailed(h));
  const error =
    listError ?? (allFailed ? new Error(hosts.map((h) => `${h.name}: ${h.reason ?? h.state}`).join(' · ')) : null);

  return { data, hosts: entries, error };
}
