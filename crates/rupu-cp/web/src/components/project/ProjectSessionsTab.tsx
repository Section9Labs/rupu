// Project Sessions tab body — paginated session list scoped to one project,
// with a client-side scope filter (All / Active / Archived) applied to the
// loaded rows.
//
// Ported from pages/ProjectSessions.tsx, reshaped into a self-contained
// component keyed off the `wsId` prop.

import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type SessionSummary } from '../../lib/api';
import { ListCard } from '../lists/ListCard';
import MetricRow from '../lists/MetricRow';
import UsageBarChart from '../charts/UsageBarChart';
import { durationBetween } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { sessionStatusDot, sessionStatusLabel, sessionStatusTone } from '../../lib/sessionStatus';
import { cn } from '../../lib/cn';
import { useInfiniteScroll } from '../../lib/useInfiniteScroll';

const PAGE = 20;

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 10)}…` : id;
}

// --- Scope filter -----------------------------------------------------------

type ScopeFilter = 'all' | 'active' | 'archived';

const SCOPE_FILTERS: { id: ScopeFilter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'active', label: 'Active' },
  { id: 'archived', label: 'Archived' },
];

// A session is "archived" when its status tone resolves to `stopped`
// (done / stopped / archived / exited per sessionStatusTone); everything
// else — running / idle / neutral — is treated as "active".
function isArchived(s: SessionSummary): boolean {
  return sessionStatusTone(s.status) === 'stopped';
}

function matchesScope(s: SessionSummary, filter: ScopeFilter): boolean {
  if (filter === 'all') return true;
  return filter === 'archived' ? isArchived(s) : !isArchived(s);
}

export default function ProjectSessionsTab({ wsId }: { wsId: string }) {
  const navigate = useNavigate();
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [scope, setScope] = useState<ScopeFilter>('all');

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    setSessions(null);
    setError(null);
    api
      .getProjectSessions(wsId, { limit: PAGE })
      .then((pageData) => {
        if (cancelled) return;
        setSessions(pageData);
        setHasMore(pageData.length >= PAGE);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load sessions');
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  const loadMore = useCallback(async () => {
    if (!wsId) return;
    const current = sessions ?? [];
    const next = await api.getProjectSessions(wsId, { offset: current.length, limit: PAGE });
    if (next.length === 0) {
      setHasMore(false);
      return;
    }
    setSessions([...current, ...next]);
    if (next.length < PAGE) setHasMore(false);
  }, [wsId, sessions]);

  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

  // Clicking the already-active chip returns it to "All".
  const toggleScope = (id: ScopeFilter) => setScope((cur) => (cur === id ? 'all' : id));

  const filtered = (sessions ?? []).filter((s) => matchesScope(s, scope));
  const usageBars = filtered.filter((s) => s.usage);

  return (
    <div className="space-y-4">
      {/* Scope filter chips */}
      <div className="flex items-center gap-2">
        <span className="text-meta font-semibold uppercase tracking-widest text-ink-mute w-14">
          Scope
        </span>
        {SCOPE_FILTERS.map((f) => (
          <button
            key={f.id}
            type="button"
            onClick={() => (f.id === 'all' ? setScope('all') : toggleScope(f.id))}
            className={cn(
              'text-xs font-medium px-3 py-1 rounded-full border transition-colors',
              scope === f.id
                ? 'bg-brand-600 text-white border-brand-600'
                : 'bg-panel text-ink-dim border-border hover:bg-slate-100',
            )}
          >
            {f.label}
          </button>
        ))}
      </div>

      {error && (
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {sessions === null && !error && (
        <div className="text-sm text-ink-dim">Loading sessions…</div>
      )}

      {sessions !== null && filtered.length === 0 && (
        <div className="rounded-xl border border-dashed border-border bg-panel/50 py-12 flex items-center justify-center">
          <p className="text-sm text-ink-mute">
            {sessions.length === 0
              ? 'No sessions for this project yet'
              : 'No sessions match this filter'}
          </p>
        </div>
      )}

      {sessions !== null && filtered.length > 0 && (
        <div className="space-y-4">
          {usageBars.length > 0 && (
            <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
              <UsageBarChart
                bars={usageBars.map((s) => ({
                  id: s.session_id,
                  label: s.agent_name,
                  to: `/sessions/${encodeURIComponent(s.session_id)}`,
                  input_tokens: s.usage?.input_tokens ?? 0,
                  output_tokens: s.usage?.output_tokens ?? 0,
                  cached_tokens: s.usage?.cached_tokens ?? 0,
                  cost_usd: s.usage?.cost_usd ?? null,
                }))}
              />
            </div>
          )}
          <ListCard>
            {filtered.map((s) => {
              const u = s.usage;
              return (
                <MetricRow
                  key={s.session_id}
                  to={`/sessions/${encodeURIComponent(s.session_id)}`}
                  header={
                    <>
                      <span className="flex items-center gap-1.5">
                        <span
                          className={cn(
                            'inline-block w-2 h-2 rounded-full shrink-0',
                            sessionStatusDot(s.status),
                          )}
                        />
                        <span className="text-note text-ink-dim">
                          {sessionStatusLabel(s.status)}
                        </span>
                      </span>
                      <span className="text-sm font-medium text-ink truncate">{s.agent_name}</span>
                      <span className="text-note text-ink-mute font-mono">
                        {shortId(s.session_id)}
                      </span>
                      <span className="text-note text-ink-mute font-mono">{s.model}</span>
                    </>
                  }
                  trailing={
                    s.active_run_id ? (
                      <button
                        type="button"
                        onClick={(e) => {
                          e.preventDefault();
                          e.stopPropagation();
                          navigate(`/runs/${encodeURIComponent(s.active_run_id ?? '')}`);
                        }}
                        className="shrink-0 inline-flex items-center rounded px-2 py-0.5 text-note font-medium ring-1 bg-blue-50 text-blue-700 ring-blue-200 hover:bg-blue-100"
                      >
                        active run
                      </button>
                    ) : undefined
                  }
                  metrics={[
                    { label: 'in', value: u ? formatTokens(u.input_tokens) : null },
                    { label: 'out', value: u ? formatTokens(u.output_tokens) : null },
                    {
                      label: 'cached',
                      value: u && u.cached_tokens ? formatTokens(u.cached_tokens) : null,
                    },
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

      {sessions !== null && filtered.length > 0 && (
        <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
          {loading
            ? 'loading more…'
            : hasMore
              ? 'scroll for more'
              : `— end of ${filtered.length} —`}
        </div>
      )}
    </div>
  );
}
