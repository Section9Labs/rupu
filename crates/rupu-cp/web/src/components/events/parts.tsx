// Small pieces of the event views (styles/events.css): the morphing glyph and
// its capsule (the graphs' glyph plus warning, severity and panel faces), and
// a number that rolls when it changes.

import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { Mark } from '../../lib/events/rail';

export const MARK_WORD: Record<Mark, string> = {
  running: 'live',
  done: 'done',
  failed: 'failed',
  awaiting: 'needs you',
  activity: 'activity',
  lifecycle: 'lifecycle',
  panel: 'panel',
  warning: 'warning',
  critical: 'critical',
  high: 'high',
  medium: 'medium',
  low: 'low',
  info: 'info',
};

/** One glyph for every mark; CSS morphs between them. */
export function EvGlyph({ mark }: { mark: Mark }) {
  return (
    <svg data-mark={mark} className="evx-glyph evx-m" viewBox="0 0 16 16" aria-hidden>
      <g className="evx-g-spin">
        <circle className="evx-g-ring" cx="8" cy="8" r="5.5" />
      </g>
      <path className="evx-g-check" d="M5 8.2l2.1 2.1 4.2-4.2" />
      <path className="evx-g-x1" d="M5.6 5.6l4.8 4.8" />
      <path className="evx-g-x2" d="M10.4 5.6l-4.8 4.8" />
      <circle className="evx-g-dot" cx="8" cy="8" r="1.9" />
      <path className="evx-g-bang1" d="M8 4.9v3.6" />
      <path className="evx-g-bang2" d="M8 11.1h.01" />
      <path className="evx-g-sev" d="M8 4.4l3.6 3.6L8 11.6 4.4 8z" />
      <path className="evx-g-eye" d="M5.2 8c.9-1.4 1.8-2.1 2.8-2.1S9.9 6.6 10.8 8c-.9 1.4-1.8 2.1-2.8 2.1S6.1 9.4 5.2 8z" />
    </svg>
  );
}

export function EvCap({ mark, word }: { mark: Mark; word?: string }) {
  const w = word ?? MARK_WORD[mark];
  return (
    <span className="evx-cap evx-m" data-mark={mark}>
      <EvGlyph mark={mark} />
      <span key={w} className="evx-word">
        {w}
      </span>
    </span>
  );
}

/** A number (or short text) that rolls out upward and the new one in from below. */
export function Rolling({ value, className }: { value: string | number; className?: string }) {
  const text = String(value);
  const [shown, setShown] = useState(text);
  const [phase, setPhase] = useState<'out' | 'in' | undefined>(undefined);
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      setShown(text);
      return;
    }
    if (text === shown) return;
    setPhase('out');
    const t = setTimeout(() => {
      setShown(text);
      setPhase('in');
      requestAnimationFrame(() => requestAnimationFrame(() => setPhase(undefined)));
    }, 160);
    return () => clearTimeout(t);
    // `shown` is what is on screen; only a new value starts a roll.
  }, [text]);
  return (
    <span className={className ? `evx-tick ${className}` : 'evx-tick'} data-phase={phase}>
      {shown}
    </span>
  );
}

/** True for one flash after `value` grows. */
export function useGrewFlash(value: number): boolean {
  const prev = useRef(value);
  const [flash, setFlash] = useState(false);
  useEffect(() => {
    const grew = value > prev.current;
    prev.current = value;
    if (!grew) return;
    setFlash(true);
    const t = setTimeout(() => setFlash(false), 480);
    return () => clearTimeout(t);
  }, [value]);
  return flash;
}

/** Re-renders every `ms` (clocks, relative ages). One timer per period,
 *  shared by every subscriber. */
const idle = new Map<number, number>();
const tickers = new Map<number, { now: number; subs: Set<() => void>; timer: ReturnType<typeof setInterval> }>();
export function useNow(ms: number): number {
  const subscribe = useCallback(
    (fn: () => void) => {
      let t = tickers.get(ms);
      if (!t) {
        const entry = { now: Date.now(), subs: new Set<() => void>(), timer: undefined as unknown as ReturnType<typeof setInterval> };
        entry.timer = setInterval(() => {
          entry.now = Date.now();
          for (const s of entry.subs) s();
        }, ms);
        tickers.set(ms, entry);
        t = entry;
      }
      t.subs.add(fn);
      return () => {
        const e = tickers.get(ms);
        if (!e) return;
        e.subs.delete(fn);
        if (e.subs.size === 0) {
          clearInterval(e.timer);
          tickers.delete(ms);
          idle.delete(ms);
        }
      };
    },
    [ms],
  );
  const get = useCallback(() => {
    const live = tickers.get(ms);
    if (live) return live.now;
    // Before the first subscription: one cached value, so the snapshot is stable.
    if (!idle.has(ms)) idle.set(ms, Date.now());
    return idle.get(ms)!;
  }, [ms]);
  return useSyncExternalStore(subscribe, get, get);
}
