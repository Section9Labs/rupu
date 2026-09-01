// Fidelity badge — always rendered next to a flow. The subsystem exists so a
// viewer can tell "we could not observe this" from "it was zero"; the badge
// is how that survives into the UI even when a row's bytes/peer-IP columns
// are dashes. Thin wrapper over the generic Badge primitive (same pattern as
// TriggerChip), mapping each fidelity level to a tone.

import { Badge, type BadgeTone } from '../ui/Badge';
import type { Fidelity } from '../../lib/netflow';

const FIDELITY_TONE: Record<Fidelity, BadgeTone> = {
  coarse: 'amber',
  http: 'green',
  full: 'sky',
};

/** Exported for the explorer's CoveragePopover fidelity legend — the ONE
 *  authored copy of what each level means, so the legend and the badge
 *  tooltips can never drift apart. */
export const FIDELITY_TITLE: Record<Fidelity, string> = {
  coarse:
    'Coarse — host, outcome and timing are real; byte counts and peer IP were not observable for this connector.',
  http: 'HTTP — exact request and response metadata from the instrumented client.',
  full: 'Full — frame-level capture from the isolated runtime.',
};

export function FidelityBadge({ fidelity }: { fidelity: Fidelity }) {
  return (
    <Badge tone={FIDELITY_TONE[fidelity]} title={FIDELITY_TITLE[fidelity]}>
      {fidelity}
    </Badge>
  );
}

export default FidelityBadge;
