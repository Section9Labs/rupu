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

/**
 * Fold one server response into the client series (append-only within an
 * epoch). Returns `prev` itself — same object — when the response changes
 * nothing (no new points, identical summary/steps/turns/partial/epoch), so a
 * 2 s poll of a quiet run does not re-render its consumers or re-draw the chart.
 */
export function mergeRunUsage(prev: RunUsageState | null, resp: RunUsageResponse): RunUsageState {
  const continues =
    prev !== null &&
    prev.epoch === resp.epoch &&
    resp.points_from > 0 &&
    resp.points_from <= prev.points.length;
  if (
    prev !== null &&
    prev.epoch === resp.epoch &&
    resp.points.length === 0 &&
    (continues ? resp.points_from === prev.points.length : prev.points.length === 0) &&
    prev.turns === resp.turns &&
    prev.partial === resp.partial &&
    JSON.stringify(prev.summary) === JSON.stringify(resp.summary) &&
    JSON.stringify(prev.steps) === JSON.stringify(resp.steps)
  ) {
    return prev;
  }
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
 * `unavailable` so callers fall back to the one-shot graph usage.
 *
 * `runKnown` (default true) says the caller has independently confirmed the
 * run exists. A freshly launched run legitimately 404s until its `run.json`
 * lands, so while `runKnown` is false a 404 is transient — no `unavailable`,
 * polling continues. Once the run is known, a 404 means the CP predates this
 * endpoint: `unavailable`, and polling for that run stops. Any other error
 * keeps the last good numbers.
 */
export function useRunUsage(
  id: string | undefined,
  host: string | undefined,
  live: boolean,
  opts?: { intervalMs?: number; runKnown?: boolean },
): { usage: RunUsageState | null; unavailable: boolean } {
  const intervalMs = opts?.intervalMs ?? 2000;
  const runKnown = opts?.runKnown ?? true;
  const key = id ? `${host ?? ''}\u0000${id}` : '';
  const [slot, setSlot] = useState<Slot>(EMPTY_SLOT);

  // Poller state shared across effect re-runs (a `live` / `runKnown` flip
  // re-runs the poll effect but must not lose the folded series).
  const activeKey = useRef<string | null>(null);
  const latest = useRef<{ key: string; usage: RunUsageState | null }>({ key: '', usage: null });
  const inFlightKey = useRef<string | null>(null);
  const queuedKey = useRef<string | null>(null); // a forced fetch arrived for this key mid-flight
  const missed = useRef(false); // a fetch was skipped because the tab was hidden
  const goneKey = useRef<string | null>(null); // endpoint 404s for a known run: stop polling
  const lastOkAt = useRef(0); // when the last successful fetch for `latest.key` landed
  const prevLive = useRef(false);
  const runKnownRef = useRef(runKnown);
  runKnownRef.current = runKnown;

  useEffect(() => {
    activeKey.current = key;
    // Drop any result of an earlier visit to this run so coming back to it
    // refetches from scratch rather than resuming a stale tail.
    setSlot((s) =>
      s.key === key && (s.usage !== null || s.unavailable) ? { key, usage: null, unavailable: false } : s,
    );
    return () => {
      activeKey.current = null;
      queuedKey.current = null;
      goneKey.current = null;
      latest.current = { key: '', usage: null };
      lastOkAt.current = 0;
      prevLive.current = false;
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
        // run just went terminal): re-run once it lands. Any other caller is
        // content with the in-flight fetch.
        if (force) queuedKey.current = key;
        return;
      }
      inFlightKey.current = key;
      missed.current = false;
      // Judged when the request goes out: a 404 for a run that was not yet
      // known then is the launch race, even if the run became known since.
      const knownAtRequest = runKnownRef.current;
      try {
        const prev = latest.current.key === key ? latest.current.usage : null;
        const resp = await api.getRunUsage(
          id,
          prev ? { host, since: prev.points.length, epoch: prev.epoch } : { host },
        );
        if (activeKey.current !== key) return; // switched runs / unmounted
        const next = mergeRunUsage(prev, resp);
        latest.current = { key, usage: next };
        lastOkAt.current = Date.now();
        // Same object back = nothing changed: bail out of the render.
        setSlot((s) => (s.key === key && s.usage === next && !s.unavailable ? s : { key, usage: next, unavailable: false }));
      } catch (e) {
        if (activeKey.current !== key) return;
        const notFound = isNotFound(e);
        if (notFound && !knownAtRequest) {
          // Run not there yet. If it has become known while this request was in
          // flight, retry right away instead of waiting on the next interval
          // (a run that is not live has none).
          if (runKnownRef.current) queuedKey.current = key;
          return;
        }
        if (notFound) goneKey.current = key;
        // A 404 (older remote CP) or a first-fetch failure leaves the caller
        // on its one-shot numbers; a later transient error keeps the last good.
        if (notFound || latest.current.key !== key || latest.current.usage === null) {
          setSlot((s) => ({ key, usage: s.key === key ? s.usage : null, unavailable: true }));
        }
      } finally {
        if (inFlightKey.current === key) inFlightKey.current = null;
        if (queuedKey.current === key && activeKey.current === key) {
          queuedKey.current = null;
          void tick(false);
        }
      }
    };

    // Kick a fetch on (re)entry — unless one that is still good is already in
    // hand: a run that loads as running flips `live` false→true (and `runKnown`
    // flips) right after mount's fetch, and refetching there doubles the
    // page-load request. A run that JUST went terminal always fetches: its
    // final numbers are newer than whatever was polled last.
    const wentTerminal = prevLive.current && !live;
    prevLive.current = live;
    const haveOk = latest.current.key === key && latest.current.usage !== null;
    const stillGood = haveOk && (!live || Date.now() - lastOkAt.current < intervalMs);
    if (wentTerminal || !stillGood) void tick(wentTerminal);

    const onVis = () => {
      if (document.visibilityState === 'visible' && (live || missed.current)) void tick(false);
    };
    document.addEventListener('visibilitychange', onVis);
    const timer = live ? window.setInterval(() => void tick(false), intervalMs) : undefined;
    return () => {
      if (timer !== undefined) window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onVis);
    };
  }, [id, host, key, live, runKnown, intervalMs]);

  return slot.key === key
    ? { usage: slot.usage, unavailable: slot.unavailable }
    : { usage: null, unavailable: false };
}
