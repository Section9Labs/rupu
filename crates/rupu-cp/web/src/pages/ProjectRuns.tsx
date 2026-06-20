// Project-scoped run list — minimal placeholder so "see all" links resolve.
// Task 9 will deepen this into a full scoped list with filtering.

import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, type RunListRow } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { StatusPill } from '../components/StatusPill';
import UsageChip from '../components/UsageChip';
import { relativeTime } from '../lib/time';

export default function ProjectRuns() {
  const { wsId } = useParams<{ wsId: string }>();
  const [runs, setRuns] = useState<RunListRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    api
      .getProjectRuns(wsId)
      .then((data) => {
        if (cancelled) return;
        setRuns(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load runs');
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  return (
    <div className="p-8 max-w-5xl">
      <header className="mb-6">
        <Link
          to={`/projects/${wsId ? encodeURIComponent(wsId) : ''}`}
          className="inline-flex items-center gap-1 text-xs text-ink-dim hover:text-ink mb-2"
        >
          <ArrowLeft size={12} />
          Back to project
        </Link>
        <h1 className="text-2xl font-semibold text-ink">Project Runs</h1>
        <p className="mt-1 text-sm text-ink-dim text-ink-mute font-mono">{wsId}</p>
      </header>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {runs === null && !error && (
        <div className="text-sm text-ink-dim">Loading runs…</div>
      )}

      {runs !== null && runs.length === 0 && (
        <div className="rounded-xl border border-dashed border-border bg-panel/50 py-12 flex items-center justify-center">
          <p className="text-sm text-ink-mute">No runs for this project yet</p>
        </div>
      )}

      {runs !== null && runs.length > 0 && (
        <ListCard>
          {runs.map((r) => (
            <Link
              key={r.id}
              to={`/runs/${encodeURIComponent(r.id)}`}
              className="flex items-center gap-4 px-4 py-3 hover:bg-slate-50 transition-colors"
            >
              <div className="min-w-0 flex-1">
                <p className="text-sm font-medium text-ink truncate">{r.workflow_name}</p>
                <p className="text-[11px] text-ink-dim mt-0.5">
                  {relativeTime(r.started_at)}
                  <UsageChip usage={r.usage} className="ml-2" />
                </p>
              </div>
              <StatusPill status={r.status} />
            </Link>
          ))}
        </ListCard>
      )}
    </div>
  );
}
