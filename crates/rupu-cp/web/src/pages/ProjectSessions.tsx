// Project-scoped sessions list — minimal placeholder so "see all" links resolve.
// Task 9 will deepen this into a full scoped list.

import { useCallback, useEffect, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, type SessionSummary } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import MetricRow from '../components/lists/MetricRow';
import UsageBarChart from '../components/charts/UsageBarChart';
import { durationBetween } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';
import { sessionStatusDot, sessionStatusLabel } from '../lib/sessionStatus';
import { cn } from '../lib/cn';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const PAGE = 20;

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 10)}…` : id;
}

export default function ProjectSessions() {
  const { wsId } = useParams<{ wsId: string }>();
  const navigate = useNavigate();
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!wsId) return;
    try {
      const pageData = await api.getProjectSessions(wsId, { limit: PAGE });
      setSessions(pageData);
      setHasMore(pageData.length >= PAGE);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load sessions');
    }
  }, [wsId]);

  useEffect(() => {
    void refresh();
    return () => {};
  }, [refresh]);

  const loadMore = async () => {
    if (!wsId) return;
    const current = sessions ?? [];
    const next = await api.getProjectSessions(wsId, { offset: current.length, limit: PAGE });
    if (next.length === 0) {
      setHasMore(false);
      return;
    }
    setSessions([...current, ...next]);
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
        <h1 className="text-2xl font-semibold text-ink">Project Sessions</h1>
        <p className="mt-1 text-sm text-ink-mute font-mono">{wsId}</p>
      </header>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {sessions === null && !error && (
        <div className="text-sm text-ink-dim">Loading sessions…</div>
      )}

      {sessions !== null && sessions.length === 0 && (
        <div className="rounded-xl border border-dashed border-border bg-panel/50 py-12 flex items-center justify-center">
          <p className="text-sm text-ink-mute">No sessions for this project yet</p>
        </div>
      )}

      {sessions !== null && sessions.length > 0 && (
        <div className="space-y-4">
          {sessions.some((s) => s.usage) && (
            <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
              <UsageBarChart bars={sessions.filter((s) => s.usage).map((s) => ({
                id: s.session_id, label: s.agent_name,
                to: `/sessions/${encodeURIComponent(s.session_id)}`,
                input_tokens: s.usage?.input_tokens ?? 0, output_tokens: s.usage?.output_tokens ?? 0,
                cached_tokens: s.usage?.cached_tokens ?? 0, cost_usd: s.usage?.cost_usd ?? null,
              }))} />
            </div>
          )}
          <ListCard>
            {sessions.map((s) => {
              const u = s.usage;
              return (
                <MetricRow
                  key={s.session_id}
                  to={`/sessions/${encodeURIComponent(s.session_id)}`}
                  header={<>
                    <span className="flex items-center gap-1.5">
                      <span className={cn('inline-block w-2 h-2 rounded-full shrink-0', sessionStatusDot(s.status))} />
                      <span className="text-[11px] text-ink-dim">{sessionStatusLabel(s.status)}</span>
                    </span>
                    <span className="text-sm font-medium text-ink truncate">{s.agent_name}</span>
                    <span className="text-[11px] text-ink-mute font-mono">{shortId(s.session_id)}</span>
                    <span className="text-[11px] text-ink-mute font-mono">{s.model}</span>
                  </>}
                  trailing={s.active_run_id ? (
                    <button
                      type="button"
                      onClick={(e) => { e.preventDefault(); e.stopPropagation(); navigate(`/runs/${encodeURIComponent(s.active_run_id ?? '')}`); }}
                      className="shrink-0 inline-flex items-center rounded px-2 py-0.5 text-[11px] font-medium ring-1 bg-blue-50 text-blue-700 ring-blue-200 hover:bg-blue-100"
                    >
                      active run
                    </button>
                  ) : undefined}
                  metrics={[
                    { label: 'in', value: u ? formatTokens(u.input_tokens) : null },
                    { label: 'out', value: u ? formatTokens(u.output_tokens) : null },
                    { label: 'cached', value: u && u.cached_tokens ? formatTokens(u.cached_tokens) : null },
                    { label: 'cost', value: u ? formatCost(u.cost_usd) : null },
                    { label: 'turns', value: s.total_turns ? String(s.total_turns) : null },
                    { label: 'duration', value: durationBetween(s.created_at, s.updated_at) },
                  ]}
                />
              );
            })}
          </ListCard>
        </div>
      )}

      {sessions !== null && sessions.length > 0 && (
        <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
          {loading ? 'loading more…' : hasMore ? 'scroll for more' : `— end of ${sessions.length} —`}
        </div>
      )}
    </div>
  );
}
