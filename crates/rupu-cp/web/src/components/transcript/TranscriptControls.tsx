/**
 * TranscriptControls — the sticky filter chips (with counts) and the
 * Expand-all / Collapse-all buttons. Presentational: `TranscriptPanel` owns
 * the state and passes callbacks.
 */

import { cn } from '../../lib/cn';
import type { FilterKind } from './eventKind';

const CHIPS: { kind: FilterKind; label: string }[] = [
  { kind: 'all', label: 'All' },
  { kind: 'said', label: 'Said' },
  { kind: 'thinking', label: 'Thinking' },
  { kind: 'commands', label: 'Commands' },
  { kind: 'findings', label: 'Findings' },
];

const RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500 focus-visible:ring-offset-1';

export interface TranscriptControlsProps {
  counts: Record<Exclude<FilterKind, 'all'>, number>;
  filter: FilterKind;
  onFilter: (f: FilterKind) => void;
  onExpandAll: () => void;
  onCollapseAll: () => void;
}

export default function TranscriptControls({
  counts,
  filter,
  onFilter,
  onExpandAll,
  onCollapseAll,
}: TranscriptControlsProps) {
  return (
    <div
      className="sticky top-0 z-10 flex flex-wrap items-center gap-2 border-b border-border bg-panel py-2"
      data-testid="transcript-controls"
    >
      <div role="group" aria-label="Filter transcript events" className="flex flex-wrap items-center gap-1.5">
        {CHIPS.map(({ kind, label }) => {
          const active = filter === kind;
          return (
            <button
              key={kind}
              type="button"
              aria-pressed={active}
              onClick={() => onFilter(kind)}
              className={cn(
                'inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-note',
                RING,
                active
                  ? 'border-brand-500 bg-surface-active text-ink'
                  : 'border-border bg-surface text-ink-dim hover:bg-surface-hover',
              )}
            >
              {label}
              {kind !== 'all' && <span className="text-meta text-ink-mute">{counts[kind]}</span>}
            </button>
          );
        })}
      </div>
      <div className="ml-auto flex items-center gap-1.5">
        <button
          type="button"
          onClick={onExpandAll}
          className={cn('rounded border border-border bg-surface px-2 py-0.5 text-note text-ink-dim hover:bg-surface-hover', RING)}
        >
          Expand all
        </button>
        <button
          type="button"
          onClick={onCollapseAll}
          className={cn('rounded border border-border bg-surface px-2 py-0.5 text-note text-ink-dim hover:bg-surface-hover', RING)}
        >
          Collapse all
        </button>
      </div>
    </div>
  );
}
