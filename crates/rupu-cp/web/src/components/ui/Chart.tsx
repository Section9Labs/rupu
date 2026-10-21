// Chart — the living chart (spec §"Charts", visual language charts-v2.html).
// One engine for every time series in the CP that isn't the token flow:
// thin stacked areas (`area`, the default) or overlaid lines (`line`) on a
// real baseline, a recessive grid with round ticks just above the data, a
// direct-labelled peak, and a quiet end dot that breathes while the series
// is live.
//
// Motion (styles.css "Chart motion"): the frame settles in from a slight
// blur, the marks are traced left to right by a sweep while their areas rise,
// a stylus glows on the leading edge, the peak stem and end dot land after
// the sweep. New data of the same shape morphs into place. Hover is a fading
// crosshair with a dot riding each series, a tooltip that follows it, and the
// future dimmed behind the pointer; hovering a legend entry isolates its
// series. Reduced motion: the finished chart fades in, nothing moves.
//
// Pure SVG drawn 1:1 with its box (never a stretched viewBox), so type and
// strokes keep their size at any width. Geometry is lib/chart/area.ts; colors
// are theme tokens (`color`) or a caller's dynamic palette (`rawColor`).

import { useEffect, useId, useRef, useState } from 'react';
import { useThemeColors, type ColorKey } from '../../lib/useThemeColors';
import { cn } from '../../lib/cn';
import { useDragSelection } from '../dashboard/useDragSelection';
import { cumulativeTops, peakIndex, seriesMax, tightScale, totals, xsFor, yOf, type Plot } from '../../lib/chart/area';
import { prefersReducedMotion, useDrawIn, useMeasuredWidth, useMorphedSeries } from '../../lib/chart/motion';
import {
  ChartLegend,
  ChartTooltip,
  CrossGradient,
  Crosshair,
  FadeGradient,
  GlowFilter,
  Stylus,
  useHoverIndex,
  type TipRow,
} from '../charts/parts';

/** Width the chart is laid out at before (or without) a measured container —
 *  the dashboard tile's width, and what jsdom tests render at. */
const DEFAULT_W = 480;
/** Default plot height in CSS pixels. */
const DEFAULT_H = 176;
/** Margins around the plot box, in CSS pixels. */
const PAD = { left: 48, right: 14, top: 26, bottom: 28 };
/** Smallest gap between two x-tick labels, and between two reference-line
 *  labels, in CSS pixels. */
const MIN_TICK_GAP = 56;
const MIN_REF_LABEL_GAP = 8;
/** Approximate advance of one monospace glyph at the 9-10px label sizes. */
const MONO_CHAR_W = 6;

/** The plot box for a `w`×`h` chart. */
export function plotFor(w: number, h: number): Plot {
  return {
    x0: PAD.left,
    x1: Math.max(PAD.left + 1, w - PAD.right),
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
 *  way it anchors. */
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

/** Index ranges where a series has something to draw: a bucket counts when it
 *  or a neighbour is non-zero, so a band that is empty for a stretch draws no
 *  line over the band beneath it. Pure, for testing. */
export function liveSegments(values: number[]): [number, number][] {
  const out: [number, number][] = [];
  let start = -1;
  for (let i = 0; i < values.length; i++) {
    const live = values[i] > 0 || (values[i - 1] ?? 0) > 0 || (values[i + 1] ?? 0) > 0;
    if (live && start < 0) start = i;
    if (!live && start >= 0) {
      out.push([start, i - 1]);
      start = -1;
    }
  }
  if (start >= 0) out.push([start, values.length - 1]);
  return out;
}

export interface ChartSeries {
  key: string;
  label: string;
  /** One value per bucket; same length/order as `dates`. */
  values: number[];
  /** Token color key. Optional when `rawColor` is given. */
  color?: ColorKey;
  /** A raw CSS color (a dynamic palette entry) that overrides `color`. */
  rawColor?: string;
  /** Strength of the band's fade at its top (default 0.3). */
  fillOpacity?: number;
  /** A value shown next to the series in the legend (e.g. its total). */
  legendValue?: React.ReactNode;
}

/** `area`: stacked fading bands with their top lines (the default).
 *  `line`: overlaid, unstacked lines, each its own hue. */
export type ChartKind = 'area' | 'line';

/** A dashed vertical reference line (e.g. a boundary). */
export interface ChartRefLine {
  /** The bucket index the line sits on. */
  index: number;
  /** A short label drawn at the top of the line. */
  label?: string;
}

export interface ChartProps {
  /** x labels, one per bucket. */
  dates: string[];
  /** Bottom-first stacking order (stacked for `area`, overlaid for `line`). */
  series: ChartSeries[];
  kind?: ChartKind;
  title?: string;
  /** Sub-line under the title. */
  subtitle?: string;
  /** Right-aligned headline figure (e.g. {value:"92%", label:"ok · 100 cycles"}). */
  headline?: { value: React.ReactNode; label?: React.ReactNode };
  /** A quiet explanatory line under the plot. */
  note?: React.ReactNode;
  /** Peak sub-label, e.g. (i,total) => "16 failed". Falls back to the bucket's date. */
  peakSubLabel?: (index: number, total: number) => string;
  /** Format values (tooltip, peak) and, unless `yTickFormat` is given, the
   *  y-tick labels. Default `String(v)`. */
  yFormat?: (v: number) => string;
  /** Format the y-tick labels (round numbers, so usually coarser than values). */
  yTickFormat?: (v: number) => string;
  /** Render the built-in legend (default `true`). */
  showLegend?: boolean;
  /** Wrap the chart in the panel card chrome (default `true`). */
  showCard?: boolean;
  /** Dashed vertical reference lines over the plot. */
  refLines?: ChartRefLine[];
  /** The latest bucket is "now": its end dot breathes (default `true`). */
  live?: boolean;
  /**
   * Marquee drag-select. Called with the ordered `(startKey, endKey)` bucket
   * keys once a real drag (not a plain click) finishes — see `useDragSelection`.
   * INERT when omitted. The keys come from `selectKeys` (default: `dates`).
   */
  onSelectRange?: (startKey: string, endKey: string) => void;
  /** Per-bucket keys handed to `onSelectRange`. Defaults to `dates`. */
  selectKeys?: string[];
  /** Per-bucket tooltip titles, richer than the compact x-tick. Defaults to `dates`. */
  tooltipLabels?: string[];
  /** Accessible name for the plot. */
  ariaLabel?: string;
  /** Plot height in CSS pixels (default 176). The width follows the container. */
  height?: number;
  className?: string;
}

/** One tooltip row — pure, exported for testing. */
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

/** Nearest bucket index to a plot-space x. */
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

const pts = (xs: number[], ys: number[], a = 0, b = xs.length - 1) => {
  let d = '';
  for (let i = a; i <= b; i++) d += `${i === a ? 'M' : 'L'}${xs[i].toFixed(1)},${ys[i].toFixed(1)}`;
  return d;
};

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
  yTickFormat,
  showLegend = true,
  showCard = true,
  refLines,
  live = true,
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
  const drawn = useDrawIn();
  const hover = useHoverIndex();
  const [focus, setFocus] = useState<string | null>(null);
  const uid = useId().replace(/:/g, '');
  const drag = useDragSelection(onSelectRange);

  const n = dates.length;
  const keys = selectKeys ?? dates;
  const stacked = kind === 'area';
  const plot = plotFor(width, height);
  const xTickY = height - 10;

  // Same-shaped new data morphs into place.
  const values = useMorphedSeries(series.map((s) => s.values));
  const raw = series.map((s) => s.values);
  const tot = totals(raw);
  const dataMax = stacked ? (tot.length ? Math.max(...tot) : 0) : seriesMax(raw);
  const scale = tightScale(dataMax * 1.02, 3, 5);
  const xs = xsFor(n, plot);
  const tops = cumulativeTops(values);
  const peak = peakIndex(stacked ? tot : raw.length ? raw[0].map((_, i) => Math.max(...raw.map((s) => s[i] ?? 0))) : []);
  const hasData = n > 0 && series.length > 0;
  const yv = (v: number) => yOf(v, scale.max, plot);

  /** y of series `si`'s mark at bucket `i`. */
  const markY = (si: number, i: number) => yv(stacked ? tops[si]?.[i] ?? 0 : values[si]?.[i] ?? 0);

  const tickIdx = spacedTicks(xs, peak);
  const refLabels = hasData && refLines ? placeRefLabels(refLines, xs, plot) : [];

  const seriesColor = (s: ChartSeries): string => s.rawColor ?? colors.get(s.color ?? 'chart.1');
  const fmtY = yFormat ?? ((v: number) => String(v));
  const fmtTick = yTickFormat ?? fmtY;

  // The series whose line tops the chart at the end: the top of the stack
  // that has a value there (an empty top band would put the dot on another
  // band's line), else the largest line.
  let endIdx = series.length - 1;
  if (hasData) {
    if (stacked) {
      for (let si = series.length - 1; si >= 0; si--) {
        if ((raw[si][n - 1] ?? 0) > 0) {
          endIdx = si;
          break;
        }
      }
    } else {
      endIdx = raw.reduce((best, s, si) => ((s[n - 1] ?? 0) > (raw[best][n - 1] ?? 0) ? si : best), 0);
    }
  }
  const endColor = series[endIdx] ? seriesColor(series[endIdx]) : colors.get('chart.1');
  const lastX = xs[n - 1];
  const endY = hasData ? markY(endIdx, n - 1) : plot.y0;

  // ── drag-select band ─────────────────────────────────────────────────────
  const bucketX = (key: string): number | null => {
    const i = keys.indexOf(key);
    return i >= 0 ? xs[i] : null;
  };
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
    return nearestIndex(xs, ((e.clientX - r.left) / r.width) * width);
  };
  const onMove = (e: React.MouseEvent) => {
    const i = indexFromEvent(e);
    if (i == null) return;
    hover.set(i);
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
    hover.set(null);
    drag.finalizeDrag();
  };

  // ── tooltip ──────────────────────────────────────────────────────────────
  const hi = hover.index;
  const tip = hi != null && hasData ? tooltipModel(series, dates, hi) : null;
  const hoverX = hi != null ? xs[hi] : 0;
  let tipRows: TipRow[] = [];
  if (tip) {
    // top of the stack first, the way the bands read
    const ordered = stacked ? [...tip.rows].reverse() : tip.rows;
    const many = ordered.length > 4;
    tipRows = ordered
      .filter((r) => !many || r.value > 0)
      .map((r) => {
        const s = series.find((x) => x.key === r.key) as ChartSeries;
        return {
          key: r.key,
          label: r.label,
          value: fmtY(r.value),
          color: seriesColor(s),
          muted: r.value === 0,
        };
      });
    if (stacked && series.length > 1) tipRows.push({ key: '__total', label: 'Total', value: fmtY(tip.total), total: true });
  }
  const tipY = hi != null && hasData ? Math.min(...series.map((_, si) => markY(si, hi))) : 0;

  const opacityOf = (s: ChartSeries) => (focus && focus !== s.key ? 0.14 : 1);
  const sweepId = `${uid}-sweep`;
  const showPeak = hasData && peak >= 0 && (stacked ? tot[peak] : seriesMax(raw)) > 0;

  return (
    <div
      className={cn(
        showCard && 'rounded-xl border border-border bg-panel p-4',
        drawn && 'cp-in',
        tip && 'cp-chart-hovering',
        className,
      )}
    >
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

      {showLegend && (series.length > 1 || series[0]?.legendValue != null) && (
        <ChartLegend
          className="mb-1"
          focus={focus}
          onFocus={setFocus}
          items={series.map((s) => ({ key: s.key, label: s.label, color: seriesColor(s), value: s.legendValue }))}
        />
      )}

      <div ref={boxRef} className="relative">
        <svg
          ref={svgRef}
          viewBox={`0 0 ${width} ${height}`}
          width="100%"
          height={height}
          className="block select-none overflow-visible"
          style={onSelectRange ? { cursor: 'crosshair' } : undefined}
          role="img"
          aria-label={ariaLabel ?? title ?? 'chart'}
          onMouseMove={onMove}
          onMouseDown={onDown}
          onMouseUp={onUp}
          onMouseLeave={onLeave}
        >
          <defs>
            <clipPath id={sweepId}>
              <rect className="cp-chart-sweep" x={plot.x0 - 6} y={0} width={plot.x1 - plot.x0 + 12} height={height} />
            </clipPath>
            {series.map((s, si) => (
              <FadeGradient
                key={s.key}
                id={`${uid}-g${si}`}
                color={seriesColor(s)}
                top={Math.min(0.5, s.fillOpacity ?? 0.3)}
                bottom={stacked && si > 0 ? 0.08 : 0.02}
              />
            ))}
            <GlowFilter id={`${uid}-glow`} />
            <CrossGradient id={`${uid}-cross`} color={colors.ink} y0={plot.yTop - 6} y1={plot.y0} />
          </defs>

          {/* frame: gridlines draw out from the axis, ticks settle in */}
          {scale.ticks.map((t, k) => {
            const y = yv(t);
            const isBase = t === 0;
            return (
              <g key={t}>
                <line
                  x1={plot.x0}
                  y1={y}
                  x2={plot.x1}
                  y2={y}
                  className={isBase ? undefined : 'cp-chart-grid'}
                  style={{
                    ['--i' as string]: k,
                    stroke: isBase ? colors.alpha('inkMute', 0.5) : colors.alpha('border', 0.7),
                  }}
                  strokeWidth={1}
                />
                <text
                  x={plot.x0 - 8}
                  y={y + 3.5}
                  textAnchor="end"
                  className="cp-chart-tick font-mono"
                  style={{ ['--i' as string]: k, fill: colors.inkMute, fontSize: 10 }}
                >
                  {fmtTick(t)}
                </text>
              </g>
            );
          })}

          {band && band.w > 0 && (
            <rect
              data-chart-band
              className="cp-chart-brush"
              x={band.x}
              y={plot.yTop - 4}
              width={band.w}
              height={plot.y0 - plot.yTop + 4}
              rx={4}
              strokeWidth={1}
              style={{
                fill: colors.alpha('brand.500', 0.1),
                stroke: colors.alpha('brand.500', 0.5),
                pointerEvents: 'none',
              }}
            />
          )}

          {/* marks: traced by the sweep, risen from the baseline */}
          {hasData && (
            <g clipPath={`url(#${sweepId})`}>
              <g className="cp-chart-rise">
                {stacked &&
                  series.map((s, si) => {
                    const top = tops[si].map(yv);
                    const bottom = si === 0 ? xs.map(() => plot.y0) : tops[si - 1].map(yv);
                    let d = pts(xs, top);
                    for (let i = n - 1; i >= 0; i--) d += `L${xs[i].toFixed(1)},${bottom[i].toFixed(1)}`;
                    return (
                      <path
                        key={`a-${s.key}`}
                        d={`${d}Z`}
                        fill={`url(#${uid}-g${si})`}
                        className="cp-chart-fade"
                        style={{ opacity: opacityOf(s) }}
                      />
                    );
                  })}
                {/* stacked lines are drawn top band first, so where a thin band
                    sits on a thick one the thick band's line stays on top */}
                {(stacked ? [...series].reverse() : series).map((s) => {
                  const si = series.indexOf(s);
                  const ys = (stacked ? tops[si] : values[si]).map(yv);
                  const segs = stacked ? liveSegments(values[si]) : [[0, n - 1] as [number, number]];
                  const d = segs.map(([a, b]) => pts(xs, ys, a, b)).join('');
                  if (!d) return null;
                  const hero = si === endIdx;
                  return (
                    <g key={`l-${s.key}`} className="cp-chart-fade" style={{ opacity: opacityOf(s) }}>
                      {hero && (
                        <path
                          d={d}
                          fill="none"
                          strokeWidth={5}
                          strokeLinejoin="round"
                          filter={`url(#${uid}-glow)`}
                          style={{ stroke: seriesColor(s), opacity: 0.32 }}
                        />
                      )}
                      <path
                        d={d}
                        fill="none"
                        strokeWidth={2}
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        style={{ stroke: seriesColor(s) }}
                      />
                    </g>
                  );
                })}
              </g>
            </g>
          )}

          {/* dashed vertical reference lines */}
          {hasData &&
            refLines?.map((r) =>
              r.index > 0 && r.index < n ? (
                <line
                  key={`ref-${r.index}`}
                  x1={xs[r.index]}
                  y1={plot.yTop}
                  x2={xs[r.index]}
                  y2={plot.y0}
                  strokeDasharray="2 4"
                  style={{ stroke: colors.border }}
                  strokeWidth={1}
                />
              ) : null,
            )}
          {refLabels.map((l, k) => (
            <text
              key={`refl-${l.index}`}
              x={l.x}
              y={plot.yTop + 8}
              textAnchor={l.anchor}
              className="cp-chart-tick font-mono"
              style={{ ['--i' as string]: k, fill: colors.inkMute, fontSize: 9 }}
            >
              {l.text}
            </text>
          ))}

          {/* the peak: a stem drops from its label to the highest point */}
          {showPeak &&
            (() => {
              const px = xs[peak];
              const value = stacked ? tot[peak] : Math.max(...raw.map((s) => s[peak] ?? 0));
              const sub = peakSubLabel ? peakSubLabel(peak, value) : dates[peak];
              const anchor = px - plot.x0 < 60 ? 'start' : plot.x1 - px < 90 ? 'end' : 'middle';
              return (
                <g pointerEvents="none">
                  <line
                    x1={px}
                    x2={px}
                    y1={plot.yTop - 8}
                    y2={yv(value) - 5}
                    strokeDasharray="2 2"
                    className="cp-chart-stem"
                    style={{ stroke: colors.alpha('inkMute', 0.8) }}
                  />
                  <text
                    x={px}
                    y={plot.yTop - 12}
                    textAnchor={anchor}
                    className="cp-chart-late"
                    style={{ fill: colors.ink, fontSize: 11.5, fontWeight: 620 }}
                  >
                    {fmtY(value)}
                    {sub && (
                      <tspan dx={5} style={{ fill: colors.inkMute, fontSize: 10.5, fontWeight: 450 }}>
                        {sub}
                      </tspan>
                    )}
                  </text>
                </g>
              );
            })()}

          {/* end dots: the latest point of each overlaid line, and the top of
              the stack; it breathes while the chart is live */}
          {hasData && (
            <g pointerEvents="none">
              {kind === 'line' &&
                series.map((s, si) =>
                  si === endIdx ? null : (
                    <circle
                      key={`end-${s.key}`}
                      className="cp-chart-pop"
                      cx={lastX}
                      cy={markY(si, n - 1)}
                      r={2.8}
                      style={{ fill: seriesColor(s) }}
                    />
                  ),
                )}
              {live && (
                <circle
                  className="cp-chart-ring cp-chart-now"
                  cx={lastX}
                  cy={endY}
                  r={4.5}
                  fill="none"
                  strokeWidth={1.5}
                  style={{ stroke: endColor }}
                />
              )}
              <circle
                className="cp-chart-pop"
                cx={lastX}
                cy={endY}
                r={3.4}
                strokeWidth={1.5}
                style={{ fill: endColor, stroke: colors.panel }}
              />
            </g>
          )}

          {hasData && (
            <Stylus
              start={drawn}
              xs={xs}
              ys={xs.map((_, i) => markY(endIdx, i))}
              x0={plot.x0 - 6}
              x1={plot.x1 + 6}
              color={endColor}
            />
          )}

          {/* x ticks */}
          {tickIdx.map((i, k) => (
            <text
              key={`x-${i}`}
              x={xs[i]}
              y={xTickY}
              textAnchor={i === 0 ? 'start' : i === n - 1 ? 'end' : 'middle'}
              className="cp-chart-tick font-mono"
              style={{ ['--i' as string]: k, fill: colors.inkMute, fontSize: 10 }}
            >
              {dates[i]}
            </text>
          ))}

          {/* hover: the future steps back behind the pointer */}
          {tip && (
            <rect
              x={hoverX}
              y={plot.yTop - 6}
              width={Math.max(0, plot.x1 - hoverX + 2)}
              height={plot.y0 - plot.yTop + 6}
              pointerEvents="none"
              style={{ fill: colors.panel, opacity: 0.42 }}
            />
          )}
          {tip && (
            <Crosshair
              x={hoverX}
              y0={plot.yTop - 6}
              y1={plot.y0}
              strokeId={`${uid}-cross`}
              ring={colors.panel}
              appear={hover.appear}
              dots={series.map((s, si) => ({ key: s.key, y: markY(si, hi as number), color: seriesColor(s) }))}
            />
          )}
        </svg>

        <ChartTooltip
          open={!!tip}
          appear={hover.appear}
          x={hoverX}
          y={tipY}
          width={width}
          title={hi != null ? tooltipLabels?.[hi] ?? tip?.date : ''}
          rows={tipRows}
        />
      </div>

      {note && <p className="mt-2 text-note text-ink-dim">{note}</p>}
    </div>
  );
}

const COUNT_MS = 750;
function easeOutCubic(t: number): number {
  return 1 - (1 - t) ** 3;
}

/** The headline figure, counting up from 0 on mount. It parses a LEADING number
 *  from a string value ("92%", "518+", "1,284") and animates just that part,
 *  keeping the suffix; anything without one renders unchanged. */
function AnimatedFigure({ value }: { value: React.ReactNode }) {
  const parsed = typeof value === 'string' ? value.match(/^([\d,]+(?:\.\d+)?)(.*)$/s) : null;
  const target = parsed ? Number(parsed[1].replace(/,/g, '')) : null;
  const suffix = parsed ? parsed[2] : '';
  const decimals = parsed && parsed[1].includes('.') ? parsed[1].split('.')[1].length : 0;
  const [n, setN] = useState<number | null>(() => (target != null && !prefersReducedMotion() ? 0 : target));

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

export default Chart;
