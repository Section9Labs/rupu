// Trigger badge — manual / cron / event, shared across project tab bodies.
// Thin wrapper over the generic Badge primitive; maps each trigger kind to a
// tone and adds the uppercase/tracking treatment.

import { Badge, type BadgeTone } from './ui/Badge';

const TRIGGER_TONE: Record<string, BadgeTone> = {
  manual: 'neutral',
  cron: 'violet',
  event: 'sky',
};

export function TriggerChip({ trigger }: { trigger: string }) {
  return (
    <Badge tone={TRIGGER_TONE[trigger] ?? 'neutral'} className="uppercase tracking-wide">
      {trigger}
    </Badge>
  );
}
