// Dashboard — "spend-forward operations". A triage ribbon up top, a per-model
// usage timeline + all-models breakdown as the hero, supporting stat tiles, and
// a recent-runs + run-status bottom rail.
//
// Data: GET /api/dashboard (point-in-time counts + recent_runs), GET /api/usage
// (windowed summary + per-model breakdown), GET /api/usage/timeline (per-bucket
// per-model series), GET /api/findings (open-findings total), GET /api/runs
// (failed-in-window count). The global range control drives the windowed calls.
//
// Polls every 15 s; clears the interval on unmount.

import { useCallback, useEffect, useRef, useState } from 'react';
import { Activity, MessageSquare, RefreshCw, Server, ShieldCheck } from 'lucide-react';
import { Cell, Pie, PieChart, ResponsiveContainer, Tooltip } from 'recharts';
import { api, type DashboardResponse, type RunStatusStr, type UsageOverview, type UsageTimelineBucket } from '../lib/api';
import { useThemeColors } from '../lib/useThemeColors';
import { StatusPill } from '../components/StatusPill';
import UsageChip from '../components/UsageChip';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import TriageRibbon from '../components/dashboard/TriageRibbon';
import UsageTimelineStacked, { type UsageMetric } from '../components/dashboard/UsageTimelineStacked';
import ModelBreakdownTable from '../components/dashboard/ModelBreakdownTable';
import { Button } from '../components/ui/Button';
import { cn } from '../lib/cn';
import { relativeTime } from '../lib/time';
import { formatCost, formatTokens } from '../lib/usage';
import { shortId } from '../lib/shortId';

// ---------------------------------------------------------------------------
// Global range control
// ---------------------------------------------------------------------------

type Range = '7d' | '30d' | 'all';

const RANGES: { key: Range; label: string }[] = [
  { key: '7d', label: '7d' },
  { key: '30d', label: '30d' },
  { key: 'all', label: 'All' },
];

/** Map a range to the windowed-query params: `since` (RFC-3339) and timeline
 *  `bucket` granularity. "All" sends the epoch as `since` — the backend defaults
 *  an absent `since` to now-30d, so we must pass an explicit floor to actually
 *  span all history (weekly-bucketed). */
function rangeParams(range: Range): { since: string; bucket: 'day' | 'week' } {
  const now = Date.now();
  const day = 86_400_000;
  if (range === '7d') return { since: new Date(now - 7 * day).toISOString(), bucket: 'day' };
  if (range === '30d') return { since: new Date(now - 30 * day).toISOString(), bucket: 'day' };
  return { since: new Date(0).toISOString(), bucket: 'week' };
}

// ---------------------------------------------------------------------------
// Run-status donut chart (bottom rail)
// ---------------------------------------------------------------------------

/** Themed per-status donut fills, resolved from the current palette. */
function statusFill(colors: ReturnType<typeof useThemeColors>): Record<RunStatusStr, string> {
  return {
    running: colors.status.running,
    completed: colors.status.completed,
    failed: colors.status.failed,
    awaiting_approval: colors.status.awaiting,
    paused: colors.status.paused,
    pending: colors.status.pending,
    rejected: colors.status.rejected,
    cancelled: colors.status.cancelled,
  };
}

const STATUS_ORDER: RunStatusStr[] = [
  'running', 'awaiting_approval', 'paused', 'pending', 'completed', 'failed', 'rejected', 'cancelled',
];

const STATUS_LABEL: Record<RunStatusStr, string> = {
  running:           'Running',
  completed:         'Completed',
  failed:            'Failed',
  awaiting_approval: 'Awaiting approval',
  paused:            'Paused',
  pending:           'Pending',
  rejected:          'Rejected',
  cancelled:         'Cancelled',
};

function RunStatusChart({ byStatus, total }: { byStatus: Record<RunStatusStr, number>; total: number }) {
  const colors = useThemeColors();
  const STATUS_FILL = statusFill(colors);
  const tooltipStyle: React.CSSProperties = {
    background: colors.panel,
    border: `1px solid ${colors.border}`,
    color: colors.ink,
    borderRadius: 6,
    fontSize: 11,
    padding: '6px 10px',
    boxShadow: '0 2px 6px rgba(0,0,0,0.18)',
  };
  if (total === 0) {
    return (
      <div className="py-12 flex flex-col items-center justify-center text-center">
        <div className="w-10 h-10 rounded-full bg-surface flex items-center justify-center mb-2">
          <Activity size={18} className="text-ink-mute" />
        </div>
        <p className="text-xs text-ink-mute">No runs yet</p>
      </div>
    );
  }
  const data = STATUS_ORDER
    .map((s) => ({ name: STATUS_LABEL[s], value: byStatus[s] ?? 0, fill: STATUS_FILL[s], key: s }))
    .filter((d) => d.value > 0);
  return (
    <div className="flex items-center gap-6">
      <div style={{ width: 140, height: 140, flexShrink: 0 }}>
        <ResponsiveContainer width="100%" height="100%">
          <PieChart>
            <Pie data={data} cx="50%" cy="50%" innerRadius={42} outerRadius={62} paddingAngle={2} dataKey="value" strokeWidth={0}>
              {data.map((d) => <Cell key={d.key} fill={d.fill} />)}
            </Pie>
            <Tooltip contentStyle={tooltipStyle} formatter={(value, name) => [`${value ?? 0}`, `${name}`]} />
          </PieChart>
        </ResponsiveContainer>
      </div>
      <ul className="flex-1 min-w-0 space-y-1.5">
        {data.map((d) => (
          <li key={d.key} className="flex items-center gap-2 text-xs">
            <span className="w-2.5 h-2.5 rounded-sm shrink-0" style={{ background: d.fill }} />
            <span className="text-ink-dim flex-1 min-w-0 truncate">{d.name}</span>
            <span className="text-ink font-medium tabular-nums">{d.value}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Secondary stat tile
// ---------------------------------------------------------------------------

function StatTile({
  icon: Icon, iconCls, label, value, sub,
}: {
  icon: React.ElementType; iconCls: string; label: string; value: number | string; sub?: string;
}) {
  return (
    <div className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-xs text-ink-dim font-medium uppercase tracking-wide">{label}</p>
          <p className="mt-1 text-2xl font-semibold text-ink tabular-nums">{value}</p>
          {sub && <p className="mt-0.5 text-xs text-ink-mute">{sub}</p>}
        </div>
        <div className={cn('w-9 h-9 rounded-lg flex items-center justify-center shrink-0', iconCls)}>
          <Icon size={17} />
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Recent runs
// ---------------------------------------------------------------------------

type RecentRun = DashboardResponse['recent_runs'][number];

const RECENT_RUN_COLUMNS: Column<RecentRun>[] = [
  {
    key: 'workflow',
    header: 'Workflow',
    sortable: true,
    sortValue: (r) => r.workflow_name,
    render: (r) => <span className="text-sm font-medium text-ink truncate">{r.workflow_name}</span>,
  },
  {
    key: 'run',
    header: 'Run',
    render: (r) => <span className="text-note text-ink-mute font-mono">{shortId(r.id)}</span>,
  },
  {
    key: 'started',
    header: 'Started',
    sortable: true,
    sortValue: (r) => (r.started_at ? Date.parse(r.started_at) : null),
    render: (r) => <span className="text-ink-mute">{relativeTime(r.started_at)}</span>,
  },
  {
    key: 'status',
    header: 'Status',
    sortable: true,
    sortValue: (r) => r.status,
    render: (r) => <StatusPill status={r.status} />,
  },
  {
    key: 'usage',
    header: 'Usage',
    align: 'right',
    render: (r) => <UsageChip usage={r.usage} />,
  },
];

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

const POLL_MS = 15_000;

export default function Dashboard() {
  const [range, setRange] = useState<Range>('30d');
  const [metric, setMetric] = useState<UsageMetric>('cost');
  const userPickedMetric = useRef(false);

  const [data, setData] = useState<DashboardResponse | null>(null);
  const [usage, setUsage] = useState<UsageOverview | null>(null);
  const [timeline, setTimeline] = useState<UsageTimelineBucket[] | null>(null);
  const [findingsTotal, setFindingsTotal] = useState<number | null>(null);
  const [failedInWindow, setFailedInWindow] = useState(0);

  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);

  const [ageSec, setAgeSec] = useState(0);
  const lastUpdatedRef = useRef<Date | null>(null);

  const load = useCallback(async () => {
    setRefreshing(true);
    const { since, bucket } = rangeParams(range);
    try {
      const d = await api.getDashboard();
      const [u, tl, findings, runs] = await Promise.all([
        api.getUsage({ since, groupBy: 'model' }).catch(() => null),
        api.getUsageTimeline({ since, bucket }).catch(() => null),
        api.getFindings().catch(() => null),
        api.getRuns({ limit: 500, host: 'local' }).catch(() => null),
      ]);

      setData(d);
      setUsage(u);
      setTimeline(tl);
      setFindingsTotal(findings ? findings.summary.total : null);

      // Failed-in-window: prefer the runs list (true windowed count); fall back
      // to the all-time status rollup when /api/runs is unavailable.
      if (runs) {
        const sinceMs = since ? Date.parse(since) : 0;
        setFailedInWindow(
          runs.filter(
            (r) => (r.status === 'failed' || r.status === 'rejected') && Date.parse(r.started_at) >= sinceMs,
          ).length,
        );
      } else {
        setFailedInWindow((d.runs.by_status.failed ?? 0) + (d.runs.by_status.rejected ?? 0));
      }

      // Default the chart to Tokens when nothing is priced (unless the user
      // explicitly chose a metric).
      if (!userPickedMetric.current) {
        const anyPriced = u?.breakdown.some((r) => r.cost_usd !== null) ?? false;
        setMetric(anyPriced ? 'cost' : 'tokens');
      }

      setError(null);
      const now = new Date();
      setLastUpdated(now);
      lastUpdatedRef.current = now;
      setAgeSec(0);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load dashboard');
    } finally {
      setRefreshing(false);
    }
  }, [range]);

  useEffect(() => {
    void load();
    const poll = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(poll);
  }, [load]);

  useEffect(() => {
    if (!lastUpdated) return;
    const ticker = window.setInterval(() => {
      const ref = lastUpdatedRef.current;
      if (ref) setAgeSec(Math.round((Date.now() - ref.getTime()) / 1000));
    }, 1000);
    return () => window.clearInterval(ticker);
  }, [lastUpdated]);

  const pickMetric = (m: UsageMetric) => {
    userPickedMetric.current = true;
    setMetric(m);
  };

  const running = data?.runs.by_status.running ?? 0;
  const awaiting = data?.runs.by_status.awaiting_approval ?? 0;
  const activeSessions = data?.sessions.active ?? 0;
  const totalSessions = data?.sessions.total ?? 0;
  const totalWorkers = data?.workers.total ?? 0;
  const coverageTargets = data?.coverage.targets ?? 0;
  const coverageAssertions = data?.coverage.assertions ?? 0;

  const summary = usage?.summary;
  const spendPartial = summary != null && summary.cost_usd !== null && !summary.priced;

  return (
    <div className="p-8">
      {/* Header */}
      <header className="flex items-center justify-between mb-6 gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Dashboard</h1>
          <p className="mt-1 text-sm text-ink-dim">Spend-forward operations at a glance.</p>
        </div>
        <div className="flex items-center gap-3">
          {/* Range segmented control */}
          <div className="inline-flex rounded-lg border border-border bg-panel p-0.5">
            {RANGES.map((r) => (
              <button
                key={r.key}
                onClick={() => setRange(r.key)}
                className={cn(
                  'px-3 py-1 text-xs font-medium rounded-md transition-colors',
                  range === r.key ? 'bg-brand-50 text-brand-700' : 'text-ink-dim hover:text-ink',
                )}
              >
                {r.label}
              </button>
            ))}
          </div>
          {lastUpdated && !refreshing && (
            <span className="text-note text-ink-mute tabular-nums">
              updated {ageSec < 5 ? 'just now' : `${ageSec}s ago`}
            </span>
          )}
          <Button variant="secondary" onClick={() => void load()} className="gap-1.5">
            <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
            Refresh
          </Button>
        </div>
      </header>

      {error && (
        <div className="mb-5 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}

      {data === null && !error && <div className="text-sm text-ink-dim">Loading…</div>}

      {data !== null && (
        <div className="space-y-8">
          {/* ── Triage ribbon ── */}
          <TriageRibbon
            running={running}
            awaiting={awaiting}
            failed={failedInWindow}
            findings={findingsTotal}
          />

          {/* ── Usage hero ── */}
          <section className="grid grid-cols-1 lg:grid-cols-3 gap-5">
            {/* Timeline + headline numbers (≈⅔) */}
            <div className="lg:col-span-2 bg-panel border border-border rounded-xl shadow-card px-5 py-4">
              <div className="flex items-start justify-between gap-4 mb-4 flex-wrap">
                <div className="flex items-baseline gap-6">
                  <div>
                    <p className="text-xs text-ink-dim font-medium uppercase tracking-wide">Spend</p>
                    <p className="mt-1 text-3xl font-semibold text-ink tabular-nums">
                      {formatCost(summary?.cost_usd ?? null)}
                      {spendPartial && <span className="text-warn">*</span>}
                    </p>
                  </div>
                  <div>
                    <p className="text-xs text-ink-dim font-medium uppercase tracking-wide">Tokens</p>
                    <p className="mt-1 text-3xl font-semibold text-ink tabular-nums">
                      {formatTokens(summary?.total_tokens ?? 0)}
                    </p>
                  </div>
                </div>
                {/* Cost | Tokens toggle */}
                <div className="inline-flex rounded-lg border border-border p-0.5">
                  {(['cost', 'tokens'] as const).map((m) => (
                    <button
                      key={m}
                      onClick={() => pickMetric(m)}
                      className={cn(
                        'px-3 py-1 text-xs font-medium rounded-md transition-colors capitalize',
                        metric === m ? 'bg-brand-50 text-brand-700' : 'text-ink-dim hover:text-ink',
                      )}
                    >
                      {m === 'cost' ? 'Cost $' : 'Tokens'}
                    </button>
                  ))}
                </div>
              </div>
              {spendPartial && (
                <p className="-mt-2 mb-3 text-note text-ink-mute">
                  <span className="text-warn">*</span> partial — some models are unpriced (see breakdown)
                </p>
              )}
              <UsageTimelineStacked buckets={timeline ?? []} metric={metric} />
            </div>

            {/* All-models breakdown (≈⅓) */}
            <div className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
              <h2 className="text-sm font-semibold text-ink-dim mb-3">Models</h2>
              <ModelBreakdownTable rows={usage?.breakdown ?? []} />
            </div>
          </section>

          {/* ── Secondary tiles ── */}
          <section className="grid grid-cols-2 gap-4 sm:grid-cols-4">
            <StatTile icon={Activity} iconCls="bg-info-bg text-info" label="Total runs" value={data.runs.total}
              sub={running > 0 ? `${running} running` : undefined} />
            <StatTile icon={MessageSquare} iconCls="bg-brand-50 text-brand-600" label="Sessions" value={activeSessions}
              sub={`${activeSessions} of ${totalSessions} active`} />
            <StatTile icon={Server} iconCls="bg-surface text-ink" label="Workers" value={totalWorkers} />
            <StatTile icon={ShieldCheck} iconCls="bg-ok-bg text-ok" label="Coverage targets" value={coverageTargets}
              sub={coverageAssertions > 0 ? `${coverageAssertions} assertions` : undefined} />
          </section>

          {/* ── Bottom rail: recent runs + run-status donut ── */}
          <section className="grid grid-cols-1 lg:grid-cols-5 gap-5">
            <div className="lg:col-span-3">
              <div className="flex items-center gap-2 mb-3 pl-1">
                <span className="w-2 h-2 rounded-full bg-brand-500" />
                <h2 className="text-sm font-semibold text-brand-700">Recent Runs</h2>
                <span className="text-xs text-ink-mute tabular-nums">{data.recent_runs.length}</span>
              </div>
              {data.recent_runs.length === 0 ? (
                <div className="rounded-xl border border-dashed border-border bg-panel/50 py-10 flex flex-col items-center justify-center text-center">
                  <div className="w-10 h-10 rounded-full bg-surface flex items-center justify-center mb-2">
                    <Activity size={17} className="text-ink-mute" />
                  </div>
                  <p className="text-xs text-ink-mute">No runs yet</p>
                </div>
              ) : (
                <SortableTable<RecentRun>
                  columns={RECENT_RUN_COLUMNS}
                  rows={data.recent_runs}
                  rowKey={(r) => r.id}
                  rowHref={(r) => `/runs/${encodeURIComponent(r.id)}`}
                  initialSort={{ key: 'started', dir: 'desc' }}
                />
              )}
            </div>
            <div className="lg:col-span-2">
              <h2 className="text-sm font-semibold text-ink-dim mb-3">Run status</h2>
              <div className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
                <RunStatusChart byStatus={data.runs.by_status} total={data.runs.total} />
              </div>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}
