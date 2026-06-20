// Agent run-stream page — standalone and session-bound agent runs.
// No DAG graph (agent runs have no workflow DAG); shows transcript_path as text.
// status and started_at are optional (standalone runs may lack them).

import { useCallback, useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { Inbox, RefreshCw } from 'lucide-react';
import { api, type AgentRunRow } from '../../lib/api';
import { ListCard } from '../../components/lists/ListCard';
import { SectionHeader } from '../../components/lists/SectionHeader';
import MetricRow from '../../components/lists/MetricRow';
import UsageBarChart from '../../components/charts/UsageBarChart';
import { cn } from '../../lib/cn';
import { relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { useInfiniteScroll } from '../../lib/useInfiniteScroll';

const PAGE = 20;

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

const SOURCE_CLS: Record<string, string> = {
  standalone: 'bg-slate-100 text-slate-600 ring-slate-200',
  session:    'bg-sky-50 text-sky-700 ring-sky-200',
};

function SourceChip({ source }: { source: string }) {
  const cls = SOURCE_CLS[source] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {source}
    </span>
  );
}

// Render the raw status string as a simple badge — AgentRunRow.status is a
// free-form string (not RunStatusStr), so we map known values to colors
// and fall back to neutral for unknown ones.
const STATUS_CLS: Record<string, string> = {
  running:           'bg-blue-50 text-blue-700 ring-blue-200',
  completed:         'bg-green-50 text-green-700 ring-green-200',
  failed:            'bg-red-50 text-red-700 ring-red-200',
  awaiting_approval: 'bg-amber-50 text-amber-800 ring-amber-200',
  rejected:          'bg-red-50 text-red-700 ring-red-200',
  pending:           'bg-slate-100 text-slate-600 ring-slate-200',
};

function StatusBadge({ status }: { status: string }) {
  const cls = STATUS_CLS[status] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-[10px] font-medium px-1.5 py-0.5', cls)}>
      {status}
    </span>
  );
}

export default function AgentRuns() {
  const [agentRuns, setAgentRuns] = useState<AgentRunRow[] | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  // Page-0 fetch (mount + 5 s refresh) — resets pagination.
  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const data = await api.getAgentRuns({ limit: PAGE });
      setAgentRuns(data);
      setHasMore(data.length >= PAGE);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load agent runs');
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const t = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(t);
  }, [refresh]);

  const loadMore = async () => {
    const current = agentRuns ?? [];
    const next = await api.getAgentRuns({ offset: current.length, limit: PAGE });
    if (next.length === 0) { setHasMore(false); return; }
    setAgentRuns([...current, ...next]);
    if (next.length < PAGE) setHasMore(false);
  };

  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

  // Sort newest-first where started_at is available; runs without it sink to
  // the bottom so that active/recent runs remain prominent.
  const sorted = [...(agentRuns ?? [])].sort((a, b) => {
    if (!a.started_at && !b.started_at) return 0;
    if (!a.started_at) return 1;
    if (!b.started_at) return -1;
    return Date.parse(b.started_at) - Date.parse(a.started_at);
  });

  return (
    <div className="p-8 max-w-5xl">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Agent Runs</h1>
          <p className="mt-1 text-sm text-ink-dim">Standalone and session-bound agent invocations.</p>
        </div>
        <button
          onClick={() => void refresh()}
          className="inline-flex items-center gap-1.5 text-xs font-medium px-3 py-1.5 rounded-md border border-border bg-panel text-ink hover:bg-slate-100"
        >
          <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
          Refresh
        </button>
      </header>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {agentRuns === null ? (
        <div className="text-sm text-ink-dim">Loading agent runs…</div>
      ) : agentRuns.length === 0 ? (
        <AgentRunsEmpty />
      ) : (
        <section>
          {sorted.length > 0 && (
            <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mb-4">
              <UsageBarChart bars={sorted.map((r) => ({
                id: r.run_id,
                label: r.agent ?? r.run_id,
                to: r.transcript_path
                  ? `/transcript?path=${encodeURIComponent(r.transcript_path)}&live=${isRunning(r.status) ? 1 : 0}`
                  : undefined,
                input_tokens: r.usage.input_tokens, output_tokens: r.usage.output_tokens,
                cached_tokens: r.usage.cached_tokens, cost_usd: r.usage.cost_usd,
              }))} />
            </div>
          )}
          <SectionHeader tone="muted" label="Agent Runs" count={sorted.length} />
          <ListCard>
            {sorted.map((r) => (
              <AgentRunEntry key={r.run_id} run={r} />
            ))}
          </ListCard>
          {sorted.length > 0 && (
            <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
              {loading ? 'loading more…' : hasMore ? 'scroll for more' : `— end of ${sorted.length} —`}
            </div>
          )}
        </section>
      )}
    </div>
  );
}

/** Returns true when the status string indicates the run is still in progress. */
function isRunning(status: string | null | undefined): boolean {
  return status === 'running' || status === 'awaiting_approval';
}

function AgentRunEntry({ run }: { run: AgentRunRow }) {
  const live = isRunning(run.status);
  const navigate = useNavigate();

  const transcriptHref = run.transcript_path
    ? `/transcript?path=${encodeURIComponent(run.transcript_path)}&live=${live ? 1 : 0}`
    : null;

  const inner = (
    <MetricRow
      header={<>
        <span className="text-sm font-medium text-ink">{run.agent ?? '—'}</span>
        <span className="text-[11px] text-ink-mute font-mono">{shortId(run.run_id)}</span>
        <SourceChip source={run.source} />
        {run.status && <StatusBadge status={run.status} />}
        {run.started_at && (
          <span className="text-[11px] text-ink-dim">started {relativeTime(run.started_at)}</span>
        )}
        {run.trigger_source && (
          <span className="text-[11px] text-ink-dim">via <span className="font-mono">{run.trigger_source}</span></span>
        )}
        {run.session_id && (
          <span className="text-[11px] text-ink-dim">
            session{' '}
            <Link
              to={`/sessions/${encodeURIComponent(run.session_id)}`}
              className="text-brand-600 hover:underline font-mono"
              onClick={(e) => e.stopPropagation()}
            >
              {shortId(run.session_id)}
            </Link>
          </span>
        )}
      </>}
      trailing={transcriptHref
        ? <span className="text-[10px] text-brand-600 font-medium whitespace-nowrap">View transcript →</span>
        : undefined}
      metrics={[
        { label: 'in', value: formatTokens(run.usage.input_tokens) },
        { label: 'out', value: formatTokens(run.usage.output_tokens) },
        { label: 'cached', value: run.usage.cached_tokens ? formatTokens(run.usage.cached_tokens) : null },
        { label: 'cost', value: formatCost(run.usage.cost_usd) },
        { label: 'duration', value: run.duration_ms != null ? formatDuration(run.duration_ms) : null },
        { label: 'turns', value: run.turns ? String(run.turns) : null },
      ]}
    />
  );

  if (transcriptHref) {
    return (
      <div
        role="link"
        tabIndex={0}
        onClick={() => navigate(transcriptHref)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            navigate(transcriptHref);
          }
        }}
        className="block hover:bg-slate-50 transition-colors cursor-pointer"
      >
        {inner}
      </div>
    );
  }

  return <div className="opacity-75">{inner}</div>;
}

function AgentRunsEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No agent runs yet</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Standalone and session-bound agent invocations will appear here once they run.
      </p>
    </div>
  );
}
