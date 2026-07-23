// Shared severity pill, used across the coverage concern tabs. Colour-coded off
// the canonical SEVERITY_STYLE ramp so it matches FindingRow / FindingCard
// (previously every severity rendered the same gray).
import { SEVERITY_STYLE, type Severity } from '../../lib/severity';

const FALLBACK = 'bg-surface text-ink-mute ring-border';

export default function SeverityChip({ severity }: { severity: string }) {
  const style = SEVERITY_STYLE[severity as Severity];
  const pill = style ? style.pill : FALLBACK;
  return (
    <span
      className={`inline-flex items-center whitespace-nowrap rounded px-1.5 py-0.5 text-meta font-medium ring-1 ${pill}`}
    >
      {severity}
    </span>
  );
}
