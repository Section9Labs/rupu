// Project-scoped run list — minimal placeholder so "see all" links resolve.
// Task 9 will deepen this into a full scoped list with filtering.

import { useCallback, useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, type RunListRow } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { StatusPill } from '../components/StatusPill';
import MetricRow from '../components/lists/MetricRow';
import UsageBarChart from '../components/charts/UsageBarChart';
import { durationBetween } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';
import { formatDuration } from '../lib/duration';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

const PAGE = 20;

export default function ProjectRuns() {
  const { wsId } = useParams<{ wsId: string }>();
  const [runs, setRuns] = useState<RunListRow[] | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!wsId) return;
    try {
      const pageData = await api.getProjectRuns(wsId, { limit: PAGE });
      setRuns(pageData);
      setHasMore(pageData.length >= PAGE);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load runs');
    }
  }, [wsId]);

  useEffect(() => {
    void refresh();
    return () => {};
  }, [refresh]);

  const loadMore = async () => {
    if (!wsId) return;
    const current = runs ?? [];
    const next = await api.getProjectRuns(wsId, { offset: current.length, limit: PAGE });
    if (next.length === 0) {
      setHasMore(false);
      return;
    }
    setRuns([...current, ...next]);
    if (next.length < PAGE) setHasMore(false);
  };

  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

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
        <div className="space-y-4">
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
            <UsageBarChart bars={runs.map((r) => ({
              id: r.id, label: r.workflow_name, to: `/runs/${encodeURIComponent(r.id)}`,
              input_tokens: r.usage.input_tokens, output_tokens: r.usage.output_tokens,
              cached_tokens: r.usage.cached_tokens, cost_usd: r.usage.cost_usd,
            }))} />
          </div>
          <ListCard>
            {runs.map((r) => (
              <MetricRow
                key={r.id}
                to={`/runs/${encodeURIComponent(r.id)}`}
                header={<>
                  <span className="text-sm font-medium text-ink truncate">{r.workflow_name}</span>
                  <span className="text-[11px] text-ink-mute font-mono">{shortId(r.id)}</span>
                </>}
                trailing={<StatusPill status={r.status} />}
                metrics={[
                  { label: 'in', value: formatTokens(r.usage.input_tokens) },
                  { label: 'out', value: formatTokens(r.usage.output_tokens) },
                  { label: 'cached', value: r.usage.cached_tokens ? formatTokens(r.usage.cached_tokens) : null },
                  { label: 'cost', value: formatCost(r.usage.cost_usd) },
                  { label: 'duration', value: r.duration_ms != null ? formatDuration(r.duration_ms) : durationBetween(r.started_at, r.finished_at) },
                  { label: 'turns', value: r.turns ? String(r.turns) : null },
                ]}
              />
            ))}
          </ListCard>
        </div>
      )}

      {runs !== null && runs.length > 0 && (
        <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
          {loading ? 'loading more…' : hasMore ? 'scroll for more' : `— end of ${runs.length} —`}
        </div>
      )}
    </div>
  );
}
