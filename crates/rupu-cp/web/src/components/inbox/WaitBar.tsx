// WaitBar — the Inbox's signature element: a thin vertical bar at the left edge
// of a row whose FILL HEIGHT is the item's wait relative to the longest wait in
// the list, in the hue of its kind (amber for a gate, cyan for a paused run).
// Sorting by age is invisible; a relative bar makes the shape of the queue
// readable at a glance — the oldest item's bar is full height.
//
// Token colours only (inline `rgb(var(--c-*))` so they read in light + dark); a
// floor keeps the shortest wait a visible sliver rather than nothing.

const TONE_VAR: Record<'awaiting' | 'paused', string> = {
  awaiting: '--c-status-awaiting',
  paused: '--c-status-paused',
};

export function WaitBar({
  fraction,
  tone,
}: {
  /** 0..1 — this item's wait over the longest wait on the board. */
  fraction: number;
  tone: 'awaiting' | 'paused';
}) {
  const pct = Math.max(12, Math.min(100, Math.round(fraction * 100)));
  const color = `rgb(var(${TONE_VAR[tone]}))`;
  return (
    <span
      className="relative flex h-8 w-[3px] shrink-0 items-end overflow-hidden rounded-full"
      style={{ background: `rgb(var(${TONE_VAR[tone]}) / 0.15)` }}
      aria-hidden
    >
      <span
        className="w-full rounded-full transition-[height] duration-500 ease-out"
        style={{ height: `${pct}%`, background: color }}
      />
    </span>
  );
}

export default WaitBar;
