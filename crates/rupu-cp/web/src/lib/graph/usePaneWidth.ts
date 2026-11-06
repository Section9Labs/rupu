// The width of a graph's canvas pane, kept current as it resizes (0 until
// measured, or where the browser can't measure). The graphs read it for the
// minimap rule: a pane narrower than `MINIMAP_MIN_PANE` has no minimap,
// since keeping its width clear would shrink the graph it maps.

import { useEffect, useState, type RefObject } from 'react';

/** Below this pane width the graphs show no minimap. */
export const MINIMAP_MIN_PANE = 900;

export function usePaneWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    setWidth(el.clientWidth);
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return width;
}
