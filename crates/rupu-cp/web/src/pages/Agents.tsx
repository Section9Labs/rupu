// Agents library — read-only list of agent files discovered by the control
// plane. Each row links to /agents/:name for the full system prompt.

import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Plus, Sparkles } from 'lucide-react';
import { api, scopeSelectorFor, type AgentSummary } from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import UsageBarChart from '../components/charts/UsageBarChart';
import AgentLauncherSheet from '../components/AgentLauncherSheet';
import { Button } from '../components/ui/Button';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import { ScopeChip } from '../components/ScopeChip';
import { formatTokens, formatCost } from '../lib/usage';
import { relativeTime } from '../lib/time';
import { cn } from '../lib/cn';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;

export default function Agents() {
  const navigate = useNavigate();
  const [agents, setAgents] = useState<AgentSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState(STEP);
  // The agent whose launcher sheet (Run) is open (null = none).
  const [launchingAgent, setLaunchingAgent] = useState<string | null>(null);
  // Session-start / delete failures — kept separate from the list-fetch
  // error above, but shown in the same banner (mirrors Sessions.tsx /
  // pages/runs/WorkflowRuns.tsx's `actionError`).
  const [actionError, setActionError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const data = await api.getAgents();
      setAgents(data);
      setVisible(STEP);
      setError(null);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Failed to load agents');
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // Row action: start a session directly (no form) and jump straight to it —
  // mirrors AgentLauncherSheet's own `onLaunch` session branch (`startSession`
  // then navigate to `/sessions/:id`), just without a prompt/target/host form.
  async function handleStartSession(name: string) {
    try {
      const res = await api.startSession(name);
      setActionError(null);
      navigate(`/sessions/${res.session_id}`);
    } catch (e: unknown) {
      setActionError(e instanceof Error ? e.message : 'Failed to start session');
    }
  }

  // Row action: delete the agent's .md definition file — same
  // confirm-then-call-then-refresh shape as Sessions.tsx's row Delete.
  //
  // Deletes by `slug` (file stem), never `name` (frontmatter display name) —
  // `DELETE /api/agents/:name` removes `<slug>.md` by file stem, and the two
  // can differ for hand- or CLI-authored files (see `AgentSummary.slug`'s
  // doc comment). `scopeSelectorFor(agent)` threads the row's exact
  // `scope_kind`/`scope_id` so the server targets THIS row's file even when
  // another repo defines the same slug — the confirm dialog names the
  // resolved layer so the operator knows exactly what's about to be removed.
  async function handleDelete(agent: AgentSummary) {
    const scopeLabel = agent.scope_kind === 'project' ? `project: ${agent.scope}` : 'global';
    if (
      !window.confirm(
        `Delete agent "${agent.name}" (${scopeLabel})? This removes the definition file. This cannot be undone.`,
      )
    ) {
      return;
    }
    try {
      const result = await api.deleteAgent(agent.slug ?? agent.name, scopeSelectorFor(agent));
      // Confirm the server actually removed the layer the operator was
      // shown — never silently trust a bare "ok" as "removed what I saw."
      if (
        agent.scope_kind &&
        (result.scope_kind !== agent.scope_kind || result.scope !== agent.scope)
      ) {
        setActionError(
          `Deleted the ${result.scope_kind === 'project' ? `project (${result.scope})` : 'global'} definition — not the ${scopeLabel} one shown here.`,
        );
      } else {
        setActionError(null);
      }
      await load();
    } catch (e: unknown) {
      setActionError(e instanceof Error ? e.message : 'Delete failed');
    }
  }

  const columns = agentColumns(setLaunchingAgent, handleStartSession, handleDelete);

  const sorted = (agents ?? []).slice().sort((a, b) => a.name.localeCompare(b.name));
  const shown = sorted.slice(0, visible);
  const { sentinelRef } = useInfiniteScroll({
    hasMore: visible < sorted.length,
    loadMore: () => setVisible((v) => v + STEP),
  });
  const bannerError = error ?? actionError;

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
        <Button
          onClick={() => navigate('/agents/new')}
          className="shrink-0 gap-1.5"
        >
          <Plus size={14} />
          New agent
        </Button>
      </header>

      {bannerError && <ErrorBanner className="mb-4">{bannerError}</ErrorBanner>}

      {agents === null ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading agents…" />
        </div>
      ) : sorted.length === 0 ? (
        <EmptyState
          icon={<Sparkles size={20} />}
          title="No agents found"
          hint={
            <>
              Add agent files under <span className="font-mono">.rupu/agents/</span> to populate this
              library.
            </>
          }
        />
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
            columns={columns}
            rows={shown}
            rowKey={(a) => a.name}
            rowHref={(a) => `/agents/${encodeURIComponent(a.name)}`}
            initialSort={{ key: 'name', dir: 'asc' }}
          />
          {sorted.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
              scroll for more
            </div>
          )}
        </section>
      )}

      {launchingAgent && (
        <AgentLauncherSheet agent={launchingAgent} onClose={() => setLaunchingAgent(null)} />
      )}
    </div>
  );
}

function agentColumns(
  onRun: (name: string) => void,
  onSession: (name: string) => void,
  onDelete: (agent: AgentSummary) => void,
): Column<AgentSummary>[] {
  return [...AGENT_BASE_COLUMNS, agentActionColumn(onRun, onSession, onDelete)];
}

/** Trailing action column: Run / Session / Delete. `interactive: true` so
 *  SortableTable renders it as a plain, unwrapped cell rather than
 *  link-wrapping it inside the row's own `rowHref` anchor (which would nest
 *  these buttons inside an `<a>`) — same treatment as Workflows.tsx's Run
 *  column and Sessions.tsx's Archive/Restore/Delete column. The wrapping div
 *  calls preventDefault()+stopPropagation() on every click that reaches it
 *  (bubbling up from whichever button was clicked) so a click here never
 *  soft- or hard-navigates the link-wrapped row — stopPropagation alone does
 *  not block the enclosing `<a>`'s native default navigation.
 *
 *  Delete renders for every row regardless of scope: `DELETE
 *  /api/agents/:name` resolves project-aware (global-then-registered-
 *  projects, by file stem — see `resolve_agent_scoped` in
 *  `rupu-cp/src/api/agents.rs`), the same layer walk `getAgent` uses, so it
 *  always removes the actual file the row/detail page displays rather than
 *  a same-named file in a different layer. `handleDelete`'s confirm dialog
 *  names the resolved scope before the operator confirms.
 */
function agentActionColumn(
  onRun: (name: string) => void,
  onSession: (name: string) => void,
  onDelete: (agent: AgentSummary) => void,
): Column<AgentSummary> {
  return {
    key: 'action',
    header: '',
    align: 'right',
    fit: true,
    interactive: true,
    render: (a) => (
      <div
        className="flex items-center justify-end gap-1"
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
        }}
      >
        <button
          type="button"
          onClick={() => onRun(a.name)}
          aria-label={`Run ${a.name}`}
          className="inline-flex items-center rounded-md border border-brand-600 bg-panel px-2.5 py-1 text-ui font-medium text-brand-700 hover:bg-brand-50"
        >
          Run
        </button>
        <Button
          variant="ring"
          onClick={() => void onSession(a.name)}
          aria-label={`Start session with ${a.name}`}
        >
          Session
        </Button>
        <Button
          variant="ring-danger"
          onClick={() => void onDelete(a)}
          aria-label={`Delete ${a.name}`}
        >
          Delete
        </Button>
      </div>
    ),
  };
}

const AGENT_BASE_COLUMNS: Column<AgentSummary>[] = [
  {
    key: 'name',
    header: 'Name',
    subject: true,
    sortable: true,
    sortValue: (a) => a.name,
    titleValue: (a) => a.name,
    render: (a) => (
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-sm font-medium text-ink">{a.name}</span>
        {a.provider && <MetaChip>{a.provider}</MetaChip>}
        {a.model && <MetaChip>{a.model}</MetaChip>}
        {a.effort && <MetaChip>effort: {a.effort}</MetaChip>}
      </div>
    ),
  },
  {
    key: 'scope',
    header: 'Scope',
    fit: true,
    sortable: true,
    sortValue: (a) => a.scope,
    render: (a) => <ScopeChip scope={a.scope} />,
  },
  {
    key: 'description',
    header: 'Description',
    fit: true,
    render: (a) =>
      a.description ? (
        <span className="text-ui text-ink-dim leading-snug truncate block max-w-md">
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
    fit: true,
    sortable: true,
    sortValue: (a) => a.run_count,
    render: (a) => <span className="text-ink">{a.run_count ? String(a.run_count) : '—'}</span>,
  },
  {
    key: 'tokens',
    header: 'Tokens',
    align: 'right',
    fit: true,
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
    fit: true,
    sortable: true,
    sortValue: (a) => a.usage?.cost_usd ?? null,
    render: (a) => (
      <span className="text-ink font-medium">{a.usage ? formatCost(a.usage.cost_usd) : '—'}</span>
    ),
  },
  {
    key: 'last_run',
    header: 'Last run',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (a) => (a.last_run ? Date.parse(a.last_run) : null),
    render: (a) => (
      <span className="text-ink-mute">{a.last_run ? relativeTime(a.last_run) : '—'}</span>
    ),
  },
];

function MetaChip({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-1.5 py-0.5 text-note font-medium ring-1 bg-surface text-ink-mute ring-border',
        className,
      )}
    >
      {children}
    </span>
  );
}
