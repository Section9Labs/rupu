// Gap tab — concerns whose in-scope files weren't all assessed. Collapsed
// accordion rows with severity + file filters and expand/collapse-all.
import { useEffect, useMemo, useState } from 'react';
import { api, type AuditReport } from '../../lib/api';
import { gapRows } from '../../lib/coverageGap';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';
import { Spinner } from '../ui/Spinner';
import CollapsibleRow from './CollapsibleRow';
import SeverityChip from './SeverityChip';
import CappedList from './CappedList';
import ExpandCollapse from './ExpandCollapse';
import { ListQueryBar, NoMatches, useFilteredRows, useListQuery } from '../query/ListQueryBar';
import { GAP_FIELDS, filesMatching } from '../../lib/query/lists/concerns';

export default function CoverageGapTab({ target, wsId }: { target: string; wsId?: string }) {
  const [report, setReport] = useState<AuditReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<Set<string>>(new Set());

  useEffect(() => {
    let cancelled = false;
    setReport(null);
    setError(null);
    api
      .getCoverageAudit(target, wsId)
      .then((d) => {
        if (!cancelled) setReport(d);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load gaps');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  const all = useMemo(() => (report ? gapRows(report) : []), [report]);
  const [q, setQ] = useListQuery();
  const { rows: kept, error: queryError, facets } = useFilteredRows(all, q, GAP_FIELDS);
  // A row lists only the gap files its query's `file:` terms keep.
  const rows = useMemo(() => kept.map((r) => ({ ...r, gap_files: filesMatching(r.gap_files, q, GAP_FIELDS) })), [kept, q]);

  function toggle(id: string) {
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  if (error) return <p className="mt-4 text-sm text-err">{error}</p>;
  if (!report) return <div className="mt-4"><Spinner label="Loading…" /></div>;
  if (all.length === 0)
    return <p className="mt-4 text-sm text-ink-dim">No gaps — every in-scope file assessed.</p>;

  return (
    <section className="mt-6">
      <ListQueryBar
        value={q}
        onChange={setQ}
        fields={GAP_FIELDS}
        facets={facets}
        total={all.length}
        shown={rows.length}
        error={queryError}
        label="Filter gaps"
        placeholder="Filter gaps… e.g. severity>=high file:src/api"
      />
      <SectionHeader
        tone="bad"
        label="Coverage gaps"
        count={rows.length}
        hint="concerns with unassessed files"
        trailing={
          <ExpandCollapse
            onExpandAll={() => setOpen(new Set(rows.map((r) => r.concern_id)))}
            onCollapseAll={() => setOpen(new Set())}
          />
        }
      />
      {rows.length === 0 ? (
        <NoMatches noun="gaps" />
      ) : (
        <ListCard>
          {rows.map((r) => (
            <CollapsibleRow
              key={r.concern_id}
              open={open.has(r.concern_id)}
              onToggle={() => toggle(r.concern_id)}
              header={
                <span className="flex items-center gap-2 flex-wrap">
                  <span className="text-sm font-medium text-ink">{r.name}</span>
                  <span className="text-note font-mono text-ink-mute">{r.concern_id}</span>
                  <SeverityChip severity={r.severity} />
                  <span className="text-meta text-warn font-medium tabular-nums">
                    {r.gap_files.length} files
                  </span>
                </span>
              }
            >
              <CappedList items={r.gap_files} />
            </CollapsibleRow>
          ))}
        </ListCard>
      )}
    </section>
  );
}
