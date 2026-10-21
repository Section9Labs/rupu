// Chart — the living chart tile (spec §"Charts"; ports charts-v2.html). One
// engine, one visual language: thin stacked areas / overlaid lines / stacked
// columns / a composed dual-axis mix, all sharing a real baseline with a
// recessive grid, round y-ticks above the max, rounded data-ends, an optional
// direct-labelled peak, a breathing now-pulse on the latest point, and an
// interruptible hover crosshair + tooltip — with a prefers-reduced-motion
// fallback (no draw-in, no pulse, static).
//
// Pure-SVG (no recharts). Geometry is the tested `lib/chart/area.ts`; token
// colors come from theme tokens via `useThemeColors` (right in light + dark,
// never a raw hex) — except where a caller supplies a dynamic palette through
// `ChartSeries.rawColor` (the usage timelines, which color per model/pivot).
// Motion is CSS classes in `styles.css`, all guarded by
// `@media (prefers-reduced-motion: reduce)`. Series are given bottom-first
// (stacking order); the last series is the top of the stack (the attention
// band) and owns the now-pulse.

import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { useThemeColors, type ColorKey } from '../../lib/useThemeColors';
import { cn } from '../../lib/cn';
import { useDragSelection } from '../dashboard/useDragSelection';
import {
  areaPath,
  bandCenters,
  bandWidth,
  cumulativeTops,
  linePoints,
  niceScale,
  peakIndex,
  roundedTopRect,
  seriesMax,
  totals,
  xsFor,
  yOf,
  type Plot,
} from '../../lib/chart/area';

/** Width the chart is laid out at before (or without) a measured container —
 *  the dashboard tile's width, and what jsdom tests render at. */
const DEFAULT_W = 480;
/** Default plot height in CSS pixels. */
const DEFAULT_H = 176;
/** Margins around the plot box, in CSS pixels. The SVG is drawn 1:1 with its
 *  container (never a fixed viewBox stretched to fit), so type, strokes and
 *  dots keep their designed size on any width. */
const PAD = { left: 48, right: 14, composedRight: 48, top: 26, bottom: 28 };
/** Smallest gap between two x-tick labels, and between two reference-line
 *  labels, in CSS pixels. */
const MIN_TICK_GAP = 56;
const MIN_REF_LABEL_GAP = 8;
/** Approximate advance of one monospace glyph at the 9-10px label sizes. */
const MONO_CHAR_W = 6;

/** The plot box for a `w`×`h` chart. */
export function plotFor(w: number, h: number, composed: boolean): Plot {
  return {
    x0: PAD.left,
    x1: Math.max(PAD.left + 1, w - (composed ? PAD.composedRight : PAD.right)),
    y0: h - PAD.bottom,
    yTop: PAD.top,
  };
}

/** x-tick indices for `n` buckets at x positions `xs`: the first and the last,
 *  the `peak` when there is room, and evenly spaced ticks in between, keeping
 *  every label at least `minGap` px from its neighbours. Pure, for testing. */
export function spacedTicks(xs: number[], peak: number, minGap = MIN_TICK_GAP): number[] {
  const n = xs.length;
  if (n === 0) return [];
  if (n === 1) return [0];
  const span = xs[n - 1] - xs[0];
  const slots = Math.max(1, Math.floor(span / (minGap * 2)));
  const candidates = [0, n - 1];
  if (peak > 0 && peak < n - 1) candidates.push(peak);
  for (let k = 1; k < slots; k++) candidates.push(Math.round(((n - 1) * k) / slots));
  const kept: number[] = [];
  for (const i of [...new Set(candidates)]) {
    if (kept.every((j) => Math.abs(xs[i] - xs[j]) >= minGap)) kept.push(i);
  }
  return kept.sort((a, b) => a - b);
}

/** A reference-line label laid out on the plot: where its text sits and which
 *  way it anchors, or `null` when it would collide with the previous one. */
export interface PlacedRefLabel {
  index: number;
  text: string;
  x: number;
  anchor: 'start' | 'middle' | 'end';
}

/** Lay out reference-line labels left to right: shorten long ones and keep
 *  them inside the plot. When two would overlap, the one naming the wider
 *  segment (the span to the next line) keeps its label and the other is
 *  dropped (its dashed line still draws). Pure, for testing. */
export function placeRefLabels(
  refs: { index: number; label?: string }[],
  xs: number[],
  plot: Plot,
): PlacedRefLabel[] {
  const sorted = [...refs]
    .filter((r) => r.label && r.index >= 0 && r.index < xs.length)
    .sort((a, b) => a.index - b.index);
  const kept: { label: PlacedRefLabel; left: number; right: number; span: number }[] = [];
  sorted.forEach((r, k) => {
    const raw = r.label as string;
    const text = raw.length > 12 ? `${raw.slice(0, 11)}…` : raw;
    const w = text.length * MONO_CHAR_W;
    const x = xs[r.index];
    const nextX = k + 1 < sorted.length ? xs[sorted[k + 1].index] : plot.x1;
    let anchor: PlacedRefLabel['anchor'] = 'start';
    let left = x + 4;
    if (left + w > plot.x1) {
      anchor = 'end';
      left = x - 4 - w;
    }
    const entry = {
      label: { index: r.index, text, x: anchor === 'start' ? x + 4 : x - 4, anchor },
      left,
      right: left + w,
      span: nextX - x,
    };
    const prev = kept[kept.length - 1];
    if (prev && left < prev.right + MIN_REF_LABEL_GAP) {
      if (entry.span > prev.span) kept[kept.length - 1] = entry;
      return;
    }
    kept.push(entry);
  });
  return kept.map((k) => k.label);
}

/** The width of the element `ref` points at, tracked as it resizes. Falls back
 *  to `fallback` until measured (and where there is no ResizeObserver). */
function useMeasuredWidth(ref: React.RefObject<HTMLElement | null>, fallback: number): number {
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
/** 1px seam between stacked column segments (app-screens §Charts). */
const SEAM = 1;

export interface ChartSeries {
  key: string;
  label: string;
  /** One value per bucket; same length/order as `dates`. */
  values: number[];
  /** Token color key — stroke + a tint fill. Optional when `rawColor` is given. */
  color?: ColorKey;
  /** A raw CSS color (e.g. a dynamic palette entry) that overrides `color`. For
   *  dynamic per-series palettes — area fills then paint via `fill`+`fillOpacity`
   *  attrs rather than a pre-alpha'd token. */
  rawColor?: string;
  /** Fill opacity for the band (default 0.16). */
  fillOpacity?: number;
  /** `composed` kind only: the mark this series draws (default `line`). */
  mark?: 'area' | 'line';
  /** `composed` kind only: which axis this series is scaled against (default `left`). */
  axis?: 'left' | 'right';
}

/** Which mark the same chart engine draws. All share one frame (grid, ticks,
 *  baseline), one reveal, one crosshair/tooltip, one now-pulse and one count-up
 *  headline — the "living" language, applied to:
 *   - `area`     stacked tinted areas + top lines (the default, unchanged);
 *   - `line`     overlaid, unstacked series lines (no fills), each its own hue;
 *   - `column`   stacked columns rising to the baseline, rounded tops, seams;
 *   - `composed` an overlaid mix where each series picks its own `mark` and is
 *                scaled against its own `axis` (left/right) — a dual-axis chart. */
export type ChartKind = 'area' | 'line' | 'column' | 'composed';

/** A dashed vertical reference line (e.g. a step/turn boundary). */
export interface ChartRefLine {
  /** The bucket index the line sits on. */
  index: number;
  /** A short label drawn at the top of the line. */
  label?: string;
}

export interface ChartProps {
  /** x labels, one per bucket. */
  dates: string[];
  /** Bottom-first stacking order (stacked for `area`/`column`; overlaid for `line`/`composed`). */
  series: ChartSeries[];
  /** The mark this chart draws (default `area`). */
  kind?: ChartKind;
  title?: string;
  /** Sub-line under the title. */
  subtitle?: string;
  /** Right-aligned headline figure (e.g. {value:"92%", label:"ok · 100 cycles"}). */
  headline?: { value: React.ReactNode; label?: React.ReactNode };
  /** A quiet explanatory line under the plot. */
  note?: React.ReactNode;
  /** Peak sub-label, e.g. (i,total) => "16 failed". Falls back to "peak". */
  peakSubLabel?: (index: number, total: number) => string;
  /** Format the (left-axis) y-tick labels. Default `String(v)`. */
  yFormat?: (v: number) => string;
  /** `composed` kind: format the right-axis y-tick labels. Defaults to `yFormat`. */
  yFormatRight?: (v: number) => string;
  /** Render the built-in legend (default `true`). */
  showLegend?: boolean;
  /** Wrap the chart in the panel card chrome (default `true`). A caller embedding
   *  the plot inside its own section passes `false` for a bare plot. */
  showCard?: boolean;
  /** Dashed vertical reference lines over the plot. */
  refLines?: ChartRefLine[];
  /**
   * Marquee drag-select. Called with the ordered `(startKey, endKey)` bucket
   * keys once a real drag (not a plain click) finishes — see `useDragSelection`
   * for the state machine (and its contract: always ordered). INERT when
   * omitted. The keys come from `selectKeys` (default: `dates`).
   */
  onSelectRange?: (startKey: string, endKey: string) => void;
  /** Per-bucket keys handed to `onSelectRange` — e.g. full `2026-06-21` day
   *  buckets while `dates` shows the short `06-21` tick. Defaults to `dates`. */
  selectKeys?: string[];
  /** Per-bucket tooltip titles — richer than the compact x-tick (e.g.
   *  `turn 3 · review` while `dates` shows just `3`). Defaults to `dates`. */
  tooltipLabels?: string[];
  /** Accessible name for the plot. */
  ariaLabel?: string;
  /** Plot height in CSS pixels (default 176). The width always follows the container. */
  height?: number;
  className?: string;
}

/** One tooltip row — pure, exported for testing. `value` stays numeric; the
 *  component formats it at render with the series' axis formatter. */
export interface TooltipRow {
  key: string;
  label: string;
  color?: ColorKey;
  rawColor?: string;
  value: number;
}

/** The tooltip model for one bucket — pure, exported for testing. */
export function tooltipModel(series: ChartSeries[], dates: string[], i: number) {
  const rows: TooltipRow[] = series.map((s) => ({
    key: s.key,
    label: s.label,
    color: s.color,
    rawColor: s.rawColor,
    value: s.values[i] ?? 0,
  }));
  const total = rows.reduce((sum, r) => sum + r.value, 0);
  return { date: dates[i] ?? '', rows, total };
}

/** Nearest bucket index to a viewBox-space x. */
function nearestIndex(xa: number[], px: number): number {
  let best = 0;
  let bestD = Infinity;
  for (let i = 0; i < xa.length; i++) {
    const d = Math.abs(xa[i] - px);
    if (d < bestD) {
      bestD = d;
      best = i;
    }
  }
  return best;
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}

/** True when the viewer asked for reduced motion (guarded for SSR/jsdom). */
function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
  try {
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  } catch {
    return false;
  }
}

const COUNT_MS = 750;
function easeOutCubic(t: number): number {
  return 1 - (1 - t) ** 3;
}

/** The headline figure, counting up from 0 on mount. It parses a LEADING number
 *  from a string value ("92%", "518+", "1,284") and animates just that part,
 *  keeping the suffix; anything without one (an em-dash, a node) renders
 *  unchanged. Reduced motion → the final value immediately. */
function AnimatedFigure({ value }: { value: React.ReactNode }) {
  const parsed = typeof value === 'string' ? value.match(/^([\d,]+(?:\.\d+)?)(.*)$/s) : null;
  const target = parsed ? Number(parsed[1].replace(/,/g, '')) : null;
  const suffix = parsed ? parsed[2] : '';
  const decimals = parsed && parsed[1].includes('.') ? parsed[1].split('.')[1].length : 0;
  const [n, setN] = useState<number | null>(() =>
    target != null && !prefersReducedMotion() ? 0 : target,
  );

  useEffect(() => {
    if (target == null || prefersReducedMotion()) {
      setN(target);
      return;
    }
    let raf = 0;
    const start = performance.now();
    const tick = (now: number) => {
      const t = Math.min(1, (now - start) / COUNT_MS);
      setN(target * easeOutCubic(t));
      if (t < 1) raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [target]);

  if (target == null || n == null) return <>{value}</>;
  const shown = decimals > 0 ? n.toFixed(decimals) : Math.round(n).toLocaleString();
  return (
    <>
      {shown}
      {suffix}
    </>
  );
}

export function Chart({
  dates,
  series,
  kind = 'area',
  title,
  subtitle,
  headline,
  note,
  peakSubLabel,
  yFormat,
  yFormatRight,
  showLegend = true,
  showCard = true,
  refLines,
  onSelectRange,
  selectKeys,
  tooltipLabels,
  ariaLabel,
  height = DEFAULT_H,
  className,
}: ChartProps) {
  const colors = useThemeColors();
  const svgRef = useRef<SVGSVGElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const width = useMeasuredWidth(boxRef, DEFAULT_W);
  const [hover, setHover] = useState<number | null>(null);
  const [drawn, setDrawn] = useState(false);
  const uid = useId().replace(/:/g, '');
  const clipId = `cp-reveal-${uid}`;
  const drag = useDragSelection(onSelectRange);

  // Trigger the draw-in on the next frame after mount (CSS handles the
  // reduced-motion fallback — the riser just appears).
  useEffect(() => {
    const id = requestAnimationFrame(() => setDrawn(true));
    return () => cancelAnimationFrame(id);
  }, []);

  const n = dates.length;
  const keys = selectKeys ?? dates;
  const valueSeries = series.map((s) => s.values);
  const composed = kind === 'composed';
  const plot = plotFor(width, height, composed);
  const xTickY = height - 12;
  // Area + column stack (scale on the per-bucket total); line/composed overlay
  // (scale on the largest single value, series never summed).
  const stacked = kind === 'area' || kind === 'column';

  /** Resolve a series' paint color: a raw palette entry wins over a token. */
  const seriesColor = (s: ChartSeries): string => s.rawColor ?? colors.get(s.color ?? 'status.running');
  /** Fill props for an area band — `fillOpacity` attr for a raw color, a
   *  pre-alpha'd token otherwise (so the Dashboard's token areas are unchanged). */
  const fillAttrs = (s: ChartSeries): { fill: string; fillOpacity?: number } =>
    s.rawColor
      ? { fill: s.rawColor, fillOpacity: s.fillOpacity ?? 0.16 }
      : { fill: colors.alpha(s.color ?? 'status.running', s.fillOpacity ?? 0.16) };

  const tot = totals(valueSeries);
  // Primary (left) scale.
  const leftVals = composed
    ? series.filter((s) => (s.axis ?? 'left') !== 'right').map((s) => s.values)
    : valueSeries;
  const dataMax = stacked ? (tot.length ? Math.max(...tot) : 0) : seriesMax(leftVals);
  const scale = niceScale(dataMax);
  // Composed charts get a second, independent scale for their right-axis series.
  const rightVals = composed ? series.filter((s) => s.axis === 'right').map((s) => s.values) : [];
  const rightScale = composed ? niceScale(seriesMax(rightVals)) : scale;
  const scaleOf = (s: ChartSeries) => (composed && s.axis === 'right' ? rightScale : scale);

  const xa = xsFor(n, plot);
  // Columns sit centred in n equal slots; area/line/composed points sit on the plot edges.
  const xc = kind === 'column' ? bandCenters(n, plot) : xa;
  const barW = bandWidth(n, plot);
  const tops = cumulativeTops(valueSeries); // per series cumulative top (stacked)
  const peak = peakIndex(tot);

  const hasData = n > 0 && series.length > 0;

  /** y of series `si`'s mark at bucket `i`: the cumulative top when stacked,
   *  the series' own value (on its axis' scale) when overlaid. */
  const markY = (si: number, i: number) =>
    yOf(stacked ? tops[si][i] : valueSeries[si][i], scaleOf(series[si]).max, plot);

  // x-ticks: first, last, the peak and evenly spaced ones between, never closer
  // than MIN_TICK_GAP px.
  const tickIdx = spacedTicks(xc, peak);
  const refLabels = hasData && refLines ? placeRefLabels(refLines, xc, plot) : [];

  // The now-pulse series: the top of the stack, or (composed) the hero area.
  const topSeries = series.length - 1;
  const composedAreaIdx = series.findIndex((s) => (s.mark ?? 'line') === 'area');
  const pulseIdx = composed && composedAreaIdx >= 0 ? composedAreaIdx : topSeries;
  const lastX = xc[n - 1];
  const lastTopY = hasData ? markY(pulseIdx, n - 1) : plot.y0;
  const pulseStroke = series[pulseIdx] ? seriesColor(series[pulseIdx]) : colors.get('status.running');

  const fmtY = yFormat ?? ((v: number) => String(v));
  const fmtYRight = yFormatRight ?? fmtY;
  const formatForSeries = (s: ChartSeries) => (composed && s.axis === 'right' ? fmtYRight : fmtY);

  // Left-axis ticks carry the hero series' hue in a composed chart (so the
  // axis reads as "this scale belongs to the area"); otherwise they're quiet.
  const leftAxisSeries = composed ? series.find((s) => (s.axis ?? 'left') !== 'right') : undefined;
  const leftTickFill = leftAxisSeries ? seriesColor(leftAxisSeries) : colors.inkDim;

  const bucketX = (key: string): number | null => {
    const i = keys.indexOf(key);
    return i >= 0 ? xc[i] : null;
  };
  // The in-progress selection band (brand-tinted), under the marks.
  const slot = n > 1 ? (plot.x1 - plot.x0) / (n - 1) : plot.x1 - plot.x0;
  let band: { x: number; w: number } | null = null;
  if (drag.band) {
    const a = bucketX(String(drag.band.start));
    const b = bucketX(String(drag.band.current));
    if (a != null && b != null) {
      const left = clamp(Math.min(a, b) - slot / 2, plot.x0, plot.x1);
      const right = clamp(Math.max(a, b) + slot / 2, plot.x0, plot.x1);
      band = { x: left, w: Math.max(0, right - left) };
    }
  }

  const indexFromEvent = (e: React.MouseEvent): number | null => {
    const svg = svgRef.current;
    if (!svg || !hasData) return null;
    const r = svg.getBoundingClientRect();
    if (r.width === 0) return null; // jsdom / not laid out
    const px = ((e.clientX - r.left) / r.width) * width;
    return nearestIndex(xc, px);
  };

  const onMove = (e: React.MouseEvent) => {
    const i = indexFromEvent(e);
    if (i == null) return;
    setHover(i);
    if (onSelectRange) drag.onMouseMove({ activeLabel: keys[i] });
  };
  const onDown = (e: React.MouseEvent) => {
    if (!onSelectRange) return;
    const i = indexFromEvent(e);
    if (i == null) return;
    drag.onMouseDown({ activeLabel: keys[i] });
  };
  const onUp = () => drag.finalizeDrag();
  const onLeave = () => {
    setHover(null);
    drag.finalizeDrag();
  };

  const tip = hover != null && hasData ? tooltipModel(series, dates, hover) : null;
  const hoverX = hover != null ? xc[hover] : 0;
  const hoverTopY = hover != null ? markY(pulseIdx, hover) : 0;

  const swatch = (color: ColorKey | undefined, rawColor: string | undefined): string =>
    rawColor ?? (color ? colors.get(color) : colors.get('status.running'));

  return (
    <div className={cn(showCard && 'rounded-xl border border-border bg-panel p-4', className)}>
      {(title || headline) && (
        <div className="mb-1 flex items-start justify-between gap-4">
          <div className="min-w-0">
            {title && <div className="text-lead font-semibold text-ink">{title}</div>}
            {subtitle && <div className="mt-0.5 text-note text-ink-dim">{subtitle}</div>}
          </div>
          {headline && (
            <div className="shrink-0 text-right">
              <div className="text-xl font-semibold leading-none tabular-nums tracking-tight text-ink">
                <AnimatedFigure value={headline.value} />
              </div>
              {headline.label && <div className="mt-1 text-note text-ink-dim">{headline.label}</div>}
            </div>
          )}
        </div>
      )}

      {/* legend */}
      {showLegend && series.length > 0 && (
        <div className="mb-1 flex flex-wrap gap-x-4 gap-y-1">
          {series.map((s) => (
            <span key={s.key} className="inline-flex items-center gap-1.5 text-note text-ink-dim">
              <span
                className="h-2 w-2 shrink-0 rounded-sm"
                style={{ background: seriesColor(s) }}
                aria-hidden
              />
              {s.label}
            </span>
          ))}
        </div>
      )}

      <div ref={boxRef} className="relative">
        <svg
          ref={svgRef}
          viewBox={`0 0 ${width} ${height}`}
          width="100%"
          height={height}
          className="block overflow-visible"
          style={onSelectRange ? { cursor: 'crosshair' } : undefined}
          role="img"
          aria-label={ariaLabel ?? title ?? 'chart'}
          onMouseMove={onMove}
          onMouseDown={onDown}
          onMouseUp={onUp}
          onMouseLeave={onLeave}
        >
          {/* left-to-right reveal: the plot area wipes open as the chart draws
              in, so the series trace across time rather than stretching up. */}
          <defs>
            <clipPath id={clipId}>
              <rect
                className={cn('cp-chart-wipe', drawn && 'cp-in')}
                x={plot.x0}
                y={plot.yTop - 6}
                width={plot.x1 - plot.x0}
                height={plot.y0 - plot.yTop + 10}
              />
            </clipPath>
          </defs>

          {/* gridlines + y ticks (left / primary axis) */}
          {scale.ticks.map((t) => {
            const y = yOf(t, scale.max, plot);
            const isBase = t === 0;
            return (
              <g key={t}>
                <line
                  x1={plot.x0}
                  y1={y}
                  x2={plot.x1}
                  y2={y}
                  style={{ stroke: isBase ? colors.border : colors.alpha('border', 0.6) }}
                  strokeWidth={1}
                />
                <text
                  x={plot.x0 - 6}
                  y={y + 3}
                  textAnchor="end"
                  className="font-mono"
                  style={{ fill: leftTickFill, fontSize: 10 }}
                >
                  {fmtY(t)}
                </text>
              </g>
            );
          })}

          {/* right-axis ticks (composed only) — labels only, no second grid */}
          {composed &&
            rightScale.ticks.map((t) => (
              <text
                key={`ry-${t}`}
                x={plot.x1 + 6}
                y={yOf(t, rightScale.max, plot) + 3}
                textAnchor="start"
                className="font-mono"
                style={{ fill: colors.inkMute, fontSize: 10 }}
              >
                {fmtYRight(t)}
              </text>
            ))}

          {/* selection band (marquee drag-select), beneath the marks */}
          {band && band.w > 0 && (
            <rect
              data-chart-band
              x={band.x}
              y={plot.yTop}
              width={band.w}
              height={plot.y0 - plot.yTop}
              style={{ fill: colors.alpha('brand.500', 0.15), pointerEvents: 'none' }}
            />
          )}

          {/* marks (area / line / column / composed), revealed left-to-right by the wipe clip */}
          {hasData && (
            <g clipPath={`url(#${clipId})`}>
              {/* area: stacked tinted bands + their top lines */}
              {kind === 'area' &&
                series.map((s, si) => {
                  const top = tops[si];
                  const bottom = si === 0 ? null : tops[si - 1];
                  return (
                    <path
                      key={`a-${s.key}`}
                      d={areaPath(top, bottom, xa, scale.max, plot)}
                      {...fillAttrs(s)}
                    />
                  );
                })}
              {kind === 'area' &&
                series.map((s, si) => (
                  <polyline
                    key={`l-${s.key}`}
                    points={linePoints(tops[si], xa, scale.max, plot)}
                    fill="none"
                    strokeWidth={2}
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    style={{ stroke: seriesColor(s) }}
                  />
                ))}

              {/* line: one overlaid line per series, on its own values, no fill */}
              {kind === 'line' &&
                series.map((s) => (
                  <polyline
                    key={`l-${s.key}`}
                    points={linePoints(s.values, xa, scale.max, plot)}
                    fill="none"
                    strokeWidth={2}
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    style={{ stroke: seriesColor(s) }}
                  />
                ))}

              {/* composed: each series draws its own mark on its own axis' scale,
                  overlaid (never stacked) — the dual-axis treatment. */}
              {composed &&
                series.map((s) => {
                  const max = scaleOf(s).max;
                  if ((s.mark ?? 'line') === 'area') {
                    return (
                      <g key={`cm-${s.key}`}>
                        <path d={areaPath(s.values, null, xa, max, plot)} {...fillAttrs(s)} />
                        <polyline
                          points={linePoints(s.values, xa, max, plot)}
                          fill="none"
                          strokeWidth={2}
                          strokeLinecap="round"
                          strokeLinejoin="round"
                          style={{ stroke: seriesColor(s) }}
                        />
                      </g>
                    );
                  }
                  return (
                    <polyline
                      key={`cm-${s.key}`}
                      points={linePoints(s.values, xa, max, plot)}
                      fill="none"
                      strokeWidth={2}
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      style={{ stroke: seriesColor(s) }}
                    />
                  );
                })}

              {/* column: stacked columns, 1px seams, a rounded top on the cap */}
              {kind === 'column' &&
                xc.map((cx, i) => {
                  // The topmost non-zero segment owns the rounded cap.
                  let cap = -1;
                  for (let si = series.length - 1; si >= 0; si--) {
                    if ((valueSeries[si][i] ?? 0) > 0) {
                      cap = si;
                      break;
                    }
                  }
                  const x = cx - barW / 2;
                  return (
                    <g key={`c-${i}`}>
                      {series.map((s, si) => {
                        if ((valueSeries[si][i] ?? 0) <= 0) return null;
                        const segTop = yOf(tops[si][i], scale.max, plot);
                        const segBot = yOf(si === 0 ? 0 : tops[si - 1][i], scale.max, plot);
                        const h = segBot - segTop - SEAM;
                        if (h <= 0) return null;
                        const fill = seriesColor(s);
                        return si === cap ? (
                          <path
                            key={`seg-${s.key}`}
                            d={roundedTopRect(x, segTop + SEAM, barW, h, 3)}
                            style={{ fill }}
                          />
                        ) : (
                          <rect
                            key={`seg-${s.key}`}
                            x={x}
                            y={segTop + SEAM}
                            width={barW}
                            height={h}
                            style={{ fill }}
                          />
                        );
                      })}
                    </g>
                  );
                })}
            </g>
          )}

          {/* dashed vertical reference lines (step/turn boundaries) */}
          {hasData &&
            refLines?.map((r) =>
              r.index > 0 && r.index < n ? (
                <line
                  key={`ref-${r.index}`}
                  x1={xc[r.index]}
                  y1={plot.yTop}
                  x2={xc[r.index]}
                  y2={plot.y0}
                  style={{ stroke: colors.border, strokeDasharray: '3 3' }}
                  strokeWidth={1}
                />
              ) : null,
            )}

          {refLabels.map((l) => (
            <text
              key={`refl-${l.index}`}
              x={l.x}
              y={plot.yTop + 8}
              textAnchor={l.anchor}
              className="font-mono"
              style={{ fill: colors.inkMute, fontSize: 9 }}
            >
              {l.text}
            </text>
          ))}

          {/* peak marker (stacked kinds only — an overlaid chart has no single
              "total" to label) */}
          {hasData && stacked && peak >= 0 && tot[peak] > 0 && (
            <g>
              <line
                x1={xc[peak]}
                y1={yOf(tot[peak], scale.max, plot)}
                x2={xc[peak]}
                y2={plot.yTop - 2}
                style={{ stroke: colors.border, strokeDasharray: '2 2' }}
                strokeWidth={1}
              />
              <text
                x={xc[peak]}
                y={plot.yTop - 6}
                textAnchor="middle"
                style={{ fill: colors.ink, fontSize: 11, fontWeight: 600 }}
              >
                {fmtY(tot[peak])}
              </text>
              <text
                x={xc[peak]}
                y={plot.yTop + 5}
                textAnchor="middle"
                className="font-mono"
                style={{ fill: colors.inkDim, fontSize: 9 }}
              >
                {peakSubLabel ? peakSubLabel(peak, tot[peak]) : 'peak'}
              </text>
            </g>
          )}

          {/* now pulse on the latest point (area/line/composed) + plain end-dots
              on the other overlaid series so every line terminates cleanly.
              Columns don't need it — the rightmost bar already reads as "now". */}
          {hasData && kind !== 'column' && (
            <>
              {(kind === 'line' || composed) &&
                series.map((s, si) =>
                  si === pulseIdx ? null : (
                    <circle
                      key={`end-${s.key}`}
                      cx={lastX}
                      cy={markY(si, n - 1)}
                      r={2.6}
                      style={{ fill: seriesColor(s) }}
                    />
                  ),
                )}
              <circle
                className="cp-chart-now"
                cx={lastX}
                cy={lastTopY}
                r={4}
                fill="none"
                strokeWidth={1.5}
                style={{ stroke: pulseStroke }}
              />
              <circle
                className="cp-chart-now-dot"
                cx={lastX}
                cy={lastTopY}
                r={3}
                style={{ fill: pulseStroke }}
              />
            </>
          )}

          {/* x ticks */}
          {tickIdx.map((i) => (
            <text
              key={`x-${i}`}
              x={xc[i]}
              y={xTickY}
              textAnchor={i === 0 ? 'start' : i === n - 1 ? 'end' : 'middle'}
              className="font-mono"
              style={{ fill: colors.inkDim, fontSize: 10 }}
            >
              {dates[i]}
            </text>
          ))}

          {/* crosshair */}
          {tip && (
            <g className="cp-chart-cross" style={{ transform: `translateX(${hoverX}px)` }}>
              <line
                x1={0}
                y1={plot.yTop}
                x2={0}
                y2={plot.y0}
                style={{ stroke: colors.get('status.pending') }}
                strokeWidth={1}
              />
              {kind !== 'column' &&
                series.map((s, si) => (
                  <circle
                    key={`d-${s.key}`}
                    cx={0}
                    cy={markY(si, hover as number)}
                    r={3.2}
                    style={{ fill: seriesColor(s), stroke: colors.panel }}
                    strokeWidth={1.5}
                  />
                ))}
            </g>
          )}
        </svg>

        {/* tooltip */}
        {tip && (
          <div
            className="pointer-events-none absolute z-10 -translate-x-1/2 -translate-y-[112%] whitespace-nowrap rounded-lg border border-border-strong bg-surface px-2.5 py-2 text-note shadow-lg"
            style={{ left: `${(hoverX / width) * 100}%`, top: `${(hoverTopY / height) * 100}%` }}
            role="status"
          >
            <div className="mb-1 font-semibold text-ink">
              {tooltipLabels?.[hover as number] ?? tip.date}
            </div>
            {tip.rows.map((r) => {
              const s = series.find((x) => x.key === r.key);
              const shown = s ? formatForSeries(s)(r.value) : String(r.value);
              return (
                <div key={r.key} className="flex items-center gap-2 leading-relaxed">
                  <span
                    className="h-2 w-2 shrink-0 rounded-sm"
                    style={{ background: swatch(r.color, r.rawColor) }}
                    aria-hidden
                  />
                  <span className="flex-1 pr-3 text-ink-dim">{r.label}</span>
                  <span className="font-mono font-semibold tabular-nums text-ink">{shown}</span>
                </div>
              );
            })}
            {stacked && (
              <div className="mt-1 flex justify-between gap-4 border-t border-border pt-1">
                <span className="text-ink-dim">Total</span>
                <span className="font-mono font-bold tabular-nums text-ink">{fmtY(tip.total)}</span>
              </div>
            )}
          </div>
        )}
      </div>

      {note && <p className="mt-2 text-note text-ink-dim">{note}</p>}
    </div>
  );
}

export default Chart;
