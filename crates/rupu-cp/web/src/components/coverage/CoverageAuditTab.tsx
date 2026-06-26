// Audit tab — per-concern coverage matrix + cross-model + serendipitous.
import { useEffect, useMemo, useState } from 'react';
import {
  api,
  normFindingSeverity,
  sevRank,
  type AuditReport,
  type ConcernCoverage,
} from '../../lib/api';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';

export default function CoverageAuditTab({ target, wsId }: { target: string; wsId?: string }) {
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
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load audit');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  const concerns = useMemo(
    () =>
      // SEV_ORDER puts critical at index 0, so ascending rank = critical→info.
      [...(report?.concerns ?? [])].sort(
        (a, b) =>
          sevRank(normFindingSeverity(a.severity)) - sevRank(normFindingSeverity(b.severity)),
      ),
    [report],
  );

  if (error) return <p className="mt-4 text-sm text-red-700">{error}</p>;
  if (!report) return <p className="mt-4 text-sm text-ink-dim">Loading…</p>;

  return (
    <div className="mt-6 space-y-6">
      <div className="flex gap-4 text-sm">
        <Stat
          label="Concerns complete"
          value={`${report.complete_concerns}/${report.total_concerns}`}
        />
        <Stat label="Gap files" value={report.total_gap_files} />
      </div>

      <section>
        <SectionHeader tone="progress" label="Per-concern coverage" count={concerns.length} />
        {concerns.length === 0 ? (
          <p className="text-sm text-ink-dim pl-1 mt-1">No catalog → no audit matrix.</p>
        ) : (
          <ListCard>
            {concerns.map((c) => (
              <ConcernRow key={c.concern_id} c={c} />
            ))}
          </ListCard>
        )}
      </section>

      {report.cross_model.length > 0 && (
        <section>
          <SectionHeader
            tone="muted"
            label="Cross-model"
            count={report.cross_model.length}
            hint="multi-model cells"
          />
          <ListCard>
            {report.cross_model.map((x, i) => (
              <div key={`${x.concern_id}:${x.file_path}:${i}`} className="px-4 py-2 text-xs">
                <span className="font-mono text-ink">{x.concern_id}</span>
                <span className="text-ink-mute"> · {x.file_path}</span>
                {x.disagreement && (
                  <span className="ml-2 text-amber-700 font-medium">disagreement</span>
                )}
              </div>
            ))}
          </ListCard>
        </section>
      )}

      {report.serendipitous.length > 0 && (
        <section>
          <SectionHeader
            tone="bad"
            label="Serendipitous"
            count={report.serendipitous.length}
            hint="unscoped findings"
          />
          <ListCard>
            {report.serendipitous.map((s) => (
              <div key={s.theme} className="px-4 py-2 text-xs">
                <span className="text-ink">{s.theme}</span>
                <span className="ml-2 text-ink-mute tabular-nums">{s.count}</span>
              </div>
            ))}
          </ListCard>
        </section>
      )}
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="rounded-lg border border-border bg-panel px-3 py-2">
      <div className="text-[11px] text-ink-mute">{label}</div>
      <div className="text-sm font-semibold text-ink tabular-nums">{value}</div>
    </div>
  );
}

function ConcernRow({ c }: { c: ConcernCoverage }) {
  const assessed = c.asserted_files.length;
  const inScope = c.in_scope_files.length;
  const pct = inScope === 0 ? 0 : Math.round((assessed / inScope) * 100);
  return (
    <div className="px-4 py-3">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-sm font-medium text-ink">{c.name}</span>
        <span className="text-[11px] font-mono text-ink-mute">{c.concern_id}</span>
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200">
          {c.severity}
        </span>
        {c.gap_files.length > 0 && (
          <span className="text-[10px] text-amber-700 font-medium">{c.gap_files.length} gap</span>
        )}
      </div>
      <div className="mt-1.5 flex items-center gap-2">
        <div className="h-1.5 flex-1 rounded bg-slate-100 overflow-hidden">
          <div className="h-full bg-brand-500" style={{ width: `${pct}%` }} />
        </div>
        <span className="text-[11px] text-ink-mute tabular-nums w-24 text-right">
          {assessed}/{inScope} files
        </span>
      </div>
      <div className="mt-1 text-[11px] text-ink-mute tabular-nums">
        clean {c.clean} · finding {c.findings} · examined {c.examined} · n/a {c.not_applicable}
      </div>
    </div>
  );
}
