// Gap tab — concerns whose in-scope files weren't all assessed. Derived from
// the same audit report the Audit tab uses.
import { useEffect, useMemo, useState } from 'react';
import { api, type AuditReport } from '../../lib/api';
import { gapRows } from '../../lib/coverageGap';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';

export default function CoverageGapTab({ target, wsId }: { target: string; wsId?: string }) {
  const [report, setReport] = useState<AuditReport | null>(null);
  const [error, setError] = useState<string | null>(null);

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

  const rows = useMemo(() => (report ? gapRows(report) : []), [report]);

  if (error) return <p className="mt-4 text-sm text-red-700">{error}</p>;
  if (!report) return <p className="mt-4 text-sm text-ink-dim">Loading…</p>;
  if (rows.length === 0)
    return <p className="mt-4 text-sm text-ink-dim">No gaps — every in-scope file assessed.</p>;

  return (
    <section className="mt-6">
      <SectionHeader
        tone="bad"
        label="Coverage gaps"
        count={rows.length}
        hint="concerns with unassessed files"
      />
      <ListCard>
        {rows.map((r) => (
          <div key={r.concern_id} className="px-4 py-3">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-sm font-medium text-ink">{r.name}</span>
              <span className="text-[11px] font-mono text-ink-mute">{r.concern_id}</span>
              <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200">
                {r.severity}
              </span>
              <span className="text-[10px] text-amber-700 font-medium tabular-nums">
                {r.gap_files.length} files
              </span>
            </div>
            <ul className="mt-1 space-y-0.5">
              {r.gap_files.map((f) => (
                <li key={f} className="text-[11px] font-mono text-ink-mute break-all">
                  {f}
                </li>
              ))}
            </ul>
          </div>
        ))}
      </ListCard>
    </section>
  );
}
