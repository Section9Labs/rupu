// StepTranscriptBrowser — file-browser layout for a `for_each` fan-out step.
//
// Replaces the right-side FanoutDrill slide-over: the fan-out's units list on
// the LEFT (state-filter pills + capped, scrollable unit rows), the selected
// unit's transcript on the RIGHT. Reuses FanoutDrill's filter pills, STATE_STYLE
// glyph map, ROW_CAP "+N more" cap, and unit-row rendering.

import { useEffect, useMemo, useState } from 'react';
import type { StepState, UnitView } from '../../lib/runGraphModel';
import { STATE_STYLE, glyphBg } from '../graph/stepStyle';
import TranscriptPanel from '../TranscriptPanel';

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

export default function StepTranscriptBrowser({
  stepId,
  units,
}: {
  stepId: string;
  units: UnitView[];
}) {
  const [filter, setFilter] = useState<Filter>('all');
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);

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

  // Auto-select the first visible unit, and re-select it whenever the current
  // selection falls outside the filtered set (e.g. after a filter change).
  useEffect(() => {
    const stillVisible =
      selectedIndex != null && filtered.some((u) => u.index === selectedIndex);
    if (!stillVisible) {
      setSelectedIndex(filtered.length > 0 ? filtered[0].index : null);
    }
  }, [filtered, selectedIndex]);

  const selectedUnit =
    selectedIndex != null ? units.find((u) => u.index === selectedIndex) ?? null : null;

  return (
    <div className="flex min-h-0 w-full flex-1">
      {/* LEFT — unit list */}
      <div className="flex w-[34%] min-w-0 flex-col border-r border-border">
        <div className="border-b border-border px-4 py-3">
          <div className="truncate font-mono text-[11px] text-ink" title={stepId}>
            {stepId}
          </div>
          <div className="text-[10px] text-ink-dim">
            {units.length} unit{units.length === 1 ? '' : 's'}
          </div>
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
                const selected = u.index === selectedIndex;
                return (
                  <li key={u.index}>
                    <button
                      type="button"
                      className={[
                        'flex w-full items-center gap-2 py-1.5 text-left text-[11px] cursor-pointer',
                        selected ? 'bg-brand-50' : 'hover:bg-slate-50',
                      ].join(' ')}
                      aria-pressed={selected}
                      onClick={() => setSelectedIndex(u.index)}
                    >
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
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
          {overflow > 0 && (
            <div className="py-2 text-center text-[11px] text-ink-mute">+{overflow} more</div>
          )}
        </div>
      </div>

      {/* RIGHT — selected unit transcript */}
      <div className="min-w-0 flex-1 overflow-y-auto">
        {selectedUnit == null ? (
          <div className="p-6 text-center text-sm text-ink-mute">No unit selected.</div>
        ) : selectedUnit.transcriptPath == null ? (
          <div className="p-6 text-center text-sm text-ink-mute">
            No transcript for this unit.
          </div>
        ) : (
          <TranscriptPanel
            key={selectedUnit.transcriptPath}
            path={selectedUnit.transcriptPath}
            live={selectedUnit.state === 'running'}
          />
        )}
      </div>
    </div>
  );
}
