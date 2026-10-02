// WarnMark — the ⚠ a run-graph node wears when its step (or one of its units)
// emitted a `step_warning`. A warning is an operator notice, NOT a status: the
// marker sits beside the node's own status glyph/label and never replaces
// them, and it lives inside an existing header row so `nodeSize` is unchanged.
// The message(s) are the hover title (the graph's one tooltip idiom — unit
// squares and rows already carry their detail as `title`).

import type { StepWarningView } from '../../lib/runGraphModel';
import { cn } from '../../lib/cn';

/** A tooltip lists at most this many warnings; a wide fan-out can warn once
 *  per unit, and the run page's banner is the full list. */
export const WARNING_TITLE_LIMIT = 10;

/** Hover text for a step's warnings: one line each, unit-scoped ones prefixed
 *  `unit N:`, capped at {@link WARNING_TITLE_LIMIT} with a `+N more` tail. */
export function warningTitle(warnings: readonly StepWarningView[]): string {
  const lines = warnings
    .slice(0, WARNING_TITLE_LIMIT)
    .map((w) => (w.index === undefined ? w.message : `unit ${w.index}: ${w.message}`));
  if (warnings.length > WARNING_TITLE_LIMIT) lines.push(`+${warnings.length - WARNING_TITLE_LIMIT} more`);
  return lines.join('\n');
}

/** The ` · ⚠ <message>` tail a unit's hover title gains when it has warnings
 *  (capped like {@link warningTitle}); empty when it has none. */
export function unitWarningSuffix(messages: readonly string[] | undefined): string {
  if (!messages || messages.length === 0) return '';
  const shown = messages.slice(0, WARNING_TITLE_LIMIT).map((m) => ` · ⚠ ${m}`);
  if (messages.length > WARNING_TITLE_LIMIT) shown.push(` · +${messages.length - WARNING_TITLE_LIMIT} more`);
  return shown.join('');
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
