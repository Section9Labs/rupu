// Dashboard — stat tiles + run-status donut chart + recent runs list.
//
// Data source: GET /api/dashboard  (DashboardResponse — point-in-time counts
// + last-N recent_runs array). NO time-series or token/cost data is available
// from the backend; this page shows only what the API truthfully exposes.
//
// Polls every 15 s; clears the interval on unmount.

import { useCallback, useEffect, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { Activity, MessageSquare, RefreshCw, Server, ShieldCheck } from 'lucide-react';
import {
  Cell,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
} from 'recharts';
import { api, type DashboardResponse, type RunStatusStr } from '../lib/api';
import { StatusPill } from '../components/StatusPill';
import { ListCard } from '../components/lists/ListCard';
import { cn } from '../lib/cn';
import { relativeTime } from '../lib/time';

// ---------------------------------------------------------------------------
// Status color map — JS color strings for recharts SVG fills.
// These are NOT Tailwind classes; they mirror the RUN_STATUS_STYLES dots.
// ---------------------------------------------------------------------------

const STATUS_FILL: Record<RunStatusStr, string> = {
  running:          '#3b82f6', // blue-500   — matches bg-blue-500 dot
  completed:        '#10b981', // emerald-500 — matches bg-green-500 dot
  failed:           '#ef4444', // red-500     — matches bg-red-500 dot
  awaiting_approval:'#f59e0b', // amber-500   — matches bg-amber-500 dot
  pending:          '#94a3b8', // slate-400   — matches bg-slate-400 dot
  rejected:         '#64748b', // slate-500   — matches bg-red-500 / slate for distinction
};

const STATUS_ORDER: RunStatusStr[] = [
  'running',
  'awaiting_approval',
  'pending',
  'completed',
  'failed',
  'rejected',
];

// Human labels for the chart tooltip (matches StatusPill labels)
const STATUS_LABEL: Record<RunStatusStr, string> = {
  running:           'Running',
  completed:         'Completed',
  failed:            'Failed',
  awaiting_approval: 'Awaiting approval',
  pending:           'Pending',
  rejected:          'Rejected',
};

// ---------------------------------------------------------------------------
// Stat tile
// ---------------------------------------------------------------------------

function StatTile({
  icon: Icon,
  iconCls,
  label,
  value,
  sub,
}: {
  icon: React.ElementType;
  iconCls: string;
  label: string;
  value: number | string;
  sub?: string;
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
// Run-status donut chart
// ---------------------------------------------------------------------------

const tooltipStyle: React.CSSProperties = {
  background: '#fff',
  border: '1px solid #e2e8f0',
  borderRadius: 6,
  fontSize: 11,
  padding: '6px 10px',
  boxShadow: '0 2px 6px rgba(0,0,0,0.06)',
};

function RunStatusChart({ byStatus, total }: { byStatus: Record<RunStatusStr, number>; total: number }) {
  if (total === 0) {
    return (
      <div className="py-12 flex flex-col items-center justify-center text-center">
        <div className="w-10 h-10 rounded-full bg-slate-100 flex items-center justify-center mb-2">
          <Activity size={18} className="text-slate-400" />
        </div>
        <p className="text-xs text-ink-mute">No runs yet</p>
      </div>
    );
  }

  // Build pie slices — skip zero buckets so the chart stays clean.
  const data = STATUS_ORDER
    .map((s) => ({ name: STATUS_LABEL[s], value: byStatus[s] ?? 0, fill: STATUS_FILL[s], key: s }))
    .filter((d) => d.value > 0);

  return (
    <div className="flex items-center gap-6">
      {/* Donut — needs an explicit height so ResponsiveContainer doesn't collapse */}
      <div style={{ width: 140, height: 140, flexShrink: 0 }}>
        <ResponsiveContainer width="100%" height="100%">
          <PieChart>
            <Pie
              data={data}
              cx="50%"
              cy="50%"
              innerRadius={42}
              outerRadius={62}
              paddingAngle={2}
              dataKey="value"
              strokeWidth={0}
            >
              {data.map((d) => (
                <Cell key={d.key} fill={d.fill} />
              ))}
            </Pie>
            <Tooltip
              contentStyle={tooltipStyle}
              formatter={(value, name) => [`${value ?? 0}`, `${name}`]}
            />
          </PieChart>
        </ResponsiveContainer>
      </div>

      {/* Legend */}
      <ul className="flex-1 min-w-0 space-y-1.5">
        {data.map((d) => (
          <li key={d.key} className="flex items-center gap-2 text-xs">
            <span
              className="w-2.5 h-2.5 rounded-sm shrink-0"
              style={{ background: d.fill }}
            />
            <span className="text-ink-dim flex-1 min-w-0 truncate">{d.name}</span>
            <span className="text-ink font-medium tabular-nums">{d.value}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Recent runs
// ---------------------------------------------------------------------------

function RecentRunRow({
  run,
}: {
  run: DashboardResponse['recent_runs'][number];
}) {
  return (
    <Link
      to={`/runs/${encodeURIComponent(run.id)}`}
      className="flex items-center gap-4 px-4 py-3 hover:bg-slate-50 transition-colors"
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-ink truncate">{run.workflow_name}</span>
          <span className="text-[11px] text-ink-mute font-mono">
            {run.id.length > 10 ? `${run.id.slice(0, 8)}…` : run.id}
          </span>
        </div>
        <p className="text-[11px] text-ink-dim mt-0.5">
          started {relativeTime(run.started_at)}
        </p>
      </div>
      <StatusPill status={run.status} />
    </Link>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

const POLL_MS = 15_000;

export default function Dashboard() {
  const [data, setData] = useState<DashboardResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);

  // Stable "seconds since last refresh" counter.
  const [ageSec, setAgeSec] = useState(0);
  const lastUpdatedRef = useRef<Date | null>(null);

  const load = useCallback(async () => {
    setRefreshing(true);
    try {
      const d = await api.getDashboard();
      setData(d);
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
  }, []);

  // Initial fetch + polling
  useEffect(() => {
    void load();
    const poll = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(poll);
  }, [load]);

  // "X s ago" ticker — updates every second while data is loaded.
  useEffect(() => {
    if (!lastUpdated) return;
    const ticker = window.setInterval(() => {
      const ref = lastUpdatedRef.current;
      if (ref) setAgeSec(Math.round((Date.now() - ref.getTime()) / 1000));
    }, 1000);
    return () => window.clearInterval(ticker);
  }, [lastUpdated]);

  const running = data?.runs.by_status.running ?? 0;
  const activeSessions = data?.sessions.active ?? 0;
  const totalSessions = data?.sessions.total ?? 0;
  const totalWorkers = data?.workers.total ?? 0;
  const coverageTargets = data?.coverage.targets ?? 0;
  const coverageAssertions = data?.coverage.assertions ?? 0;

  return (
    <div className="p-8 max-w-5xl">
      {/* Page header */}
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Dashboard</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Control plane at a glance.
          </p>
        </div>
        <div className="flex items-center gap-3">
          {lastUpdated && !refreshing && (
            <span className="text-[11px] text-ink-mute tabular-nums">
              updated {ageSec < 5 ? 'just now' : `${ageSec}s ago`}
            </span>
          )}
          <button
            onClick={() => void load()}
            className="inline-flex items-center gap-1.5 text-xs font-medium px-3 py-1.5 rounded-md border border-border bg-panel text-ink hover:bg-slate-100 transition-colors"
          >
            <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
            Refresh
          </button>
        </div>
      </header>

      {/* Error banner */}
      {error && (
        <div className="mb-5 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {/* Loading skeleton */}
      {data === null && !error && (
        <div className="text-sm text-ink-dim">Loading…</div>
      )}

      {data !== null && (
        <div className="space-y-8">
          {/* ── Stat tiles ── */}
          <section className="grid grid-cols-2 gap-4 sm:grid-cols-4">
            <StatTile
              icon={Activity}
              iconCls="bg-blue-50 text-blue-600"
              label="Total runs"
              value={data.runs.total}
              sub={running > 0 ? `${running} running` : undefined}
            />
            <StatTile
              icon={MessageSquare}
              iconCls="bg-brand-50 text-brand-600"
              label="Sessions"
              value={activeSessions}
              sub={`${activeSessions} of ${totalSessions} active`}
            />
            <StatTile
              icon={Server}
              iconCls="bg-slate-100 text-slate-600"
              label="Workers"
              value={totalWorkers}
            />
            <StatTile
              icon={ShieldCheck}
              iconCls="bg-green-50 text-green-600"
              label="Coverage targets"
              value={coverageTargets}
              sub={coverageAssertions > 0 ? `${coverageAssertions} assertions` : undefined}
            />
          </section>

          {/* ── Run status distribution ── */}
          <section>
            <h2 className="text-sm font-semibold text-ink-dim mb-3">Run status distribution</h2>
            <div className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
              <RunStatusChart
                byStatus={data.runs.by_status}
                total={data.runs.total}
              />
            </div>
          </section>

          {/* ── Recent runs ── */}
          <section>
            <div className="flex items-center gap-2 mb-3 pl-1">
              <span className="w-2 h-2 rounded-full bg-brand-500" />
              <h2 className="text-sm font-semibold text-brand-700">Recent Runs</h2>
              <span className="text-xs text-ink-mute tabular-nums">
                {data.recent_runs.length}
              </span>
            </div>

            {data.recent_runs.length === 0 ? (
              <div className="rounded-xl border border-dashed border-border bg-panel/50 py-10 flex flex-col items-center justify-center text-center">
                <div className="w-10 h-10 rounded-full bg-slate-100 flex items-center justify-center mb-2">
                  <Activity size={17} className="text-slate-400" />
                </div>
                <p className="text-xs text-ink-mute">No runs yet</p>
              </div>
            ) : (
              <ListCard>
                {data.recent_runs.map((r) => (
                  <RecentRunRow key={r.id} run={r} />
                ))}
              </ListCard>
            )}
          </section>
        </div>
      )}
    </div>
  );
}
