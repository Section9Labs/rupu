// Autoflow run-stream page — execution history for autoflow cycles.
// Each row shows cycle metadata and links individual run IDs to their graphs.

import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Inbox, RefreshCw } from 'lucide-react';
import { api, type AutoflowCycleRow } from '../../lib/api';
import { ListCard } from '../../components/lists/ListCard';
import { SectionHeader } from '../../components/lists/SectionHeader';
import { cn } from '../../lib/cn';
import { durationBetween, relativeTime } from '../../lib/time';

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

const MODE_CLS: Record<string, string> = {
  ask:       'bg-amber-50 text-amber-700 ring-amber-200',
  bypass:    'bg-green-50 text-green-700 ring-green-200',
  readonly:  'bg-slate-100 text-slate-600 ring-slate-200',
};

function ModeChip({ mode }: { mode: string }) {
  const cls = MODE_CLS[mode] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {mode}
    </span>
  );
}

export default function AutoflowRuns() {
  const [cycles, setCycles] = useState<AutoflowCycleRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const load = useCallback(async () => {
    setRefreshing(true);
    try {
      const data = await api.getAutoflowRuns();
      setCycles(data);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load autoflow cycles');
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void load();
    const t = window.setInterval(() => void load(), 5000);
    return () => window.clearInterval(t);
  }, [load]);

  // Separate in-progress (no finished_at or very recent) from done cycles.
  // AutoflowCycleRow always has finished_at, so we sort newest-first.
  const sorted = [...(cycles ?? [])].sort(
    (a, b) => Date.parse(b.started_at) - Date.parse(a.started_at),
  );

  return (
    <div className="p-8 max-w-5xl">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Autoflow Cycles</h1>
          <p className="mt-1 text-sm text-ink-dim">Autoflow scheduling cycles across this control plane.</p>
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

      {cycles === null ? (
        <div className="text-sm text-ink-dim">Loading autoflow cycles…</div>
      ) : cycles.length === 0 ? (
        <AutoflowRunsEmpty />
      ) : (
        <section>
          <SectionHeader tone="muted" label="Cycles" count={sorted.length} />
          <ListCard>
            {sorted.map((c) => (
              <AutoflowCycleRow key={c.cycle_id} cycle={c} />
            ))}
          </ListCard>
        </section>
      )}
    </div>
  );
}

function AutoflowCycleRow({ cycle }: { cycle: AutoflowCycleRow }) {
  const hasFailed = cycle.failed_cycles > 0;
  return (
    <div className="flex items-start gap-4 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-sm font-medium text-ink font-mono">{shortId(cycle.cycle_id)}</span>
          <ModeChip mode={cycle.mode} />
          {cycle.worker_name && (
            <span className="text-[11px] text-ink-mute">{cycle.worker_name}</span>
          )}
        </div>
        <div className="text-[11px] text-ink-dim mt-0.5">
          started {relativeTime(cycle.started_at)}
          {' · '}
          {durationBetween(cycle.started_at, cycle.finished_at)}
        </div>
        <div className={cn('text-[11px] mt-1', hasFailed ? 'text-red-600' : 'text-ink-dim')}>
          ran {cycle.ran_cycles}
          {' · '}
          skipped {cycle.skipped_cycles}
          {hasFailed && (
            <span className="text-red-600"> · failed {cycle.failed_cycles}</span>
          )}
          {' '}
          of {cycle.workflow_count}
        </div>
        {cycle.run_ids.length > 0 && (
          <div className="flex items-center gap-1.5 flex-wrap mt-1.5">
            <span className="text-[10px] text-ink-mute uppercase tracking-wide">runs:</span>
            {cycle.run_ids.map((rid) => (
              <Link
                key={rid}
                to={`/runs/${encodeURIComponent(rid)}`}
                className="text-[11px] font-mono text-brand-600 hover:underline"
              >
                {shortId(rid)}
              </Link>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function AutoflowRunsEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No autoflow cycles yet</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Autoflow scheduling cycles will appear here once the autoflow worker runs.
      </p>
    </div>
  );
}
