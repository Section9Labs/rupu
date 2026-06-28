// SplitPane — a vertical (stacked) resizable splitter.
//
// Renders `top` over `bottom` with a draggable horizontal divider between them.
// `ratio` is the TOP pane's fraction of the container height (local state, clamped
// to [minRatio, maxRatio]). Pointer drag computes the fraction from the pointer's
// clientY relative to the container; the divider is also keyboard-accessible
// (role="separator", ArrowUp/ArrowDown adjust the ratio). Pure presentational.

import { useCallback, useRef, useState } from 'react';

interface SplitPaneProps {
  top: React.ReactNode;
  bottom: React.ReactNode;
  /** Initial top-pane fraction. Default 0.62. */
  defaultRatio?: number;
  /** Smallest top-pane fraction. Default 0.25. */
  minRatio?: number;
  /** Largest top-pane fraction. Default 0.8. */
  maxRatio?: number;
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

const KEY_STEP = 0.03;

export default function SplitPane({
  top,
  bottom,
  defaultRatio = 0.62,
  minRatio = 0.25,
  maxRatio = 0.8,
}: SplitPaneProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [ratio, setRatio] = useState(() => clamp(defaultRatio, minRatio, maxRatio));

  const setFromClientY = useCallback(
    (clientY: number) => {
      const el = containerRef.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      if (rect.height <= 0) return;
      const frac = (clientY - rect.top) / rect.height;
      setRatio(clamp(frac, minRatio, maxRatio));
    },
    [minRatio, maxRatio],
  );

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      e.preventDefault();
      const divider = e.currentTarget;
      divider.setPointerCapture(e.pointerId);
      const onMove = (ev: PointerEvent) => setFromClientY(ev.clientY);
      const onUp = (ev: PointerEvent) => {
        divider.releasePointerCapture(ev.pointerId);
        divider.removeEventListener('pointermove', onMove);
        divider.removeEventListener('pointerup', onUp);
      };
      divider.addEventListener('pointermove', onMove);
      divider.addEventListener('pointerup', onUp);
    },
    [setFromClientY],
  );

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setRatio((r) => clamp(r - KEY_STEP, minRatio, maxRatio));
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        setRatio((r) => clamp(r + KEY_STEP, minRatio, maxRatio));
      }
    },
    [minRatio, maxRatio],
  );

  return (
    <div ref={containerRef} className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 overflow-hidden" style={{ flexBasis: `${ratio * 100}%` }}>
        {top}
      </div>

      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize graph and YAML panes"
        aria-valuenow={Math.round(ratio * 100)}
        aria-valuemin={Math.round(minRatio * 100)}
        aria-valuemax={Math.round(maxRatio * 100)}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onKeyDown={onKeyDown}
        className="group flex h-2.5 shrink-0 cursor-row-resize touch-none items-center justify-center border-y border-border bg-panel hover:bg-surface-hover focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500"
      >
        <span className="h-1 w-10 rounded-full bg-border group-hover:bg-ink-mute" aria-hidden />
      </div>

      <div className="min-h-0 flex-1 overflow-hidden">{bottom}</div>
    </div>
  );
}
