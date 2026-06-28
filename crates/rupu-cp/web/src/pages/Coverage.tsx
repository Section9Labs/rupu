// Coverage list — shows how far each target's security assessment has
// progressed. Rows sorted by findings desc then assertion_lines desc so the
// most-interesting targets float to the top. Each row links to the per-target
// detail view (/coverage/:target).

import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Inbox, ShieldCheck, ShieldOff } from 'lucide-react';
import { api, type CoverageSummary } from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { cn } from '../lib/cn';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;

export default function Coverage() {
  const [targets, setTargets] = useState<CoverageSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState(STEP);

  useEffect(() => {
    let cancelled = false;
    api
      .getCoverage()
      .then((data) => {
        if (cancelled) return;
        setTargets(data);
        setVisible(STEP);
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

  // Window the FLAT sequence of target rows (across all groups) to `visible`,
  // so the cap applies to the total number of rendered rows. Each group keeps
  // only the slice of its rows that falls inside the global window; groups that
  // start past the window render empty and are skipped below.
  const { sentinelRef } = useInfiniteScroll({
    hasMore: visible < all.length,
    loadMore: () => setVisible((v) => v + STEP),
  });

  let offset = 0;
  const windowedGroups = groups.map((g) => {
    const start = offset;
    offset += g.rows.length;
    // How many of this group's rows fall inside the global [0, visible) window.
    const take = Math.max(0, Math.min(g.rows.length, visible - start));
    return { project: g.project, total: g.rows.length, rows: g.rows.slice(0, take) };
  });

  return (
    <div className="p-8">
      <header className="mb-6">
        <div className="flex items-center justify-between gap-4">
          <h1 className="text-2xl font-semibold text-ink">Coverage</h1>
          <Link
            to="/coverage/templates"
            className="text-xs font-medium text-brand-700 hover:text-brand-500"
          >
            Templates →
          </Link>
        </div>
        <p className="mt-1 text-sm text-ink-dim">
          Assessment activity across all registered projects — how many concern assertions have
          been recorded per target, and how many findings were raised.
        </p>
      </header>

      {error && (
        <div className="mb-4 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}

      {targets === null ? (
        <div className="text-sm text-ink-dim">Loading coverage…</div>
      ) : all.length === 0 ? (
        <EmptyState />
      ) : (
        <div className="space-y-8">
          {windowedGroups
            .filter((g) => g.rows.length > 0)
            .map((g) => (
              <section key={g.project}>
                <SectionHeader
                  tone="muted"
                  label={g.project}
                  count={g.total}
                  hint="sorted by findings ↓ then activity ↓"
                />
                <CoverageGroupTable rows={g.rows} maxLines={maxLines} />
              </section>
            ))}
          {all.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
              scroll for more
            </div>
          )}
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

/**
 * One project group's targets as a SortableTable. Columns: Target (mono) |
 * Assertions (right, with comparative bar) | Findings (right) | Catalog. Rows
 * link to the per-target detail view. Sortable on Target / Assertions /
 * Findings; default order is the findings ↓ / activity ↓ pre-sort.
 */
function CoverageGroupTable({
  rows,
  maxLines,
}: {
  rows: CoverageSummary[];
  maxLines: number;
}) {
  const columns: Column<CoverageSummary>[] = [
    {
      key: 'target',
      header: 'Target',
      sortable: true,
      sortValue: (t) => t.target_id,
      render: (t) => (
        <span className="font-mono text-sm font-medium text-ink truncate block">{t.target_id}</span>
      ),
    },
    {
      key: 'assertions',
      header: 'Assertions',
      align: 'right',
      width: 'w-56',
      sortable: true,
      sortValue: (t) => t.assertion_lines,
      render: (t) => {
        const barPct = maxLines > 0 ? (t.assertion_lines / maxLines) * 100 : 0;
        return (
          <div className="flex items-center justify-end gap-2">
            <div className="w-24 h-1.5 bg-surface rounded-full overflow-hidden">
              <div
                className="h-full bg-brand-500 rounded-full transition-all"
                style={{ width: `${barPct.toFixed(1)}%` }}
              />
            </div>
            <span className="tabular-nums whitespace-nowrap">{t.assertion_lines}</span>
          </div>
        );
      },
    },
    {
      key: 'findings',
      header: 'Findings',
      align: 'right',
      width: 'w-28',
      sortable: true,
      sortValue: (t) => t.findings,
      render: (t) => <FindingsBadge count={t.findings} hasFindings={t.findings > 0} />,
    },
    {
      key: 'catalog',
      header: 'Catalog',
      align: 'right',
      width: 'w-24',
      render: (t) => <CatalogBadge present={t.has_catalog} />,
    },
  ];

  return (
    <SortableTable<CoverageSummary>
      columns={columns}
      rows={rows}
      rowKey={(t) => `${t.ws_id}/${t.target_id}`}
      rowHref={(t) =>
        `/coverage/${encodeURIComponent(t.target_id)}?ws_id=${encodeURIComponent(t.ws_id)}`
      }
    />
  );
}

function CatalogBadge({ present }: { present: boolean }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded px-2 py-0.5 text-note font-medium ring-1',
        present
          ? 'bg-ok-bg text-ok ring-ok/30'
          : 'bg-surface text-ink-mute ring-border',
      )}
    >
      {present ? <ShieldCheck size={11} /> : <ShieldOff size={11} />}
      {present ? 'yes' : '—'}
    </span>
  );
}

function FindingsBadge({ count, hasFindings }: { count: number; hasFindings: boolean }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-2 py-0.5 text-note font-medium tabular-nums ring-1',
        hasFindings
          ? 'bg-err-bg text-sev-high ring-err/30'
          : 'bg-surface text-ink-mute ring-border',
      )}
    >
      {count} finding{count !== 1 ? 's' : ''}
    </span>
  );
}

function EmptyState() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-surface flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No coverage data</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Run an assessment workflow to start recording coverage assertions and findings.
      </p>
    </div>
  );
}
