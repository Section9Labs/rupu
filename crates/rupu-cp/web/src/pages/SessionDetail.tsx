// Session detail — identity fields + turn-runs list (sessions-as-containers).
// The turn-runs are sourced by filtering getAgentRuns() by session_id client-side;
// no separate session-runs endpoint is needed (resolved open decision).
// Route: /sessions/:id

import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft, RefreshCw } from 'lucide-react';
import { api, type SessionSummary, type AgentRunRow, type UsageTimelinePoint } from '../lib/api';
import { cn } from '../lib/cn';
import { absoluteTime, relativeTime } from '../lib/time';
import { sessionStatusDot, sessionStatusLabel } from '../lib/sessionStatus';
import UsageChip from '../components/UsageChip';
import RunUsageTimeline from '../components/charts/RunUsageTimeline';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

function isRunning(status: string | null | undefined): boolean {
  return status === 'running' || status === 'awaiting_approval';
}

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

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function SessionDetailPage() {
  const { id = '' } = useParams<{ id: string }>();

  const [session, setSession] = useState<SessionSummary | null>(null);
  const [sessionError, setSessionError] = useState<string | null>(null);

  const [turnRuns, setTurnRuns] = useState<AgentRunRow[] | null>(null);
  const [runsError, setRunsError] = useState<string | null>(null);
  const [runsRefreshing, setRunsRefreshing] = useState(false);

  // Aggregated per-turn token series across this session's runs (no step
  // boundaries → no separators).
  const [series, setSeries] = useState<UsageTimelinePoint[]>([]);

  // Fetch session identity
  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setSession(null);
    setSessionError(null);
    api
      .getSession(id)
      .then((data) => {
        if (cancelled) return;
        setSession(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setSessionError(e instanceof Error ? e.message : 'Failed to load session');
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  // Aggregated per-turn usage timeline for this session.
  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setSeries([]);
    api
      .getSessionUsageTimeline(id)
      .then((pts) => {
        if (cancelled) return;
        setSeries(pts);
      })
      .catch(() => {
        if (cancelled) return;
        setSeries([]);
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  // Fetch + filter agent runs for this session
  const loadTurnRuns = () => {
    if (!id) return;
    setRunsRefreshing(true);
    api
      .getAgentRuns()
      .then((all) => {
        setTurnRuns(all.filter((r) => r.session_id === id));
        setRunsError(null);
      })
      .catch((e: unknown) => {
        setRunsError(e instanceof Error ? e.message : 'Failed to load turn runs');
      })
      .finally(() => setRunsRefreshing(false));
  };

  useEffect(() => {
    loadTurnRuns();
    // Poll every 5 s so live runs surface quickly
    const t = window.setInterval(loadTurnRuns, 5000);
    return () => window.clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  if (sessionError) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {sessionError}
        </div>
      </div>
    );
  }

  if (!session) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 text-sm text-ink-dim">Loading…</div>
      </div>
    );
  }

  // Sort turn-runs newest-first
  const sortedTurns = [...(turnRuns ?? [])].sort((a, b) => {
    if (!a.started_at && !b.started_at) return 0;
    if (!a.started_at) return 1;
    if (!b.started_at) return -1;
    return Date.parse(b.started_at) - Date.parse(a.started_at);
  });

  return (
    <div className="p-8 max-w-5xl">
      <BackLink />

      {/* Session identity header */}
      <header className="mt-3">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-2xl font-semibold text-ink break-all font-mono">
            {session.session_id}
          </h1>
          <span className="inline-flex items-center gap-1.5">
            <span
              className={cn('inline-block w-2 h-2 rounded-full', sessionStatusDot(session.status))}
            />
            <span className="text-[12px] text-ink-dim">{sessionStatusLabel(session.status)}</span>
          </span>
          {session.usage && <UsageChip usage={session.usage} className="ml-2" />}
        </div>
      </header>

      {/* Aggregated per-turn token usage across this session's runs */}
      <section className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mt-3">
        <h2 className="text-xs font-semibold text-ink-dim uppercase tracking-wide mb-2">
          Token usage by turn
        </h2>
        <RunUsageTimeline series={series} />
      </section>

      {/* Identity fields */}
      <section className="mt-6">
        <dl className="bg-panel border border-border rounded-xl shadow-card divide-y divide-border overflow-hidden">
          <Field label="Agent" value={session.agent_name} mono />
          <Field label="Model" value={session.model} mono />
          <Field label="Scope" value={session.scope} />
          <Field label="Status" value={sessionStatusLabel(session.status)} />
          <Field label="Total turns" value={String(session.total_turns)} />
          <Field label="Target" value={session.target ?? '—'} mono={!!session.target} />
          <Field
            label="Created"
            value={`${absoluteTime(session.created_at)} (${relativeTime(session.created_at)})`}
          />
          <Field
            label="Updated"
            value={`${absoluteTime(session.updated_at)} (${relativeTime(session.updated_at)})`}
          />
          <div className="flex items-baseline gap-4 px-4 py-3">
            <dt className="w-32 shrink-0 text-[12px] font-medium text-ink-mute">Active run</dt>
            <dd className="min-w-0 flex-1 text-sm text-ink">
              {session.active_run_id ? (
                <Link
                  to={`/runs/${encodeURIComponent(session.active_run_id)}`}
                  className="inline-flex items-center rounded px-2 py-0.5 text-[12px] font-medium ring-1 bg-blue-50 text-blue-700 ring-blue-200 hover:bg-blue-100 font-mono"
                >
                  {session.active_run_id}
                </Link>
              ) : (
                <span className="text-ink-dim">—</span>
              )}
            </dd>
          </div>
        </dl>
      </section>

      {/* Turn-runs: session as container */}
      <section className="mt-8">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-base font-semibold text-ink">
            Turn Runs
            {turnRuns !== null && (
              <span className="ml-2 text-sm font-normal text-ink-dim">({sortedTurns.length})</span>
            )}
          </h2>
          <button
            type="button"
            onClick={loadTurnRuns}
            className="inline-flex items-center gap-1.5 text-xs font-medium px-3 py-1.5 rounded-md border border-border bg-panel text-ink hover:bg-slate-100"
          >
            <RefreshCw size={12} className={cn(runsRefreshing && 'animate-spin')} />
            Refresh
          </button>
        </div>

        {runsError && (
          <div className="mb-3 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
            {runsError}
          </div>
        )}

        {turnRuns === null ? (
          <div className="text-sm text-ink-dim">Loading turn runs…</div>
        ) : sortedTurns.length === 0 ? (
          <div className="rounded-xl border border-dashed border-border bg-panel/50 py-10 flex flex-col items-center justify-center text-center">
            <p className="text-sm text-ink-dim">No turn runs recorded for this session yet.</p>
          </div>
        ) : (
          <div className="rounded-xl border border-border bg-panel shadow-card divide-y divide-border overflow-hidden">
            {sortedTurns.map((run) => (
              <TurnRunRow key={run.run_id} run={run} />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Turn run row
// ---------------------------------------------------------------------------

function TurnRunRow({ run }: { run: AgentRunRow }) {
  const live = isRunning(run.status);
  const transcriptHref = run.transcript_path
    ? `/transcript?path=${encodeURIComponent(run.transcript_path)}&live=${live ? 1 : 0}`
    : null;

  const inner = (
    <div className="flex items-start gap-4 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-sm font-medium text-ink font-mono">{shortId(run.run_id)}</span>
          {run.agent && (
            <span className="text-[11px] text-ink-dim">{run.agent}</span>
          )}
          {run.status && <StatusBadge status={run.status} />}
          {transcriptHref && (
            <span className="ml-auto text-[10px] text-brand-600 font-medium">View transcript →</span>
          )}
        </div>

        <div className="text-[11px] text-ink-dim mt-0.5 flex items-center gap-3 flex-wrap">
          {run.started_at ? (
            <span>started {relativeTime(run.started_at)}</span>
          ) : (
            <span className="text-ink-mute">no timing</span>
          )}
          {run.trigger_source && (
            <span>via <span className="font-mono">{run.trigger_source}</span></span>
          )}
        </div>

        {run.transcript_path && (
          <div className="mt-1 text-[10px] text-ink-mute font-mono truncate" title={run.transcript_path}>
            {run.transcript_path}
          </div>
        )}
      </div>
    </div>
  );

  if (transcriptHref) {
    return (
      <Link
        to={transcriptHref}
        className="block hover:bg-slate-50 transition-colors"
      >
        {inner}
      </Link>
    );
  }

  return <div>{inner}</div>;
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

function Field({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-baseline gap-4 px-4 py-3">
      <dt className="w-32 shrink-0 text-[12px] font-medium text-ink-mute">{label}</dt>
      <dd className={cn('min-w-0 flex-1 text-sm text-ink break-all', mono && 'font-mono')}>
        {value}
      </dd>
    </div>
  );
}

function BackLink() {
  return (
    <Link
      to="/sessions"
      className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
    >
      <ArrowLeft size={14} />
      Sessions
    </Link>
  );
}
