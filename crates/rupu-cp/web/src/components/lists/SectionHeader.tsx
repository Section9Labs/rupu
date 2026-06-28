import { cn } from '../../lib/cn';

// Tone of the colored dot + label. Ported from Okesu so a "good"-tone section
// here is the same green as elsewhere in the design system.
export type SectionTone =
  | 'good' // green   — succeeded / completed
  | 'progress' // brand   — running / active
  | 'warn' // yellow  — degraded / awaiting
  | 'bad' // red     — failed / rejected
  | 'critical' // purple  — critical
  | 'low' // amber   — low / awaiting
  | 'muted'; // slate   — pending / info

const TONE: Record<SectionTone, { dot: string; text: string }> = {
  good: { dot: 'bg-ok', text: 'text-ok' },
  progress: { dot: 'bg-brand-500', text: 'text-brand-700' },
  warn: { dot: 'bg-warn', text: 'text-warn' },
  bad: { dot: 'bg-err', text: 'text-err' },
  critical: { dot: 'bg-purple-500', text: 'text-purple-700' },
  low: { dot: 'bg-warn', text: 'text-warn' },
  muted: { dot: 'bg-ink-mute', text: 'text-ink' },
};

export function SectionHeader({
  tone,
  label,
  count,
  hint,
  leading,
}: {
  tone: SectionTone;
  label: string;
  count: number;
  hint?: string;
  // Optional element rendered before the dot.
  leading?: React.ReactNode;
}) {
  const t = TONE[tone];
  return (
    <header className="flex items-center gap-2 mb-2 pl-1">
      {leading}
      <span className={cn('w-2 h-2 rounded-full', t.dot)} />
      <h2 className={cn('text-sm font-semibold', t.text)}>{label}</h2>
      <span className="text-xs text-ink-mute tabular-nums">{count}</span>
      {hint && <span className="text-note text-ink-mute ml-1">{hint}</span>}
    </header>
  );
}
