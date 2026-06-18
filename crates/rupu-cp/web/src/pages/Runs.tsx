// Runs list — grouped by lifecycle (Active / Completed / Failed-Rejected),
// each row a ListCard row linking to the live Run detail view. Polls every 5s
// plus a manual refresh button. The browser twin of rupu's live TUI run list.

import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Inbox, RefreshCw } from 'lucide-react';
import { api, type RunRecord, type RunStatusStr } from '../lib/api';
import { StatusPill } from '../components/StatusPill';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader, type SectionTone } from '../components/lists/SectionHeader';
import { cn } from '../lib/cn';
import { durationBetween, relativeTime } from '../lib/time';

const ACTIVE: RunStatusStr[] = ['running', 'pending', 'awaiting_approval'];
const TERMINAL_OK: RunStatusStr[] = ['completed'];
const TERMINAL_BAD: RunStatusStr[] = ['failed', 'rejected'];

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

export default function Runs() {
  const [runs, setRuns] = useState<RunRecord[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const load = useCallback(async () => {
    setRefreshing(true);
    try {
      const data = await api.getRuns();
      setRuns(data);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load runs');
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void load();
    const t = window.setInterval(() => void load(), 5000);
    return () => window.clearInterval(t);
  }, [load]);

  const active = (runs ?? []).filter((r) => ACTIVE.includes(r.status));
  const done = (runs ?? []).filter((r) => TERMINAL_OK.includes(r.status));
  const bad = (runs ?? []).filter((r) => TERMINAL_BAD.includes(r.status));

  return (
    <div className="p-8 max-w-5xl">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Runs</h1>
          <p className="mt-1 text-sm text-ink-dim">Workflow runs across this control plane.</p>
        </div>
        <button
          onClick={() => void load()}
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

      {runs === null ? (
        <div className="text-sm text-ink-dim">Loading runs…</div>
      ) : runs.length === 0 ? (
        <EmptyState />
      ) : (
        <div className="space-y-6">
          <RunSection tone="progress" label="Active" runs={active} />
          <RunSection tone="good" label="Completed" runs={done} />
          <RunSection tone="bad" label="Failed / Rejected" runs={bad} />
        </div>
      )}
    </div>
  );
}

function RunSection({
  tone,
  label,
  runs,
}: {
  tone: SectionTone;
  label: string;
  runs: RunRecord[];
}) {
  if (runs.length === 0) return null;
  return (
    <section>
      <SectionHeader tone={tone} label={label} count={runs.length} />
      <ListCard>
        {runs.map((r) => (
          <RunRow key={r.id} run={r} />
        ))}
      </ListCard>
    </section>
  );
}

function RunRow({ run }: { run: RunRecord }) {
  return (
    <Link
      to={`/runs/${encodeURIComponent(run.id)}`}
      className="flex items-center gap-4 px-4 py-3 hover:bg-slate-50 transition-colors"
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-ink truncate">{run.workflow_name}</span>
          <span className="text-[11px] text-ink-mute font-mono">{shortId(run.id)}</span>
        </div>
        <div className="text-[11px] text-ink-dim mt-0.5">
          started {relativeTime(run.started_at)}
          {' · '}
          {durationBetween(run.started_at, run.finished_at)}
        </div>
      </div>
      <StatusPill status={run.status} />
    </Link>
  );
}

function EmptyState() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No runs yet</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Workflow runs will appear here once you dispatch one from the CLI, the desktop app, or a
        scheduled trigger.
      </p>
    </div>
  );
}
