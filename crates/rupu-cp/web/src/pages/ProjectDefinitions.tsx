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
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader } from '../components/lists/SectionHeader';
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
        'inline-flex items-center rounded ring-1 text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5',
        cls,
      )}
    >
      {s}
    </span>
  );
}

const TRIGGER_CLS: Record<string, string> = {
  cron: 'bg-violet-50 text-violet-700 ring-violet-200',
  event: 'bg-sky-50 text-sky-700 ring-sky-200',
  manual: 'bg-slate-100 text-slate-600 ring-slate-200',
};

function TriggerChip({ trigger }: { trigger: string }) {
  const cls = TRIGGER_CLS[trigger] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span
      className={cn(
        'inline-flex items-center rounded ring-1 text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5',
        cls,
      )}
    >
      {trigger}
    </span>
  );
}

function MetaChip({ children }: { children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200">
      {children}
    </span>
  );
}

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
    <div className="max-w-5xl">
      <header className="px-8 pt-8 pb-3">
        <Link
          to={`/projects/${encodedId}`}
          className="text-[11px] font-medium text-brand-600 hover:text-brand-700"
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
    <section>
      <SectionHeader tone="muted" label="Agents" count={agents.length} />
      <ListCard>
        {agents.map((a) => (
          <Link
            key={a.name}
            to={`/agents/${encodeURIComponent(a.name)}`}
            className="flex items-start gap-4 px-4 py-3 hover:bg-slate-50 transition-colors"
          >
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-sm font-medium text-ink truncate">{a.name}</span>
                <ScopeChip scope={a.scope} />
                {a.provider && <MetaChip>{a.provider}</MetaChip>}
                {a.model && <MetaChip>{a.model}</MetaChip>}
              </div>
              {a.description && (
                <p className="mt-1 text-[12px] text-ink-dim leading-snug line-clamp-2">
                  {a.description}
                </p>
              )}
            </div>
          </Link>
        ))}
      </ListCard>
    </section>
  );
}

function WorkflowsTab({ workflows }: { workflows: WorkflowSummary[] | null }) {
  if (workflows === null) return <div className="text-sm text-ink-dim">Loading workflows…</div>;
  if (workflows.length === 0) return <EmptyDefs label="workflows" />;
  return (
    <section>
      <SectionHeader tone="muted" label="Workflows" count={workflows.length} />
      <ListCard>
        {workflows.map((w) => (
          <Link
            key={w.name}
            to={`/workflows/${encodeURIComponent(w.name)}`}
            className="flex items-center gap-4 px-4 py-3 hover:bg-slate-50 transition-colors"
          >
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 flex-wrap">
                <span className="text-sm font-medium text-ink truncate">{w.name}</span>
                <ScopeChip scope={w.scope} />
              </div>
            </div>
          </Link>
        ))}
      </ListCard>
    </section>
  );
}

function AutoflowsTab({ autoflows }: { autoflows: AutoflowDefRow[] | null }) {
  if (autoflows === null) return <div className="text-sm text-ink-dim">Loading autoflows…</div>;
  if (autoflows.length === 0) return <EmptyDefs label="autoflows" />;
  return (
    <section>
      <SectionHeader tone="muted" label="Autoflows" count={autoflows.length} />
      <ListCard>
        {autoflows.map((d) => (
          <div key={d.name} className="flex items-center gap-4 px-4 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 flex-wrap">
                <span className="text-sm font-medium text-ink truncate">{d.name}</span>
                <ScopeChip scope={d.scope} />
                <TriggerChip trigger={d.trigger} />
              </div>
            </div>
          </div>
        ))}
      </ListCard>
    </section>
  );
}
