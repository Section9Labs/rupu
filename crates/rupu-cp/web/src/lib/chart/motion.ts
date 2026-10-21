// Chart motion helpers shared by every chart (the CSS half lives in
// styles.css under "Chart motion"). Pure functions are exported for tests;
// the hooks are thin.

import { useEffect, useLayoutEffect, useRef, useState } from 'react';

/** True when the viewer asked for reduced motion (guarded for SSR/jsdom). */
export function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
  try {
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  } catch {
    return false;
  }
}

/** The sweep's duration and curve: `--chart-sweep` and `--ease-out` in
 *  styles.css. The stylus follows the same curve so it sits on the edge. */
export const SWEEP_MS = 1400;
export const EASE_OUT: [number, number, number, number] = [0.23, 1, 0.32, 1];
export const EASE_IN_OUT: [number, number, number, number] = [0.77, 0, 0.175, 1];

/** A CSS `cubic-bezier(x1, y1, x2, y2)` timing function, solved for y at a
 *  given time fraction `t` (Newton's method with a bisection fallback). */
export function cubicBezier([x1, y1, x2, y2]: [number, number, number, number]): (t: number) => number {
  const bx = (s: number) => 3 * (1 - s) ** 2 * s * x1 + 3 * (1 - s) * s * s * x2 + s ** 3;
  const by = (s: number) => 3 * (1 - s) ** 2 * s * y1 + 3 * (1 - s) * s * s * y2 + s ** 3;
  const dx = (s: number) => 3 * (1 - s) ** 2 * x1 + 6 * (1 - s) * s * (x2 - x1) + 3 * s * s * (1 - x2);
  return (t: number) => {
    if (t <= 0) return 0;
    if (t >= 1) return 1;
    let s = t;
    for (let k = 0; k < 6; k++) {
      const d = dx(s);
      if (Math.abs(d) < 1e-6) break;
      const next = s - (bx(s) - t) / d;
      if (next < 0 || next > 1) break;
      s = next;
    }
    if (Math.abs(bx(s) - t) > 1e-4) {
      let lo = 0;
      let hi = 1;
      for (let k = 0; k < 30; k++) {
        s = (lo + hi) / 2;
        if (bx(s) < t) lo = s;
        else hi = s;
      }
    }
    return by(s);
  };
}

/** `true` from the second frame after mount: the class switch (`cp-in`) that
 *  starts every CSS draw-in. Two frames so the initial styles are committed
 *  before the transition starts. */
export function useDrawIn(): boolean {
  const [drawn, setDrawn] = useState(false);
  useEffect(() => {
    let b = 0;
    const a = requestAnimationFrame(() => {
      b = requestAnimationFrame(() => setDrawn(true));
    });
    return () => {
      cancelAnimationFrame(a);
      cancelAnimationFrame(b);
    };
  }, []);
  return drawn;
}

/** The width of the element `ref` points at, tracked as it resizes. Falls back
 *  to `fallback` until measured (and where there is no ResizeObserver). */
export function useMeasuredWidth(ref: React.RefObject<HTMLElement | null>, fallback: number): number {
  const [w, setW] = useState(fallback);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const next = Math.round(el.getBoundingClientRect().width);
      if (next > 0) setW(next);
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return w;
}

/** Interpolate two equal-shaped series sets; `null` when their shapes differ
 *  (a morph needs a point-for-point match). Pure, for testing. */
export function lerpSeries(from: number[][], to: number[][], k: number): number[][] | null {
  if (from.length !== to.length) return null;
  const out: number[][] = [];
  for (let s = 0; s < to.length; s++) {
    const a = from[s];
    const b = to[s];
    if (a.length !== b.length) return null;
    out.push(b.map((v, i) => a[i] + (v - a[i]) * k));
  }
  return out;
}

const MORPH_MS = 420;
const easeInOut = cubicBezier(EASE_IN_OUT);

/** The values to draw for `target`: when new data arrives with the same shape
 *  (a live refresh), the marks morph to it over ~420ms; a new shape (more or
 *  fewer buckets or series) is drawn as is. Reduced motion never morphs. */
export function useMorphedSeries(target: number[][]): number[][] {
  const [shown, setShown] = useState(target);
  const shownRef = useRef(target);
  const signature = JSON.stringify(target);
  useEffect(() => {
    const from = shownRef.current;
    const next = target;
    const sameShape = lerpSeries(from, next, 1) !== null;
    if (!sameShape || prefersReducedMotion()) {
      shownRef.current = next;
      setShown(next);
      return;
    }
    let raf = 0;
    const start = performance.now();
    const tick = (now: number) => {
      const k = Math.min(1, (now - start) / MORPH_MS);
      const v = lerpSeries(from, next, easeInOut(k)) ?? next;
      shownRef.current = v;
      setShown(v);
      if (k < 1) raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
    // `signature` stands for `target`'s contents: callers rebuild the arrays
    // on every render, so identity would morph forever.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature]);
  // A new shape is drawn as is on the render it arrives, never through a
  // frame of the old one (callers index the result by the new shape).
  return lerpSeries(shown, target, 1) !== null ? shown : target;
}

/** Where the stylus sits at time fraction `t` of the sweep: x on the plot and
 *  the trace's y there (interpolated between buckets). Pure, for testing. */
export function stylusAt(
  t: number,
  xs: number[],
  ys: number[],
  x0: number,
  x1: number,
  ease: (t: number) => number = cubicBezier(EASE_OUT),
): { x: number; y: number } | null {
  const n = xs.length;
  if (n === 0) return null;
  const x = x0 + ease(t) * (x1 - x0);
  if (n === 1) return { x, y: ys[0] };
  let i = 0;
  while (i < n - 2 && xs[i + 1] < x) i++;
  const span = xs[i + 1] - xs[i];
  const f = span > 0 ? Math.max(0, Math.min(1, (x - xs[i]) / span)) : 0;
  return { x, y: ys[i] + (ys[i + 1] - ys[i]) * f };
}
