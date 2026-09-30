import { useEffect, useRef, useState } from 'react';
import { api, type RunUsageResponse, type UsageTimelinePoint } from './api';
import type { UsageSummary } from './usage';

export interface RunUsageState {
  summary: UsageSummary;
  steps: Record<string, UsageSummary>;
  turns: number;
  partial: boolean;
  epoch: string;
  points: UsageTimelinePoint[];
}

/** Fold one server response into the client series (append-only within an epoch). */
export function mergeRunUsage(prev: RunUsageState | null, resp: RunUsageResponse): RunUsageState {
  const continues =
    prev !== null &&
    prev.epoch === resp.epoch &&
    resp.points_from > 0 &&
    resp.points_from <= prev.points.length;
  const points = continues ? prev.points.slice(0, resp.points_from).concat(resp.points) : resp.points;
  return {
    summary: resp.summary,
    steps: resp.steps,
    turns: resp.turns,
    partial: resp.partial,
    epoch: resp.epoch,
    points,
  };
}

interface Slot {
  /** Which (host, run) this slot belongs to — a stale slot is never returned. */
  key: string;
  usage: RunUsageState | null;
  unavailable: boolean;
}

const EMPTY_SLOT: Slot = { key: '', usage: null, unavailable: false };

function isNotFound(e: unknown): boolean {
  return typeof e === 'object' && e !== null && (e as { status?: unknown }).status === 404;
}

/**
 * Live run usage (spec 2026-09-29 §7): fetch once, then poll every
 * `intervalMs` (default 2000) while `live` and the tab is visible; one final
 * fetch when `live` turns false. A failing endpoint (older remote CP) sets
 * `unavailable` so callers fall back to the one-shot graph usage; a 404 stops
 * polling for that run, any other error keeps the last good numbers.
 */
export function useRunUsage(
  id: string | undefined,
  host: string | undefined,
  live: boolean,
  opts?: { intervalMs?: number },
): { usage: RunUsageState | null; unavailable: boolean } {
  const intervalMs = opts?.intervalMs ?? 2000;
  const key = id ? `${host ?? ''}\u0000${id}` : '';
  const [slot, setSlot] = useState<Slot>(EMPTY_SLOT);

  // Poller state shared across effect re-runs (a `live` flip re-runs the poll
  // effect but must not lose the folded series or double-fetch).
  const activeKey = useRef<string | null>(null);
  const latest = useRef<{ key: string; usage: RunUsageState | null }>({ key: '', usage: null });
  const inFlightKey = useRef<string | null>(null);
  const queued = useRef(false); // a forced fetch arrived while one was in flight
  const missed = useRef(false); // a fetch was skipped because the tab was hidden
  const goneKey = useRef<string | null>(null); // endpoint 404s for this key: stop polling

  useEffect(() => {
    activeKey.current = key;
    return () => {
      activeKey.current = null;
    };
  }, [key]);

  useEffect(() => {
    if (!id) return;
    const tick = async (force: boolean): Promise<void> => {
      if (goneKey.current === key) return;
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') {
        missed.current = true;
        return;
      }
      if (inFlightKey.current === key) {
        // The in-flight response may predate what the caller wants to see (the
        // run just went terminal): re-run once it lands.
        if (force) queued.current = true;
        return;
      }
      inFlightKey.current = key;
      missed.current = false;
      try {
        const prev = latest.current.key === key ? latest.current.usage : null;
        const resp = await api.getRunUsage(
          id,
          prev ? { host, since: prev.points.length, epoch: prev.epoch } : { host },
        );
        if (activeKey.current !== key) return; // switched runs / unmounted
        const next = mergeRunUsage(prev, resp);
        latest.current = { key, usage: next };
        setSlot({ key, usage: next, unavailable: false });
      } catch (e) {
        if (activeKey.current !== key) return;
        const notFound = isNotFound(e);
        if (notFound) goneKey.current = key;
        // A 404 (older remote CP) or a first-fetch failure leaves the caller
        // on its one-shot numbers; a later transient error keeps the last good.
        if (notFound || latest.current.key !== key || latest.current.usage === null) {
          setSlot((s) => ({ key, usage: s.key === key ? s.usage : null, unavailable: true }));
        }
      } finally {
        if (inFlightKey.current === key) inFlightKey.current = null;
        if (queued.current && activeKey.current === key) {
          queued.current = false;
          void tick(false);
        }
      }
    };
    void tick(true);
    const onVis = () => {
      if (document.visibilityState === 'visible' && (live || missed.current)) void tick(false);
    };
    document.addEventListener('visibilitychange', onVis);
    const timer = live ? window.setInterval(() => void tick(false), intervalMs) : undefined;
    return () => {
      if (timer !== undefined) window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onVis);
    };
  }, [id, host, key, live, intervalMs]);

  return slot.key === key
    ? { usage: slot.usage, unavailable: slot.unavailable }
    : { usage: null, unavailable: false };
}
