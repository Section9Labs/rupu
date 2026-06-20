// Agents library — read-only list of agent files discovered by the control
// plane. Each row links to /agents/:name for the full system prompt.

import { useEffect, useState } from 'react';
import { Sparkles } from 'lucide-react';
import { api, type AgentSummary } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader } from '../components/lists/SectionHeader';
import MetricRow from '../components/lists/MetricRow';
import UsageBarChart from '../components/charts/UsageBarChart';
import { formatTokens, formatCost } from '../lib/usage';
import { cn } from '../lib/cn';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;

export default function Agents() {
  const [agents, setAgents] = useState<AgentSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState(STEP);

  useEffect(() => {
    let cancelled = false;
    api
      .getAgents()
      .then((data) => {
        if (cancelled) return;
        setAgents(data);
        setVisible(STEP);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load agents');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const sorted = (agents ?? []).slice().sort((a, b) => a.name.localeCompare(b.name));
  const shown = sorted.slice(0, visible);
  const { sentinelRef } = useInfiniteScroll({
    hasMore: visible < sorted.length,
    loadMore: () => setVisible((v) => v + STEP),
  });

  return (
    <div className="p-8 max-w-5xl">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Agents</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Agent files discovered across this control plane — provider, model, and the system prompt
          each one runs with.
        </p>
      </header>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {agents === null ? (
        <div className="text-sm text-ink-dim">Loading agents…</div>
      ) : sorted.length === 0 ? (
        <EmptyState />
      ) : (
        <section>
          <div className="mb-4 rounded-xl border border-border bg-panel/50 p-4">
            <UsageBarChart
              bars={sorted.map((a) => ({
                id: a.name,
                label: a.name,
                input_tokens: a.usage?.input_tokens ?? 0,
                output_tokens: a.usage?.output_tokens ?? 0,
                cached_tokens: a.usage?.cached_tokens ?? 0,
                cost_usd: a.usage?.cost_usd ?? null,
                to: `/agents/${encodeURIComponent(a.name)}`,
              }))}
            />
          </div>
          <SectionHeader tone="muted" label="Agents" count={sorted.length} />
          <ListCard>
            {shown.map((a) => (
              <AgentRow key={a.name} agent={a} />
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

function AgentRow({ agent: a }: { agent: AgentSummary }) {
  return (
    <MetricRow
      to={`/agents/${encodeURIComponent(a.name)}`}
      header={
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-medium text-ink truncate">{a.name}</span>
            {a.provider && <MetaChip>{a.provider}</MetaChip>}
            {a.model && <MetaChip>{a.model}</MetaChip>}
            {a.effort && <MetaChip>effort: {a.effort}</MetaChip>}
          </div>
          {a.description && (
            <p className="mt-1 text-[12px] text-ink-dim leading-snug line-clamp-2">
              {a.description}
            </p>
          )}
        </div>
      }
      metrics={[
        { label: 'runs', value: a.run_count ? String(a.run_count) : null },
        { label: 'tokens', value: a.usage ? formatTokens(a.usage.total_tokens) : null },
        { label: 'cost', value: a.usage ? formatCost(a.usage.cost_usd) : null },
      ]}
    />
  );
}

function MetaChip({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200',
        className,
      )}
    >
      {children}
    </span>
  );
}

function EmptyState() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Sparkles size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No agents found</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Add agent files under <span className="font-mono">.rupu/agents/</span> to populate this
        library.
      </p>
    </div>
  );
}
