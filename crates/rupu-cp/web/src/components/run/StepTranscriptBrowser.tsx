// StepTranscriptBrowser — file-browser layout for a `for_each` fan-out step.
//
// The fan-out's units list on the LEFT (the query line + capped, scrollable
// unit rows), the selected unit's transcript on the RIGHT: the STATE_STYLE
// glyph map, the ROW_CAP "+N more" cap (the query narrows past it), and
// unit-row rendering.

import { useEffect, useState } from 'react';
import type { UnitView } from '../../lib/runGraphModel';
import { UNIT_FIELDS } from '../../lib/query/lists/units';
import { stateStyle, glyphBg } from '../graph/stepStyle';
import { useThemeColors } from '../../lib/useThemeColors';
import TranscriptPanel from '../TranscriptPanel';
import { AgentName } from '../codename/AgentName';
import { ListQueryBar, NoMatches, useFilteredRows } from '../query/ListQueryBar';

const ROW_CAP = 300;

export default function StepTranscriptBrowser({
  stepId,
  units,
  initialUnitIndex,
  runId,
  host,
  agent,
}: {
  stepId: string;
  units: UnitView[];
  /** The agent every unit runs (for_each). Omit for panel/parallel steps
   *  whose units each run their own agent. */
  agent?: string;
  /**
   * Seed the initially-selected unit (e.g. the unit-square the user clicked in
   * the graph). Falls back to the first visible unit if this index isn't in the
   * filtered set. When undefined, keeps the default auto-select-first behavior.
   */
  initialUnitIndex?: number;
  /** Run id, threaded straight through to `TranscriptPanel` for the
   *  source-preview affordance — see `TranscriptPanel`'s `runId` doc. */
  runId?: string;
  /** Remote host id, threaded alongside `runId`. */
  host?: string;
}) {
  const colors = useThemeColors();
  const [q, setQ] = useState('');
  const [selectedIndex, setSelectedIndex] = useState<number | null>(
    initialUnitIndex ?? null,
  );

  const { rows: filtered, error: queryError, facets } = useFilteredRows(units, q, UNIT_FIELDS);
  const shown = filtered.slice(0, ROW_CAP);
  const overflow = filtered.length - shown.length;

  // Auto-select the first visible unit, and re-select it whenever the current
  // selection falls outside the filtered set (e.g. after a query change).
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
          <div className="truncate font-mono text-note text-ink" title={stepId}>
            {stepId}
          </div>
          <div className="text-meta text-ink-dim">
            {units.length} unit{units.length === 1 ? '' : 's'}
          </div>
        </div>

        <div className="border-b border-border px-4 py-2">
          <ListQueryBar
            className="mb-0"
            value={q}
            onChange={setQ}
            fields={UNIT_FIELDS}
            facets={facets}
            total={units.length}
            shown={filtered.length}
            error={queryError}
            label="Filter units"
            placeholder="Filter units… e.g. state:failed has:warning"
          />
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-2">
          {shown.length === 0 ? (
            <NoMatches noun="units" />
          ) : (
            <ul className="divide-y divide-border">
              {shown.map((u) => {
                const st = stateStyle(colors, u.state);
                const selected = u.index === selectedIndex;
                return (
                  <li key={u.index}>
                    <button
                      type="button"
                      className={[
                        'flex w-full items-center gap-2 py-1.5 text-left text-note cursor-pointer',
                        selected ? 'bg-brand-50' : 'hover:bg-surface-hover',
                      ].join(' ')}
                      aria-pressed={selected}
                      onClick={() => setSelectedIndex(u.index)}
                    >
                      <span
                        className="inline-flex h-[13px] w-[13px] shrink-0 items-center justify-center rounded-[3px] text-[8px] font-bold leading-none text-white"
                        style={{ background: glyphBg(colors, u.state) }}
                        aria-hidden
                      >
                        {st.glyph}
                      </span>
                      {/* Identity first (who), item key second (context) — spec §9. */}
                      {u.codename ? (
                        <>
                          <span data-testid="unit-identity" className="min-w-0 shrink truncate text-ink">
                            <AgentName
                              codename={u.codename}
                              derived={u.codenameDerived}
                              agent={u.agent ?? agent}
                              provider={u.provider}
                              model={u.model}
                            />
                          </span>
                          <span
                            data-testid="unit-key"
                            className="min-w-0 truncate font-mono text-ink-dim"
                            title={u.key}
                          >
                            {u.key}
                          </span>
                        </>
                      ) : (
                        <span className="truncate font-mono text-ink" title={u.key}>
                          {u.key}
                        </span>
                      )}
                      <span
                        className="ml-auto shrink-0 text-meta font-medium uppercase tracking-wide"
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
            <div className="py-2 text-center text-note text-ink-mute">+{overflow} more</div>
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
            runId={runId}
            host={host}
          />
        )}
      </div>
    </div>
  );
}
