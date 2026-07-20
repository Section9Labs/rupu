// useDragSelection — the marquee drag-select state machine behind the usage
// graph's date-window selection (Task W3).
//
// Extracted out of `UsageTimelineStacked` for two reasons:
//   1. It's the actually-testable core of the interaction. Recharts computes
//      a chart's `activeLabel` from real DOM layout (mouse position -> chart
//      coordinate -> nearest tick), which jsdom cannot reproduce (no
//      ResizeObserver-driven layout) — so `UsageTimelineStacked.test.tsx`
//      can only assert wiring/inertness, not a full pixel drag. This hook
//      lets `@testing-library/react`'s `renderHook` drive the state machine
//      directly with synthetic `{activeLabel}` args, exactly as Recharts
//      would call `onMouseDown`/`onMouseMove`.
//   2. INERT BY CONSTRUCTION when `onSelectRange` is undefined — every
//      handler no-ops immediately, so the dashboard's other
//      `UsageTimelineStacked` consumers (which don't pass it) see zero
//      behavior change just by having these handlers wired to the chart.
//
// `resolveDragRange` (a plain pure function) is the ordering/no-op logic
// `finalizeDrag` defers to; this hook owns only the start/move/finalize
// state transitions.

import { useState } from 'react';
import { resolveDragRange } from '../../lib/usage/resolveDragRange';

/** The subset of Recharts' `MouseHandlerDataParam` this hook reads. */
export interface DragMouseState {
  activeLabel?: string | number;
}

export interface DragBand {
  /** The bucket the drag started on. */
  start: string | number;
  /** The bucket the pointer is currently over (or `start`, before any move). */
  current: string | number;
}

export interface DragSelection {
  /** The in-progress selection band, or `null` when no drag is active. Not
   *  ordered — `start` may be after `current` mid-drag; only the finalized
   *  `onSelectRange` call is ordered. */
  band: DragBand | null;
  onMouseDown: (state: DragMouseState) => void;
  onMouseMove: (state: DragMouseState) => void;
  /** Ends the drag — call from both `onMouseUp` and `onMouseLeave` (a
   *  pointer-up outside the plot never reaches the chart's `onMouseUp`, so
   *  `onMouseLeave` is the only clean way to end a drag that exits the
   *  plot area). */
  finalizeDrag: () => void;
}

/**
 * `onSelectRange` is optional and, when absent, makes every handler a no-op
 * — see the file header. `startDay`/`endDay` passed to it are always
 * ordered `startDay <= endDay` regardless of drag direction.
 */
export function useDragSelection(onSelectRange?: (startDay: string, endDay: string) => void): DragSelection {
  const [dragStart, setDragStart] = useState<string | number | undefined>(undefined);
  const [dragCurrent, setDragCurrent] = useState<string | number | undefined>(undefined);

  const onMouseDown = (state: DragMouseState) => {
    if (!onSelectRange || state.activeLabel == null) return;
    setDragStart(state.activeLabel);
    setDragCurrent(state.activeLabel);
  };

  const onMouseMove = (state: DragMouseState) => {
    if (!onSelectRange || dragStart == null || state.activeLabel == null) return;
    setDragCurrent(state.activeLabel);
  };

  const finalizeDrag = () => {
    if (!onSelectRange || dragStart == null) return;
    const resolved = resolveDragRange(
      String(dragStart),
      dragCurrent != null ? String(dragCurrent) : undefined,
    );
    if (resolved) onSelectRange(resolved.startDay, resolved.endDay);
    setDragStart(undefined);
    setDragCurrent(undefined);
  };

  return {
    band: dragStart != null ? { start: dragStart, current: dragCurrent ?? dragStart } : null,
    onMouseDown,
    onMouseMove,
    finalizeDrag,
  };
}
