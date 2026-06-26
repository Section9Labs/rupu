// Build › Autoflows — autoflow-enabled workflow definitions.
// Shows the static list of workflows that have autoflow triggers configured.

import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Inbox, RefreshCw } from 'lucide-react';
import { api, type AutoflowDefRow } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader } from '../components/lists/SectionHeader';
import { cn } from '../lib/cn';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;

const TRIGGER_CLS: Record<string, string> = {
  cron:  'bg-violet-50 text-violet-700 ring-violet-200',
  event: 'bg-sky-50 text-sky-700 ring-sky-200',
};

function TriggerChip({ trigger }: { trigger: string }) {
  const cls = TRIGGER_CLS[trigger] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {trigger}
    </span>
  );
}

const SCOPE_CLS: Record<string, string> = {
  workspace:  'bg-slate-100 text-slate-600 ring-slate-200',
  repository: 'bg-emerald-50 text-emerald-700 ring-emerald-200',
  global:     'bg-indigo-50 text-indigo-700 ring-indigo-200',
};

function ScopeChip({ scope }: { scope: string }) {
  const cls = SCOPE_CLS[scope] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-[10px] font-medium px-1.5 py-0.5', cls)}>
      {scope}
    </span>
  );
}

export default function AutoflowsDefs() {
  const [defs, setDefs] = useState<AutoflowDefRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [visible, setVisible] = useState(STEP);

  const load = useCallback(async () => {
    setRefreshing(true);
    try {
      const data = await api.getAutoflowDefs();
      setDefs(data);
      setVisible(STEP);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load autoflow definitions');
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const shown = (defs ?? []).slice(0, visible);
  const { sentinelRef } = useInfiniteScroll({
    hasMore: visible < (defs?.length ?? 0),
    loadMore: () => setVisible((v) => v + STEP),
  });

  return (
    <div className="p-8 max-w-5xl">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Autoflows</h1>
          <p className="mt-1 text-sm text-ink-dim">Workflows with autoflow triggers configured.</p>
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

      {defs === null ? (
        <div className="text-sm text-ink-dim">Loading autoflow definitions…</div>
      ) : defs.length === 0 ? (
        <AutoflowsEmpty />
      ) : (
        <section>
          <SectionHeader tone="muted" label="Autoflow Workflows" count={defs.length} />
          <ListCard>
            {shown.map((d) => (
              <AutoflowDefEntry key={d.name} def={d} />
            ))}
          </ListCard>
          {defs.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
              scroll for more
            </div>
          )}
        </section>
      )}
    </div>
  );
}

function AutoflowDefEntry({ def }: { def: AutoflowDefRow }) {
  // Autoflows are workflows with `autoflow.enabled`, so they reuse the workflow
  // detail page — keyed by file stem (`slug`), not the parsed display name.
  return (
    <Link
      to={`/workflows/${encodeURIComponent(def.slug)}`}
      className="flex items-center gap-4 px-4 py-3 hover:bg-slate-50"
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-sm font-medium text-ink">{def.name}</span>
          <TriggerChip trigger={def.trigger} />
          <ScopeChip scope={def.scope} />
        </div>
      </div>
    </Link>
  );
}

function AutoflowsEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No autoflow-enabled workflows</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Workflows with autoflow triggers configured will appear here.
      </p>
    </div>
  );
}
