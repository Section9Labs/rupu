// TokenFlow — a run's (or a session's) tokens, turn by turn, as one flow:
// the context each turn sent rises above the baseline, the output the model
// wrote back hangs below it on its own scale (two panels sharing the turn
// axis, never two scales on one panel), and the cache reads show as a band
// inside the context. Stages run across the top as lanes.
//
// Live: a running run glides as turns arrive, and its latest point breathes.
// Explore: drag across the plot to zoom into those turns, click a stage lane
// (or its legend entry) to zoom to the stage; the window and both scales
// animate to the new range. Double-click, Escape or the chip resets.
//
// Thousands of turns are binned to the pixels available, keeping each
// bucket's largest turn (a spike never averages away) with the bucket median
// as a thin "typical" line. See lib/chart/flow.ts for the data rules and
// styles.css "Chart motion" for the motion vocabulary.

import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { RotateCcw } from 'lucide-react';
import type { UsageTimelinePoint } from '../../lib/api';
import { useThemeColors } from '../../lib/useThemeColors';
import { cn } from '../../lib/cn';
import { tightScale } from '../../lib/chart/area';
import { useDrawIn, useMeasuredWidth, useMorphedSeries } from '../../lib/chart/motion';
import {
  binFlow,
  brushWindow,
  flowTotals,
  stageLanes,
  stageTotals,
  turnTicks,
  turnsPerBin,
  type FlowRow,
} from '../../lib/chart/flow';
import {
  ChartLegend,
  ChartTooltip,
  CountUp,
  CrossGradient,
  Crosshair,
  FadeGradient,
  GlowFilter,
  Stylus,
  useHoverIndex,
  type TipRow,
} from './parts';

/** Abbreviated token count: `2000 → "2k"`, `1_200_000 → "1.2M"`. */
export function formatTokens(v: number): string {
  const n = Math.abs(v);
  const trim = (x: number) => x.toFixed(1).replace(/\.0$/, '');
  if (n >= 1e9) return `${trim(n / 1e9)}B`;
  if (n >= 1e6) return `${trim(n / 1e6)}M`;
  if (n >= 1e3) return `${trim(n / 1e3)}k`;
  return String(Math.round(n));
}
const fmtInt = (v: number) => Math.round(v).toLocaleString('en-US');

/** Layout, in CSS pixels (the SVG is drawn 1:1 with its box). */
const L = 56;
const R = 16;
const UP_H = 168;
const GAP = 16;
const DN_H = 72;
const BOTTOM = 24;
/** Smallest bucket, in px: thinner than this and the line turns to hair. */
const BIN_PX = 5;
const DEFAULT_W = 720;

type SeriesKey = 'in' | 'cache' | 'out';

export interface TokenFlowProps {
  points: UsageTimelinePoint[];
  /** Draw stage lanes across the top (a workflow run's steps). */
  stages?: boolean;
  /** The run is still going: its end breathes and new turns glide in. */
  live?: boolean;
  className?: string;
  /** The zoomed range of point indices (inclusive), when the page owns it;
   *  `null` is the whole series. Omit to let the chart keep its own. */
  zoom?: [number, number] | null;
  /** Told whenever the zoom changes (a drag, a stage click, a reset). */
  onZoomChange?: (zoom: [number, number] | null) => void;
}

export default function TokenFlow({
  points,
  stages = true,
  live = false,
  className,
  zoom: zoomProp,
  onZoomChange,
}: TokenFlowProps) {
  const colors = useThemeColors();
  const svgRef = useRef<SVGSVGElement>(null);
  const [width, boxRef] = useMeasuredWidth(DEFAULT_W);
  // the draw-in plays when the turns arrive, not on the empty first frame
  const drawn = useDrawIn(points.length > 0);
  const uid = useId().replace(/:/g, '');
  const hover = useHoverIndex();
  const [focusSeries, setFocusSeries] = useState<SeriesKey | null>(null);
  const [focusStage, setFocusStage] = useState<string | null>(null);
  const [ownZoom, setOwnZoom] = useState<[number, number] | null>(null);
  const zoom = zoomProp !== undefined ? zoomProp : ownZoom;
  const controlled = zoomProp !== undefined;
  const setZoom = useCallback(
    (z: [number, number] | null) => {
      if (!controlled) setOwnZoom(z);
      onZoomChange?.(z);
    },
    [controlled, onZoomChange],
  );
  const [drag, setDrag] = useState<{ x0: number; x1: number } | null>(null);

  const n = points.length;
  // A zoom the data no longer covers (another run, fewer turns) is dropped.
  const zoomed = zoom && zoom[1] <= n - 1 ? zoom : null;
  useEffect(() => {
    if (zoom && !zoomed) setZoom(null);
  }, [zoom, zoomed, setZoom]);
  // The first data replaces the empty placeholder window: snap to it rather
  // than morph out of the placeholder (which would read as a zoom-out).
  const hadData = useRef(false);
  const firstData = n > 0 && !hadData.current;
  useEffect(() => {
    if (n > 0) hadData.current = true;
  }, [n]);
  const cIn = colors.get('chart.1');
  const cOut = colors.get('chart.2');
  const cCache = colors.get('chart.5');

  const lanes = useMemo(() => (stages ? stageLanes(points) : []), [points, stages]);
  const totals = useMemo(() => flowTotals(points), [points]);
  const byStage = useMemo(() => stageTotals(points), [points]);
  const hasLanes = lanes.length > 0 && lanes.some((l) => l.label);

  // ── geometry ────────────────────────────────────────────────────────────
  const LANE = hasLanes ? 30 : 0;
  const T = LANE + 24;
  const base = T + UP_H;
  const dnTop = base + GAP;
  const H = dnTop + DN_H + BOTTOM;
  const pw = Math.max(40, width - L - R);
  const maxBins = Math.max(24, Math.floor(pw / BIN_PX));

  // The window (in point-index space) we're heading to, and its scales.
  const full: [number, number] = n > 1 ? [0, n - 1] : [-1, 1];
  const goal = zoomed ?? full;
  const goalPer = turnsPerBin(goal[1] - goal[0] + 1, maxBins);
  const goalRows = useMemo(() => binFlow(points, goal[0], goal[1], goalPer), [points, goal[0], goal[1], goalPer]); // eslint-disable-line react-hooks/exhaustive-deps
  const upGoal = tightScale(Math.max(0, ...goalRows.map((r) => r.context)) * 1.03, 3, 5);
  const dnGoal = tightScale(Math.max(0, ...goalRows.map((r) => r.out)) * 1.03, 2, 3);

  // The window and both scale maxima glide together to the goal (zoom,
  // reset, a live run's new turns); see useMorphedSeries.
  const [[va, vb, upMax, dnMax]] = useMorphedSeries([[goal[0], goal[1], upGoal.max, dnGoal.max]], firstData);
  const span = Math.max(1e-6, vb - va);
  const xOf = (idx: number) => L + ((idx - va) / span) * pw;
  // binned for the window actually shown, so a zoom never bins every turn
  const per = turnsPerBin(span + 1, maxBins);
  const rows = useMemo(() => binFlow(points, va - per, vb + per, per), [points, va, vb, per]);
  const yUp = (v: number) => base - (v / upMax) * UP_H;
  const yDn = (v: number) => dnTop + (v / dnMax) * DN_H;
  const xs = rows.map((r) => xOf(r.at));

  // ── paths ───────────────────────────────────────────────────────────────
  const line = (vals: number[], y: (v: number) => number) =>
    vals.map((v, i) => `${i ? 'L' : 'M'}${xs[i].toFixed(1)},${y(v).toFixed(1)}`).join('');
  const areaDown = (d: string, yBase: number) =>
    rows.length ? `${d}L${xs[xs.length - 1].toFixed(1)},${yBase}L${xs[0].toFixed(1)},${yBase}Z` : '';
  const ctxLine = line(rows.map((r) => r.context), yUp);
  const cacheLine = line(rows.map((r) => r.cached), yUp);
  const typLine = line(rows.map((r) => r.typical), yUp);
  const outLine = line(rows.map((r) => r.out), yDn);
  const hasCache = totals.cached > 0;

  // ── annotations ─────────────────────────────────────────────────────────
  const inView = (r: FlowRow) => r.at >= va - 0.5 && r.at <= vb + 0.5;
  const peak = rows.reduce<FlowRow | null>((best, r) => (inView(r) && (!best || r.context > best.context) ? r : best), null);
  const last = rows[rows.length - 1];
  const endVisible = !!last && last.i1 === n - 1 && xOf(last.at) <= L + pw + 1;
  const ticks = turnTicks(Math.max(0, va), Math.min(n - 1, vb), pw / span);

  // ── hover ───────────────────────────────────────────────────────────────
  const hoverRow = hover.index != null ? rows.find((r) => hover.index! >= r.i0 && hover.index! <= r.i1) ?? null : null;
  const pxFromEvent = (e: React.MouseEvent) => {
    const svg = svgRef.current;
    if (!svg) return null;
    const r = svg.getBoundingClientRect();
    if (r.width === 0) return null;
    return ((e.clientX - r.left) / r.width) * width;
  };
  const indexAt = (px: number) => {
    let best: FlowRow | null = null;
    let bestD = Infinity;
    for (const r of rows) {
      const d = Math.abs(xOf(r.at) - px);
      if (d < bestD && inView(r)) {
        bestD = d;
        best = r;
      }
    }
    return best ? Math.round(best.at) : null;
  };
  const onMove = (e: React.MouseEvent) => {
    const px = pxFromEvent(e);
    if (px == null) return;
    if (drag) {
      setDrag({ ...drag, x1: px });
      return;
    }
    hover.set(indexAt(px));
  };
  const onDown = (e: React.MouseEvent) => {
    const px = pxFromEvent(e);
    if (px == null || e.button !== 0) return;
    setDrag({ x0: px, x1: px });
  };
  const endDrag = () => {
    if (!drag) return;
    const win = brushWindow(drag.x0, drag.x1, [va, vb], { left: L, right: L + pw });
    setDrag(null);
    if (win) {
      hover.set(null);
      setZoom(win[0] <= 0 && win[1] >= n - 1 ? null : [Math.max(0, win[0]), Math.min(n - 1, win[1])]);
    }
  };
  const zoomToStage = (label: string) => {
    const own = lanes.filter((l) => l.label === label);
    if (!own.length) return;
    const a = Math.min(...own.map((l) => l.from));
    const b = Math.max(...own.map((l) => l.to));
    if (b - a < 2) return;
    hover.set(null);
    setZoom(a <= 0 && b >= n - 1 ? null : [a, b]);
  };

  // Escape resets a zoom.
  useEffect(() => {
    if (!zoom) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setZoom(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [zoom, setZoom]);

  if (n === 0) {
    return <div className="py-8 text-center text-note text-ink-mute">No per-turn usage yet</div>;
  }

  // ── focus (legend / stage) ──────────────────────────────────────────────
  const seriesOpacity = (k: SeriesKey) => (focusSeries && focusSeries !== k ? 0.15 : 1);
  const plotRight = L + pw;
  const laneX = (from: number, to: number) => {
    const x0 = Math.max(L, xOf(from - 0.5));
    const x1 = Math.min(plotRight, xOf(to + 0.5));
    return { x0, x1, w: Math.max(0, x1 - x0) };
  };

  // ── tooltip ─────────────────────────────────────────────────────────────
  let tipRows: TipRow[] = [];
  let tipTitle: React.ReactNode = '';
  let tipFooter: React.ReactNode = null;
  if (hoverRow) {
    const single = hoverRow.i0 === hoverRow.i1;
    tipTitle = (
      <>
        {single ? `Turn ${fmtInt(hoverRow.i0 + 1)}` : `Turns ${fmtInt(hoverRow.i0 + 1)}–${fmtInt(hoverRow.i1 + 1)}`}
        {hoverRow.stage && <span className="font-normal text-ink-dim"> · {hoverRow.stage}</span>}
      </>
    );
    const pct = hoverRow.context ? Math.round((hoverRow.cached / hoverRow.context) * 100) : 0;
    tipRows = [
      { key: 'in', label: single ? 'Context in' : 'Largest context', value: formatTokens(hoverRow.context), color: cIn },
      ...(hoverRow.cached
        ? [{ key: 'cache', label: `from cache · ${pct}%`, value: formatTokens(hoverRow.cached), color: cCache, sub: true }]
        : []),
      { key: 'out', label: single ? 'Output' : 'Largest output', value: formatTokens(hoverRow.out), color: cOut },
    ];
    if (!single) {
      tipRows.push({
        key: 'sum',
        label: `${fmtInt(hoverRow.i1 - hoverRow.i0 + 1)} turns · in / out`,
        value: `${formatTokens(hoverRow.sumIn)} / ${formatTokens(hoverRow.sumOut)}`,
        total: true,
      });
    } else if (hoverRow.out > 0) {
      tipFooter = `${fmtInt(hoverRow.context / hoverRow.out)} : 1 in to out`;
    }
  }
  const hoverX = hoverRow ? xOf(hoverRow.at) : 0;

  // ── header ──────────────────────────────────────────────────────────────
  const ratio = totals.tokensOut ? Math.round(totals.tokensIn / totals.tokensOut) : 0;
  const cachePct = totals.tokensIn ? Math.round((totals.cached / totals.tokensIn) * 100) : 0;
  const stageCount = byStage.filter((s) => s.label).length;
  const seriesLegend = [
    { key: 'in', label: 'Context in', color: cIn, value: formatTokens(totals.tokensIn) },
    ...(hasCache ? [{ key: 'cache', label: 'Cache reads', color: cCache, value: formatTokens(totals.cached) }] : []),
    { key: 'out', label: 'Output', color: cOut, value: formatTokens(totals.tokensOut) },
    ...(per > 1 ? [{ key: 'typ', label: 'Typical context', color: colors.alpha('chart.1', 0.6), hollow: true }] : []),
  ];

  const brush = drag && Math.abs(drag.x1 - drag.x0) >= 6 ? drag : null;
  const brushWin = brush ? brushWindow(brush.x0, brush.x1, [va, vb], { left: L, right: plotRight }) : null;
  const clip = `${uid}-sweep`;
  const plotClip = `${uid}-plot`;

  return (
    <div className={cn('select-none', drawn && 'cp-in', hoverRow && 'cp-chart-hovering', className)}>
      {/* header: what flowed, in one line */}
      <div className="mb-2 flex flex-wrap items-start justify-between gap-x-6 gap-y-2">
        <div className="min-w-0">
          <div className="text-lead font-semibold text-ink">Token flow</div>
          <div className="mt-0.5 text-note tabular-nums text-ink-dim">
            {fmtInt(n)} turn{n === 1 ? '' : 's'}
            {stageCount > 1 && ` across ${stageCount} stages`}
            {live && (
              <span className="ml-1.5 inline-flex items-center gap-1 text-status-running">
                <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-status-running" aria-hidden />
                live
              </span>
            )}
          </div>
        </div>
        <div className="text-right">
          <div className="text-xl font-semibold leading-none tracking-tight tabular-nums text-ink">
            <span style={{ color: cIn }}>
              <CountUp value={totals.tokensIn} format={formatTokens} />
            </span>
            <span className="mx-1.5 text-ink-mute">→</span>
            <span style={{ color: cOut }}>
              <CountUp value={totals.tokensOut} format={formatTokens} />
            </span>
          </div>
          <div className="mt-1 text-note tabular-nums text-ink-dim">
            in → out{ratio > 0 && ` · ${fmtInt(ratio)} : 1`}
            {hasCache && ` · ${cachePct}% from cache`}
          </div>
        </div>
      </div>

      <ChartLegend
        className="mb-1"
        items={seriesLegend}
        focus={focusSeries}
        onFocus={(k) => setFocusSeries(k === 'typ' ? 'in' : (k as SeriesKey | null))}
      />

      <div ref={boxRef} className="relative">
        <svg
          ref={svgRef}
          viewBox={`0 0 ${width} ${H}`}
          width="100%"
          height={H}
          className="block overflow-visible"
          role="img"
          aria-label={`Token flow: ${fmtInt(n)} turns, ${formatTokens(totals.tokensIn)} in, ${formatTokens(totals.tokensOut)} out`}
        >
          <defs>
            <clipPath id={clip}>
              <rect className="cp-chart-sweep" x={L - 8} y={0} width={pw + 16} height={H} />
            </clipPath>
            <clipPath id={plotClip}>
              <rect x={L} y={0} width={pw} height={H} />
            </clipPath>
            <FadeGradient id={`${uid}-gin`} color={cIn} top={0.34} bottom={0.03} />
            <FadeGradient id={`${uid}-gcache`} color={cCache} top={0.42} bottom={0.1} />
            <FadeGradient id={`${uid}-gout`} color={cOut} top={0.34} bottom={0.03} flip />
            <GlowFilter id={`${uid}-glow`} />
            <CrossGradient id={`${uid}-cross`} color={colors.ink} y0={T - 6} y1={dnTop + DN_H} />
          </defs>

          {/* stage lanes: a bar and a name per stretch, a quiet divider down
              the plot where each new stage starts */}
          {hasLanes && (
            <g clipPath={`url(#${plotClip})`}>
              {lanes.map((l, k) => {
                const { x0, w } = laneX(l.from, l.to);
                if (w <= 0) return null;
                const on = focusStage === l.label;
                const dim = focusStage != null && !on;
                const turns = l.to - l.from + 1;
                const fitsName = w > l.label.length * 6.6 + 10;
                const fitsSub = w > l.label.length * 6.6 + `${fmtInt(turns)} turns`.length * 6 + 22;
                return (
                  <g
                    key={`lane-${k}`}
                    className="cp-chart-fade cursor-zoom-in"
                    style={{ opacity: dim ? 0.35 : 1 }}
                    onMouseEnter={() => setFocusStage(l.label)}
                    onMouseLeave={() => setFocusStage(null)}
                    onClick={() => zoomToStage(l.label)}
                  >
                    <rect x={x0} y={0} width={w} height={LANE + 2} fill="transparent" />
                    <rect
                      x={x0 + 1}
                      y={LANE - 9}
                      width={Math.max(1, w - 2)}
                      height={3}
                      rx={1.5}
                      className="cp-chart-fade"
                      style={{ fill: on ? colors.ink : colors.alpha('inkMute', 0.55) }}
                    />
                    {fitsName && (
                      <text
                        x={x0 + 2}
                        y={LANE - 15}
                        className="cp-chart-tick"
                        style={{ ['--i' as string]: k, fill: on ? colors.ink : colors.inkDim, fontSize: 11, fontWeight: 560 }}
                      >
                        {l.label}
                        {fitsSub && (
                          <tspan dx={6} style={{ fill: colors.inkMute, fontWeight: 450 }}>
                            {fmtInt(turns)} turns
                          </tspan>
                        )}
                      </text>
                    )}
                    {k > 0 && (
                      <line
                        x1={x0}
                        x2={x0}
                        y1={T - 4}
                        y2={dnTop + DN_H}
                        strokeDasharray="2 4"
                        style={{ stroke: colors.border }}
                      />
                    )}
                  </g>
                );
              })}
            </g>
          )}

          {/* frame: input grid above the baseline, output grid below it */}
          {upGoal.ticks.slice(1).map((t, k) => {
            const y = yUp(t);
            if (y < T - 6) return null;
            return (
              <g key={`up-${t}`} className="cp-chart-appear">
                <line
                  x1={L}
                  x2={plotRight}
                  y1={y}
                  y2={y}
                  className="cp-chart-grid"
                  style={{ ['--i' as string]: k, stroke: colors.alpha('border', 0.7) }}
                />
                <text
                  x={L - 10}
                  y={y + 3.5}
                  textAnchor="end"
                  className="cp-chart-tick font-mono"
                  style={{ ['--i' as string]: k, fill: colors.inkMute, fontSize: 10 }}
                >
                  {formatTokens(t)}
                </text>
              </g>
            );
          })}
          {dnGoal.ticks.slice(1).map((t, k) => {
            const y = yDn(t);
            if (y > dnTop + DN_H + 6) return null;
            return (
              <g key={`dn-${t}`} className="cp-chart-appear">
                <line
                  x1={L}
                  x2={plotRight}
                  y1={y}
                  y2={y}
                  strokeDasharray="3 3"
                  className="cp-chart-grid"
                  style={{ ['--i' as string]: k + 4, stroke: colors.alpha('border', 0.6) }}
                />
                <text
                  x={L - 10}
                  y={y + 3.5}
                  textAnchor="end"
                  className="cp-chart-tick font-mono"
                  style={{ ['--i' as string]: k + 4, fill: colors.inkMute, fontSize: 10 }}
                >
                  {formatTokens(t)}
                </text>
              </g>
            );
          })}
          <text
            x={L - 10}
            y={base - 5}
            textAnchor="end"
            className="cp-chart-tick"
            style={{ fill: cIn, fontSize: 11, fontWeight: 600 }}
          >
            in ↑
          </text>
          <text
            x={L - 10}
            y={dnTop + 13}
            textAnchor="end"
            className="cp-chart-tick"
            style={{ ['--i' as string]: 1, fill: cOut, fontSize: 11, fontWeight: 600 }}
          >
            out ↓
          </text>

          {/* the brush: the turns a drag is about to zoom into */}
          {brush && (
            <g className="cp-chart-brush" pointerEvents="none">
              <rect
                x={Math.max(L, Math.min(brush.x0, brush.x1))}
                y={T - 4}
                width={Math.min(plotRight, Math.max(brush.x0, brush.x1)) - Math.max(L, Math.min(brush.x0, brush.x1))}
                height={dnTop + DN_H - T + 4}
                rx={4}
                style={{ fill: colors.alpha('brand.500', 0.1), stroke: colors.alpha('brand.500', 0.55) }}
                strokeWidth={1}
              />
            </g>
          )}

          {/* marks: traced by the sweep, risen from their baselines */}
          <g clipPath={`url(#${clip})`}>
            <g clipPath={`url(#${plotClip})`}>
              <g className="cp-chart-rise">
                <g className="cp-chart-fade" style={{ opacity: seriesOpacity('in') }}>
                  <path d={areaDown(ctxLine, base)} fill={`url(#${uid}-gin)`} />
                </g>
                {hasCache && (
                  <g className="cp-chart-fade" style={{ opacity: seriesOpacity('cache') }}>
                    <path d={areaDown(cacheLine, base)} fill={`url(#${uid}-gcache)`} />
                    <path
                      d={cacheLine}
                      fill="none"
                      strokeWidth={1}
                      strokeLinejoin="round"
                      style={{ stroke: colors.alpha('chart.5', 0.7) }}
                    />
                  </g>
                )}
                <g className="cp-chart-fade" style={{ opacity: seriesOpacity('in') }}>
                  {per > 1 && (
                    <path
                      d={typLine}
                      fill="none"
                      strokeWidth={1.25}
                      strokeDasharray="1 3"
                      strokeLinecap="round"
                      style={{ stroke: colors.alpha('chart.1', 0.55) }}
                    />
                  )}
                  <path
                    d={ctxLine}
                    fill="none"
                    strokeWidth={6}
                    strokeLinejoin="round"
                    filter={`url(#${uid}-glow)`}
                    style={{ stroke: cIn, opacity: 0.35 }}
                  />
                  <path d={ctxLine} fill="none" strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" style={{ stroke: cIn }} />
                </g>
              </g>
              <g className="cp-chart-rise cp-down">
                <g className="cp-chart-fade" style={{ opacity: seriesOpacity('out') }}>
                  <path d={areaDown(outLine, dnTop)} fill={`url(#${uid}-gout)`} />
                  <path
                    d={outLine}
                    fill="none"
                    strokeWidth={6}
                    strokeLinejoin="round"
                    filter={`url(#${uid}-glow)`}
                    style={{ stroke: cOut, opacity: 0.3 }}
                  />
                  <path d={outLine} fill="none" strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" style={{ stroke: cOut }} />
                </g>
              </g>
            </g>
          </g>

          {/* baselines */}
          <line x1={L} x2={plotRight} y1={base + 0.5} y2={base + 0.5} style={{ stroke: colors.alpha('inkMute', 0.5) }} />
          <line x1={L} x2={plotRight} y1={dnTop - 0.5} y2={dnTop - 0.5} style={{ stroke: colors.alpha('inkMute', 0.3) }} />

          {/* the stage in focus: everything else steps back */}
          {hasLanes &&
            lanes.map((l, k) => {
              const { x0, w } = laneX(l.from, l.to);
              if (w <= 0) return null;
              return (
                <rect
                  key={`veil-${k}`}
                  x={x0}
                  y={T - 6}
                  width={w}
                  height={dnTop + DN_H - T + 8}
                  pointerEvents="none"
                  className="cp-chart-fade"
                  style={{ fill: colors.panel, opacity: focusStage != null && focusStage !== l.label ? 0.62 : 0 }}
                />
              );
            })}

          {/* the largest context in view */}
          {peak && peak.context > 0 && (() => {
            const px = xOf(peak.at);
            const label = `largest context · turn ${fmtInt(peak.i0 + 1)}`;
            const w = (formatTokens(peak.context).length + label.length) * 6.3 + 10;
            const right = px + 8 + w <= plotRight;
            return (
              <g pointerEvents="none">
                <line
                  x1={px}
                  x2={px}
                  y1={T - 8}
                  y2={yUp(peak.context) - 5}
                  strokeDasharray="2 2"
                  className="cp-chart-stem"
                  style={{ stroke: colors.alpha('inkMute', 0.8) }}
                />
                <text
                  x={right ? px + 7 : px - 7}
                  y={T - 9}
                  textAnchor={right ? 'start' : 'end'}
                  className="cp-chart-late"
                  style={{ fill: colors.ink, fontSize: 12, fontWeight: 620 }}
                >
                  {formatTokens(peak.context)}
                  <tspan dx={6} style={{ fill: colors.inkMute, fontSize: 11, fontWeight: 450 }}>
                    {label}
                  </tspan>
                </text>
              </g>
            );
          })()}

          {/* the end of the run: dots on both traces, breathing while live */}
          {endVisible && last && (
            <g pointerEvents="none">
              {live && (
                <>
                  <circle className="cp-chart-ring" cx={xOf(last.at)} cy={yUp(last.context)} r={4.5} fill="none" strokeWidth={1.5} style={{ stroke: cIn }} />
                  <circle className="cp-chart-ring" cx={xOf(last.at)} cy={yDn(last.out)} r={4.5} fill="none" strokeWidth={1.5} style={{ stroke: cOut }} />
                </>
              )}
              <circle className="cp-chart-pop" cx={xOf(last.at)} cy={yUp(last.context)} r={3.4} strokeWidth={1.5} style={{ fill: cIn, stroke: colors.panel }} />
              <circle className="cp-chart-pop" cx={xOf(last.at)} cy={yDn(last.out)} r={3.4} strokeWidth={1.5} style={{ fill: cOut, stroke: colors.panel }} />
            </g>
          )}

          {/* the stylus on each trace while the sweep draws */}
          <Stylus start={drawn} xs={xs} ys={rows.map((r) => yUp(r.context))} x0={L - 8} x1={L + pw + 8} color={cIn} />
          <Stylus start={drawn} xs={xs} ys={rows.map((r) => yDn(r.out))} x0={L - 8} x1={L + pw + 8} color={cOut} />

          {/* turn axis */}
          {ticks.map((i, k) => {
            const x = xOf(i);
            if (x < L - 1 || x > plotRight + 1) return null;
            return (
              <text
                key={`x-${i}`}
                x={x}
                y={H - 6}
                textAnchor={x - L < 18 ? 'start' : plotRight - x < 18 ? 'end' : 'middle'}
                className="cp-chart-tick font-mono"
                style={{ ['--i' as string]: k, fill: colors.inkMute, fontSize: 10 }}
              >
                {fmtInt(i + 1)}
              </text>
            );
          })}

          {/* crosshair */}
          {hoverRow && !drag && (
            <Crosshair
              x={hoverX}
              y0={T - 6}
              y1={dnTop + DN_H}
              strokeId={`${uid}-cross`}
              ring={colors.panel}
              appear={hover.appear}
              dots={[
                { key: 'in', y: yUp(hoverRow.context), color: cIn },
                ...(hoverRow.cached ? [{ key: 'cache', y: yUp(hoverRow.cached), color: cCache }] : []),
                { key: 'out', y: yDn(hoverRow.out), color: cOut },
              ]}
            />
          )}

          {/* pointer surface: hover, drag to zoom, double-click to reset */}
          <rect
            x={L}
            y={T - 6}
            width={pw}
            height={dnTop + DN_H - T + 6}
            fill="transparent"
            style={{ cursor: drag ? 'ew-resize' : 'crosshair' }}
            onMouseMove={onMove}
            onMouseDown={onDown}
            onMouseUp={endDrag}
            onMouseLeave={() => {
              hover.set(null);
              endDrag();
            }}
            onDoubleClick={() => setZoom(null)}
          />
        </svg>

        <ChartTooltip
          open={!!hoverRow && !drag}
          appear={hover.appear}
          x={hoverX}
          y={hoverRow ? Math.max(T + 4, yUp(hoverRow.context)) : 0}
          width={width}
          title={tipTitle}
          rows={tipRows}
          footer={tipFooter}
        />

        {/* zoom chip */}
        {(zoomed || brushWin) && (
          <div className="pointer-events-none absolute right-0 flex justify-end" style={{ top: T - 30 }}>
            <div className="cp-chart-chip pointer-events-auto inline-flex items-center gap-2 rounded-full border border-border bg-surface px-2.5 py-1 text-note tabular-nums text-ink-dim shadow-sm">
              {brushWin ? (
                <span>
                  Zoom to turns {fmtInt(brushWin[0] + 1)}–{fmtInt(Math.min(n, brushWin[1] + 1))}
                </span>
              ) : (
                zoomed && (
                  <>
                    <span>
                      Turns {fmtInt(zoomed[0] + 1)}–{fmtInt(zoomed[1] + 1)}
                      <span className="text-ink-mute"> of {fmtInt(n)}</span>
                    </span>
                    <button
                      type="button"
                      onClick={() => setZoom(null)}
                      className="inline-flex items-center gap-1 rounded-full text-ink transition-colors hover:text-brand-500"
                    >
                      <RotateCcw className="h-3 w-3" aria-hidden />
                      Reset
                    </button>
                  </>
                )
              )}
            </div>
          </div>
        )}
      </div>

      {/* stages: their share of the context, hover to focus, click to zoom */}
      {hasLanes && stageCount > 1 && (
        <ul className="mt-2 flex flex-wrap gap-x-4 gap-y-1.5" onMouseLeave={() => setFocusStage(null)}>
          {byStage.map((s) => {
            const share = totals.tokensIn ? s.tokensIn / totals.tokensIn : 0;
            const pct = share * 100;
            return (
              <li key={s.label}>
                <button
                  type="button"
                  className={cn(
                    'cp-chart-fade inline-flex cursor-zoom-in items-center gap-2 rounded text-note text-ink-dim outline-none focus-visible:ring-2 focus-visible:ring-brand-500/40',
                    focusStage && focusStage !== s.label && 'opacity-40',
                  )}
                  onMouseEnter={() => setFocusStage(s.label)}
                  onFocus={() => setFocusStage(s.label)}
                  onBlur={() => setFocusStage(null)}
                  onClick={() => zoomToStage(s.label)}
                >
                  <span className="relative h-1 w-8 overflow-hidden rounded-full bg-border" aria-hidden>
                    <span
                      className="absolute inset-y-0 left-0 rounded-full bg-ink-dim"
                      style={{ width: `${Math.max(4, Math.min(100, pct))}%` }}
                    />
                  </span>
                  <span>{s.label}</span>
                  <span className="font-semibold tabular-nums text-ink">{formatTokens(s.tokensIn)}</span>
                  <span className="tabular-nums text-ink-mute">{pct > 0 && pct < 1 ? '<1' : Math.round(pct)}%</span>
                </button>
              </li>
            );
          })}
        </ul>
      )}

      <p className="mt-2 text-note text-ink-mute">
        Above the line, the context each turn sent; below it, what the model wrote back (its own scale).
        {n > 8 && ' Drag across the chart to zoom into turns'}
        {n > 8 && hasLanes && stageCount > 1 && ', or click a stage'}
        {n > 8 && '.'}
      </p>
    </div>
  );
}
