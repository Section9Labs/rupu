// usePerHostPagedList — per-host progressive list loading for the Activity
// tables (spec docs/superpowers/specs/2026-10-01-rupu-cp-progressive-per-host-loading-design.md §6.3).
//
// `host: null` lists every registered host: `/api/hosts/registered` (a
// store read, no SSH), then one request per host, merged by the watermark
// rule. `host: '<id>'` is one slice, so a page has one code path whatever
// its filter. The engine (engine.ts) owns per-host state; this hook owns one
// engine per filter generation and the cadence timers:
//   local  — every 5 s on polling tables (unchanged)
//   remote — every 60 s while the tab is visible, plus on tab focus
//   manual — Refresh re-reads the host list and refreshes every host

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, apiErrorMessage } from '../api';
import { useInfiniteScroll } from '../useInfiniteScroll';
import { PerHostListEngine, type PerHostFetchParams } from './engine';
import { rowAccess, type HostSeed, type HostSlice, type HostTagged } from './types';
import { watermarkMerge } from './watermarkMerge';

export type { PerHostFetchParams } from './engine';

export const LOCAL_POLL_MS = 5_000;
export const REMOTE_POLL_MS = 60_000;

const LOCAL_SEED: HostSeed = { id: 'local', name: 'Local', transport_kind: 'local' };

export interface UsePerHostPagedListOptions<T> {
  /** One host id, or `null` for every registered host. */
  host: string | null;
  fetch: (p: PerHostFetchParams) => Promise<T[]>;
  timeField: keyof T & string;
  idField: keyof T & string;
  /** Reactive "start over" trigger, compared by index (same contract as usePagedList). */
  deps: unknown[];
  poll?: boolean;
}

export interface UsePerHostPagedListResult<T> {
  rows: T[];
  slices: HostSlice<T>[];
  /** True until the host list is known and at least one host has answered. */
  loading: boolean;
  /** Set when the host list could not be read, or when EVERY host failed. */
  error: string | null;
  hasMore: boolean;
  ended: boolean;
  sentinelRef: (el: HTMLDivElement | null) => void;
  refresh: () => void;
  refreshHost: (hostId: string) => void;
  retryPaging: (hostId: string) => void;
  removeRow: (hostId: string, id: string) => void;
}

const toSeed = <T>(s: HostSlice<T>): HostSeed => ({ id: s.hostId, name: s.name, transport_kind: s.transportKind });

export function usePerHostPagedList<T extends HostTagged>({
  host,
  fetch,
  timeField,
  idField,
  deps,
  poll = false,
}: UsePerHostPagedListOptions<T>): UsePerHostPagedListResult<T> {
  // `fetch` closes over live filter state and is a fresh identity every
  // render, so it is read through a ref, never listed as a dependency (the
  // same reasoning as usePagedList).
  const fetchRef = useRef(fetch);
  fetchRef.current = fetch;
  const hostRef = useRef(host);
  hostRef.current = host;
  const access = useMemo(() => rowAccess<T>(timeField, idField), [timeField, idField]);

  const allDeps = [host, ...deps];
  const prevDepsRef = useRef<unknown[]>([]);
  const genRef = useRef(0);
  if (allDeps.length !== prevDepsRef.current.length || allDeps.some((d, i) => !Object.is(d, prevDepsRef.current[i]))) {
    prevDepsRef.current = allDeps;
    genRef.current += 1;
  }
  const gen = genRef.current;

  const [slices, setSlices] = useState<HostSlice<T>[]>([]);
  const [hostsKnown, setHostsKnown] = useState(false);
  const [listError, setListError] = useState<string | null>(null);
  const engineRef = useRef<PerHostListEngine<T> | null>(null);

  useEffect(() => {
    const engine = new PerHostListEngine<T>((p) => fetchRef.current(p), access, setSlices, hostRef.current === null);
    engineRef.current = engine;
    setSlices([]);
    setHostsKnown(false);
    setListError(null);
    const one = hostRef.current;
    if (one !== null) {
      engine.start([{ id: one, name: one === 'local' ? 'Local' : one, transport_kind: one === 'local' ? 'local' : '' }]);
      setHostsKnown(true);
    } else {
      api.getRegisteredHosts().then(
        (hs) => {
          if (engineRef.current !== engine) return;
          engine.start(hs);
          setHostsKnown(true);
        },
        (e: unknown) => {
          if (engineRef.current !== engine) return;
          setListError(`Could not list hosts (${apiErrorMessage(e)}); showing this host only.`);
          engine.start([LOCAL_SEED]);
          setHostsKnown(true);
        },
      );
    }
    return () => engine.dispose();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `gen` IS the deps-changed signal (see above)
  }, [gen, access]);

  useEffect(() => {
    if (!poll) return;
    const local = setInterval(() => engineRef.current?.pollLocal(), LOCAL_POLL_MS);
    const remote = setInterval(() => {
      if (document.visibilityState === 'visible') engineRef.current?.pollRemote();
    }, REMOTE_POLL_MS);
    const onVisible = () => {
      if (document.visibilityState !== 'visible') return;
      engineRef.current?.pollLocal();
      engineRef.current?.pollRemote();
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      clearInterval(local);
      clearInterval(remote);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [poll, gen]);

  const merged = useMemo(() => watermarkMerge(slices, access), [slices, access]);
  const loading = !hostsKnown || (slices.length > 0 && slices.every((s) => s.state === 'loading'));
  const allFailed = slices.length > 0 && slices.every((s) => s.state === 'offline' || s.state === 'unavailable');
  const error =
    listError ??
    (allFailed
      ? slices.length === 1
        ? slices[0].reason ?? 'request failed'
        : slices.map((s) => `${s.name}: ${s.reason ?? s.state}`).join(' · ')
      : null);

  const loadMore = useCallback(() => engineRef.current?.loadMore() ?? Promise.resolve(), []);
  const { sentinelRef } = useInfiniteScroll({ hasMore: merged.hasMore && !loading, loadMore });

  const refresh = useCallback(() => {
    const engine = engineRef.current;
    if (!engine || engine.current.length === 0) return;
    if (hostRef.current !== null) {
      engine.reconcile(engine.current.map(toSeed));
      return;
    }
    api.getRegisteredHosts().then(
      (hs) => {
        if (engineRef.current !== engine) return;
        // The host list is readable again: "showing this host only" is no longer true.
        setListError(null);
        engine.reconcile(hs);
      },
      () => {
        if (engineRef.current === engine) engine.reconcile(engine.current.map(toSeed));
      },
    );
  }, []);
  const refreshHost = useCallback((id: string) => engineRef.current?.refreshHost(id), []);
  const retryPaging = useCallback((id: string) => void engineRef.current?.retryPaging(id), []);
  const removeRow = useCallback((hostId: string, id: string) => engineRef.current?.removeRow(hostId, id), []);

  return {
    rows: merged.visible,
    slices,
    loading,
    error,
    hasMore: merged.hasMore,
    ended: merged.ended,
    sentinelRef,
    refresh,
    refreshHost,
    retryPaging,
    removeRow,
  };
}
