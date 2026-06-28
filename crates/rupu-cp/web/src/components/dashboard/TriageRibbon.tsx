// Triage ribbon — a thin full-width row of clickable signal chips that surface
// what needs attention right now: in-flight runs, approvals waiting, recent
// failures, and open findings. Each chip links to the relevant list page.
//
// Static Tailwind only (no dynamic class strings) so the JIT keeps every class.

import { Link } from 'react-router-dom';
import { AlertTriangle, Loader2, Pause, ShieldAlert } from 'lucide-react';
import { cn } from '../../lib/cn';

interface ChipDef {
  label: string;
  value: number;
  to: string;
  icon: React.ElementType;
  /** Tone when value > 0. `muted` always renders slate. */
  tone: 'blue' | 'amber' | 'red' | 'violet';
  /** Pulse the icon when value > 0 (used by the amber approvals chip). */
  pulse?: boolean;
}

const TONE_ACTIVE: Record<ChipDef['tone'], string> = {
  blue: 'border-info/30 bg-info-bg text-info hover:bg-info-bg',
  amber: 'border-warn/30 bg-warn-bg text-warn hover:bg-warn-bg',
  red: 'border-err/30 bg-err-bg text-err hover:bg-err-bg',
  violet: 'border-violet-200 bg-violet-50 text-violet-700 hover:bg-violet-100',
};

const ZERO_CLS = 'border-border bg-panel text-ink-mute hover:bg-surface-hover';

function Chip({ def }: { def: ChipDef }) {
  const active = def.value > 0;
  const Icon = def.icon;
  return (
    <Link
      to={def.to}
      className={cn(
        'flex items-center gap-2.5 rounded-xl border px-4 py-3 transition-colors',
        active ? TONE_ACTIVE[def.tone] : ZERO_CLS,
      )}
    >
      <Icon size={16} className={cn('shrink-0', active && def.pulse && 'animate-pulse')} />
      <span className="text-xl font-semibold tabular-nums leading-none">{def.value}</span>
      <span className="text-xs font-medium leading-tight">{def.label}</span>
    </Link>
  );
}

export default function TriageRibbon({
  running,
  awaiting,
  failed,
  findings,
}: {
  running: number;
  awaiting: number;
  failed: number;
  /** Open findings total, or `null` when the count is unavailable (chip hidden). */
  findings: number | null;
}) {
  const chips: ChipDef[] = [
    { label: 'Running', value: running, to: '/runs/workflows', icon: Loader2, tone: 'blue' },
    { label: 'Awaiting approval', value: awaiting, to: '/runs/workflows', icon: Pause, tone: 'amber', pulse: true },
    { label: 'Failed', value: failed, to: '/runs/workflows', icon: AlertTriangle, tone: 'red' },
  ];
  if (findings !== null) {
    chips.push({ label: 'Open findings', value: findings, to: '/findings', icon: ShieldAlert, tone: 'violet' });
  }

  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
      {chips.map((c) => (
        <Chip key={c.label} def={c} />
      ))}
    </div>
  );
}
