// Window virtualization for long tables that scroll with the page.
//
// Only the rows intersecting the viewport (plus an overscan on each side)
// are mounted; spacer rows above and below stand in for the rest, sized
// from each row's measured height (estimated until measured), so page
// height, scrollbar and scroll position match a fully rendered table.
// Scroll is observed on the document in the capture phase, which catches
// whichever ancestor actually scrolls.

import { useEffect, useRef, useState } from 'react';

/** Row height assumed before a row has been measured (and in jsdom). */
export const ESTIMATED_ROW_PX = 41;
/** Rows mounted beyond the viewport on each side. */
export const OVERSCAN_ROWS = 20;

export function useWindowVirtualRows(keys: string[], enabled: boolean) {
  const bodyRef = useRef<HTMLTableSectionElement | null>(null);
  const heights = useRef(new Map<string, number>());
  const [viewport, setViewport] = useState({ top: 0, height: 0 });

  useEffect(() => {
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

  const measure = (key: string) => (el: HTMLTableRowElement | null) => {
    if (!el) return;
    const h = el.getBoundingClientRect().height;
    if (h > 0) heights.current.set(key, h);
  };

  if (!enabled) {
    return { bodyRef, start: 0, end: keys.length, topPx: 0, bottomPx: 0, measure };
  }

  const h = (k: string) => heights.current.get(k) ?? ESTIMATED_ROW_PX;
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
  let topPx = 0;
  for (let i = 0; i < start; i++) topPx += h(keys[i]);
  let bottomPx = 0;
  for (let i = end; i < keys.length; i++) bottomPx += h(keys[i]);
  return { bodyRef, start, end, topPx, bottomPx, measure };
}
