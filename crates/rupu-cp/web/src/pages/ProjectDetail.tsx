// Project detail — tabbed shell. Loads the project bundle once, paints the
// persistent identity header + 5 rollup tiles, then a TabBar that routes
// between six tab bodies (Overview / Runs / Findings / Sessions / Coverage /
// Config). The active tab is driven by the `tab` prop, set per-route in
// App.tsx.

import { lazy, Suspense, useEffect, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import {
  Activity,
  Code2,
  GitBranch,
  GitFork,
  LayoutDashboard,
  ListOrdered,
  MessageSquare,
  Network as NetworkIcon,
  Settings as SettingsIcon,
  ShieldAlert,
  ShieldCheck,
} from 'lucide-react';
import {
  api,
  type ProjectDetail as ProjectDetailType,
  type ProjectAssessedPct,
} from '../lib/api';
import { TabBar, TabButton } from '../components/TabBar';
import { ProjectCustomerMenu } from '../components/customers/ProjectCustomerMenu';
import { PricingErrorMark } from '../components/customers/PricingErrorMark';
import { Spinner } from '../components/ui/Spinner';
import { KpiStrip, type KpiItem } from '../components/ui/KpiStrip';
import { Skeleton } from '../components/ui/Skeleton';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import ProjectOverviewTab from '../components/project/ProjectOverviewTab';
import ProjectRunsTab from '../components/project/ProjectRunsTab';
import ProjectFindingsTab from '../components/project/ProjectFindingsTab';
import ProjectSessionsTab from '../components/project/ProjectSessionsTab';
import ProjectCoverageTab from '../components/project/ProjectCoverageTab';
import ProjectNetworkTab from '../components/project/ProjectNetworkTab';
import ProjectConfigTab from '../components/project/ProjectConfigTab';
import { relativeTime } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';

// The Code tab's subtree (FileTree/CodeViewer/InlineFindingCard, which in turn
// reaches the heavy `manualChunks.markdown` group via Markdown) is loaded via
// React.lazy, mirroring CodeEditor.tsx/ExpressionField.tsx's Impl-splitting
// pattern, so that code only ships to the browser once the Code tab is opened
// rather than with every ProjectDetail chunk load.
const ProjectCodeTab = lazy(() => import('../components/project/ProjectCodeTab'));

export type ProjectTab =
  | 'overview'
  | 'runs'
  | 'findings'
  | 'code'
  | 'sessions'
  | 'coverage'
  | 'network'
  | 'config';

// ---------------------------------------------------------------------------
// Rollup tile
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Page (shell)
// ---------------------------------------------------------------------------

export default function ProjectDetail({ tab = 'overview' }: { tab?: ProjectTab }) {
  const { wsId } = useParams<{ wsId: string }>();
  const navigate = useNavigate();
  const [detail, setDetail] = useState<ProjectDetailType | null>(null);
  // Bumped when the header assigns / unassigns the project's customer: the
  // Config tab's provenance and locks come from that customer's layer, so it
  // re-reads its view.
  const [customerChanges, setCustomerChanges] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [notFound, setNotFound] = useState(false);
  /** assessed_pct fetched lazily in parallel — undefined = still loading,
   * null = no catalog / backend returned null. */
  const [assessedPct, setAssessedPct] = useState<ProjectAssessedPct | undefined>(undefined);

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    api
      .getProject(wsId)
      .then((data) => {
        if (cancelled) return;
        setDetail(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        if (e instanceof Error && e.message.includes('404')) {
          setNotFound(true);
        } else {
          setError(e instanceof Error ? e.message : 'Failed to load project');
        }
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  // Parallel effect: fetch assessed_pct without blocking the overview render.
  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    api
      .getProjectAssessedPct(wsId)
      .then((data) => {
        if (cancelled) return;
        setAssessedPct(data);
      })
      .catch(() => {
        // Swallow errors — assessed % is non-critical; tile stays in "…" state.
        if (!cancelled) setAssessedPct({ assessed_pct: null });
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  // --- Not-found state ---
  if (notFound) {
    return (
      <div className="p-8">
        <EmptyState
          icon={<Activity size={20} />}
          title="Project not found"
          hint={
            <>
              No project with ID <span className="font-mono">{wsId}</span> is registered on this
              control plane.
            </>
          }
          action={
            <Link to="/projects" className="text-ui font-medium text-brand-600 hover:text-brand-700">
              ← Back to projects
            </Link>
          }
        />
      </div>
    );
  }

  // --- Error state ---
  if (error) {
    return (
      <div className="p-8">
        <ErrorBanner>{error}</ErrorBanner>
      </div>
    );
  }

  // --- Loading state ---
  if (detail === null) {
    return (
      <div className="p-8 space-y-6" role="status" aria-label="Loading project…">
        <span className="sr-only">Loading project…</span>
        <div className="rounded-xl border border-border bg-panel px-5 py-4" aria-hidden>
          <Skeleton className="h-5 w-48" />
          <Skeleton className="mt-2 h-3.5 w-72" />
        </div>
        <div className="overflow-hidden rounded-xl border border-border bg-panel" aria-hidden>
          <div className="-ml-px -mt-px grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5">
            {[0, 1, 2, 3, 4].map((i) => (
              <div key={i} className="border-l border-t border-border p-4">
                <Skeleton className="h-3 w-16" />
                <Skeleton className="mt-3 h-6 w-12" />
              </div>
            ))}
          </div>
        </div>
      </div>
    );
  }

  const { project: p, runs, sessions, coverage, usage } = detail;

  // assessed_pct arrives from the lazy parallel endpoint.
  // `assessedPct === undefined` means still in-flight → show "…".
  const rawPct = assessedPct !== undefined ? assessedPct.assessed_pct : undefined;
  const pct = rawPct != null ? Math.min(100, Math.max(0, rawPct)) : null;
  // "…" while loading, "—" when loaded but null (no catalog).
  const pctLabel =
    assessedPct === undefined ? '…' : pct !== null ? `${Math.round(pct)}%` : '—';

  const encodedId = encodeURIComponent(p.ws_id);

  // Project rollups as the one KPI strip (spec §"KPIs": type, not tiles; the
  // attention value — open findings — in red).
  const kpis: KpiItem[] = [
    {
      label: 'Runs',
      value: runs.total,
      meta: runs.running > 0 ? `${runs.running} running` : 'none running',
    },
    {
      label: 'Sessions',
      value: sessions.total,
      meta: sessions.active > 0 ? `${sessions.active} active` : 'none active',
    },
    { label: 'Coverage', value: pctLabel },
    {
      label: 'Findings',
      value: coverage.findings,
      attention: coverage.findings > 0,
      meta: coverage.findings > 0 ? 'needs attention' : 'all clear',
    },
    ...(usage
      ? [
          {
            label: 'Usage',
            value: (
              <span className="inline-flex items-center gap-1.5">
                {formatCost(usage.cost_usd) + (usage.cost_usd !== null && !usage.priced ? '*' : '')}
                <PricingErrorMark error={usage.pricing_error} />
              </span>
            ),
            meta: `${formatTokens(usage.total_tokens)} tok`,
          } as KpiItem,
        ]
      : []),
  ];

  return (
    <div className="p-8 space-y-6">
      {/* ── Identity header ── */}
      <header className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <h1 className="text-lg font-bold text-ink">{p.name}</h1>
          <ProjectCustomerMenu
            wsId={p.ws_id}
            customer={p.customer}
            onChange={(next) => {
              setDetail((d) => (d ? { ...d, project: { ...d.project, customer: next } } : d));
              setCustomerChanges((n) => n + 1);
            }}
          />
        </div>
        <div className="mt-1.5 flex items-center flex-wrap gap-x-4 gap-y-1 text-note text-ink-dim">
          <span className="font-mono">{p.path}</span>
          {p.repo_remote && (
            <span className="inline-flex items-center gap-1">
              <GitFork size={11} className="text-ink-mute" />
              {p.repo_remote}
            </span>
          )}
          {p.branch && (
            <span className="inline-flex items-center gap-1">
              <GitBranch size={11} className="text-ink-mute" />
              {p.branch}
            </span>
          )}
          {p.last_run_at && <span>last run {relativeTime(p.last_run_at)}</span>}
          <span className="font-mono text-ink-mute">{p.ws_id}</span>
        </div>
      </header>

      {/* ── Rollup KPIs ── */}
      <KpiStrip items={kpis} />

      {/* ── Tab bar ── */}
      <div className="-mx-8">
        <TabBar>
          <TabButton
            active={tab === 'overview'}
            onClick={() => navigate(`/projects/${encodedId}`)}
            icon={LayoutDashboard}
            label="Overview"
          />
          <TabButton
            active={tab === 'runs'}
            onClick={() => navigate(`/projects/${encodedId}/runs`)}
            icon={ListOrdered}
            label="Runs"
          />
          <TabButton
            active={tab === 'findings'}
            onClick={() => navigate(`/projects/${encodedId}/findings`)}
            icon={ShieldAlert}
            label="Findings"
          />
          <TabButton
            active={tab === 'code'}
            onClick={() => navigate(`/projects/${encodedId}/code`)}
            icon={Code2}
            label="Code"
          />
          <TabButton
            active={tab === 'sessions'}
            onClick={() => navigate(`/projects/${encodedId}/sessions`)}
            icon={MessageSquare}
            label="Sessions"
          />
          <TabButton
            active={tab === 'coverage'}
            onClick={() => navigate(`/projects/${encodedId}/coverage`)}
            icon={ShieldCheck}
            label="Coverage"
          />
          <TabButton
            active={tab === 'network'}
            onClick={() => navigate(`/projects/${encodedId}/network`)}
            icon={NetworkIcon}
            label="Network"
          />
          <TabButton
            active={tab === 'config'}
            onClick={() => navigate(`/projects/${encodedId}/config`)}
            icon={SettingsIcon}
            label="Config"
          />
        </TabBar>
      </div>

      {/* ── Active tab body ── */}
      {tab === 'overview' && (
        <ProjectOverviewTab detail={detail} wsId={p.ws_id} pctLabel={pctLabel} pct={pct} />
      )}
      {tab === 'runs' && <ProjectRunsTab wsId={p.ws_id} />}
      {tab === 'findings' && <ProjectFindingsTab wsId={p.ws_id} />}
      {tab === 'code' && (
        <Suspense
          fallback={
            <div className="flex h-40 items-center justify-center">
              <Spinner label="Loading…" />
            </div>
          }
        >
          <ProjectCodeTab
            wsId={p.ws_id}
            repoHomeUrl={p.repo_home_url ?? null}
            repoRemote={p.repo_remote ?? null}
            branch={p.branch ?? null}
          />
        </Suspense>
      )}
      {tab === 'sessions' && <ProjectSessionsTab wsId={p.ws_id} />}
      {tab === 'coverage' && <ProjectCoverageTab wsId={p.ws_id} />}
      {tab === 'network' && <ProjectNetworkTab wsId={p.ws_id} />}
      {tab === 'config' && <ProjectConfigTab wsId={p.ws_id} reloadKey={customerChanges} />}
    </div>
  );
}
