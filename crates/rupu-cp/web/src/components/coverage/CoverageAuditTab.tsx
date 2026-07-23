// Audit tab — per-concern coverage matrix + cross-model + serendipitous.
// Per-concern rows are collapsed accordions: the header is the coverage summary
// (bar + counts); expanding reveals the asserted / gap file lists.
import { useEffect, useMemo, useState } from 'react';
import {
  api,
  normFindingSeverity,
  sevRank,
  type AuditReport,
  type ConcernCoverage,
} from '../../lib/api';
import { filterConcerns } from '../../lib/coverageFilter';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';
import { Spinner } from '../ui/Spinner';
import CollapsibleRow from './CollapsibleRow';
import SeverityChip from './SeverityChip';
import CappedList from './CappedList';
import ConcernControls from './ConcernControls';

export default function CoverageAuditTab({ target, wsId }: { target: string; wsId?: string }) {
  const [report, setReport] = useState<AuditReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [severity, setSeverity] = useState('all');
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
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load audit');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  const concerns = useMemo(
    () =>
      filterConcerns(
        // SEV_ORDER puts critical at index 0, so ascending rank = critical→info.
        [...(report?.concerns ?? [])].sort(
          (a, b) =>
            sevRank(normFindingSeverity(a.severity)) - sevRank(normFindingSeverity(b.severity)),
        ),
        severity,
      ),
    [report, severity],
  );

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
          <>
            <ConcernControls
              severity={severity}
              onSeverity={setSeverity}
              onExpandAll={() => setOpen(new Set(concerns.map((c) => c.concern_id)))}
              onCollapseAll={() => setOpen(new Set())}
              total={concerns.length}
            />
            <ListCard>
              {concerns.map((c) => (
                <ConcernRow
                  key={c.concern_id}
                  c={c}
                  open={open.has(c.concern_id)}
                  onToggle={() => toggle(c.concern_id)}
                />
              ))}
            </ListCard>
          </>
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
                  <span className="ml-2 text-warn font-medium">disagreement</span>
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
      <div className="text-note text-ink-mute">{label}</div>
      <div className="text-sm font-semibold text-ink tabular-nums">{value}</div>
    </div>
  );
}

function ConcernRow({
  c,
  open,
  onToggle,
}: {
  c: ConcernCoverage;
  open: boolean;
  onToggle: () => void;
}) {
  const assessed = c.asserted_files.length;
  const inScope = c.in_scope_files.length;
  const pct = inScope === 0 ? 0 : Math.round((assessed / inScope) * 100);
  return (
    <CollapsibleRow
      open={open}
      onToggle={onToggle}
      header={
        <span className="block">
          <span className="flex items-center gap-2 flex-wrap">
            <span className="text-sm font-medium text-ink">{c.name}</span>
            <span className="text-note font-mono text-ink-mute">{c.concern_id}</span>
            <SeverityChip severity={c.severity} />
            {c.gap_files.length > 0 && (
              <span className="text-meta text-warn font-medium">
                {c.gap_files.length} gap
              </span>
            )}
          </span>
          <span className="mt-1.5 flex items-center gap-2">
            <span className="h-1.5 flex-1 rounded bg-surface overflow-hidden">
              <span className="block h-full bg-brand-500" style={{ width: `${pct}%` }} />
            </span>
            <span className="text-note text-ink-mute tabular-nums w-24 text-right">
              {assessed}/{inScope} files
            </span>
          </span>
          <span className="mt-1 block text-note text-ink-mute tabular-nums">
            clean {c.clean} · finding {c.findings} · examined {c.examined} · n/a {c.not_applicable}
          </span>
        </span>
      }
    >
      {c.asserted_files.length > 0 && (
        <div className="mb-2">
          <p className="text-note font-medium text-ink-dim mb-0.5">Asserted</p>
          <CappedList items={c.asserted_files} />
        </div>
      )}
      {c.gap_files.length > 0 && (
        <div>
          <p className="text-note font-medium text-warn mb-0.5">Gap</p>
          <CappedList items={c.gap_files} />
        </div>
      )}
      {c.asserted_files.length === 0 && c.gap_files.length === 0 && (
        <p className="text-note text-ink-mute">No in-scope files.</p>
      )}
    </CollapsibleRow>
  );
}
