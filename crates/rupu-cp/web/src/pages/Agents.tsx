// Agents library — read-only list of agent files discovered by the control
// plane. Each row links to /agents/:name for the full system prompt.

import { useEffect, useId, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Plus, Sparkles } from 'lucide-react';
import { api, type AgentSummary } from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import UsageBarChart from '../components/charts/UsageBarChart';
import CodeEditor from '../components/CodeEditor';
import { formatTokens, formatCost } from '../lib/usage';
import { cn } from '../lib/cn';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;

const NEW_AGENT_TEMPLATE = `---
name: my-agent
description: A short description.
provider: anthropic
model: claude-sonnet-4-6
---

You are a helpful agent. ...
`;

export default function Agents() {
  const [agents, setAgents] = useState<AgentSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState(STEP);
  const [createOpen, setCreateOpen] = useState(false);

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
    <div className="p-8">
      <header className="mb-6 flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Agents</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Agent files discovered across this control plane — provider, model, and the system prompt
            each one runs with.
          </p>
        </div>
        <button
          type="button"
          onClick={() => setCreateOpen(true)}
          className="inline-flex shrink-0 items-center gap-1.5 rounded-md bg-brand-600 px-3 py-1.5 text-[12px] font-medium text-white hover:bg-brand-700"
        >
          <Plus size={14} />
          New agent
        </button>
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
          <SortableTable<AgentSummary>
            columns={AGENT_COLUMNS}
            rows={shown}
            rowKey={(a) => a.name}
            rowHref={(a) => `/agents/${encodeURIComponent(a.name)}`}
            initialSort={{ key: 'name', dir: 'asc' }}
          />
          {sorted.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
              scroll for more
            </div>
          )}
        </section>
      )}

      {createOpen && <NewAgentModal onClose={() => setCreateOpen(false)} />}
    </div>
  );
}

function NewAgentModal({ onClose }: { onClose: () => void }) {
  const navigate = useNavigate();
  const titleId = useId();
  const [raw, setRaw] = useState(NEW_AGENT_TEMPLATE);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);

  async function create() {
    if (creating) return;
    setCreating(true);
    setError(null);
    try {
      const created = await api.createAgent(raw);
      navigate(`/agents/${encodeURIComponent(created.name)}`);
      onClose();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Failed to create agent');
      setCreating(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/40 p-4 pt-[8vh]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="w-full max-w-2xl rounded-xl border border-border bg-panel shadow-card"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="border-b border-border px-5 py-4">
          <h2 id={titleId} className="text-base font-semibold text-ink">
            New agent
          </h2>
          <p className="mt-1 text-[12px] text-ink-dim">
            Edit the definition below. It is validated server-side before it is saved.
          </p>
        </div>

        <div className="space-y-3 px-5 py-4">
          <CodeEditor
            value={raw}
            onChange={setRaw}
            language="markdown"
            ariaLabel="New agent definition"
          />
          {error && (
            <p role="alert" className="text-[12px] font-medium text-red-700">
              {error}
            </p>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-border px-5 py-3">
          <button
            type="button"
            onClick={onClose}
            disabled={creating}
            className="inline-flex items-center rounded-md border border-border bg-white px-3 py-1.5 text-[12px] font-medium text-ink-dim hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-60"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={create}
            disabled={creating}
            className="inline-flex items-center rounded-md bg-brand-600 px-3 py-1.5 text-[12px] font-medium text-white hover:bg-brand-700 disabled:cursor-not-allowed disabled:opacity-60"
          >
            {creating ? 'Creating…' : 'Create'}
          </button>
        </div>
      </div>
    </div>
  );
}

const AGENT_COLUMNS: Column<AgentSummary>[] = [
  {
    key: 'name',
    header: 'Name',
    sortable: true,
    sortValue: (a) => a.name,
    render: (a) => (
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-sm font-medium text-ink truncate">{a.name}</span>
        {a.provider && <MetaChip>{a.provider}</MetaChip>}
        {a.model && <MetaChip>{a.model}</MetaChip>}
        {a.effort && <MetaChip>effort: {a.effort}</MetaChip>}
      </div>
    ),
  },
  {
    key: 'description',
    header: 'Description',
    render: (a) =>
      a.description ? (
        <span className="text-[12px] text-ink-dim leading-snug truncate block max-w-md">
          {a.description}
        </span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'runs',
    header: 'Runs',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (a) => a.run_count,
    render: (a) => <span className="text-ink">{a.run_count ? String(a.run_count) : '—'}</span>,
  },
  {
    key: 'tokens',
    header: 'Tokens',
    align: 'right',
    width: 'w-24',
    sortable: true,
    sortValue: (a) => a.usage?.total_tokens ?? null,
    render: (a) => (
      <span className="text-ink-dim">{a.usage ? formatTokens(a.usage.total_tokens) : '—'}</span>
    ),
  },
  {
    key: 'cost',
    header: 'Cost',
    align: 'right',
    width: 'w-24',
    sortable: true,
    sortValue: (a) => a.usage?.cost_usd ?? null,
    render: (a) => (
      <span className="text-ink font-medium">{a.usage ? formatCost(a.usage.cost_usd) : '—'}</span>
    ),
  },
];

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
