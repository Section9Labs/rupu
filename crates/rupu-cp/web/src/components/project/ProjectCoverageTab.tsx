// Project Coverage tab body — coverage-target list scoped to one project.
// Ported from pages/ProjectCoverage.tsx; self-fetches on the `wsId` prop.
// No filter chips.

import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { ShieldCheck } from 'lucide-react';
import { api, type ProjectCoverageRow } from '../../lib/api';
import { ListCard } from '../lists/ListCard';

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
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {rows === null && !error && <div className="text-sm text-ink-dim">Loading coverage…</div>}

      {rows !== null && rows.length === 0 && (
        <div className="rounded-xl border border-dashed border-border bg-panel/50 py-12 flex flex-col items-center justify-center text-center">
          <div className="w-10 h-10 rounded-full bg-slate-100 flex items-center justify-center mb-2">
            <ShieldCheck size={18} className="text-ink-mute" />
          </div>
          <p className="text-sm text-ink-mute">No coverage targets for this project yet</p>
        </div>
      )}

      {rows !== null && rows.length > 0 && (
        <ListCard>
          {rows.map((r) => (
            <Link
              key={r.target_id}
              to={`/coverage/${encodeURIComponent(r.target_id)}${
                wsId ? `?ws_id=${encodeURIComponent(wsId)}` : ''
              }`}
              className="flex items-center gap-4 px-4 py-3 hover:bg-panel/60 transition-colors"
            >
              <div className="min-w-0 flex-1">
                <p className="text-sm font-medium text-ink truncate font-mono">{r.target_id}</p>
                <p className="text-note text-ink-dim mt-0.5">
                  {r.assertion_lines} assertion{r.assertion_lines !== 1 ? 's' : ''}
                  {r.has_catalog ? ' · has catalog' : ''}
                </p>
              </div>
              {r.findings > 0 && (
                <span className="shrink-0 inline-flex items-center rounded px-2 py-0.5 text-note font-medium ring-1 bg-red-50 text-red-700 ring-red-200">
                  {r.findings} finding{r.findings !== 1 ? 's' : ''}
                </span>
              )}
            </Link>
          ))}
        </ListCard>
      )}
    </div>
  );
}
