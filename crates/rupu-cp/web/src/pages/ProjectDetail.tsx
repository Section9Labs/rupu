// Project detail — tabbed shell. Loads the project bundle once, paints the
// persistent identity header + 5 rollup tiles, then a TabBar that routes
// between five tab bodies (Overview / Runs / Findings / Sessions / Coverage).
// The active tab is driven by the `tab` prop, set per-route in App.tsx.

import { useEffect, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import {
  Activity,
  GitBranch,
  GitFork,
  LayoutDashboard,
  ListOrdered,
  MessageSquare,
  ShieldAlert,
  ShieldCheck,
} from 'lucide-react';
import {
  api,
  type ProjectDetail as ProjectDetailType,
  type ProjectAssessedPct,
} from '../lib/api';
import { TabBar, TabButton } from '../components/TabBar';
import ProjectOverviewTab from '../components/project/ProjectOverviewTab';
import ProjectRunsTab from '../components/project/ProjectRunsTab';
import ProjectFindingsTab from '../components/project/ProjectFindingsTab';
import ProjectSessionsTab from '../components/project/ProjectSessionsTab';
import ProjectCoverageTab from '../components/project/ProjectCoverageTab';
import { relativeTime } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';

export type ProjectTab = 'overview' | 'runs' | 'findings' | 'sessions' | 'coverage';

// ---------------------------------------------------------------------------
// Rollup tile
// ---------------------------------------------------------------------------

function RollupTile({
  label,
  value,
  sub,
  children,
}: {
  label: string;
  value: string | number;
  sub?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
      <p className="text-[9px] font-semibold uppercase tracking-widest text-ink-mute mb-1">
        {label}
      </p>
      <p className="text-2xl font-bold text-ink tabular-nums leading-none">{value}</p>
      {sub && <p className="mt-1 text-[11px] text-ink-dim">{sub}</p>}
      {children}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Page (shell)
// ---------------------------------------------------------------------------

export default function ProjectDetail({ tab = 'overview' }: { tab?: ProjectTab }) {
  const { wsId } = useParams<{ wsId: string }>();
  const navigate = useNavigate();
  const [detail, setDetail] = useState<ProjectDetailType | null>(null);
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
        <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
          <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
            <Activity size={20} className="text-ink-mute" />
          </div>
          <h2 className="text-sm font-medium text-ink">Project not found</h2>
          <p className="mt-1 text-xs text-ink-dim max-w-xs">
            No project with ID <span className="font-mono">{wsId}</span> is registered on this
            control plane.
          </p>
          <Link
            to="/projects"
            className="mt-4 text-xs font-medium text-brand-600 hover:text-brand-700"
          >
            ← Back to projects
          </Link>
        </div>
      </div>
    );
  }

  // --- Error state ---
  if (error) {
    return (
      <div className="p-8">
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      </div>
    );
  }

  // --- Loading state ---
  if (detail === null) {
    return (
      <div className="p-8">
        <div className="text-sm text-ink-dim">Loading project…</div>
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

  return (
    <div className="p-8 space-y-6">
      {/* ── Identity header ── */}
      <header className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
        <h1 className="text-lg font-bold text-ink">{p.name}</h1>
        <div className="mt-1.5 flex items-center flex-wrap gap-x-4 gap-y-1 text-[11px] text-ink-dim">
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

      {/* ── Rollup tiles ── */}
      <section className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-5">
        <RollupTile
          label="Runs"
          value={runs.total}
          sub={runs.running > 0 ? `${runs.running} running` : 'none running'}
        />
        <RollupTile
          label="Sessions"
          value={sessions.total}
          sub={sessions.active > 0 ? `${sessions.active} active` : 'none active'}
        />
        <RollupTile label="Coverage" value={pctLabel}>
          {/* Progress bar — width via inline style (CSS value, not Tailwind class) */}
          <div className="mt-2 h-1.5 rounded-full bg-slate-200 overflow-hidden">
            {pct !== null ? (
              <div
                className="h-full rounded-full bg-gradient-to-r from-brand-500 to-green-500"
                style={{ width: `${pct}%` }}
              />
            ) : null}
          </div>
        </RollupTile>
        <RollupTile label="Findings" value={coverage.findings}>
          {coverage.findings > 0 && (
            <p className="mt-1 text-[11px] text-red-600 font-medium flex items-center gap-1">
              <ShieldAlert size={11} />
              needs attention
            </p>
          )}
          {coverage.findings === 0 && (
            <p className="mt-1 text-[11px] text-green-600 flex items-center gap-1">
              <ShieldCheck size={11} />
              all clear
            </p>
          )}
        </RollupTile>
        {usage && (
          <RollupTile
            label="Usage"
            value={
              formatCost(usage.cost_usd) +
              (usage.cost_usd !== null && !usage.priced ? '*' : '')
            }
            sub={`${formatTokens(usage.total_tokens)} tok`}
          />
        )}
      </section>

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
        </TabBar>
      </div>

      {/* ── Active tab body ── */}
      {tab === 'overview' && (
        <ProjectOverviewTab detail={detail} wsId={p.ws_id} pctLabel={pctLabel} pct={pct} />
      )}
      {tab === 'runs' && <ProjectRunsTab wsId={p.ws_id} />}
      {tab === 'findings' && <ProjectFindingsTab wsId={p.ws_id} />}
      {tab === 'sessions' && <ProjectSessionsTab wsId={p.ws_id} />}
      {tab === 'coverage' && <ProjectCoverageTab wsId={p.ws_id} />}
    </div>
  );
}
