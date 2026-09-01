// Activity strip — the full-range context histogram with drag-to-zoom,
// beside the existing `TimeRangePicker` presets and the server-echo
// `NetflowWindowReadout`.
//
// The histogram is ALWAYS the scope's whole retained history (server-
// chosen bounds, `HistogramView.from/to`), never the applied window —
// it's the context a window is dragged against. The readout beside it is
// built SOLELY from the server's `window` echo, same rule as everywhere
// else. Drag commits only when the selection spans more than one bucket
// (a click or micro-drag is noise, not intent).
//
// Bar/overlay tints go through `useThemeColors` (per-bucket alpha needs a
// literal rgb string; same bridge the retired NetflowGraph used).

import { useRef, useState } from 'react';
import type { PointerEvent as ReactPointerEvent } from 'react';
import { absoluteTime } from '../../../lib/time';
import { useThemeColors } from '../../../lib/useThemeColors';
import type { HistogramView } from '../../../lib/netflow';
import NetflowWindowReadout from '../NetflowWindowReadout';
import type { NetflowWindowEcho } from '../ScopeDisclosure';
import TimeRangePicker, { type TimeRangeValue } from '../TimeRangePicker';

export interface ActivityStripProps {
  histogram: HistogramView;
  range: TimeRangeValue;
  /** Picker-initiated change (preset / custom apply). */
  onRangeChange: (value: TimeRangeValue) => void;
  /** Drag-initiated zoom — separate from `onRangeChange` so the owner can
   *  push the outgoing window onto its zoom stack for "Zoom out". */
  onZoom: (fromIso: string, toIso: string) => void;
  /** The server's own `window` echo — `null` only before the first load. */
  appliedWindow: NetflowWindowEcho | null;
}

interface DragState {
  a: number; // ms epoch
  b: number;
}

export function ActivityStrip({
  histogram,
  range,
  onRangeChange,
  onZoom,
  appliedWindow,
}: ActivityStripProps) {
  const colors = useThemeColors();
  const chartRef = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<DragState | null>(null);

  const t0 = histogram.from ? Date.parse(histogram.from) : null;
  const t1 = histogram.to ? Date.parse(histogram.to) : null;
  const span = t0 !== null && t1 !== null ? Math.max(1, t1 - t0) : null;

  const maxCalls = Math.max(1, ...histogram.buckets.map((b) => b.calls));

  function timeAt(e: ReactPointerEvent): number | null {
    if (span === null || t0 === null || !chartRef.current) return null;
    const rect = chartRef.current.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    return t0 + frac * span;
  }

  function onPointerDown(e: ReactPointerEvent) {
    const t = timeAt(e);
    if (t === null) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ a: t, b: t });
  }

  function onPointerMove(e: ReactPointerEvent) {
    if (!drag) return;
    const t = timeAt(e);
    if (t !== null) setDrag({ a: drag.a, b: t });
  }

  function onPointerUp() {
    if (!drag || span === null) return;
    const from = Math.min(drag.a, drag.b);
    const to = Math.max(drag.a, drag.b);
    setDrag(null);
    // Commit only when the selection spans more than one bucket.
    if (to - from > span / histogram.buckets.length) {
      onZoom(new Date(from).toISOString(), new Date(to).toISOString());
    }
  }

  // Overlay: the in-progress drag wins; otherwise the SERVER-echoed
  // window (never local picker state), mapped into the histogram bounds.
  let overlay: { from: number; to: number } | null = null;
  if (drag) {
    overlay = { from: Math.min(drag.a, drag.b), to: Math.max(drag.a, drag.b) };
  } else if (appliedWindow && t0 !== null && t1 !== null) {
    const wf = appliedWindow.from !== null ? Date.parse(appliedWindow.from) : t0;
    const wt = appliedWindow.to !== null ? Date.parse(appliedWindow.to) : t1;
    if (appliedWindow.from !== null || appliedWindow.to !== null) {
      overlay = { from: wf, to: wt };
    }
  }

  return (
    <div className="rounded-xl border border-border bg-panel px-4 pb-2.5 pt-3.5">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-3">
        <TimeRangePicker value={range} onChange={onRangeChange} />
        <div className="flex flex-wrap items-center gap-2">
          {appliedWindow && <NetflowWindowReadout appliedWindow={appliedWindow} />}
          {span !== null && (
            <p className="text-note text-ink-mute">Drag on the chart to zoom a window.</p>
          )}
        </div>
      </div>
      <div
        ref={chartRef}
        role="img"
        aria-label="Recorded flows over the full retained range; drag to zoom"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        className="relative flex h-16 touch-none select-none items-end gap-px cursor-crosshair"
      >
        {histogram.buckets.map((b, i) => (
          <div
            key={i}
            className="pointer-events-none flex h-full flex-1 flex-col justify-end"
          >
            <div
              className="rounded-t-[1px]"
              style={{
                height: `${Math.round((100 * b.errors) / maxCalls)}%`,
                background: colors.alpha('status.failed', 0.8),
              }}
            />
            <div
              className="rounded-[1px]"
              style={{
                height: `${Math.round((100 * (b.calls - b.errors)) / maxCalls)}%`,
                background: colors.alpha('brand.500', 0.45),
              }}
            />
          </div>
        ))}
        {overlay && span !== null && t0 !== null && (
          <div
            data-testid="activity-window-overlay"
            className="pointer-events-none absolute inset-y-0"
            style={{
              left: `${Math.max(0, (100 * (overlay.from - t0)) / span)}%`,
              width: `${Math.max(0.5, Math.min(100, (100 * (overlay.to - overlay.from)) / span))}%`,
              background: colors.alpha('brand.500', 0.08),
              borderLeft: `1px solid ${colors.alpha('brand.500', 0.55)}`,
              borderRight: `1px solid ${colors.alpha('brand.500', 0.55)}`,
            }}
          />
        )}
      </div>
      <div className="flex justify-between pt-1 text-meta text-ink-mute">
        <span>{histogram.from ? absoluteTime(histogram.from) : '—'}</span>
        <span>
          {t0 !== null && t1 !== null && span !== null
            ? absoluteTime(new Date(t0 + span / 2).toISOString())
            : ''}
        </span>
        <span>{histogram.to ? absoluteTime(histogram.to) : '—'}</span>
      </div>
    </div>
  );
}

export default ActivityStrip;
