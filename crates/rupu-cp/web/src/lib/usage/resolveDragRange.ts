// resolveDragRange — pure ordering/no-op logic behind the usage graph's
// marquee drag-select (Task W3).
//
// Factored out of `UsageTimelineStacked` (and its `useDragSelection` state
// machine) because it's the one piece that's trivially unit-testable:
// Recharts computes a chart's `activeLabel` from real DOM layout (mouse
// position -> chart coordinate -> nearest tick), which jsdom cannot
// reproduce (no ResizeObserver-driven layout), so the mouse-drag plumbing
// itself is exercised via `useDragSelection.test.ts` instead, with this
// function as its ordering primitive.
//
// Day-bucket labels are `YYYY-MM-DD` strings (see `UsageTimelineBucket.bucket`
// in `../usage.ts`), so lexicographic ordering is equivalent to chronological
// ordering — no `Date` parsing needed.

export interface DragRange {
  startDay: string;
  endDay: string;
}

/**
 * Resolve a raw drag (mousedown bucket -> mouseup bucket) into an ordered
 * `{startDay, endDay}` range, or `null` when there was no real drag to act
 * on: either endpoint missing (e.g. the pointer never registered a bucket —
 * released outside the plot) or `start === end` (a click with no movement,
 * which must NOT collapse the window to a single day).
 */
export function resolveDragRange(
  start: string | undefined,
  end: string | undefined,
): DragRange | null {
  if (start == null || end == null) return null;
  if (start === end) return null;
  return start < end ? { startDay: start, endDay: end } : { startDay: end, endDay: start };
}
