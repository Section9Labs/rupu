// NodeFrame — the shared outer card of every run-graph node. It owns the
// node's motion (styles/graph-motion.css): its entrance slot, its state
// (`data-state` drives the glow, the bar shimmer and the badge spin), the
// one-shot cue a state change plays, and the hover lift / selection ring
// layers. Node components render their own content inside it.

import type { CSSProperties, ReactNode } from 'react';
import type { StepState } from '../../lib/runGraphModel';
import { useStateCue } from '../../lib/graph/runMotion';
import { cn } from '../../lib/cn';

export function NodeFrame({
  state,
  slot,
  className,
  style,
  testId,
  children,
}: {
  state: StepState;
  /** Entrance slot (see `entranceSlots`); 0 when unknown. */
  slot?: number;
  className?: string;
  style?: CSSProperties;
  testId?: string;
  children: ReactNode;
}) {
  const { cue, key } = useStateCue(state);
  return (
    <div
      data-testid={testId}
      data-state={state}
      data-cue={cue ?? undefined}
      className={cn('rgx-node relative', className)}
      style={{ ['--i' as string]: slot ?? 0, ...style }}
    >
      <span className="rgx-glow" aria-hidden />
      <span className="rgx-lift" aria-hidden />
      {children}
      <span className="rgx-sel" aria-hidden />
      {cue && <span key={key} className={`rgx-cue rgx-cue-${cue}`} aria-hidden />}
    </div>
  );
}

/** The kind-colored bar along a card's top edge; it shimmers while running. */
export function KindBar({ color }: { color: string }) {
  return (
    <div className="rgx-bar" style={{ background: color }} aria-hidden>
      <span className="rgx-sheen" />
    </div>
  );
}

/** The square status glyph badge. */
export function StateBadge({
  state,
  color,
  glyph,
  small,
}: {
  state: StepState;
  color: string;
  glyph: string;
  /** A sub-step's badge (parallel / panel rows): smaller, and no landing pop. */
  small?: boolean;
}) {
  return (
    <span
      data-state={state}
      className={cn(
        'rgx-badge inline-flex shrink-0 items-center justify-center font-bold leading-none text-white',
        small ? 'h-3 w-3 rounded-[3px] text-[8px]' : 'rgx-main h-[15px] w-[15px] rounded text-meta',
      )}
      style={{ background: color }}
      aria-hidden
    >
      <span>{glyph}</span>
    </span>
  );
}
