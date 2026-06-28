// Shared severity pill, used across the coverage concern tabs. Colour-coded off
// the canonical SEVERITY_STYLE ramp so it matches FindingRow / FindingCard
// (previously every severity rendered the same gray).
import { SEVERITY_STYLE, type Severity } from '../../lib/severity';

const FALLBACK = 'bg-slate-100 text-ink-mute ring-slate-200';

export default function SeverityChip({ severity }: { severity: string }) {
  const style = SEVERITY_STYLE[severity as Severity];
  const pill = style ? style.pill : FALLBACK;
  return (
    <span
      className={`inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 ${pill}`}
    >
      {severity}
    </span>
  );
}
