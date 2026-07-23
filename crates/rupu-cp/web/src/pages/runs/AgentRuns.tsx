// Agent run-stream page — standalone and session-bound agent runs.
// No DAG graph (agent runs have no workflow DAG); shows transcript_path as text.
// status and started_at are optional (standalone runs may lack them).
// Three lifecycle pills (Running / Completed / Failed-Rejected), fetch/paginate/
// poll machinery owned by the shared `usePagedList` hook. Running polls every
// 5 s (page 0 only, spliced back over the head of the list — see
// `usePagedList`'s doc comment for why that doesn't reset a scrolled view).

import { useState } from 'react';
import { Link } from 'react-router-dom';
import { RefreshCw } from 'lucide-react';
import { api, type AgentRunRow, type RunStatusStr } from '../../lib/api';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import UsageBarChart from '../../components/charts/UsageBarChart';
import { Button } from '../../components/ui/Button';
import { FilterBar } from '../../components/ui/FilterBar';
import { FilterPills } from '../../components/ui/FilterPills';
import { EmptyState } from '../../components/ui/EmptyState';
import { ErrorBanner } from '../../components/ui/ErrorBanner';
import { Spinner } from '../../components/ui/Spinner';
import { Badge } from '../../components/ui/Badge';
import { StatusPill } from '../../components/StatusPill';
import HostSelect, { ALL_HOSTS } from '../../components/HostSelect';
import { cn } from '../../lib/cn';
import { shortId } from '../../lib/shortId';
import { relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { usePagedList } from '../../lib/usePagedList';

type Tab = 'active' | 'completed' | 'failed';

const LIFECYCLE_OPTIONS: { value: Tab; label: string }[] = [
  { value: 'active', label: 'Running' },
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed / Rejected' },
];

export default function AgentRuns() {
  const [tab, setTab] = useState<Tab>('active');
  // Default to 'local' → fast server-side path; ALL_HOSTS → fan-out.
  const [hostFilter, setHostFilter] = useState<string>('local');

  const { rows, loading, error, hasMore, sentinelRef, refresh } = usePagedList<AgentRunRow>({
    fetch: ({ offset, limit }) => {
      const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
      return api.getAgentRuns({ lifecycle: tab, offset, limit, host });
    },
    deps: [tab, hostFilter],
    poll: tab === 'active',
  });

  // Sort newest-first where started_at is available; runs without it sink to
  // the bottom so that active/recent runs remain prominent. Feeds both the
  // usage chart (left-to-right order) and the table's initial sort.
  const sorted = [...rows].sort((a, b) => {
    if (!a.started_at && !b.started_at) return 0;
    if (!a.started_at) return 1;
    if (!b.started_at) return -1;
    return Date.parse(b.started_at) - Date.parse(a.started_at);
  });

  return (
    <div className="p-8">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Agent Runs</h1>
          <p className="mt-1 text-sm text-ink-dim">Standalone and session-bound agent invocations.</p>
        </div>
        <Button variant="secondary" onClick={() => refresh()} className="gap-1.5">
          <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
          Refresh
        </Button>
      </header>

      <FilterBar
        filters={
          <FilterPills options={LIFECYCLE_OPTIONS} value={tab} onChange={(v) => setTab(v as Tab)} />
        }
        scope={<HostSelect allowAll ariaLabel="Host filter" value={hostFilter} onChange={setHostFilter} />}
      />

      <div className="mt-5">
        {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

        {loading && rows.length === 0 ? (
          <div className="py-16 flex items-center justify-center">
            <Spinner label="Loading agent runs…" />
          </div>
        ) : sorted.length === 0 ? (
          <EmptyState
            title="No agent runs yet"
            hint="Standalone and session-bound agent invocations will appear here once they run."
          />
        ) : (
          <section>
            <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mb-4">
              <UsageBarChart bars={sorted.map((r) => {
                const hostSuffix = r.host_id && r.host_id !== 'local'
                  ? `&host=${encodeURIComponent(r.host_id)}`
                  : '';
                return {
                  id: r.run_id,
                  label: r.agent ?? r.run_id,
                  to: r.transcript_path
                    ? `/transcript?path=${encodeURIComponent(r.transcript_path)}&live=${isRunning(r.status) ? 1 : 0}${hostSuffix}`
                    : undefined,
                  input_tokens: r.usage.input_tokens, output_tokens: r.usage.output_tokens,
                  cached_tokens: r.usage.cached_tokens, cost_usd: r.usage.cost_usd,
                };
              })} />
            </div>
            <SortableTable<AgentRunRow>
              columns={AGENT_RUN_COLUMNS}
              rows={sorted}
              rowKey={(r) => r.run_id}
              initialSort={{ key: 'started', dir: 'desc' }}
            />
            <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
              {loading ? 'loading more…' : hasMore ? 'scroll for more' : `— end of ${sorted.length} —`}
            </div>
          </section>
        )}
      </div>
    </div>
  );
}

/** Returns true when the status string indicates the run is still in progress. */
function isRunning(status: string | null | undefined): boolean {
  return status === 'running' || status === 'awaiting_approval';
}

// Session-branch agent-run rows carry the CLI's own wire vocabulary
// (`"ok" | "error" | "aborted"` — see rupu-cp's api/run_streams.rs /
// api/sessions.rs), NOT the run-status lexicon `StatusPill` speaks
// (`RunStatusStr`). Standalone rows already use the run lexicon directly.
// Map the CLI vocabulary onto it before rendering so a session row's "ok"
// doesn't hit StatusPill's unknown-status fallback (neutral tone, AlertCircle
// icon) instead of the green Completed pill it actually means. Statuses
// already in the run lexicon pass through unchanged.
function normalizeAgentRunStatus(s: string): RunStatusStr {
  switch (s) {
    case 'ok':
      return 'completed';
    case 'error':
      return 'failed';
    case 'aborted':
      return 'cancelled';
    default:
      return s as RunStatusStr;
  }
}

const AGENT_RUN_COLUMNS: Column<AgentRunRow>[] = [
  {
    key: 'agent',
    header: 'Agent',
    subject: true,
    sortable: true,
    sortValue: (r) => r.agent ?? null,
    titleValue: (r) => r.agent ?? r.run_id,
    render: (r) => (
      <div className="min-w-0">
        <span className="block truncate text-sm font-medium text-ink" title={r.agent ?? undefined}>
          {r.agent ?? '—'}
        </span>
        {(r.trigger_source || r.session_id) && (
          <div className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5">
            {r.trigger_source && (
              <span className="text-note text-ink-dim">
                via <span className="font-mono">{r.trigger_source}</span>
              </span>
            )}
            {r.session_id && (
              <span className="text-note text-ink-dim">
                session{' '}
                <Link
                  to={`/sessions/${encodeURIComponent(r.session_id)}`}
                  className="text-brand-600 hover:underline font-mono"
                >
                  {shortId(r.session_id)}
                </Link>
              </span>
            )}
          </div>
        )}
      </div>
    ),
  },
  {
    key: 'run',
    header: 'Run',
    fit: true,
    // Agent runs are NOT in the workflow run-store (/api/runs/:id would 404), so
    // the Run id links to the transcript view — matching the page's pre-table
    // behavior. No transcript_path → render the id as plain text.
    // Append &host= for remote runs so the transcript view can proxy-fetch.
    render: (r) => {
      const hostSuffix = r.host_id && r.host_id !== 'local'
        ? `&host=${encodeURIComponent(r.host_id)}`
        : '';
      const href = r.transcript_path
        ? `/transcript?path=${encodeURIComponent(r.transcript_path)}&live=${isRunning(r.status) ? 1 : 0}${hostSuffix}`
        : null;
      return href ? (
        <Link to={href} className="text-note text-ink-mute font-mono hover:underline">
          {shortId(r.run_id)}
        </Link>
      ) : (
        <span className="text-note text-ink-mute font-mono">{shortId(r.run_id)}</span>
      );
    },
  },
  {
    key: 'host',
    header: 'Host',
    fit: true,
    sortable: true,
    sortValue: (r) => r.host_id ?? 'local',
    render: (r) => (
      <span className="text-note text-ink-mute font-mono">{r.host_id ?? 'local'}</span>
    ),
  },
  {
    key: 'source',
    header: 'Source',
    fit: true,
    sortable: true,
    sortValue: (r) => r.source,
    render: (r) => (
      <Badge tone={r.source === 'session' ? 'sky' : 'neutral'} className="uppercase tracking-wide">
        {r.source}
      </Badge>
    ),
  },
  {
    key: 'status',
    header: 'Status',
    fit: true,
    sortable: true,
    sortValue: (r) => r.status ?? null,
    render: (r) =>
      r.status ? (
        <StatusPill status={normalizeAgentRunStatus(r.status)} />
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'in',
    header: 'In',
    fit: true,
    align: 'right',
    sortable: true,
    sortValue: (r) => r.usage.input_tokens,
    render: (r) => <span className="text-ink-dim">{formatTokens(r.usage.input_tokens)}</span>,
  },
  {
    key: 'out',
    header: 'Out',
    fit: true,
    align: 'right',
    sortable: true,
    sortValue: (r) => r.usage.output_tokens,
    render: (r) => <span className="text-ink-dim">{formatTokens(r.usage.output_tokens)}</span>,
  },
  {
    key: 'cached',
    header: 'Cached',
    fit: true,
    align: 'right',
    sortable: true,
    sortValue: (r) => r.usage.cached_tokens,
    render: (r) =>
      r.usage.cached_tokens ? (
        <span className="text-ink-dim">{formatTokens(r.usage.cached_tokens)}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'cost',
    header: 'Cost',
    fit: true,
    align: 'right',
    sortable: true,
    sortValue: (r) => r.usage.cost_usd,
    render: (r) => <span className="text-ink font-medium">{formatCost(r.usage.cost_usd)}</span>,
  },
  {
    key: 'duration',
    header: 'Duration',
    fit: true,
    align: 'right',
    sortable: true,
    sortValue: (r) => r.duration_ms ?? null,
    render: (r) => <span className="text-ink-dim">{formatDuration(r.duration_ms)}</span>,
  },
  {
    key: 'turns',
    header: 'Turns',
    fit: true,
    align: 'right',
    sortable: true,
    sortValue: (r) => r.turns,
    render: (r) => <span className="text-ink">{r.turns ? String(r.turns) : '—'}</span>,
  },
  {
    key: 'started',
    header: 'Started',
    fit: true,
    align: 'right',
    sortable: true,
    sortValue: (r) => (r.started_at ? Date.parse(r.started_at) : null),
    render: (r) => (
      <span className="text-ink-mute">{r.started_at ? relativeTime(r.started_at) : '—'}</span>
    ),
  },
];
