// Coverage list — shows how far each target's security assessment has
// progressed. Rows sorted by findings desc then assertion_lines desc so the
// most-interesting targets float to the top. Each row links to the per-target
// detail view (/coverage/:target).

import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Inbox, ShieldCheck, ShieldOff } from 'lucide-react';
import { api, type CoverageSummary } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader } from '../components/lists/SectionHeader';
import { cn } from '../lib/cn';

export default function Coverage() {
  const [targets, setTargets] = useState<CoverageSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getCoverage()
      .then((data) => {
        if (cancelled) return;
        setTargets(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load coverage');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const all = targets ?? [];

  // Max assertion_lines across all targets — used to scale comparative bars
  // consistently across every project group.
  const maxLines = all.reduce((m, t) => Math.max(m, t.assertion_lines), 1);

  // Group targets by their owning project. Within a group, sort by findings ↓
  // then activity ↓. Groups are ordered by project name asc.
  const groups = groupByProject(all);

  return (
    <div className="p-8 max-w-5xl">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Coverage</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Assessment activity across all registered projects — how many concern assertions have
          been recorded per target, and how many findings were raised.
        </p>
      </header>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {targets === null ? (
        <div className="text-sm text-ink-dim">Loading coverage…</div>
      ) : all.length === 0 ? (
        <EmptyState />
      ) : (
        <div className="space-y-8">
          {groups.map((g) => (
            <section key={g.project}>
              <SectionHeader
                tone="muted"
                label={g.project}
                count={g.rows.length}
                hint="sorted by findings ↓ then activity ↓"
              />
              <ListCard>
                {g.rows.map((t) => (
                  <TargetRow key={`${t.ws_id}/${t.target_id}`} target={t} maxLines={maxLines} />
                ))}
              </ListCard>
            </section>
          ))}
        </div>
      )}
    </div>
  );
}

/** A project group: a project name + its sorted target rows. */
interface ProjectGroup {
  project: string;
  rows: CoverageSummary[];
}

/**
 * Bucket targets by `project`, sort each bucket by findings ↓ then activity ↓,
 * and return the groups ordered by project name asc.
 */
function groupByProject(targets: CoverageSummary[]): ProjectGroup[] {
  const byProject = new Map<string, CoverageSummary[]>();
  for (const t of targets) {
    const key = t.project || t.ws_id;
    const bucket = byProject.get(key);
    if (bucket) bucket.push(t);
    else byProject.set(key, [t]);
  }
  return Array.from(byProject.entries())
    .map(([project, rows]) => ({
      project,
      rows: rows.slice().sort((a, b) => {
        if (b.findings !== a.findings) return b.findings - a.findings;
        return b.assertion_lines - a.assertion_lines;
      }),
    }))
    .sort((a, b) => a.project.localeCompare(b.project));
}

function TargetRow({
  target,
  maxLines,
}: {
  target: CoverageSummary;
  maxLines: number;
}) {
  const barPct = maxLines > 0 ? (target.assertion_lines / maxLines) * 100 : 0;
  const hasFindings = target.findings > 0;

  return (
    <Link
      to={`/coverage/${encodeURIComponent(target.target_id)}?ws_id=${encodeURIComponent(target.ws_id)}`}
      className="flex items-center gap-4 px-4 py-3 hover:bg-slate-50 transition-colors"
    >
      {/* Target name */}
      <div className="min-w-0 flex-1">
        <span className="text-sm font-medium text-ink truncate block">{target.target_id}</span>

        {/* Comparative activity bar — labeled explicitly as "N assertions" */}
        <div className="mt-1.5 flex items-center gap-2">
          <div className="flex-1 max-w-[180px] h-1.5 bg-slate-100 rounded-full overflow-hidden">
            <div
              className="h-full bg-brand-500 rounded-full transition-all"
              style={{ width: `${barPct.toFixed(1)}%` }}
            />
          </div>
          <span className="text-[11px] text-ink-mute tabular-nums whitespace-nowrap">
            {target.assertion_lines} assertion{target.assertion_lines !== 1 ? 's' : ''}
          </span>
        </div>
      </div>

      {/* Catalog badge */}
      <CatalogBadge present={target.has_catalog} />

      {/* Findings count badge */}
      <FindingsBadge count={target.findings} hasFindings={hasFindings} />
    </Link>
  );
}

function CatalogBadge({ present }: { present: boolean }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-medium ring-1',
        present
          ? 'bg-green-50 text-green-700 ring-green-200'
          : 'bg-slate-100 text-ink-mute ring-slate-200',
      )}
    >
      {present ? <ShieldCheck size={11} /> : <ShieldOff size={11} />}
      {present ? 'catalog' : 'no catalog'}
    </span>
  );
}

function FindingsBadge({ count, hasFindings }: { count: number; hasFindings: boolean }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-2 py-0.5 text-[11px] font-medium tabular-nums ring-1',
        hasFindings
          ? 'bg-red-50 text-sev-high ring-red-200'
          : 'bg-slate-100 text-ink-mute ring-slate-200',
      )}
    >
      {count} finding{count !== 1 ? 's' : ''}
    </span>
  );
}

function EmptyState() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No coverage data</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Run an assessment workflow to start recording coverage assertions and findings.
      </p>
    </div>
  );
}
