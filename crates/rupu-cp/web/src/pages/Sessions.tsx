// Sessions list — agent sessions tracked by the control plane, grouped by
// scope (active first, then archived). Each row links to /sessions/:id. Status
// is `unknown` on the wire, so it's coerced via lib/sessionStatus.

import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { MessageSquare } from 'lucide-react';
import { api, type SessionSummary } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader, type SectionTone } from '../components/lists/SectionHeader';
import MetricRow from '../components/lists/MetricRow';
import UsageBarChart from '../components/charts/UsageBarChart';
import { cn } from '../lib/cn';
import { durationBetween } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';
import { sessionStatusDot, sessionStatusLabel } from '../lib/sessionStatus';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

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
  const navigate = useNavigate();
  const u = session.usage;
  return (
    <MetricRow
      to={`/sessions/${encodeURIComponent(session.session_id)}`}
      header={<>
        <span className="flex items-center gap-1.5">
          <span className={cn('inline-block w-2 h-2 rounded-full', sessionStatusDot(session.status))} />
          <span className="text-[11px] text-ink-dim">{sessionStatusLabel(session.status)}</span>
        </span>
        <span className="text-sm font-medium text-ink truncate">{session.agent_name}</span>
        <span className="text-[11px] text-ink-mute font-mono">{shortId(session.session_id)}</span>
        <span className="text-[11px] text-ink-mute font-mono">{session.model}</span>
      </>}
      trailing={session.active_run_id ? (
        <button
          type="button"
          onClick={(e) => { e.preventDefault(); e.stopPropagation(); navigate(`/runs/${encodeURIComponent(session.active_run_id ?? '')}`); }}
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
        { label: 'turns', value: session.total_turns ? String(session.total_turns) : null },
        { label: 'duration', value: durationBetween(session.created_at, session.updated_at) },
      ]}
    />
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
