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

import { useEffect, useRef, useState } from 'react';
import { useThemeColors, type ColorKey } from '../../lib/useThemeColors';
import { cn } from '../../lib/cn';
import {
  areaPath,
  cumulativeTops,
  linePoints,
  niceScale,
  peakIndex,
  totals,
  xsFor,
  yOf,
  type Plot,
} from '../../lib/chart/area';

const VB_W = 480;
const VB_H = 176;
const PLOT: Plot = { x0: 48, x1: 466, y0: 148, yTop: 26 };
const XTICK_Y = 164;

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

export interface ChartProps {
  /** x labels, one per bucket. */
  dates: string[];
  /** Bottom-first stacking order. */
  series: ChartSeries[];
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

export function Chart({
  dates,
  series,
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

  // Trigger the draw-in on the next frame after mount (CSS handles the
  // reduced-motion fallback — the riser just appears).
  useEffect(() => {
    const id = requestAnimationFrame(() => setDrawn(true));
    return () => cancelAnimationFrame(id);
  }, []);

  const n = dates.length;
  const valueSeries = series.map((s) => s.values);
  const tot = totals(valueSeries);
  const scale = niceScale(Math.max(1, ...tot));
  const xa = xsFor(n, PLOT);
  const tops = cumulativeTops(valueSeries); // per series cumulative top
  const peak = peakIndex(tot);

  const hasData = n > 0;

  // x-ticks: first, peak, last (deduped, in order).
  const tickIdx = [...new Set([0, peak, n - 1].filter((i) => i >= 0 && i < n))].sort((a, b) => a - b);

  const topSeries = series.length - 1;
  const lastX = xa[n - 1];
  const lastTopY = hasData ? yOf(tot[n - 1], scale.max, PLOT) : PLOT.y0;
  const pulseColor = series[topSeries]?.color ?? 'status.running';

  const onMove = (e: React.MouseEvent) => {
    const svg = svgRef.current;
    if (!svg || !hasData) return;
    const r = svg.getBoundingClientRect();
    if (r.width === 0) return; // jsdom / not laid out
    const px = ((e.clientX - r.left) / r.width) * VB_W;
    setHover(nearestIndex(xa, px));
  };

  const tip = hover != null && hasData ? tooltipModel(series, dates, hover) : null;
  const hoverX = hover != null ? xa[hover] : 0;
  const hoverTopY = hover != null ? yOf(tot[hover], scale.max, PLOT) : 0;

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
                {headline.value}
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

          {/* stacked areas + lines (draw-in) */}
          {hasData && (
            <g className={cn('cp-chart-riser', drawn && 'cp-in')}>
              {series.map((s, si) => {
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
              {series.map((s, si) => (
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
            </g>
          )}

          {/* peak marker */}
          {hasData && peak >= 0 && tot[peak] > 0 && (
            <g>
              <line
                x1={xa[peak]}
                y1={yOf(tot[peak], scale.max, PLOT)}
                x2={xa[peak]}
                y2={PLOT.yTop - 2}
                style={{ stroke: colors.border, strokeDasharray: '2 2' }}
                strokeWidth={1}
              />
              <text
                x={xa[peak]}
                y={PLOT.yTop - 6}
                textAnchor="middle"
                style={{ fill: colors.ink, fontSize: 11, fontWeight: 600 }}
              >
                {tot[peak]}
              </text>
              <text
                x={xa[peak]}
                y={PLOT.yTop + 5}
                textAnchor="middle"
                className="font-mono"
                style={{ fill: colors.inkDim, fontSize: 9 }}
              >
                {peakSubLabel ? peakSubLabel(peak, tot[peak]) : 'peak'}
              </text>
            </g>
          )}

          {/* now pulse (latest point, top of stack) */}
          {hasData && (
            <>
              <circle
                className="cp-chart-now"
                cx={lastX}
                cy={lastTopY}
                r={4}
                fill="none"
                strokeWidth={1.5}
                style={{ stroke: colors.get(pulseColor) }}
              />
              <circle cx={lastX} cy={lastTopY} r={3} style={{ fill: colors.get(pulseColor) }} />
            </>
          )}

          {/* x ticks */}
          {tickIdx.map((i) => (
            <text
              key={`x-${i}`}
              x={xa[i]}
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
              {series.map((s, si) => (
                <circle
                  key={`d-${s.key}`}
                  cx={0}
                  cy={yOf(tops[si][hover as number], scale.max, PLOT)}
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
            <div className="mt-1 flex justify-between gap-4 border-t border-border pt-1">
              <span className="text-ink-dim">Total</span>
              <span className="font-mono font-bold tabular-nums text-ink">{tip.total}</span>
            </div>
          </div>
        )}
      </div>

      {note && <p className="mt-2 text-note text-ink-dim">{note}</p>}
    </div>
  );
}

export default Chart;
