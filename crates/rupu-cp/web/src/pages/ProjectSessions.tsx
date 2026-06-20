// Project-scoped sessions list — minimal placeholder so "see all" links resolve.
// Task 9 will deepen this into a full scoped list.

import { useCallback, useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, type SessionSummary } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { relativeTime } from '../lib/time';
import { sessionStatusDot, sessionStatusLabel } from '../lib/sessionStatus';
import { cn } from '../lib/cn';
import UsageChip from '../components/UsageChip';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const PAGE = 20;

export default function ProjectSessions() {
  const { wsId } = useParams<{ wsId: string }>();
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
        <ListCard>
          {sessions.map((s) => (
            <Link
              key={s.session_id}
              to={`/sessions/${encodeURIComponent(s.session_id)}`}
              className="flex items-center gap-4 px-4 py-3 hover:bg-slate-50 transition-colors"
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className={cn('inline-block w-2 h-2 rounded-full shrink-0', sessionStatusDot(s.status))} />
                  <span className="text-sm font-medium text-ink truncate">{s.agent_name}</span>
                  <span className="text-[11px] text-ink-mute">{sessionStatusLabel(s.status)}</span>
                </div>
                <p className="text-[11px] text-ink-dim mt-0.5">
                  {s.total_turns} turn{s.total_turns !== 1 ? 's' : ''} · updated{' '}
                  {relativeTime(s.updated_at)}
                  {s.usage && <UsageChip usage={s.usage} className="ml-2" />}
                </p>
              </div>
            </Link>
          ))}
        </ListCard>
      )}

      {sessions !== null && sessions.length > 0 && (
        <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
          {loading ? 'loading more…' : hasMore ? 'scroll for more' : `— end of ${sessions.length} —`}
        </div>
      )}
    </div>
  );
}
