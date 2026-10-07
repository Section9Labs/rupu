import type { ReactNode } from 'react';
import { cn } from '../../lib/cn';

const SUB_TONE = {
  dim: 'text-ink-dim',
  warn: 'text-warn',
  err: 'text-err font-medium',
  ok: 'text-ok',
} as const;

/** A rollup tile (the ProjectDetail tile markup): small uppercase label, big
 *  value, optional sub-line in a tone. `data-testid="tile-<id>"`. */
export function StatTile({
  id,
  label,
  value,
  sub,
  subTone = 'dim',
}: {
  id: string;
  label: string;
  value: ReactNode;
  sub?: ReactNode;
  subTone?: keyof typeof SUB_TONE;
}) {
  return (
    <div data-testid={`tile-${id}`} className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 min-w-0">
      <p className="text-[9px] font-semibold uppercase tracking-widest text-ink-mute mb-1">{label}</p>
      <div className="text-2xl font-bold text-ink tabular-nums leading-none">{value}</div>
      {sub && <p className={cn('mt-1 text-note', SUB_TONE[subTone])}>{sub}</p>}
    </div>
  );
}

export default StatTile;
