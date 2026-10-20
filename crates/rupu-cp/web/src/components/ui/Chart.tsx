// Chart — the living stacked-area tile (spec §"Charts"; ports charts-v2.html
// CHART 1/2). Thin stacked areas that split a measure, a real baseline with a
// recessive grid, round y-ticks above the max, rounded data-ends, a direct-
// labelled peak, a breathing now-pulse on the latest point, and an
// interruptible hover crosshair + tooltip — with a prefers-reduced-motion
// fallback (no draw-in, no pulse, static).
//
// Pure-SVG (no recharts). Geometry is the tested `lib/chart/area.ts`; colors
// come from theme tokens via `useThemeColors` (so they're right in light + dark
// and never a raw hex). Motion is CSS classes in `styles.css`, all guarded by
// `@media (prefers-reduced-motion: reduce)`. Series are given bottom-first
// (stacking order); the last series is the top of the stack (the attention band)
// and owns the now-pulse.

import { useEffect, useId, useRef, useState } from 'react';
import { useThemeColors, type ColorKey } from '../../lib/useThemeColors';
import { cn } from '../../lib/cn';
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

const VB_W = 480;
const VB_H = 176;
const PLOT: Plot = { x0: 48, x1: 466, y0: 148, yTop: 26 };
const XTICK_Y = 164;
/** 1px seam between stacked column segments (app-screens §Charts). */
const SEAM = 1;

export interface ChartSeries {
  key: string;
  label: string;
  /** One value per bucket; same length/order as `dates`. */
  values: number[];
  /** Token color key — stroke + a tint fill. */
  color: ColorKey;
  /** Fill opacity for the band (default 0.16). */
  fillOpacity?: number;
}

/** Which mark the same chart engine draws. All three share one frame (grid,
 *  ticks, baseline), one reveal, one crosshair/tooltip, one now-pulse and one
 *  count-up headline — the "living" language, applied to:
 *   - `area`   stacked tinted areas + top lines (the default, unchanged);
 *   - `line`   overlaid, unstacked series lines (no fills), each its own hue;
 *   - `column` stacked columns rising to the baseline, rounded tops, seams. */
export type ChartKind = 'area' | 'line' | 'column';

export interface ChartProps {
  /** x labels, one per bucket. */
  dates: string[];
  /** Bottom-first stacking order (stacked for `area`/`column`; overlaid for `line`). */
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
  /** Accessible name for the plot. */
  ariaLabel?: string;
  className?: string;
}

/** The tooltip model for one bucket — pure, exported for testing. */
export function tooltipModel(series: ChartSeries[], dates: string[], i: number) {
  const rows = series.map((s) => ({ key: s.key, label: s.label, color: s.color, value: s.values[i] ?? 0 }));
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
  ariaLabel,
  className,
}: ChartProps) {
  const colors = useThemeColors();
  const svgRef = useRef<SVGSVGElement>(null);
  const [hover, setHover] = useState<number | null>(null);
  const [drawn, setDrawn] = useState(false);
  const uid = useId().replace(/:/g, '');
  const clipId = `cp-reveal-${uid}`;

  // Trigger the draw-in on the next frame after mount (CSS handles the
  // reduced-motion fallback — the riser just appears).
  useEffect(() => {
    const id = requestAnimationFrame(() => setDrawn(true));
    return () => cancelAnimationFrame(id);
  }, []);

  const n = dates.length;
  const valueSeries = series.map((s) => s.values);
  // Area + column stack (scale on the per-bucket total); line overlays
  // (scale on the largest single value, series never summed).
  const stacked = kind !== 'line';
  const tot = totals(valueSeries);
  const dataMax = stacked ? (tot.length ? Math.max(...tot) : 0) : seriesMax(valueSeries);
  const scale = niceScale(Math.max(1, dataMax));
  const xa = xsFor(n, PLOT);
  // Columns sit centred in n equal slots; area/line points sit on the plot edges.
  const xc = kind === 'column' ? bandCenters(n, PLOT) : xa;
  const barW = bandWidth(n, PLOT);
  const tops = cumulativeTops(valueSeries); // per series cumulative top
  const peak = peakIndex(tot);

  const hasData = n > 0;

  /** y of series `si`'s mark at bucket `i`: the cumulative top when stacked,
   *  the series' own value when overlaid (line). */
  const markY = (si: number, i: number) =>
    yOf(stacked ? tops[si][i] : valueSeries[si][i], scale.max, PLOT);

  // x-ticks: first, peak, last (deduped, in order).
  const tickIdx = [...new Set([0, peak, n - 1].filter((i) => i >= 0 && i < n))].sort((a, b) => a - b);

  const topSeries = series.length - 1;
  const lastX = xc[n - 1];
  const lastTopY = hasData ? markY(topSeries, n - 1) : PLOT.y0;
  const pulseColor = series[topSeries]?.color ?? 'status.running';

  const onMove = (e: React.MouseEvent) => {
    const svg = svgRef.current;
    if (!svg || !hasData) return;
    const r = svg.getBoundingClientRect();
    if (r.width === 0) return; // jsdom / not laid out
    const px = ((e.clientX - r.left) / r.width) * VB_W;
    setHover(nearestIndex(xc, px));
  };

  const tip = hover != null && hasData ? tooltipModel(series, dates, hover) : null;
  const hoverX = hover != null ? xc[hover] : 0;
  const hoverTopY = hover != null ? markY(topSeries, hover) : 0;

  return (
    <div className={cn('rounded-xl border border-border bg-panel p-4', className)}>
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
      {series.length > 0 && (
        <div className="mb-1 flex flex-wrap gap-x-4 gap-y-1">
          {series.map((s) => (
            <span key={s.key} className="inline-flex items-center gap-1.5 text-note text-ink-dim">
              <span
                className="h-2 w-2 shrink-0 rounded-sm"
                style={{ background: colors.get(s.color) }}
                aria-hidden
              />
              {s.label}
            </span>
          ))}
        </div>
      )}

      <div className="relative">
        <svg
          ref={svgRef}
          viewBox={`0 0 ${VB_W} ${VB_H}`}
          width="100%"
          height="auto"
          className="block overflow-visible"
          role="img"
          aria-label={ariaLabel ?? title ?? 'chart'}
          onMouseMove={onMove}
          onMouseLeave={() => setHover(null)}
        >
          {/* left-to-right reveal: the plot area wipes open as the chart draws
              in, so the series trace across time rather than stretching up. */}
          <defs>
            <clipPath id={clipId}>
              <rect
                className={cn('cp-chart-wipe', drawn && 'cp-in')}
                x={PLOT.x0}
                y={PLOT.yTop - 6}
                width={PLOT.x1 - PLOT.x0}
                height={PLOT.y0 - PLOT.yTop + 10}
              />
            </clipPath>
          </defs>

          {/* gridlines + y ticks */}
          {scale.ticks.map((t) => {
            const y = yOf(t, scale.max, PLOT);
            const isBase = t === 0;
            return (
              <g key={t}>
                <line
                  x1={PLOT.x0}
                  y1={y}
                  x2={PLOT.x1}
                  y2={y}
                  style={{ stroke: isBase ? colors.border : colors.alpha('border', 0.6) }}
                  strokeWidth={1}
                />
                <text
                  x={PLOT.x0 - 6}
                  y={y + 3}
                  textAnchor="end"
                  className="font-mono"
                  style={{ fill: colors.inkDim, fontSize: 10 }}
                >
                  {t}
                </text>
              </g>
            );
          })}

          {/* marks (area / line / column), revealed left-to-right by the wipe clip */}
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
                      d={areaPath(top, bottom, xa, scale.max, PLOT)}
                      style={{ fill: colors.alpha(s.color, s.fillOpacity ?? 0.16) }}
                    />
                  );
                })}
              {kind === 'area' &&
                series.map((s, si) => (
                  <polyline
                    key={`l-${s.key}`}
                    points={linePoints(tops[si], xa, scale.max, PLOT)}
                    fill="none"
                    strokeWidth={2}
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    style={{ stroke: colors.get(s.color) }}
                  />
                ))}

              {/* line: one overlaid line per series, on its own values, no fill */}
              {kind === 'line' &&
                series.map((s) => (
                  <polyline
                    key={`l-${s.key}`}
                    points={linePoints(s.values, xa, scale.max, PLOT)}
                    fill="none"
                    strokeWidth={2}
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    style={{ stroke: colors.get(s.color) }}
                  />
                ))}

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
                        const segTop = yOf(tops[si][i], scale.max, PLOT);
                        const segBot = yOf(si === 0 ? 0 : tops[si - 1][i], scale.max, PLOT);
                        const h = segBot - segTop - SEAM;
                        if (h <= 0) return null;
                        const fill = colors.get(s.color);
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

          {/* peak marker (stacked kinds only — an overlaid-line chart has no
              single "total" to label) */}
          {hasData && stacked && peak >= 0 && tot[peak] > 0 && (
            <g>
              <line
                x1={xc[peak]}
                y1={yOf(tot[peak], scale.max, PLOT)}
                x2={xc[peak]}
                y2={PLOT.yTop - 2}
                style={{ stroke: colors.border, strokeDasharray: '2 2' }}
                strokeWidth={1}
              />
              <text
                x={xc[peak]}
                y={PLOT.yTop - 6}
                textAnchor="middle"
                style={{ fill: colors.ink, fontSize: 11, fontWeight: 600 }}
              >
                {tot[peak]}
              </text>
              <text
                x={xc[peak]}
                y={PLOT.yTop + 5}
                textAnchor="middle"
                className="font-mono"
                style={{ fill: colors.inkDim, fontSize: 9 }}
              >
                {peakSubLabel ? peakSubLabel(peak, tot[peak]) : 'peak'}
              </text>
            </g>
          )}

          {/* now pulse on the latest point (area/line) + plain end-dots on the
              other overlaid lines so every series terminates cleanly. Columns
              don't need it — the rightmost bar already reads as "now". */}
          {hasData && kind !== 'column' && (
            <>
              {kind === 'line' &&
                series.map((s, si) =>
                  si === topSeries ? null : (
                    <circle
                      key={`end-${s.key}`}
                      cx={lastX}
                      cy={markY(si, n - 1)}
                      r={2.6}
                      style={{ fill: colors.get(s.color) }}
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
                style={{ stroke: colors.get(pulseColor) }}
              />
              <circle
                className="cp-chart-now-dot"
                cx={lastX}
                cy={lastTopY}
                r={3}
                style={{ fill: colors.get(pulseColor) }}
              />
            </>
          )}

          {/* x ticks */}
          {tickIdx.map((i) => (
            <text
              key={`x-${i}`}
              x={xc[i]}
              y={XTICK_Y}
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
                y1={PLOT.yTop}
                x2={0}
                y2={PLOT.y0}
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
                    style={{ fill: colors.get(s.color), stroke: colors.panel }}
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
            style={{ left: `${(hoverX / VB_W) * 100}%`, top: `${(hoverTopY / VB_H) * 100}%` }}
            role="status"
          >
            <div className="mb-1 font-semibold text-ink">{tip.date}</div>
            {tip.rows.map((r) => (
              <div key={r.key} className="flex items-center gap-2 leading-relaxed">
                <span
                  className="h-2 w-2 shrink-0 rounded-sm"
                  style={{ background: colors.get(r.color) }}
                  aria-hidden
                />
                <span className="flex-1 pr-3 text-ink-dim">{r.label}</span>
                <span className="font-mono font-semibold tabular-nums text-ink">{r.value}</span>
              </div>
            ))}
            {stacked && (
              <div className="mt-1 flex justify-between gap-4 border-t border-border pt-1">
                <span className="text-ink-dim">Total</span>
                <span className="font-mono font-bold tabular-nums text-ink">{tip.total}</span>
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
