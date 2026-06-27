// Workflows library — read-only list of workflow definitions discovered by the
// control plane. Each row links to /workflows/:name for the steps + raw YAML.

import { useEffect, useId, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Plus, Workflow as WorkflowIcon } from 'lucide-react';
import { api, type WorkflowSummary } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader } from '../components/lists/SectionHeader';
import MetricRow from '../components/lists/MetricRow';
import LauncherSheet from '../components/LauncherSheet';
import CodeEditor from '../components/CodeEditor';
import UsageBarChart from '../components/charts/UsageBarChart';
import { formatTokens, formatCost } from '../lib/usage';
import { relativeTime } from '../lib/time';
import { cn } from '../lib/cn';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;

const NEW_WORKFLOW_TEMPLATE = `name: my-workflow
description: ...
steps:
  - id: step-one
    agent: my-agent
    prompt: Describe the task here
`;

export default function Workflows() {
  const [workflows, setWorkflows] = useState<WorkflowSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState(STEP);
  // The workflow whose launcher sheet is open (null = none).
  const [launching, setLaunching] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);

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
      <header className="mb-6 flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Workflows</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Workflow definitions discovered across this control plane — open one to inspect its steps
            and raw YAML.
          </p>
        </div>
        <button
          type="button"
          onClick={() => setCreateOpen(true)}
          className="inline-flex shrink-0 items-center gap-1.5 rounded-md bg-brand-600 px-3 py-1.5 text-[12px] font-medium text-white hover:bg-brand-700"
        >
          <Plus size={14} />
          New workflow
        </button>
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
              <WorkflowRow
                key={`${w.scope}:${w.name}`}
                workflow={w}
                onRun={() => setLaunching(w.name)}
              />
            ))}
          </ListCard>
          {sorted.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
              scroll for more
            </div>
          )}
        </section>
      )}

      {launching && (
        <LauncherSheet workflow={launching} onClose={() => setLaunching(null)} />
      )}

      {createOpen && <NewWorkflowModal onClose={() => setCreateOpen(false)} />}
    </div>
  );
}

function NewWorkflowModal({ onClose }: { onClose: () => void }) {
  const navigate = useNavigate();
  const titleId = useId();
  const [raw, setRaw] = useState(NEW_WORKFLOW_TEMPLATE);
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
      const created = await api.createWorkflow(raw);
      const newName =
        typeof created.workflow.name === 'string' ? created.workflow.name : null;
      if (newName) {
        navigate(`/workflows/${encodeURIComponent(newName)}`);
      }
      onClose();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Failed to create workflow');
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
            New workflow
          </h2>
          <p className="mt-1 text-[12px] text-ink-dim">
            Edit the definition below. It is validated server-side before it is saved.
          </p>
        </div>

        <div className="space-y-3 px-5 py-4">
          <CodeEditor
            value={raw}
            onChange={setRaw}
            language="yaml"
            ariaLabel="New workflow definition"
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

function WorkflowRow({ workflow: w, onRun }: { workflow: WorkflowSummary; onRun: () => void }) {
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
        <div className="flex shrink-0 items-center gap-3">
          {w.last_run && (
            <span className="text-[11px] text-ink-mute tabular-nums">
              {relativeTime(w.last_run)}
            </span>
          )}
          <button
            type="button"
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              onRun();
            }}
            aria-label={`Run ${w.name}`}
            className="inline-flex items-center rounded-md border border-brand-600 bg-white px-2.5 py-1 text-[12px] font-medium text-brand-700 hover:bg-brand-50"
          >
            Run
          </button>
        </div>
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
