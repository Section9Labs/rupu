// Workflows library — read-only list of workflow definitions discovered by the
// control plane. Each row links to /workflows/:name for the steps + raw YAML.

import { useEffect, useState } from 'react';
import { Workflow as WorkflowIcon } from 'lucide-react';
import { api, type WorkflowSummary } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader } from '../components/lists/SectionHeader';
import MetricRow from '../components/lists/MetricRow';
import UsageBarChart from '../components/charts/UsageBarChart';
import { formatTokens, formatCost } from '../lib/usage';
import { relativeTime } from '../lib/time';
import { cn } from '../lib/cn';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;

export default function Workflows() {
  const [workflows, setWorkflows] = useState<WorkflowSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState(STEP);

  useEffect(() => {
    let cancelled = false;
    api
      .getWorkflows()
      .then((data) => {
        if (cancelled) return;
        setWorkflows(data);
        setVisible(STEP);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load workflows');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const sorted = (workflows ?? []).slice().sort((a, b) => a.name.localeCompare(b.name));
  const shown = sorted.slice(0, visible);
  const { sentinelRef } = useInfiniteScroll({
    hasMore: visible < sorted.length,
    loadMore: () => setVisible((v) => v + STEP),
  });

  return (
    <div className="p-8 max-w-5xl">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Workflows</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Workflow definitions discovered across this control plane — open one to inspect its steps
          and raw YAML.
        </p>
      </header>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {workflows === null ? (
        <div className="text-sm text-ink-dim">Loading workflows…</div>
      ) : sorted.length === 0 ? (
        <EmptyState />
      ) : (
        <section>
          <div className="mb-4 rounded-xl border border-border bg-panel/50 p-4">
            <UsageBarChart
              bars={sorted.map((w) => ({
                id: `${w.scope}:${w.name}`,
                label: w.name,
                input_tokens: w.usage?.input_tokens ?? 0,
                output_tokens: w.usage?.output_tokens ?? 0,
                cached_tokens: w.usage?.cached_tokens ?? 0,
                cost_usd: w.usage?.cost_usd ?? null,
                to: `/workflows/${encodeURIComponent(w.name)}`,
              }))}
            />
          </div>
          <SectionHeader tone="muted" label="Workflows" count={sorted.length} />
          <ListCard>
            {shown.map((w) => (
              <WorkflowRow key={`${w.scope}:${w.name}`} workflow={w} />
            ))}
          </ListCard>
          {sorted.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
              scroll for more
            </div>
          )}
        </section>
      )}
    </div>
  );
}

function WorkflowRow({ workflow: w }: { workflow: WorkflowSummary }) {
  return (
    <MetricRow
      to={`/workflows/${encodeURIComponent(w.name)}`}
      header={
        <>
          <span className="text-sm font-medium text-ink truncate">{w.name}</span>
          <ScopeChip scope={w.scope} />
        </>
      }
      trailing={
        w.last_run ? (
          <span className="shrink-0 text-[11px] text-ink-mute tabular-nums">
            {relativeTime(w.last_run)}
          </span>
        ) : undefined
      }
      metrics={[
        { label: 'runs', value: w.run_count ? String(w.run_count) : null },
        { label: 'tokens', value: w.usage ? formatTokens(w.usage.total_tokens) : null },
        { label: 'cost', value: w.usage ? formatCost(w.usage.cost_usd) : null },
      ]}
    />
  );
}

export function ScopeChip({ scope }: { scope: string }) {
  const isGlobal = scope.toLowerCase() === 'global';
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-2 py-0.5 text-[11px] font-medium ring-1',
        isGlobal
          ? 'bg-violet-50 text-violet-700 ring-violet-200'
          : 'bg-slate-100 text-ink-mute ring-slate-200',
      )}
    >
      {scope}
    </span>
  );
}

function EmptyState() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <WorkflowIcon size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No workflows found</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Add workflow YAML under <span className="font-mono">.rupu/workflows/</span> to populate this
        library.
      </p>
    </div>
  );
}
