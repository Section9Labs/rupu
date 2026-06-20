// Project overview dashboard — layout A from the approved mockup.
// Identity header + 4 rollup tiles + Recent runs / Coverage / Sessions sections.

import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import {
  Activity,
  GitBranch,
  GitFork,
  Library,
  MessageSquare,
  ShieldAlert,
  ShieldCheck,
} from 'lucide-react';
import { api, type ProjectDetail as ProjectDetailType, type ProjectAssessedPct } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { StatusPill } from '../components/StatusPill';
import { relativeTime } from '../lib/time';

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
// Section header with "see all" link
// ---------------------------------------------------------------------------

function SectionTitle({ title, href }: { title: string; href: string }) {
  return (
    <div className="flex items-center justify-between mb-2">
      <h2 className="text-sm font-semibold text-ink">{title}</h2>
      <Link
        to={href}
        className="text-[11px] font-medium text-brand-600 hover:text-brand-700 transition-colors"
      >
        see all →
      </Link>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Trigger chip (reuse the pattern from WorkflowRuns)
// ---------------------------------------------------------------------------

const TRIGGER_CHIP_CLS: Record<string, string> = {
  manual: 'bg-slate-100 text-slate-600',
  cron: 'bg-violet-50 text-violet-700',
  event: 'bg-sky-50 text-sky-700',
};

function TriggerChip({ trigger }: { trigger: string }) {
  const cls = TRIGGER_CHIP_CLS[trigger] ?? 'bg-slate-100 text-slate-600';
  return (
    <span
      className={`inline-flex items-center rounded text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5 ${cls}`}
    >
      {trigger}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function ProjectDetail() {
  const { wsId } = useParams<{ wsId: string }>();
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
      <div className="p-8 max-w-5xl">
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
      <div className="p-8 max-w-5xl">
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      </div>
    );
  }

  // --- Loading state ---
  if (detail === null) {
    return (
      <div className="p-8 max-w-5xl">
        <div className="text-sm text-ink-dim">Loading project…</div>
      </div>
    );
  }

  const { project: p, runs, sessions, coverage, recent_runs } = detail;

  // assessed_pct arrives from the lazy parallel endpoint.
  // `assessedPct === undefined` means still in-flight → show "…".
  const rawPct =
    assessedPct !== undefined ? assessedPct.assessed_pct : undefined;
  const pct =
    rawPct != null ? Math.min(100, Math.max(0, rawPct)) : null;
  // "…" while loading, "—" when loaded but null (no catalog).
  const pctLabel =
    assessedPct === undefined ? '…' : pct !== null ? `${Math.round(pct)}%` : '—';

  const encodedId = encodeURIComponent(p.ws_id);

  return (
    <div className="p-8 max-w-5xl space-y-6">

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
          {p.last_run_at && (
            <span>last run {relativeTime(p.last_run_at)}</span>
          )}
          <span className="font-mono text-ink-mute">{p.ws_id}</span>
        </div>
      </header>

      {/* ── Rollup tiles ── */}
      <section className="grid grid-cols-2 gap-4 sm:grid-cols-4">
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
        <RollupTile
          label="Findings"
          value={coverage.findings}
        >
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
      </section>

      {/* ── Recent runs ── */}
      <section>
        <SectionTitle title="Recent runs" href={`/projects/${encodedId}/runs`} />
        {recent_runs.length === 0 ? (
          <div className="rounded-xl border border-dashed border-border bg-panel/50 py-8 flex items-center justify-center">
            <p className="text-xs text-ink-mute">No runs yet</p>
          </div>
        ) : (
          <ListCard>
            {recent_runs.map((r) => (
              <Link
                key={r.id}
                to={`/runs/${encodeURIComponent(r.id)}`}
                className="flex items-center gap-4 px-4 py-3 hover:bg-slate-50 transition-colors"
              >
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-ink truncate">{r.workflow_name}</span>
                    <TriggerChip trigger={r.trigger} />
                  </div>
                  <p className="text-[11px] text-ink-dim mt-0.5">
                    {relativeTime(r.started_at)}
                  </p>
                </div>
                <StatusPill status={r.status} />
              </Link>
            ))}
          </ListCard>
        )}
      </section>

      {/* ── Bottom split: Coverage + Sessions ── */}
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">

        {/* Coverage summary */}
        <section>
          <div className="flex items-center justify-between mb-2">
            <h2 className="text-sm font-semibold text-ink">Coverage</h2>
            <Link
              to={`/projects/${encodedId}/coverage`}
              className="text-[11px] font-medium text-brand-600 hover:text-brand-700 transition-colors"
            >
              open →
            </Link>
          </div>
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 text-[12px] text-ink-dim">
            {coverage.targets === 0 ? (
              <span className="text-ink-mute">No coverage targets yet</span>
            ) : (
              <>
                <span className="font-medium text-ink">{coverage.targets}</span>{' '}
                target{coverage.targets !== 1 ? 's' : ''} ·{' '}
                <span className={pct !== null ? 'font-medium text-ink' : 'text-ink-mute'}>
                  {pctLabel}
                </span>{' '}
                assessed ·{' '}
                <span className={coverage.findings > 0 ? 'font-medium text-red-600' : 'font-medium text-ink'}>
                  {coverage.findings}
                </span>{' '}
                finding{coverage.findings !== 1 ? 's' : ''}
              </>
            )}
          </div>
        </section>

        {/* Sessions summary */}
        <section>
          <div className="flex items-center justify-between mb-2">
            <h2 className="text-sm font-semibold text-ink">Sessions</h2>
            <Link
              to={`/projects/${encodedId}/sessions`}
              className="text-[11px] font-medium text-brand-600 hover:text-brand-700 transition-colors"
            >
              see all →
            </Link>
          </div>
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
            {sessions.total === 0 ? (
              <p className="text-[12px] text-ink-mute">No sessions yet</p>
            ) : (
              <div className="flex items-center gap-3 text-[12px] text-ink-dim">
                <span className="inline-flex items-center gap-1.5">
                  <MessageSquare size={12} className="text-ink-mute" />
                  <span className="font-medium text-ink">{sessions.total}</span> total
                </span>
                {sessions.active > 0 && (
                  <span className="inline-flex items-center gap-1.5">
                    <span className="w-1.5 h-1.5 rounded-full bg-blue-500" />
                    <span className="font-medium text-ink">{sessions.active}</span> active
                  </span>
                )}
              </div>
            )}
          </div>
        </section>
      </div>

      {/* ── Definitions ── */}
      <section>
        <SectionTitle title="Definitions" href={`/projects/${encodedId}/definitions`} />
        <Link
          to={`/projects/${encodedId}/definitions`}
          className="block bg-panel border border-border rounded-xl shadow-card px-4 py-3 hover:bg-slate-50 transition-colors"
        >
          <div className="flex items-center gap-2.5 text-[12px] text-ink-dim">
            <Library size={14} className="text-ink-mute" />
            <span>
              Agents, workflows &amp; autoflows visible to this project
              <span className="text-ink-mute"> — global + project-local </span>
              <span className="font-mono">.rupu/</span>
            </span>
          </div>
        </Link>
      </section>
    </div>
  );
}
