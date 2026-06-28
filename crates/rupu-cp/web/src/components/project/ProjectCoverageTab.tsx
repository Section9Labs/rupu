// Project Coverage tab body — coverage-target list scoped to one project.
// Ported from pages/ProjectCoverage.tsx; self-fetches on the `wsId` prop.
// No filter chips. Rows render via the shared SortableTable.

import { useEffect, useState } from 'react';
import { ShieldCheck } from 'lucide-react';
import { api, type ProjectCoverageRow } from '../../lib/api';
import SortableTable, { type Column } from '../lists/SortableTable';

const COVERAGE_COLUMNS: Column<ProjectCoverageRow>[] = [
  {
    key: 'target',
    header: 'Target',
    sortable: true,
    sortValue: (r) => r.target_id,
    render: (r) => <span className="text-sm font-medium text-ink truncate font-mono">{r.target_id}</span>,
  },
  {
    key: 'assertions',
    header: 'Assertions',
    align: 'right',
    width: 'w-28',
    sortable: true,
    sortValue: (r) => r.assertion_lines,
    render: (r) => <span className="text-ink-dim">{r.assertion_lines}</span>,
  },
  {
    key: 'catalog',
    header: 'Catalog',
    width: 'w-24',
    render: (r) =>
      r.has_catalog ? (
        <span className="text-note text-ink-dim">yes</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'findings',
    header: 'Findings',
    align: 'right',
    width: 'w-24',
    sortable: true,
    sortValue: (r) => r.findings,
    render: (r) =>
      r.findings > 0 ? (
        <span className="text-err font-medium">{r.findings}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
];

export default function ProjectCoverageTab({ wsId }: { wsId: string }) {
  const [rows, setRows] = useState<ProjectCoverageRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    setRows(null);
    setError(null);
    api
      .getProjectCoverage(wsId)
      .then((data) => {
        if (cancelled) return;
        setRows(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load coverage');
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  return (
    <div className="space-y-4">
      {error && (
        <div className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}

      {rows === null && !error && <div className="text-sm text-ink-dim">Loading coverage…</div>}

      {rows !== null && rows.length === 0 && (
        <div className="rounded-xl border border-dashed border-border bg-panel/50 py-12 flex flex-col items-center justify-center text-center">
          <div className="w-10 h-10 rounded-full bg-surface flex items-center justify-center mb-2">
            <ShieldCheck size={18} className="text-ink-mute" />
          </div>
          <p className="text-sm text-ink-mute">No coverage targets for this project yet</p>
        </div>
      )}

      {rows !== null && rows.length > 0 && (
        <SortableTable<ProjectCoverageRow>
          columns={COVERAGE_COLUMNS}
          rows={rows}
          rowKey={(r) => r.target_id}
          rowHref={(r) =>
            `/coverage/${encodeURIComponent(r.target_id)}${
              wsId ? `?ws_id=${encodeURIComponent(wsId)}` : ''
            }`
          }
        />
      )}
    </div>
  );
}
