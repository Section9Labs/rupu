// FanoutDrill — the drill-in panel for a large `for_each` step. A state-
// filterable, scrollable list of units; each row is glyph · key · state ·
// transcript path. Faithful to the fanout-loop mockup's drill-in list.
//
// No transcript route exists yet, so `transcriptPath` is rendered as plain
// monospace text (not fabricated into a link). Rows are capped with an explicit
// "+N more" line — never silently truncated.

import { useMemo, useState } from 'react';
import { X } from 'lucide-react';
import type { StepState, UnitView } from '../lib/runGraphModel';
import { STATE_STYLE, glyphBg } from './graph/stepStyle';

const ROW_CAP = 300;

type Filter = 'all' | 'running' | 'done' | 'failed';

const FILTERS: { key: Filter; label: string }[] = [
  { key: 'all', label: 'all' },
  { key: 'running', label: 'running' },
  { key: 'done', label: 'done' },
  { key: 'failed', label: 'failed' },
];

function matches(filter: Filter, state: StepState): boolean {
  if (filter === 'all') return true;
  return state === filter;
}

export default function FanoutDrill({
  stepId,
  units,
  onClose,
}: {
  stepId?: string;
  units: UnitView[];
  onClose: () => void;
}) {
  const [filter, setFilter] = useState<Filter>('all');

  const counts = useMemo(() => {
    const c: Record<Filter, number> = { all: units.length, running: 0, done: 0, failed: 0 };
    for (const u of units) {
      if (u.state === 'running') c.running += 1;
      else if (u.state === 'done') c.done += 1;
      else if (u.state === 'failed') c.failed += 1;
    }
    return c;
  }, [units]);

  const filtered = useMemo(() => units.filter((u) => matches(filter, u.state)), [units, filter]);
  const shown = filtered.slice(0, ROW_CAP);
  const overflow = filtered.length - shown.length;

  return (
    <div className="fixed inset-y-0 right-0 z-40 flex w-full max-w-md flex-col border-l border-border bg-panel shadow-xl">
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <div className="min-w-0">
          <div className="text-sm font-semibold text-ink">Fan-out units</div>
          {stepId && <div className="truncate font-mono text-[11px] text-ink-dim">{stepId}</div>}
        </div>
        <button
          type="button"
          onClick={onClose}
          className="rounded p-1 text-ink-dim hover:bg-slate-100 hover:text-ink"
          aria-label="Close"
        >
          <X size={16} />
        </button>
      </div>

      <div className="flex flex-wrap gap-1.5 border-b border-border px-4 py-2">
        {FILTERS.map((f) => (
          <button
            key={f.key}
            type="button"
            onClick={() => setFilter(f.key)}
            className={[
              'rounded-full px-2.5 py-0.5 text-[11px] font-medium tabular-nums transition-colors',
              filter === f.key
                ? 'bg-brand-500 text-white'
                : 'bg-slate-100 text-slate-600 hover:bg-slate-200',
            ].join(' ')}
          >
            {f.label} ({counts[f.key]})
          </button>
        ))}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-2">
        {shown.length === 0 ? (
          <div className="py-8 text-center text-sm text-ink-dim">No units match this filter.</div>
        ) : (
          <ul className="divide-y divide-slate-100">
            {shown.map((u) => {
              const st = STATE_STYLE[u.state];
              return (
                <li key={u.index} className="flex items-center gap-2 py-1.5 text-[11px]">
                  <span
                    className="inline-flex h-[13px] w-[13px] shrink-0 items-center justify-center rounded-[3px] text-[8px] font-bold leading-none text-white"
                    style={{ background: glyphBg(u.state) }}
                    aria-hidden
                  >
                    {st.glyph}
                  </span>
                  <span className="truncate font-mono text-ink" title={u.key}>
                    {u.key}
                  </span>
                  <span
                    className="ml-auto shrink-0 text-[10px] font-medium uppercase tracking-wide"
                    style={{ color: st.color }}
                  >
                    {st.label}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
        {overflow > 0 && (
          <div className="py-2 text-center text-[11px] text-ink-mute">+{overflow} more</div>
        )}
        {shown.some((u) => u.transcriptPath) && (
          <div className="mt-3 border-t border-dashed border-border pt-2">
            <div className="mb-1 text-[10px] font-semibold uppercase tracking-wide text-ink-mute">
              Transcript paths
            </div>
            <ul className="space-y-0.5">
              {shown
                .filter((u) => u.transcriptPath)
                .map((u) => (
                  <li key={u.index} className="truncate font-mono text-[10px] text-ink-dim" title={u.transcriptPath}>
                    {u.transcriptPath}
                  </li>
                ))}
            </ul>
          </div>
        )}
      </div>
    </div>
  );
}
