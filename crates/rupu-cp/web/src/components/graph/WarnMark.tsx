// WarnMark — the ⚠ a run-graph node wears when its step (or one of its units)
// emitted a `step_warning`. A warning is an operator notice, NOT a status: the
// marker sits beside the node's own status glyph/label and never replaces
// them, and it lives inside an existing header row so `nodeSize` is unchanged.
// The message(s) are the hover title (the graph's one tooltip idiom — unit
// squares and rows already carry their detail as `title`).

import type { StepWarningView } from '../../lib/runGraphModel';
import { cn } from '../../lib/cn';

/** Hover text for a step's warnings: one line each, unit-scoped ones prefixed
 *  `unit N:`. */
export function warningTitle(warnings: readonly StepWarningView[]): string {
  return warnings
    .map((w) => (w.index === undefined ? w.message : `unit ${w.index}: ${w.message}`))
    .join('\n');
}

export default function WarnMark({
  warnings,
  className,
}: {
  warnings: readonly StepWarningView[] | undefined;
  className?: string;
}) {
  if (!warnings || warnings.length === 0) return null;
  const n = warnings.length;
  return (
    <span
      data-testid="rg-warn"
      role="img"
      aria-label={`${n} warning${n === 1 ? '' : 's'}`}
      title={warningTitle(warnings)}
      className={cn('shrink-0 text-ui font-bold leading-none text-warn', className)}
    >
      ⚠
    </span>
  );
}
