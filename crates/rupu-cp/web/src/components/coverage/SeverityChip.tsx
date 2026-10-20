// Shared severity pill — the ONE status/severity chip of the visual language
// (spec §"Chips"), so a finding's severity reads the same in the findings
// table, the coverage concern tabs and a report header. Colour comes from the
// canonical SEVERITY_STYLE ramp (StatusChip resolves severity tones from the
// same map); an unrecognised value falls back to the neutral tone.

import { StatusChip, type ChipTone } from '../ui/StatusChip';

const SEVERITIES: ReadonlySet<string> = new Set(['critical', 'high', 'medium', 'low', 'info']);

export default function SeverityChip({ severity }: { severity: string }) {
  return (
    <StatusChip tone={SEVERITIES.has(severity) ? (severity as ChipTone) : 'neutral'}>
      {severity}
    </StatusChip>
  );
}
