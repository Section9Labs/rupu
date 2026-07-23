// Workflows library — read-only list of workflow definitions discovered by the
// control plane. Each row links to /workflows/:name for the steps + raw YAML.

import { useEffect, useId, useMemo, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { Plus, Sparkles, Workflow as WorkflowIcon } from 'lucide-react';
import { api, type ProviderModels, type WorkflowSummary } from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import LauncherSheet from '../components/LauncherSheet';
import CodeEditor from '../components/CodeEditor';
import { Button } from '../components/ui/Button';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import { ScopeChip } from '../components/ScopeChip';
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

  // `setLaunching` is a stable useState setter → build the columns once.
  const columns = useMemo(() => workflowColumns(setLaunching), []);

  const sorted = (workflows ?? []).slice().sort((a, b) => a.name.localeCompare(b.name));
  const shown = sorted.slice(0, visible);
  const { sentinelRef } = useInfiniteScroll({
    hasMore: visible < sorted.length,
    loadMore: () => setVisible((v) => v + STEP),
  });

  return (
    <div className="p-8">
      <header className="mb-6 flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Workflows</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Workflow definitions discovered across this control plane — open one to inspect its steps
            and raw YAML.
          </p>
        </div>
        <Button onClick={() => setCreateOpen(true)} className="shrink-0 gap-1.5">
          <Plus size={14} />
          New workflow
        </Button>
      </header>

      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {workflows === null ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading workflows…" />
        </div>
      ) : sorted.length === 0 ? (
        <EmptyState
          icon={<WorkflowIcon size={20} />}
          title="No workflows found"
          hint={
            <>
              Add workflow YAML under <span className="font-mono">.rupu/workflows/</span> to populate
              this library.
            </>
          }
        />
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
          <SortableTable<WorkflowSummary>
            columns={columns}
            rows={shown}
            rowKey={(w) => `${w.scope}:${w.name}`}
            initialSort={{ key: 'last_run', dir: 'desc' }}
          />
          {sorted.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
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

  const [mode, setMode] = useState<'describe' | 'edit'>('describe');
  const [description, setDescription] = useState('');
  const [models, setModels] = useState<ProviderModels[]>([]);
  const [genProvider, setGenProvider] = useState<string>('');
  const [generating, setGenerating] = useState(false);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);

  useEffect(() => {
    api
      .generateModels()
      .then((m) => {
        setModels(m);
        const def = m.find((x) => x.is_default) ?? m[0];
        if (def) setGenProvider(def.provider);
        if (m.length === 0) setMode('edit');
      })
      .catch(() => setMode('edit'));
  }, []);

  async function generate() {
    if (generating || !description.trim()) return;
    setGenerating(true);
    setError(null);
    try {
      const sel = models.find((m) => m.provider === genProvider);
      const out = await api.generateWorkflow({
        description,
        provider: genProvider || undefined,
        model: sel?.models[0],
      });
      setRaw(out.raw);
      setMode('edit');
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Failed to generate workflow');
    } finally {
      setGenerating(false);
    }
  }

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
          <p className="mt-1 text-ui text-ink-dim">
            Describe what you want, or edit the raw definition directly.
          </p>
        </div>

        <div className="space-y-3 px-5 py-4">
          <div className="flex gap-1 rounded-lg border border-border p-1 text-ui">
            <button
              type="button"
              onClick={() => setMode('describe')}
              disabled={models.length === 0}
              className={cn(
                'flex-1 rounded-md px-3 py-1.5',
                mode === 'describe' ? 'bg-panel-2 text-ink' : 'text-ink-dim',
              )}
            >
              Describe
            </button>
            <button
              type="button"
              onClick={() => setMode('edit')}
              className={cn(
                'flex-1 rounded-md px-3 py-1.5',
                mode === 'edit' ? 'bg-panel-2 text-ink' : 'text-ink-dim',
              )}
            >
              Edit raw
            </button>
          </div>

          {mode === 'describe' ? (
            <>
              <label htmlFor="workflow-desc" className="block text-ui text-ink-dim">
                Describe the workflow you want
              </label>
              <textarea
                id="workflow-desc"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                rows={5}
                className="w-full rounded-lg border border-border bg-panel-2 p-2 text-ui text-ink"
                placeholder="e.g. review changed files, then fix anything high severity"
              />
              <div className="flex items-center gap-2">
                <select
                  value={genProvider}
                  onChange={(e) => setGenProvider(e.target.value)}
                  className="rounded-lg border border-border bg-panel-2 px-2 py-1.5 text-ui text-ink"
                  aria-label="Generation provider"
                >
                  {models.map((m) => (
                    <option key={m.provider} value={m.provider}>
                      {m.provider} · {m.models[0]}
                    </option>
                  ))}
                </select>
                <Button
                  onClick={() => void generate()}
                  disabled={generating || !description.trim()}
                >
                  <Sparkles size={14} />
                  {generating ? 'Generating…' : 'Generate'}
                </Button>
              </div>
            </>
          ) : (
            <CodeEditor
              value={raw}
              onChange={setRaw}
              language="yaml"
              ariaLabel="New workflow definition"
            />
          )}

          {error && (
            <p role="alert" className="text-ui font-medium text-err">
              {error}
            </p>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-border px-5 py-3">
          <Button variant="secondary" onClick={onClose} disabled={creating}>
            Cancel
          </Button>
          <Button onClick={create} disabled={creating}>
            {creating ? 'Creating…' : 'Create'}
          </Button>
        </div>
      </div>
    </div>
  );
}

function workflowColumns(onRun: (name: string) => void): Column<WorkflowSummary>[] {
  return [
    {
      key: 'name',
      header: 'Name',
      subject: true,
      sortable: true,
      sortValue: (w) => w.name,
      titleValue: (w) => w.name,
      render: (w) => (
        <Link
          to={`/workflows/${encodeURIComponent(w.name)}`}
          className="text-sm font-medium text-ink hover:underline"
        >
          {w.name}
        </Link>
      ),
    },
    {
      key: 'scope',
      header: 'Scope',
      fit: true,
      sortable: true,
      sortValue: (w) => w.scope,
      render: (w) => <ScopeChip scope={w.scope} />,
    },
    {
      key: 'runs',
      header: 'Runs',
      align: 'right',
      fit: true,
      sortable: true,
      sortValue: (w) => w.run_count,
      render: (w) => (
        <span className="text-ink">{w.run_count ? String(w.run_count) : '—'}</span>
      ),
    },
    {
      key: 'tokens',
      header: 'Tokens',
      align: 'right',
      fit: true,
      sortable: true,
      sortValue: (w) => w.usage?.total_tokens ?? null,
      render: (w) => (
        <span className="text-ink-dim">{w.usage ? formatTokens(w.usage.total_tokens) : '—'}</span>
      ),
    },
    {
      key: 'cost',
      header: 'Cost',
      align: 'right',
      fit: true,
      sortable: true,
      sortValue: (w) => w.usage?.cost_usd ?? null,
      render: (w) => (
        <span className="text-ink font-medium">{w.usage ? formatCost(w.usage.cost_usd) : '—'}</span>
      ),
    },
    {
      key: 'last_run',
      header: 'Last run',
      align: 'right',
      fit: true,
      sortable: true,
      sortValue: (w) => (w.last_run ? Date.parse(w.last_run) : null),
      render: (w) => (
        <span className="text-ink-mute">{w.last_run ? relativeTime(w.last_run) : '—'}</span>
      ),
    },
    {
      key: 'action',
      header: '',
      align: 'right',
      fit: true,
      render: (w) => (
        <button
          type="button"
          onClick={() => onRun(w.name)}
          aria-label={`Run ${w.name}`}
          className="inline-flex items-center rounded-md border border-brand-600 bg-panel px-2.5 py-1 text-ui font-medium text-brand-700 hover:bg-brand-50"
        >
          Run
        </button>
      ),
    },
  ];
}
