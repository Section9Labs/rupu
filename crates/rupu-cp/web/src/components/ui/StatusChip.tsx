// StatusChip — the ONE pill of the common visual language (spec §"Chips"):
// a single pill shape for run/step status, finding severity and config origin.
// Each tone is a pale tint of its own hue with same-hue readable text; a dot
// appears only for a live status (pass `dot`). This is the shape the spec wants
// every screen to converge on — `StatusPill` / `Badge` / `TriggerChip` migrate
// onto it in the app-wide rollout (follow-up); the generic `ui/Chip` stays the
// low-level ring-pill atom.
//
// Token discipline: status tones reuse `STATUS[*].pillClass` / `dotClass` and
// severity tones reuse `SEVERITY_STYLE[*].pill` / `bar`, so the chip can never
// drift from the canonical palettes. Origin tones match the app's shipped
// provenance convention (global = info/blue, project = ok/green — see
// `ScopeChip` / Settings provenance) rather than inventing a third origin hue.
// Only static Tailwind class strings; no `bg-${x}` expressions.

import { cn } from '../../lib/cn';
import { STATUS, normalizeStatusKey } from '../../lib/status';
import { SEVERITY_STYLE, type Severity } from '../../lib/severity';

/** Every tone the one chip can take. */
export type ChipTone =
  // run/step status (dot-eligible)
  | 'running'
  | 'completed'
  | 'done'
  | 'failed'
  | 'awaiting_approval'
  | 'paused'
  | 'pending'
  | 'cancelled'
  | 'rejected'
  | 'skipped'
  // finding severity
  | 'critical'
  | 'high'
  | 'medium'
  | 'low'
  | 'info'
  // config origin
  | 'origin-global'
  | 'origin-project'
  // generic
  | 'neutral';

interface ToneStyle {
  /** bg + text + ring combo. */
  cls: string;
  /** dot background class (used only when `dot`). */
  dot: string;
}

const SEVERITY_TONES: ReadonlySet<ChipTone> = new Set([
  'critical',
  'high',
  'medium',
  'low',
  'info',
]);

const STATIC_TONES: Partial<Record<ChipTone, ToneStyle>> = {
  'origin-global': { cls: 'bg-info-bg text-info ring-info/30', dot: 'bg-info' },
  'origin-project': { cls: 'bg-ok-bg text-ok ring-ok/30', dot: 'bg-ok' },
  neutral: { cls: 'bg-surface text-ink-dim ring-border', dot: 'bg-status-pending' },
};

/** Resolve a tone to its bg/text/ring + dot classes. */
function toneStyle(tone: ChipTone): ToneStyle {
  const stat = STATIC_TONES[tone];
  if (stat) return stat;
  if (SEVERITY_TONES.has(tone)) {
    const s = SEVERITY_STYLE[tone as Severity];
    return { cls: s.pill, dot: s.bar };
  }
  // The rest are status keys (done ↔ completed normalized).
  const key = normalizeStatusKey(tone);
  const d = STATUS[key];
  return { cls: d.pillClass, dot: d.dotClass };
}

export interface StatusChipProps {
  tone: ChipTone;
  /** A dot before the label — use ONLY for a live/attention status, never for
   *  severity or origin (the spec reserves the dot for status). */
  dot?: boolean;
  children: React.ReactNode;
  className?: string;
  title?: string;
}

export function StatusChip({ tone, dot = false, children, className, title }: StatusChipProps) {
  const { cls, dot: dotCls } = toneStyle(tone);
  return (
    <span
      title={title}
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-note font-semibold leading-tight ring-1',
        cls,
        className,
      )}
    >
      {dot && <span className={cn('h-1.5 w-1.5 shrink-0 rounded-full', dotCls)} aria-hidden />}
      {children}
    </span>
  );
}

export default StatusChip;
