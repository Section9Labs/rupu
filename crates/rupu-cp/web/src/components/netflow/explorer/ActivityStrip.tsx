// Activity strip — the full-range context chart with drag-to-zoom, beside
// the server-echo `NetflowWindowReadout`. The window itself is the
// explorer's query (`since:` / `until:`); a drag writes it there.
//
// The chart is ALWAYS the scope's whole retained history (server-chosen
// bounds, `HistogramView.from/to`), never the applied window — it's the
// context a window is dragged against. The readout beside it is built
// SOLELY from the server's `window` echo, same rule as everywhere else. Drag
// commits only when the selection spans more than one bucket (a click or
// micro-drag is noise, not intent).
//
// Drawn in the living-chart language (styles.css "Chart motion"): recorded
// flows as an area rising from the baseline, the errors among them as a red
// band inside it, traced in by the sweep. The applied window is lit and
// everything outside it steps back; a drag shows the same, live, with its
// range and flow count. Hover names a bucket's exact counts.

import { useId, useRef, useState } from 'react';
import type { PointerEvent as ReactPointerEvent } from 'react';
import { absoluteTime } from '../../../lib/time';
import { useThemeColors } from '../../../lib/useThemeColors';
import { cn } from '../../../lib/cn';
import { useDrawIn, useMeasuredWidth } from '../../../lib/chart/motion';
import { tightScale } from '../../../lib/chart/area';
import type { HistogramView } from '../../../lib/netflow';
import { ChartTooltip, CrossGradient, Crosshair, FadeGradient, GlowFilter, Stylus, useHoverIndex } from '../../charts/parts';
import NetflowWindowReadout from '../NetflowWindowReadout';
import type { NetflowWindowEcho } from '../ScopeDisclosure';

export interface ActivityStripProps {
  histogram: HistogramView;
  /** Drag-initiated zoom; the owner pushes the outgoing window onto its
   *  zoom stack for "Zoom out". */
  onZoom: (fromIso: string, toIso: string) => void;
  /** The server's own `window` echo — `null` only before the first load. */
  appliedWindow: NetflowWindowEcho | null;
}

interface DragState {
  a: number; // ms epoch
  b: number;
}

const H = 104;
const TOP = 18;
const BOTTOM = 20;
const fmtInt = (v: number) => Math.round(v).toLocaleString('en-US');
const fmtCompact = (v: number) =>
  new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 }).format(v);

/** A short time label for a tick, as fine as the span needs. Pure, for testing. */
export function tickLabel(ms: number, spanMs: number): string {
  const d = new Date(ms);
  if (spanMs <= 36 * 3600_000) return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  if (spanMs <= 10 * 86400_000)
    return `${d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })} ${d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}`;
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

export function ActivityStrip({ histogram, onZoom, appliedWindow }: ActivityStripProps) {
  const colors = useThemeColors();
  const chartRef = useRef<HTMLDivElement>(null);
  const width = useMeasuredWidth(chartRef, 700);
  const drawn = useDrawIn();
  const hover = useHoverIndex();
  const uid = useId().replace(/:/g, '');
  const [drag, setDrag] = useState<DragState | null>(null);

  const t0 = histogram.from ? Date.parse(histogram.from) : null;
  const t1 = histogram.to ? Date.parse(histogram.to) : null;
  const span = t0 !== null && t1 !== null ? Math.max(1, t1 - t0) : null;
  const buckets = histogram.buckets;
  const n = buckets.length;

  function timeAt(e: ReactPointerEvent): number | null {
    if (span === null || t0 === null || !chartRef.current) return null;
    const rect = chartRef.current.getBoundingClientRect();
    if (rect.width === 0) return null;
    const frac = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    return t0 + frac * span;
  }

  function onPointerDown(e: ReactPointerEvent) {
    const t = timeAt(e);
    if (t === null) return;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    hover.set(null);
    setDrag({ a: t, b: t });
  }

  function onPointerMove(e: ReactPointerEvent) {
    const t = timeAt(e);
    if (t === null) return;
    if (drag) {
      setDrag({ a: drag.a, b: t });
      return;
    }
    if (span !== null && t0 !== null && n > 0) hover.set(Math.min(n - 1, Math.floor(((t - t0) / span) * n)));
  }

  function onPointerUp() {
    if (!drag || span === null) return;
    const from = Math.min(drag.a, drag.b);
    const to = Math.max(drag.a, drag.b);
    setDrag(null);
    // Commit only when the selection spans more than one bucket.
    if (to - from > span / n) {
      onZoom(new Date(from).toISOString(), new Date(to).toISOString());
    }
  }

  // The lit window: the in-progress drag wins; otherwise the SERVER-echoed
  // window (never the query's own reading of it), within the chart bounds.
  let lit: { from: number; to: number } | null = null;
  if (drag) {
    lit = { from: Math.min(drag.a, drag.b), to: Math.max(drag.a, drag.b) };
  } else if (appliedWindow && t0 !== null && t1 !== null) {
    if (appliedWindow.from !== null || appliedWindow.to !== null) {
      lit = {
        from: appliedWindow.from !== null ? Date.parse(appliedWindow.from) : t0,
        to: appliedWindow.to !== null ? Date.parse(appliedWindow.to) : t1,
      };
    }
  }

  // ── geometry ──────────────────────────────────────────────────────────
  const base = H - BOTTOM;
  const scale = tightScale(Math.max(1, ...buckets.map((b) => b.calls)) * 1.04, 2, 3);
  const xOfT = (t: number) => (t0 !== null && span !== null ? ((t - t0) / span) * width : 0);
  const xc = buckets.map((_, i) => ((i + 0.5) / Math.max(1, n)) * width);
  const yOf = (v: number) => base - (v / scale.max) * (base - TOP);
  const trace = (vals: number[]) => {
    if (!n) return '';
    const ys = vals.map(yOf);
    let d = `M0,${ys[0].toFixed(1)}`;
    xc.forEach((x, i) => (d += `L${x.toFixed(1)},${ys[i].toFixed(1)}`));
    return `${d}L${width},${ys[n - 1].toFixed(1)}`;
  };
  const area = (d: string) => (d ? `${d}L${width},${base}L0,${base}Z` : '');
  const callsLine = trace(buckets.map((b) => b.calls));
  const errLine = trace(buckets.map((b) => b.errors));
  const hasErrors = buckets.some((b) => b.errors > 0);
  const litX0 = lit ? Math.max(0, xOfT(lit.from)) : 0;
  const litX1 = lit ? Math.min(width, Math.max(litX0 + 1, xOfT(lit.to))) : 0;
  const litFlows =
    lit && t0 !== null && span !== null
      ? buckets.reduce((sum, b, i) => {
          const bt = t0 + ((i + 0.5) / n) * span;
          return bt >= lit!.from && bt <= lit!.to ? sum + b.calls : sum;
        }, 0)
      : 0;

  const ticks =
    t0 !== null && span !== null
      ? (() => {
          const count = width > 640 ? 5 : 3;
          return Array.from({ length: count }, (_, k) => t0 + (span * k) / (count - 1));
        })()
      : [];

  // ── hover ─────────────────────────────────────────────────────────────
  const hi = hover.index;
  const hb = hi != null ? buckets[hi] : null;
  const bucketFrom = hi != null && t0 !== null && span !== null ? t0 + (span * hi) / n : null;

  const sweep = `${uid}-sweep`;
  const dim = colors.alpha('panel', 0.62);

  return (
    <div className="rounded-xl border border-border bg-panel px-4 pb-2.5 pt-3.5">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-2">
          {appliedWindow && <NetflowWindowReadout appliedWindow={appliedWindow} />}
          {span !== null && (
            <p className="text-note text-ink-mute">Drag on the chart to zoom a window.</p>
          )}
        </div>
        {span !== null && (
          <div className="flex items-center gap-4 text-note text-ink-dim">
            <span className="inline-flex items-center gap-1.5">
              <span className="h-2 w-2 rounded-[2px]" style={{ background: colors.get('chart.1') }} aria-hidden />
              Recorded flows
            </span>
            {hasErrors && (
              <span className="inline-flex items-center gap-1.5">
                <span className="h-2 w-2 rounded-[2px]" style={{ background: colors.get('status.failed') }} aria-hidden />
                Errors
              </span>
            )}
          </div>
        )}
      </div>
      <div
        ref={chartRef}
        role="img"
        aria-label="Recorded flows over the full retained range; drag to zoom"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerLeave={() => hover.set(null)}
        // An interrupted drag (touch-scroll interception, lost capture)
        // must not leave the strip in permanent drag mode — the next
        // unrelated pointer sequence would silently commit a window the
        // user never dragged.
        onPointerCancel={() => setDrag(null)}
        onLostPointerCapture={() => setDrag(null)}
        className={cn('relative touch-none select-none', span !== null && 'cursor-crosshair', drawn && 'cp-in')}
      >
        <svg viewBox={`0 0 ${width} ${H}`} width="100%" height={H} className="block overflow-visible" aria-hidden>
          <defs>
            <clipPath id={sweep}>
              <rect className="cp-chart-sweep" x={-4} y={0} width={width + 8} height={H} />
            </clipPath>
            <FadeGradient id={`${uid}-g`} color={colors.get('chart.1')} top={0.36} bottom={0.03} />
            <FadeGradient id={`${uid}-e`} color={colors.get('status.failed')} top={0.55} bottom={0.12} />
            <GlowFilter id={`${uid}-glow`} />
            <CrossGradient id={`${uid}-x`} color={colors.ink} y0={TOP - 6} y1={base} />
          </defs>

          {scale.ticks.slice(1).map((t, k) => (
            <g key={t}>
              <line
                x1={0}
                x2={width}
                y1={yOf(t)}
                y2={yOf(t)}
                className="cp-chart-grid"
                style={{ ['--i' as string]: k, stroke: colors.alpha('border', 0.6) }}
              />
              <text
                x={2}
                y={yOf(t) - 4}
                className="cp-chart-tick font-mono"
                style={{ ['--i' as string]: k, fill: colors.inkMute, fontSize: 9.5 }}
              >
                {fmtCompact(t)}
              </text>
            </g>
          ))}

          {n > 0 && span !== null && (
            <g clipPath={`url(#${sweep})`}>
              <g className="cp-chart-rise">
                <path d={area(callsLine)} fill={`url(#${uid}-g)`} />
                {hasErrors && <path d={area(errLine)} fill={`url(#${uid}-e)`} />}
                <path
                  d={callsLine}
                  fill="none"
                  strokeWidth={5}
                  filter={`url(#${uid}-glow)`}
                  style={{ stroke: colors.get('chart.1'), opacity: 0.3 }}
                />
                <path
                  d={callsLine}
                  fill="none"
                  strokeWidth={1.75}
                  strokeLinejoin="round"
                  strokeLinecap="round"
                  style={{ stroke: colors.get('chart.1') }}
                />
                {hasErrors && (
                  <path
                    d={errLine}
                    fill="none"
                    strokeWidth={1.25}
                    strokeLinejoin="round"
                    style={{ stroke: colors.get('status.failed') }}
                  />
                )}
              </g>
            </g>
          )}
          <line x1={0} x2={width} y1={base + 0.5} y2={base + 0.5} style={{ stroke: colors.alpha('inkMute', 0.5) }} />

          {n > 0 && span !== null && (
            <Stylus
              start={drawn}
              xs={xc}
              ys={buckets.map((b) => yOf(b.calls))}
              x0={-4}
              x1={width + 4}
              color={colors.get('chart.1')}
            />
          )}

          {/* the window: lit, with everything outside it stepped back */}
          {lit && span !== null && (
            <g data-testid="activity-window-overlay" pointerEvents="none" className={drag ? 'cp-chart-brush' : undefined}>
              <rect x={0} y={TOP - 8} width={litX0} height={base - TOP + 8} style={{ fill: dim }} />
              <rect x={litX1} y={TOP - 8} width={Math.max(0, width - litX1)} height={base - TOP + 8} style={{ fill: dim }} />
              <rect
                x={litX0}
                y={TOP - 8}
                width={litX1 - litX0}
                height={base - TOP + 8}
                rx={3}
                style={{ fill: colors.alpha('brand.500', drag ? 0.1 : 0.05) }}
              />
              {[litX0, litX1].map((x, k) => (
                <g key={k}>
                  <line x1={x} x2={x} y1={TOP - 8} y2={base} style={{ stroke: colors.alpha('brand.500', 0.8) }} strokeWidth={1.25} />
                  <rect
                    x={x - 2.5}
                    y={(TOP + base) / 2 - 9}
                    width={5}
                    height={18}
                    rx={2.5}
                    style={{ fill: colors.get('brand.500'), stroke: colors.panel }}
                    strokeWidth={1.5}
                  />
                </g>
              ))}
            </g>
          )}

          {/* time axis */}
          {span !== null &&
            ticks.map((t, k) => (
              <text
                key={k}
                x={xOfT(t)}
                y={H - 5}
                textAnchor={k === 0 ? 'start' : k === ticks.length - 1 ? 'end' : 'middle'}
                className="cp-chart-tick font-mono"
                style={{ ['--i' as string]: k, fill: colors.inkMute, fontSize: 10 }}
              >
                {tickLabel(t, span)}
              </text>
            ))}

          {hb && hi != null && !drag && (
            <Crosshair
              x={xc[hi]}
              y0={TOP - 6}
              y1={base}
              strokeId={`${uid}-x`}
              ring={colors.panel}
              appear={hover.appear}
              dots={[
                { key: 'calls', y: yOf(hb.calls), color: colors.get('chart.1') },
                ...(hb.errors ? [{ key: 'err', y: yOf(hb.errors), color: colors.get('status.failed') }] : []),
              ]}
            />
          )}
        </svg>

        {/* the drag's range, live */}
        {drag && lit && span !== null && litX1 - litX0 > 2 && (
          <div
            className="cp-chart-chip pointer-events-none absolute -top-7 whitespace-nowrap rounded-full border border-border bg-surface px-2.5 py-0.5 text-note tabular-nums text-ink-dim shadow-sm"
            style={{ left: Math.min(Math.max(0, (litX0 + litX1) / 2 - 110), width - 220) }}
          >
            {tickLabel(lit.from, span)} → {tickLabel(lit.to, span)}
            <span className="text-ink"> · {fmtInt(litFlows)} flows</span>
          </div>
        )}

        <ChartTooltip
          open={!!hb && !drag}
          appear={hover.appear}
          x={hi != null ? xc[hi] : 0}
          y={hb ? Math.max(TOP, yOf(hb.calls)) : 0}
          width={width}
          title={bucketFrom !== null ? absoluteTime(new Date(bucketFrom).toISOString()) : ''}
          rows={
            hb
              ? [
                  { key: 'calls', label: 'Flows', value: fmtInt(hb.calls), color: colors.get('chart.1') },
                  {
                    key: 'err',
                    label: hb.calls ? `Errors · ${Math.round((hb.errors / hb.calls) * 100)}%` : 'Errors',
                    value: fmtInt(hb.errors),
                    color: colors.get('status.failed'),
                    muted: hb.errors === 0,
                  },
                ]
              : []
          }
        />
      </div>
    </div>
  );
}

export default ActivityStrip;
