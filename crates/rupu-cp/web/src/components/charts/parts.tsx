// Pieces every chart shares, so they look and move the same way: the stylus
// that rides the draw-in sweep, the hover crosshair with its riding dots, the
// tooltip, the interactive legend and the gradient/glow defs. Motion classes
// are in styles.css ("Chart motion").

import { useEffect, useRef, useState } from 'react';
import { cn } from '../../lib/cn';
import { SWEEP_MS, cubicBezier, EASE_OUT, prefersReducedMotion, stylusAt } from '../../lib/chart/motion';

const sweepEase = cubicBezier(EASE_OUT);

/** A glowing point that rides the leading edge of the sweep along one trace,
 *  from the moment `start` turns true until the sweep ends, then removes
 *  itself. `x0`/`x1` are the sweep clip's left and right edges. Nothing under
 *  reduced motion. */
export function Stylus({
  start,
  xs,
  ys,
  x0,
  x1,
  color,
}: {
  start: boolean;
  xs: number[];
  ys: number[];
  x0: number;
  x1: number;
  color: string;
}) {
  const ref = useRef<SVGGElement>(null);
  const geo = useRef({ xs, ys, x0, x1 });
  geo.current = { xs, ys, x0, x1 };
  const [done, setDone] = useState(() => prefersReducedMotion());

  useEffect(() => {
    if (!start || done) return;
    let raf = 0;
    const t0 = performance.now();
    const frame = (now: number) => {
      const t = Math.min(1, (now - t0) / SWEEP_MS);
      const g = geo.current;
      const p = stylusAt(t, g.xs, g.ys, g.x0, g.x1, sweepEase);
      const el = ref.current;
      if (el && p) {
        el.setAttribute('transform', `translate(${p.x.toFixed(1)} ${p.y.toFixed(1)})`);
        // fade in over the first few frames, out as the sweep settles
        el.style.opacity = String(t < 0.04 ? t / 0.04 : t > 0.82 ? Math.max(0, (1 - t) / 0.18) : 1);
      }
      if (t < 1) raf = requestAnimationFrame(frame);
      else setDone(true);
    };
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [start, done]);

  if (done || xs.length < 2) return null;
  return (
    <g ref={ref} className="cp-chart-stylus" style={{ opacity: 0 }} aria-hidden>
      <circle r={10} style={{ fill: color, opacity: 0.16 }} />
      <circle r={5.5} style={{ fill: color, opacity: 0.3 }} />
      <circle r={3} style={{ fill: color }} />
      <circle r={1.3} style={{ fill: 'white', opacity: 0.9 }} />
    </g>
  );
}

/** A vertical fade (top → bottom) for an area band: strongest at the band's
 *  own top, a faint tint at its bottom. Units follow the band's own box, so
 *  every stacked band fades on its own. */
export function FadeGradient({
  id,
  color,
  top,
  bottom,
  flip = false,
}: {
  id: string;
  color: string;
  top: number;
  bottom: number;
  /** Fade upward instead (a band hanging below its baseline). */
  flip?: boolean;
}) {
  return (
    <linearGradient id={id} x1="0" y1={flip ? '1' : '0'} x2="0" y2={flip ? '0' : '1'}>
      <stop offset="0" style={{ stopColor: color, stopOpacity: top }} />
      <stop offset="1" style={{ stopColor: color, stopOpacity: bottom }} />
    </linearGradient>
  );
}

/** The soft blur a glow line is drawn through. */
export function GlowFilter({ id }: { id: string }) {
  return (
    <filter id={id} x="-10%" y="-40%" width="120%" height="180%">
      <feGaussianBlur stdDeviation="3" />
    </filter>
  );
}

/** A crosshair gradient that fades out at both ends of the line. */
export function CrossGradient({ id, color, y0, y1 }: { id: string; color: string; y0: number; y1: number }) {
  return (
    <linearGradient id={id} x1="0" y1={y0} x2="0" y2={y1} gradientUnits="userSpaceOnUse">
      <stop offset="0" style={{ stopColor: color, stopOpacity: 0 }} />
      <stop offset="0.12" style={{ stopColor: color, stopOpacity: 0.55 }} />
      <stop offset="0.88" style={{ stopColor: color, stopOpacity: 0.55 }} />
      <stop offset="1" style={{ stopColor: color, stopOpacity: 0 }} />
    </linearGradient>
  );
}

/** The hover crosshair: a fading vertical line at `x` with a dot riding each
 *  trace. It glides between buckets (90ms) except on the first frame it
 *  appears (`appear`), when it jumps straight to the pointer. */
export function Crosshair({
  x,
  y0,
  y1,
  strokeId,
  dots,
  ring,
  appear,
}: {
  x: number;
  y0: number;
  y1: number;
  strokeId: string;
  dots: { key: string; y: number; color: string }[];
  /** The panel color that rings each dot, separating it from the line. */
  ring: string;
  appear: boolean;
}) {
  return (
    <g
      className={cn('cp-chart-cross', appear && 'cp-appear')}
      style={{ transform: `translateX(${x}px)` }}
      aria-hidden
    >
      <line x1={0} y1={y0} x2={0} y2={y1} stroke={`url(#${strokeId})`} strokeWidth={1} />
      {dots.map((d) => (
        <g key={d.key} className="cp-chart-cross-dot" style={{ transform: `translateY(${d.y}px)` }}>
          <circle r={8} style={{ fill: d.color, opacity: 0.16 }} />
          <circle r={3.6} strokeWidth={1.75} style={{ fill: d.color, stroke: ring }} />
        </g>
      ))}
    </g>
  );
}

/** One tooltip row: swatch, label, value. A `total` row sits under a rule. */
export interface TipRow {
  key: string;
  label: React.ReactNode;
  value: React.ReactNode;
  color?: string;
  total?: boolean;
  /** Indent and quiet the row (a part of the row above it). */
  sub?: boolean;
  muted?: boolean;
}

/** The hover tooltip, anchored above (`x`, `y`) in the plot's own pixels and
 *  kept inside `width`. Its box follows the crosshair; its card scales in from
 *  the point it names on first show and stays put (no re-entrance) after. */
export function ChartTooltip({
  open,
  appear,
  x,
  y,
  width,
  title,
  rows,
  footer,
}: {
  open: boolean;
  appear: boolean;
  x: number;
  y: number;
  width: number;
  title: React.ReactNode;
  rows: TipRow[];
  footer?: React.ReactNode;
}) {
  const half = 100;
  const cx = Math.max(half, Math.min(width - half, x));
  return (
    <div
      className={cn('cp-chart-tip', open && 'cp-on', appear && 'cp-appear')}
      style={{ transform: `translate(${cx}px, ${y}px)` }}
      role="status"
      aria-hidden={!open}
    >
      <div style={{ transform: 'translate(-50%, calc(-100% - 14px))' }}>
        <div className="cp-chart-tip-card min-w-[176px] whitespace-nowrap rounded-[10px] border border-border-strong bg-surface/95 px-3 py-2 text-note shadow-lg backdrop-blur-sm">
          <div className="mb-1 font-semibold text-ink">{title}</div>
          {rows.map((r) => (
            <div
              key={r.key}
              className={cn(
                'flex items-center gap-2 leading-relaxed',
                r.total && 'mt-1 border-t border-border pt-1',
                r.sub && 'pl-4',
              )}
            >
              {r.color && (
                <span className="h-2 w-2 shrink-0 rounded-[2px]" style={{ background: r.color }} aria-hidden />
              )}
              <span className={cn('flex-1 pr-4', r.muted || r.sub ? 'text-ink-mute' : 'text-ink-dim')}>{r.label}</span>
              <span
                className={cn(
                  'font-mono tabular-nums',
                  r.total ? 'font-bold text-ink' : r.muted ? 'text-ink-mute' : 'font-semibold text-ink',
                )}
              >
                {r.value}
              </span>
            </div>
          ))}
          {footer && <div className="mt-1.5 text-ink-mute">{footer}</div>}
        </div>
      </div>
    </div>
  );
}

/** Tracks a hover index and whether this frame is its first (`appear`), so
 *  the crosshair and tooltip jump to the pointer on entry and glide after. */
export function useHoverIndex(): {
  index: number | null;
  appear: boolean;
  set: (i: number | null) => void;
} {
  const [index, setIndex] = useState<number | null>(null);
  const [appear, setAppear] = useState(false);
  const prev = useRef<number | null>(null);
  const set = (i: number | null) => {
    if (prev.current == null && i != null) setAppear(true);
    prev.current = i;
    setIndex(i);
  };
  useEffect(() => {
    if (!appear) return;
    const id = requestAnimationFrame(() => setAppear(false));
    return () => cancelAnimationFrame(id);
  }, [appear]);
  return { index, appear, set };
}

/** One legend entry. Hovering (or focusing) it isolates its series. */
export interface LegendItem {
  key: string;
  label: React.ReactNode;
  color: string;
  value?: React.ReactNode;
  /** A dashed swatch: the series is a line drawn dashed, or a band within another. */
  hollow?: boolean;
}

export function ChartLegend({
  items,
  focus,
  onFocus,
  className,
}: {
  items: LegendItem[];
  focus: string | null;
  onFocus: (key: string | null) => void;
  className?: string;
}) {
  return (
    <ul className={cn('flex flex-wrap gap-x-4 gap-y-1', className)} onMouseLeave={() => onFocus(null)}>
      {items.map((it) => (
        <li key={it.key}>
          <button
            type="button"
            className={cn(
              'cp-chart-fade inline-flex items-center gap-1.5 rounded text-note text-ink-dim outline-none focus-visible:ring-2 focus-visible:ring-brand-500/40',
              focus && focus !== it.key && 'opacity-40',
            )}
            onMouseEnter={() => onFocus(it.key)}
            onFocus={() => onFocus(it.key)}
            onBlur={() => onFocus(null)}
          >
            <span
              className="h-2 w-2 shrink-0 rounded-[2px]"
              style={
                it.hollow
                  ? { boxShadow: `inset 0 0 0 1.5px ${it.color}` }
                  : { background: it.color }
              }
              aria-hidden
            />
            <span className="max-w-[180px] truncate">{it.label}</span>
            {it.value != null && <span className="font-semibold tabular-nums text-ink">{it.value}</span>}
          </button>
        </li>
      ))}
    </ul>
  );
}

const COUNT_MS = 750;
const countEase = cubicBezier(EASE_OUT);

/** A number that counts up from 0 on mount (and glides to a new value when
 *  it changes), rendered through `format`. Reduced motion shows the value. */
export function CountUp({ value, format }: { value: number; format: (v: number) => string }) {
  const [shown, setShown] = useState(() => (prefersReducedMotion() ? value : 0));
  const from = useRef(shown);
  from.current = shown;
  useEffect(() => {
    if (prefersReducedMotion()) {
      setShown(value);
      return;
    }
    const start = performance.now();
    const a = from.current;
    let raf = 0;
    const tick = (now: number) => {
      const t = Math.min(1, (now - start) / COUNT_MS);
      setShown(a + (value - a) * countEase(t));
      if (t < 1) raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [value]);
  return <>{format(shown)}</>;
}

/** A live sparkline: a thin line with a fading area over the last few
 *  samples, its latest point breathing. Each new sample slides the trace one
 *  step left (WAAPI, ~600ms), so it reads as a stream, not a redraw.
 *  Reduced motion: it redraws in place. */
export function Sparkline({
  values,
  color,
  width = 72,
  height = 26,
  panel,
}: {
  values: number[];
  color: string;
  width?: number;
  height?: number;
  /** The background the end dot is ringed against. */
  panel: string;
}) {
  const gRef = useRef<SVGGElement>(null);
  const uid = useRef(`spark-${Math.random().toString(36).slice(2, 8)}`).current;
  const n = values.length;
  const step = n > 1 ? (width - 6) / (n - 1) : 0;
  const max = Math.max(1, ...values);
  const ys = values.map((v) => height - 3 - (v / max) * (height - 7));
  const xs = values.map((_, i) => i * step);
  const line = xs.map((x, i) => `${i ? 'L' : 'M'}${x.toFixed(1)},${ys[i].toFixed(1)}`).join('');
  const signature = values.join(',');
  const first = useRef(true);

  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    const g = gRef.current;
    if (!g || prefersReducedMotion() || typeof g.animate !== 'function') return;
    g.animate([{ transform: `translateX(${step}px)` }, { transform: 'translateX(0)' }], {
      duration: 600,
      easing: 'cubic-bezier(0.77, 0, 0.175, 1)',
    });
  }, [signature, step]);

  if (n < 2) return null;
  const lastX = xs[n - 1];
  const lastY = ys[n - 1];
  return (
    <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} className="block overflow-visible" aria-hidden>
      <defs>
        <FadeGradient id={`${uid}-g`} color={color} top={0.35} bottom={0} />
        <clipPath id={`${uid}-c`}>
          <rect x={0} y={-4} width={width} height={height + 8} />
        </clipPath>
      </defs>
      <g clipPath={`url(#${uid}-c)`}>
        <g ref={gRef}>
          <path d={`${line}L${lastX},${height}L0,${height}Z`} fill={`url(#${uid}-g)`} />
          <path d={line} fill="none" strokeWidth={1.5} strokeLinejoin="round" strokeLinecap="round" style={{ stroke: color }} />
        </g>
      </g>
      <circle className="cp-chart-ring" cx={lastX} cy={lastY} r={3} fill="none" strokeWidth={1.25} style={{ stroke: color, opacity: 1 }} />
      <circle cx={lastX} cy={lastY} r={2.4} strokeWidth={1.25} style={{ fill: color, stroke: panel }} />
    </svg>
  );
}
