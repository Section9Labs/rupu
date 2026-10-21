import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { prefersReducedMotion } from '../chart/motion';
import { replayDuration, timeBounds, type TimelineBand, type TimelineItem } from './timeline';

export interface TimelineState {
  /** `null` = live / the whole run; otherwise the instant being shown. */
  t: number | null;
  playing: boolean;
  replay(): void;
  scrub(t: number): void;
  /** Back to live / the whole run. */
  release(): void;
}

export function useTimeline({
  items,
  bands,
  nowMs,
}: {
  items: TimelineItem[];
  bands: TimelineBand[];
  live: boolean;
  nowMs: number | null;
}): TimelineState {
  const [t, setT] = useState<number | null>(null);
  const [playing, setPlaying] = useState(false);
  const raf = useRef(0);
  const bounds = useMemo(() => timeBounds(items, bands, nowMs), [items, bands, nowMs]);
  // A replay reads the bounds it started with: a poll adding an item never
  // restarts or ends it.
  const span = useRef(bounds);
  span.current = bounds;

  const stop = useCallback(() => {
    cancelAnimationFrame(raf.current);
    raf.current = 0;
    setPlaying(false);
  }, []);

  const release = useCallback(() => {
    stop();
    setT(null);
  }, [stop]);

  const scrub = useCallback(
    (to: number) => {
      stop();
      setT(to);
    },
    [stop],
  );

  const replay = useCallback(() => {
    stop();
    if (prefersReducedMotion()) {
      setT(null);
      return;
    }
    const { start, end } = span.current;
    const dur = replayDuration(start, end);
    const t0 = performance.now();
    setPlaying(true);
    setT(start);
    const frame = (now: number) => {
      const f = Math.min(1, (now - t0) / dur);
      if (f >= 1) {
        raf.current = 0;
        setPlaying(false);
        setT(null);
        return;
      }
      setT(start + (span.current.end - start) * f);
      raf.current = requestAnimationFrame(frame);
    };
    raf.current = requestAnimationFrame(frame);
  }, [stop]);

  useEffect(() => () => cancelAnimationFrame(raf.current), []);

  return { t, playing, replay, scrub, release };
}
