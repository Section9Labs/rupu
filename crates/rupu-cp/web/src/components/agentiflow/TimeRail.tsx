// TimeRail — the timelapse axis beside a long agentiflow flow.
//
// A goal-directed engagement can run for hours or days over hundreds of rounds.
// The flow graph itself lives in a fixed-height scroll pane (so it never grows
// into a mile-high page); this rail is its time axis: faint day bands with date
// labels, adaptive hour ticks, a subtle activity texture (one mark per round, so
// you see the run's rhythm in time), and a draggable viewport lens that tracks —
// and scrubs — the scroll position. It maps CONTENT position to TIME through the
// round anchors (round N happened at time T, at content-y Y), so the lens sits
// where the visible slice actually falls in time, not merely by scroll fraction.
//
// Rendered only when the flow overflows its pane (long runs). Short runs keep the
// exact look they have today — no pane cap, no rail.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';

/** A (content-y, time) fix point — the start node, each round node, the stop /
 *  live tail. Sorted by `y` ascending with non-decreasing `t`. */
export interface TimeAnchor {
  y: number;
  t: number; // epoch ms
}

const AXIS_X = 28; // the vertical axis line's x within the rail
const RAIL_W = 88;
const MIN_LENS = 20; // the lens never shrinks below this (very long runs)

// "Nice" tick steps, 5 minutes → 2 days, for the adaptive hour/day ticks.
const STEPS_MS = [5, 10, 15, 30, 60, 120, 180, 360, 720, 1440, 2880].map((m) => m * 60_000);

function hexA(hex: string, a: number): string {
  const h = hex.replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  const n = parseInt(full, 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}

const MON = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const pad = (n: number) => (n < 10 ? `0${n}` : `${n}`);
const hhmm = (t: number) => {
  const d = new Date(t);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
};
const dayLabel = (t: number) => {
  const d = new Date(t);
  return `${MON[d.getMonth()]} ${d.getDate()}`;
};
const midnight = (t: number) => {
  const d = new Date(t);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};

/** Interpolate the time at a content-y position through the anchors. */
function tAtY(anchors: TimeAnchor[], y: number): number {
  const n = anchors.length;
  if (n === 0) return 0;
  if (y <= anchors[0].y) return anchors[0].t;
  if (y >= anchors[n - 1].y) return anchors[n - 1].t;
  for (let i = 0; i < n - 1; i++) {
    const a = anchors[i];
    const b = anchors[i + 1];
    if (y >= a.y && y <= b.y) {
      const f = (y - a.y) / Math.max(b.y - a.y, 1);
      return a.t + f * (b.t - a.t);
    }
  }
  return anchors[n - 1].t;
}

/** Interpolate the content-y for a given time through the anchors (lens drag). */
function yAtT(anchors: TimeAnchor[], t: number): number {
  const n = anchors.length;
  if (n === 0) return 0;
  if (t <= anchors[0].t) return anchors[0].y;
  if (t >= anchors[n - 1].t) return anchors[n - 1].y;
  for (let i = 0; i < n - 1; i++) {
    const a = anchors[i];
    const b = anchors[i + 1];
    if (t >= a.t && t <= b.t) {
      const f = (t - a.t) / Math.max(b.t - a.t, 1);
      return a.y + f * (b.y - a.y);
    }
  }
  return anchors[n - 1].y;
}

export default function TimeRail({
  flowRef,
  anchors,
  tint,
}: {
  flowRef: React.RefObject<HTMLDivElement>;
  anchors: TimeAnchor[];
  tint: string;
}) {
  const railRef = useRef<HTMLDivElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);
  const clockRef = useRef<HTMLDivElement>(null);
  const [railH, setRailH] = useState(0);
  const dragging = useRef(false);

  const startT = anchors.length ? anchors[0].t : 0;
  const endT = anchors.length ? anchors[anchors.length - 1].t : startT + 1;
  const span = Math.max(endT - startT, 1);
  const yOfT = (t: number) => ((t - startT) / span) * railH;

  // Keep the rail's measured height in sync with the pane it sits beside.
  useLayoutEffect(() => {
    const el = railRef.current;
    if (!el) return;
    const measure = () => setRailH(el.clientHeight);
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Day bands (alternating faint fill + a date label) across the run's span.
  const bands = useMemo(() => {
    if (railH === 0) return [];
    const out: { top: number; height: number; alt: boolean; label: string; t: number }[] = [];
    let d = midnight(startT);
    let i = 0;
    while (d < endT) {
      const next = d + 86_400_000;
      const top = yOfT(Math.max(d, startT));
      const bottom = yOfT(Math.min(next, endT));
      out.push({ top, height: Math.max(bottom - top, 0), alt: i % 2 === 1, label: dayLabel(d + 1), t: d });
      d = next;
      i++;
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [startT, endT, railH]);

  // Adaptive hour ticks: aim for ~one label per 64px of rail.
  const ticks = useMemo(() => {
    if (railH === 0) return [];
    const target = Math.max(3, Math.floor(railH / 64));
    const step = STEPS_MS.find((s) => span / s <= target) ?? STEPS_MS[STEPS_MS.length - 1];
    if (step >= 86_400_000) return []; // day-scale → the day bands already label it
    const out: { y: number; label: string }[] = [];
    for (let t = Math.ceil(startT / step) * step; t <= endT; t += step) {
      const d = new Date(t);
      if (d.getHours() === 0 && d.getMinutes() === 0) continue; // midnight = a day band
      out.push({ y: yOfT(t), label: hhmm(t) });
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [startT, endT, railH]);

  // Activity texture: one short mark per round, deduped so dense runs stay clean.
  const marks = useMemo(() => {
    if (railH === 0) return [];
    const out: number[] = [];
    let last = -Infinity;
    for (const a of anchors) {
      const y = yOfT(a.t);
      if (y - last >= 3) {
        out.push(y);
        last = y;
      }
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [anchors, railH]);

  // Move the lens + clock imperatively on scroll — the graph never re-renders.
  useEffect(() => {
    const flow = flowRef.current;
    if (!flow || railH === 0) return;
    const update = () => {
      const topT = tAtY(anchors, flow.scrollTop);
      const botT = tAtY(anchors, flow.scrollTop + flow.clientHeight);
      const y0 = yOfT(topT);
      const h = Math.max(MIN_LENS, yOfT(botT) - y0);
      const top = Math.min(y0, railH - h);
      const thumb = thumbRef.current;
      if (thumb) {
        thumb.style.top = `${Math.max(0, top)}px`;
        thumb.style.height = `${h}px`;
      }
      const clock = clockRef.current;
      if (clock) {
        clock.textContent = `${dayLabel(topT)} ${hhmm(topT)}`;
        clock.style.top = `${Math.min(Math.max(0, top), railH - 18)}px`;
      }
    };
    update();
    flow.addEventListener('scroll', update, { passive: true });
    return () => flow.removeEventListener('scroll', update);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [flowRef, anchors, railH, startT, endT]);

  // Drag the lens to scrub: pointer-y → time → content-y → scroll there.
  const seek = (clientY: number) => {
    const rail = railRef.current;
    const flow = flowRef.current;
    if (!rail || !flow) return;
    const rect = rail.getBoundingClientRect();
    const f = Math.min(1, Math.max(0, (clientY - rect.top) / Math.max(rect.height, 1)));
    const t = startT + f * span;
    flow.scrollTop = yAtT(anchors, t) - flow.clientHeight / 2;
  };
  const onPointerDown = (e: React.PointerEvent) => {
    dragging.current = true;
    railRef.current?.setPointerCapture(e.pointerId);
    seek(e.clientY);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    if (dragging.current) seek(e.clientY);
  };
  const endDrag = (e: React.PointerEvent) => {
    dragging.current = false;
    railRef.current?.releasePointerCapture(e.pointerId);
  };

  return (
    <div
      ref={railRef}
      className="relative shrink-0 select-none border-l border-border"
      style={{ width: RAIL_W, cursor: 'ns-resize', touchAction: 'none' }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      aria-hidden
    >
      {/* day bands + date labels */}
      {bands.map((b) => (
        <div key={`band-${b.t}`} className="absolute left-0 right-0" style={{ top: b.top, height: b.height, background: b.alt ? hexA('#808080', 0.06) : 'transparent' }}>
          <span className="absolute left-1.5 top-1 text-[10px] font-medium tracking-wide text-ink-dim">{b.label}</span>
        </div>
      ))}

      {/* the axis line */}
      <div className="absolute top-0 bottom-0" style={{ left: AXIS_X, width: 1, background: 'var(--border, rgba(127,127,127,0.25))' }} />

      {/* activity texture — one mark per round */}
      {marks.map((y, i) => (
        <div key={`m-${i}`} className="absolute" style={{ left: AXIS_X - 7, top: y, width: 5, height: 1.5, borderRadius: 1, background: tint, opacity: 0.5 }} />
      ))}

      {/* hour ticks + labels */}
      {ticks.map((tk, i) => (
        <div key={`t-${i}`}>
          <div className="absolute" style={{ left: AXIS_X, top: tk.y, width: 6, height: 1, background: 'var(--border, rgba(127,127,127,0.3))' }} />
          <span className="absolute text-[10px] tabular-nums text-ink-mute" style={{ left: AXIS_X + 9, top: tk.y, transform: 'translateY(-50%)' }}>
            {tk.label}
          </span>
        </div>
      ))}

      {/* the viewport lens (draggable) */}
      <div ref={thumbRef} className="absolute rounded-md" style={{ left: AXIS_X - 9, width: 18, top: 0, height: MIN_LENS, border: `1px solid ${tint}`, background: hexA(tint, 0.16) }}>
        <div className="absolute left-1/2 -translate-x-1/2" style={{ top: '50%', width: 8, height: 1, marginTop: -2, background: tint, opacity: 0.8 }} />
        <div className="absolute left-1/2 -translate-x-1/2" style={{ top: '50%', width: 8, height: 1, marginTop: 1, background: tint, opacity: 0.8 }} />
      </div>

      {/* the time-at-viewport-top readout, riding the lens */}
      <div
        ref={clockRef}
        className="pointer-events-none absolute left-1 rounded border border-border bg-surface px-1 py-0.5 text-[10px] font-medium tabular-nums text-ink-mute"
        style={{ top: 0 }}
      />
    </div>
  );
}
