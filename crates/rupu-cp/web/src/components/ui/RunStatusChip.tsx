// RunStatusChip — a run/step status rendered as the ONE status chip of the
// common visual language (spec §"Chips"; the `NeedsYou` hero set the
// precedent): a pale tint of the status's own hue with a leading dot, the
// canonical label from `lib/status`. This is what the run-list screens render
// in their Status column, replacing the older icon-bearing `StatusPill` there
// (`StatusPill` stays for the run-detail header and the graph, which still
// want the icon + liveness motion).
//
// Every run status maps 1:1 onto a `ChipTone` (same names), so the tone is the
// status key; the label comes from the canonical descriptor so a list and the
// dashboard never spell a status differently. The dot is always shown, matching
// the approved mock's list rows and `NeedsYou`.

import { StatusChip, type ChipTone } from './StatusChip';
import { STATUS, normalizeStatusKey } from '../../lib/status';
import type { RunStatusStr } from '../../lib/api';

export function RunStatusChip({ status }: { status: RunStatusStr }) {
  const d = STATUS[normalizeStatusKey(status)];
  return (
    <StatusChip tone={status as ChipTone} dot>
      {d?.label ?? status}
    </StatusChip>
  );
}

export default RunStatusChip;
