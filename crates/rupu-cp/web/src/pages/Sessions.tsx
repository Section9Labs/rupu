// Sessions list — agent sessions tracked by the control plane, grouped by
// scope (active first, then archived). Each row links to /sessions/:id. Status
// is `unknown` on the wire, so it's coerced via lib/sessionStatus.

import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { MessageSquare } from 'lucide-react';
import { api, type SessionSummary } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader, type SectionTone } from '../components/lists/SectionHeader';
import { cn } from '../lib/cn';
import { relativeTime } from '../lib/time';
import { sessionStatusDot, sessionStatusLabel } from '../lib/sessionStatus';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';
import UsageChip from '../components/UsageChip';

const PAGE = 20;

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 10)}…` : id;
}

// Section ordering: active scope first, then archived, then anything else.
function scopeRank(scope: string): number {
  const s = scope.toLowerCase();
  if (s === 'active') return 0;
  if (s === 'archived') return 1;
  return 2;
}

function scopeTone(scope: string): SectionTone {
  const s = scope.toLowerCase();
  if (s === 'active') return 'progress';
  if (s === 'archived') return 'muted';
  return 'muted';
}

export default function Sessions() {
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const pageData = await api.getSessions({ limit: PAGE });
      setSessions(pageData);
      setHasMore(pageData.length >= PAGE);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load sessions');
    }
  }, []);

  useEffect(() => {
    void refresh();
    return () => {};
  }, [refresh]);

  const loadMore = async () => {
    const current = sessions ?? [];
    const next = await api.getSessions({ offset: current.length, limit: PAGE });
    if (next.length === 0) {
      setHasMore(false);
      return;
    }
    setSessions([...current, ...next]);
    if (next.length < PAGE) setHasMore(false);
  };

  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

  // Group by scope.
  const groups = new Map<string, SessionSummary[]>();
  for (const s of sessions ?? []) {
    const arr = groups.get(s.scope) ?? [];
    arr.push(s);
    groups.set(s.scope, arr);
  }
  const orderedScopes = [...groups.keys()].sort(
    (a, b) => scopeRank(a) - scopeRank(b) || a.localeCompare(b),
  );

  return (
    <div className="p-8 max-w-5xl">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Sessions</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Agent sessions tracked by this control plane — active conversations and their archived
          history.
        </p>
      </header>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {sessions === null ? (
        <div className="text-sm text-ink-dim">Loading sessions…</div>
      ) : sessions.length === 0 ? (
        <EmptyState />
      ) : (
        <div className="space-y-6">
          {orderedScopes.map((scope) => {
            const rows = groups.get(scope) ?? [];
            return (
              <section key={scope}>
                <SectionHeader tone={scopeTone(scope)} label={scope} count={rows.length} />
                <ListCard>
                  {rows.map((s) => (
                    <SessionRow key={s.session_id} session={s} />
                  ))}
                </ListCard>
              </section>
            );
          })}

          <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
            {loading ? 'loading more…' : hasMore ? 'scroll for more' : `— end of ${sessions.length} —`}
          </div>
        </div>
      )}
    </div>
  );
}

function SessionRow({ session }: { session: SessionSummary }) {
  // The active-run badge is a sibling Link, not nested inside the row Link
  // (anchor-in-anchor is invalid HTML). The row Link fills the available width.
  return (
    <div className="flex items-center gap-4 px-4 py-3 hover:bg-slate-50 transition-colors">
      <Link to={`/sessions/${encodeURIComponent(session.session_id)}`} className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="flex items-center gap-1.5">
            <span className={cn('inline-block w-2 h-2 rounded-full', sessionStatusDot(session.status))} />
            <span className="text-[11px] text-ink-dim">{sessionStatusLabel(session.status)}</span>
          </span>
          <span className="text-sm font-medium text-ink truncate">{session.agent_name}</span>
          <span className="text-[11px] text-ink-mute font-mono">{shortId(session.session_id)}</span>
        </div>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-3 text-[11px] text-ink-mute">
          <span className="font-mono">{session.model}</span>
          <span className="tabular-nums">
            {session.total_turns} turn{session.total_turns !== 1 ? 's' : ''}
          </span>
          <span>updated {relativeTime(session.updated_at)}</span>
          {session.usage && <UsageChip usage={session.usage} className="ml-2" />}
        </div>
      </Link>

      {session.active_run_id && (
        <Link
          to={`/runs/${encodeURIComponent(session.active_run_id)}`}
          className="shrink-0 inline-flex items-center rounded px-2 py-0.5 text-[11px] font-medium ring-1 bg-blue-50 text-blue-700 ring-blue-200 hover:bg-blue-100"
        >
          active run
        </Link>
      )}
    </div>
  );
}

function EmptyState() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <MessageSquare size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No sessions yet</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Sessions appear here once an agent conversation is started against this control plane.
      </p>
    </div>
  );
}
