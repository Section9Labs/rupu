// Window virtualization for long tables that scroll with the page.
//
// Only the rows intersecting the viewport (plus an overscan on each side)
// are mounted; spacer rows above and below stand in for the rest, sized
// from each row's measured height (estimated until measured), so page
// height, scrollbar and scroll position match a fully rendered table.
// Row heights are tracked by one ResizeObserver (so spacers follow real
// heights, including after a width-only resize re-wraps rows); rows not yet
// measured use the average measured height. Scroll is observed on the
// document in the capture phase, which catches whichever ancestor actually
// scrolls.
//
// The row holding keyboard focus stays mounted at its true position even
// when it leaves the window (the spacer on its side is split around it), so
// scrolling never drops focus to <body>.

import { useLayoutEffect, useRef, useState } from 'react';

/** Row height assumed before any row has been measured (and in jsdom). */
export const ESTIMATED_ROW_PX = 41;
/** Rows mounted beyond the viewport on each side. */
export const OVERSCAN_ROWS = 20;

/** One piece of the virtual body, in document order: a spacer standing in
 *  for unmounted rows, or a run of mounted rows `[start, end)`. */
export type VirtualSlot =
  | { kind: 'spacer'; key: string; px: number }
  | { kind: 'rows'; start: number; end: number };

export function useWindowVirtualRows(keys: string[], enabled: boolean) {
  const bodyRef = useRef<HTMLTableSectionElement | null>(null);
  const heights = useRef(new Map<string, number>());
  const elements = useRef(new Map<string, HTMLTableRowElement>());
  const targetKeys = useRef(new WeakMap<Element, string>());
  const observer = useRef<ResizeObserver | null>(null);
  const refCallbacks = useRef(new Map<string, (el: HTMLTableRowElement | null) => void>());
  const [viewport, setViewport] = useState({ top: 0, height: 0 });
  // Bumped when a measured height actually changed, to re-size the spacers.
  const [, setMeasureVersion] = useState(0);
  // The row that holds focus (or contains the focused element).
  const [focusedKey, setFocusedKey] = useState<string | null>(null);

  useLayoutEffect(() => {
    const body = bodyRef.current;
    if (!enabled || !body) return;
    const rowKeyOf = (target: EventTarget | null): string | undefined => {
      const tr = target instanceof Element ? target.closest('tr') : null;
      return tr && body.contains(tr) ? targetKeys.current.get(tr) : undefined;
    };
    const onFocusIn = (e: FocusEvent) => {
      const key = rowKeyOf(e.target);
      setFocusedKey(key ?? null);
    };
    const onFocusOut = (e: FocusEvent) => {
      // Focus moving within the same row keeps it pinned.
      if (rowKeyOf(e.relatedTarget) !== undefined) return;
      setFocusedKey(null);
    };
    body.addEventListener('focusin', onFocusIn);
    body.addEventListener('focusout', onFocusOut);
    return () => {
      body.removeEventListener('focusin', onFocusIn);
      body.removeEventListener('focusout', onFocusOut);
      setFocusedKey(null);
    };
  }, [enabled]);

  // Layout effect: the first above-threshold paint already has the full
  // visible window (no extra frame showing only overscan rows).
  useLayoutEffect(() => {
    if (!enabled) return;
    const update = () => {
      const el = bodyRef.current;
      if (!el) return;
      const top = -el.getBoundingClientRect().top;
      const height = window.innerHeight;
      setViewport((v) => (v.top === top && v.height === height ? v : { top, height }));
    };
    update();
    document.addEventListener('scroll', update, { capture: true, passive: true });
    window.addEventListener('resize', update);
    return () => {
      document.removeEventListener('scroll', update, { capture: true });
      window.removeEventListener('resize', update);
    };
  }, [enabled]);

  // One observer per enabled hook. Row ref callbacks run before this effect
  // on first mount, so it also adopts every row already registered.
  useLayoutEffect(() => {
    if (!enabled || typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver((entries) => {
      let changed = false;
      for (const entry of entries) {
        const key = targetKeys.current.get(entry.target);
        if (key === undefined) continue;
        const h = entry.borderBoxSize?.[0]?.blockSize ?? entry.target.getBoundingClientRect().height;
        if (!(h > 0)) continue;
        const prev = heights.current.get(key);
        if (prev === undefined || Math.abs(prev - h) > 0.01) {
          heights.current.set(key, h);
          changed = true;
        }
      }
      if (changed) setMeasureVersion((v) => v + 1);
    });
    observer.current = ro;
    for (const el of elements.current.values()) ro.observe(el);
    return () => {
      ro.disconnect();
      observer.current = null;
    };
  }, [enabled]);

  // Stable per key, so React does not detach/reattach row refs every render.
  const measure = (key: string) => {
    let cb = refCallbacks.current.get(key);
    if (!cb) {
      cb = (el) => {
        const prev = elements.current.get(key);
        if (prev && prev !== el) {
          observer.current?.unobserve(prev);
          elements.current.delete(key);
        }
        if (!el) return;
        elements.current.set(key, el);
        targetKeys.current.set(el, key);
        if (observer.current) {
          observer.current.observe(el);
        } else if (typeof ResizeObserver === 'undefined') {
          const h = el.getBoundingClientRect().height;
          if (h > 0) heights.current.set(key, h);
        }
      };
      refCallbacks.current.set(key, cb);
    }
    return cb;
  };

  if (!enabled) {
    const slots: VirtualSlot[] = [{ kind: 'rows', start: 0, end: keys.length }];
    return { bodyRef, start: 0, end: keys.length, topPx: 0, bottomPx: 0, slots, measure };
  }

  let sum = 0;
  for (const v of heights.current.values()) sum += v;
  const avg = heights.current.size > 0 ? sum / heights.current.size : ESTIMATED_ROW_PX;
  const h = (k: string) => heights.current.get(k) ?? avg;
  const visTop = Math.max(0, viewport.top);
  const visBottom = viewport.top + viewport.height;
  let first = 0;
  let acc = 0;
  while (first < keys.length && acc + h(keys[first]) <= visTop) {
    acc += h(keys[first]);
    first++;
  }
  let last = first;
  let accEnd = acc;
  while (last < keys.length && accEnd < visBottom) {
    accEnd += h(keys[last]);
    last++;
  }
  const start = Math.max(0, first - OVERSCAN_ROWS);
  const end = Math.min(keys.length, last + OVERSCAN_ROWS);
  const span = (from: number, to: number) => {
    let px = 0;
    for (let i = from; i < to; i++) px += h(keys[i]);
    return px;
  };
  const slots: VirtualSlot[] = [];
  const spacer = (key: string, from: number, to: number) => {
    const px = span(from, to);
    if (px > 0) slots.push({ kind: 'spacer', key, px });
  };
  // A focused row outside [start, end) stays mounted at its true index.
  const pinned = focusedKey === null ? -1 : keys.indexOf(focusedKey);
  if (pinned >= 0 && pinned < start) {
    spacer('top', 0, pinned);
    slots.push({ kind: 'rows', start: pinned, end: pinned + 1 });
    spacer('pinned', pinned + 1, start);
  } else {
    spacer('top', 0, start);
  }
  slots.push({ kind: 'rows', start, end });
  if (pinned >= end) {
    spacer('pinned', end, pinned);
    slots.push({ kind: 'rows', start: pinned, end: pinned + 1 });
    spacer('bottom', pinned + 1, keys.length);
  } else {
    spacer('bottom', end, keys.length);
  }
  const topPx = span(0, start);
  const bottomPx = span(end, keys.length);
  return { bodyRef, start, end, topPx, bottomPx, slots, measure };
}
