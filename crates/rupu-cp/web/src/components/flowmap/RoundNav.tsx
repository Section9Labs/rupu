// Moving through a long agentiflow (FlowMap): the round navigator, a slim
// scrollbar of rounds pinned to the canvas's left edge (one mark per round in
// its state's colour, merged when they don't fit, the rounds in view
// highlighted; click or drag to move there), and the toolbar's previous /
// next round with "round N / M". Both read the viewport themselves
// (`useViewport`), so a pan re-renders only them, never the map.

import { useCallback, useRef, type RefObject } from 'react';
import { useViewport } from '@xyflow/react';
import { ChevronDown, ChevronUp } from 'lucide-react';
import type { StepState } from '../../lib/runGraphModel';
import { indexAt, mergeTicks, roundAt, roundsInView } from '../../lib/flow/camera';

/** A mark's height and gap, in px: merged when the rounds don't fit. */
const MARK_PX = 3;

export interface RoundsGeometry {
  /** Each round card's flow-y top, in order. */
  tops: number[];
  /** The map's bottom (the last round's block ends there). */
  end: number;
  /** Each round's label number. */
  labels: number[];
}

/** The pane's size, read when needed (not reactive). */
function paneSize(pane: RefObject<HTMLElement | null>) {
  return { width: pane.current?.clientWidth ?? 0, height: pane.current?.clientHeight ?? 0 };
}

/** The view's flow-y span. */
function viewSpan(vp: { y: number; zoom: number }, height: number) {
  return { top: -vp.y / vp.zoom, bottom: (height - vp.y) / vp.zoom };
}

/** The round the view is at: the one at a fifth of the way down. */
export function useCurrentRound(geo: RoundsGeometry, pane: RefObject<HTMLElement | null>): number {
  const vp = useViewport();
  const { top, bottom } = viewSpan(vp, paneSize(pane).height);
  return roundAt(geo.tops, top + (bottom - top) * 0.2);
}

export function RoundNavigator({
  geo,
  states,
  pane,
  onGo,
}: {
  geo: RoundsGeometry;
  states: StepState[];
  pane: RefObject<HTMLElement | null>;
  onGo: (index: number, how: 'smooth' | 'jump') => void;
}) {
  const vp = useViewport();
  const el = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const n = geo.tops.length;
  const height = el.current?.clientHeight ?? 0;
  const ticks = mergeTicks(states, height > 0 ? height / MARK_PX : 80);
  const { top, bottom } = viewSpan(vp, paneSize(pane).height);
  const inView = roundsInView(geo.tops, geo.end, top, bottom);
  const pct = (i: number) => `${(i / Math.max(1, n)) * 100}%`;

  const indexOf = useCallback(
    (clientY: number) => {
      const r = el.current!.getBoundingClientRect();
      return indexAt(n, r.height > 0 ? (clientY - r.top) / r.height : 0);
    },
    [n],
  );

  if (n < 2) return null;
  return (
    <div
      ref={el}
      className="afx-nav nodrag nowheel"
      role="slider"
      tabIndex={-1}
      aria-label="Rounds"
      aria-valuemin={geo.labels[0]}
      aria-valuemax={geo.labels[n - 1]}
      aria-valuenow={geo.labels[inView?.[0] ?? 0]}
      data-testid="round-nav"
      onPointerDown={(e) => {
        dragging.current = true;
        e.currentTarget.setPointerCapture?.(e.pointerId);
        onGo(indexOf(e.clientY), 'smooth');
      }}
      onPointerMove={(e) => {
        if (dragging.current && e.buttons) onGo(indexOf(e.clientY), 'jump');
      }}
      onPointerUp={() => {
        dragging.current = false;
      }}
      onPointerCancel={() => {
        dragging.current = false;
      }}
    >
      {ticks.map((t) => (
        <i
          key={t.from}
          className="afx-nav-mark gx-s"
          data-state={t.state}
          style={{ top: pct(t.from), height: `max(1px, calc(${pct(t.to + 1)} - ${pct(t.from)} - 1px))` }}
        />
      ))}
      {inView && (
        <span
          className="afx-nav-view"
          aria-hidden
          style={{ top: pct(inView[0]), height: `calc(${pct(inView[1] + 1)} - ${pct(inView[0])})` }}
        />
      )}
    </div>
  );
}

export function RoundStepper({
  geo,
  pane,
  onGo,
}: {
  geo: RoundsGeometry;
  pane: RefObject<HTMLElement | null>;
  onGo: (index: number, how: 'smooth' | 'jump') => void;
}) {
  const cur = useCurrentRound(geo, pane);
  const n = geo.tops.length;
  if (n < 2) return null;
  return (
    <div className="afx-stepper" role="group" aria-label="Rounds">
      <button type="button" className="afx-pill" aria-label="Previous round" disabled={cur <= 0} onClick={() => onGo(cur - 1, 'smooth')}>
        <ChevronUp size={13} aria-hidden />
      </button>
      <span className="afx-stepper-at" aria-live="polite">
        round <b>{geo.labels[cur]}</b> / {geo.labels[n - 1]}
      </span>
      <button type="button" className="afx-pill" aria-label="Next round" disabled={cur >= n - 1} onClick={() => onGo(cur + 1, 'smooth')}>
        <ChevronDown size={13} aria-hidden />
      </button>
      <span className="afx-stepper-keys" title="With the map focused">j / k</span>
    </div>
  );
}
