// Project › Definitions — the agents / workflows / autoflows visible to a
// single project (global defs merged with the project's local
// `<path>/.rupu/{agents,workflows}`, project shadowing global by name).
// Each row carries a scope badge: project (local override) or global.

import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { Sparkles, Workflow as WorkflowIcon, Zap } from 'lucide-react';
import {
  api,
  type AgentSummary,
  type AutoflowDefRow,
  type WorkflowSummary,
} from '../lib/api';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { TriggerChip } from '../components/TriggerChip';
import { TabBar, TabButton } from '../components/TabBar';
import { cn } from '../lib/cn';

type Tab = 'agents' | 'workflows' | 'autoflows';

// ── Scope badge ─────────────────────────────────────────────────────────────

const SCOPE_CLS: Record<string, string> = {
  project: 'bg-emerald-50 text-emerald-700 ring-emerald-200',
  global: 'bg-indigo-50 text-indigo-700 ring-indigo-200',
};

function ScopeChip({ scope }: { scope?: string }) {
  const s = scope ?? 'global';
  const cls = SCOPE_CLS[s] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span
      className={cn(
        'inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5',
        cls,
      )}
    >
      {s}
    </span>
  );
}

// ── Columns ─────────────────────────────────────────────────────────────────

const AGENT_COLUMNS: Column<AgentSummary>[] = [
  {
    key: 'name',
    header: 'Name',
    sortable: true,
    sortValue: (a) => a.name,
    render: (a) => (
      <Link
        to={`/agents/${encodeURIComponent(a.name)}`}
        className="text-sm font-medium text-ink truncate hover:underline"
      >
        {a.name}
      </Link>
    ),
  },
  {
    key: 'scope',
    header: 'Scope',
    width: 'w-24',
    sortable: true,
    sortValue: (a) => a.scope ?? 'global',
    render: (a) => <ScopeChip scope={a.scope} />,
  },
  {
    key: 'provider',
    header: 'Provider',
    width: 'w-32',
    sortable: true,
    sortValue: (a) => a.provider ?? null,
    render: (a) =>
      a.provider ? (
        <span className="text-note text-ink-dim">{a.provider}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'model',
    header: 'Model',
    width: 'w-40',
    sortable: true,
    sortValue: (a) => a.model ?? null,
    render: (a) =>
      a.model ? (
        <span className="text-note text-ink-mute font-mono">{a.model}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'description',
    header: 'Description',
    render: (a) => (
      <span className="text-ui text-ink-dim truncate block max-w-md">{a.description ?? ''}</span>
    ),
  },
];

const WORKFLOW_COLUMNS: Column<WorkflowSummary>[] = [
  {
    key: 'name',
    header: 'Name',
    sortable: true,
    sortValue: (w) => w.name,
    render: (w) => (
      <Link
        to={`/workflows/${encodeURIComponent(w.name)}`}
        className="text-sm font-medium text-ink truncate hover:underline"
      >
        {w.name}
      </Link>
    ),
  },
  {
    key: 'scope',
    header: 'Scope',
    width: 'w-24',
    sortable: true,
    sortValue: (w) => w.scope,
    render: (w) => <ScopeChip scope={w.scope} />,
  },
];

const AUTOFLOW_COLUMNS: Column<AutoflowDefRow>[] = [
  {
    key: 'name',
    header: 'Name',
    sortable: true,
    sortValue: (d) => d.name,
    render: (d) => (
      <Link
        to={`/workflows/${encodeURIComponent(d.slug)}`}
        className="text-sm font-medium text-ink truncate hover:underline"
      >
        {d.name}
      </Link>
    ),
  },
  {
    key: 'trigger',
    header: 'Trigger',
    width: 'w-28',
    sortable: true,
    sortValue: (d) => d.trigger,
    render: (d) => <TriggerChip trigger={d.trigger} />,
  },
  {
    key: 'scope',
    header: 'Scope',
    width: 'w-24',
    sortable: true,
    sortValue: (d) => d.scope,
    render: (d) => <ScopeChip scope={d.scope} />,
  },
];

// ── Empty state ─────────────────────────────────────────────────────────────

function EmptyDefs({ label }: { label: string }) {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-12 flex flex-col items-center justify-center text-center">
      <p className="text-sm font-medium text-ink">No {label} visible</p>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Global and project-local <span className="font-mono">.rupu/</span> {label} will appear here.
      </p>
    </div>
  );
}

// ── Page ────────────────────────────────────────────────────────────────────

export default function ProjectDefinitions() {
  const { wsId } = useParams<{ wsId: string }>();
  const [tab, setTab] = useState<Tab>('agents');

  const [agents, setAgents] = useState<AgentSummary[] | null>(null);
  const [workflows, setWorkflows] = useState<WorkflowSummary[] | null>(null);
  const [autoflows, setAutoflows] = useState<AutoflowDefRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    Promise.all([
      api.getProjectAgents(wsId),
      api.getProjectWorkflows(wsId),
      api.getProjectAutoflows(wsId),
    ])
      .then(([a, w, f]) => {
        if (cancelled) return;
        setAgents(a);
        setWorkflows(w);
        setAutoflows(f);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load definitions');
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  const encodedId = encodeURIComponent(wsId ?? '');

  return (
    <div>
      <header className="px-8 pt-8 pb-3">
        <Link
          to={`/projects/${encodedId}`}
          className="text-note font-medium text-brand-600 hover:text-brand-700"
        >
          ← Back to project
        </Link>
        <h1 className="mt-2 text-2xl font-semibold text-ink">Definitions</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Agents, workflows, and autoflows visible to this project — global defs merged with the
          project&apos;s local <span className="font-mono">.rupu/</span> overrides.
        </p>
      </header>

      <TabBar>
        <TabButton
          active={tab === 'agents'}
          onClick={() => setTab('agents')}
          icon={Sparkles}
          label={`Agents${agents ? ` (${agents.length})` : ''}`}
        />
        <TabButton
          active={tab === 'workflows'}
          onClick={() => setTab('workflows')}
          icon={WorkflowIcon}
          label={`Workflows${workflows ? ` (${workflows.length})` : ''}`}
        />
        <TabButton
          active={tab === 'autoflows'}
          onClick={() => setTab('autoflows')}
          icon={Zap}
          label={`Autoflows${autoflows ? ` (${autoflows.length})` : ''}`}
        />
      </TabBar>

      <div className="px-8 py-6">
        {error && (
          <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
            {error}
          </div>
        )}

        {tab === 'agents' && <AgentsTab agents={agents} />}
        {tab === 'workflows' && <WorkflowsTab workflows={workflows} />}
        {tab === 'autoflows' && <AutoflowsTab autoflows={autoflows} />}
      </div>
    </div>
  );
}

// ── Tabs ────────────────────────────────────────────────────────────────────

function AgentsTab({ agents }: { agents: AgentSummary[] | null }) {
  if (agents === null) return <div className="text-sm text-ink-dim">Loading agents…</div>;
  if (agents.length === 0) return <EmptyDefs label="agents" />;
  return (
    <SortableTable<AgentSummary>
      columns={AGENT_COLUMNS}
      rows={agents}
      rowKey={(a) => a.name}
      initialSort={{ key: 'name', dir: 'asc' }}
    />
  );
}

function WorkflowsTab({ workflows }: { workflows: WorkflowSummary[] | null }) {
  if (workflows === null) return <div className="text-sm text-ink-dim">Loading workflows…</div>;
  if (workflows.length === 0) return <EmptyDefs label="workflows" />;
  return (
    <SortableTable<WorkflowSummary>
      columns={WORKFLOW_COLUMNS}
      rows={workflows}
      rowKey={(w) => w.name}
      initialSort={{ key: 'name', dir: 'asc' }}
    />
  );
}

function AutoflowsTab({ autoflows }: { autoflows: AutoflowDefRow[] | null }) {
  if (autoflows === null) return <div className="text-sm text-ink-dim">Loading autoflows…</div>;
  if (autoflows.length === 0) return <EmptyDefs label="autoflows" />;
  return (
    <SortableTable<AutoflowDefRow>
      columns={AUTOFLOW_COLUMNS}
      rows={autoflows}
      rowKey={(d) => d.name}
      initialSort={{ key: 'name', dir: 'asc' }}
    />
  );
}
