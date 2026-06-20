// Projects registry — lists all workspaces tracked by this control plane.
// Each row is a project card that links to /projects/:wsId for the overview.

import { useEffect, useState } from 'react';
import { FolderGit2, GitBranch, GitFork } from 'lucide-react';
import { api, type ProjectRow } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import MetricRow from '../components/lists/MetricRow';
import UsageBarChart from '../components/charts/UsageBarChart';
import { formatTokens, formatCost } from '../lib/usage';
import { relativeTime } from '../lib/time';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;

export default function Projects() {
  const [projects, setProjects] = useState<ProjectRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState(STEP);

  useEffect(() => {
    let cancelled = false;
    api
      .getProjects()
      .then((data) => {
        if (cancelled) return;
        setProjects(data);
        setVisible(STEP);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load projects');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const all = projects ?? [];
  const shown = all.slice(0, visible);
  const { sentinelRef } = useInfiniteScroll({
    hasMore: visible < all.length,
    loadMore: () => setVisible((v) => v + STEP),
  });

  return (
    <div className="p-8 max-w-5xl">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Projects</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Workspaces registered with this control plane — each with its own runs, sessions, and
          coverage.
        </p>
      </header>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {projects === null && !error && (
        <div className="text-sm text-ink-dim">Loading projects…</div>
      )}

      {projects !== null && projects.length === 0 && <ProjectsEmpty />}

      {projects !== null && projects.length > 0 && (
        <>
          <div className="mb-4 rounded-xl border border-border bg-panel/50 p-4">
            <UsageBarChart
              bars={all.map((p) => ({
                id: p.ws_id,
                label: p.name,
                input_tokens: p.usage?.input_tokens ?? 0,
                output_tokens: p.usage?.output_tokens ?? 0,
                cached_tokens: p.usage?.cached_tokens ?? 0,
                cost_usd: p.usage?.cost_usd ?? null,
                to: `/projects/${encodeURIComponent(p.ws_id)}`,
              }))}
            />
          </div>
          <ListCard>
            {shown.map((p) => (
              <ProjectRow key={p.ws_id} project={p} />
            ))}
          </ListCard>
          {all.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
              scroll for more
            </div>
          )}
        </>
      )}
    </div>
  );
}

function ProjectRow({ project: p }: { project: ProjectRow }) {
  const lastActive = p.last_active ?? p.last_run_at;
  return (
    <MetricRow
      to={`/projects/${encodeURIComponent(p.ws_id)}`}
      header={
        <>
          <span className="text-sm font-semibold text-ink">{p.name}</span>
          <span className="text-[11px] text-ink-mute font-mono truncate max-w-xs">{p.path}</span>
          {p.repo_remote && (
            <span className="inline-flex items-center gap-1 text-[10px] text-slate-600 bg-slate-100 rounded px-1.5 py-0.5">
              <GitFork size={10} />
              {p.repo_remote}
            </span>
          )}
          {p.branch && (
            <span className="inline-flex items-center gap-1 text-[10px] text-slate-600 bg-slate-100 rounded px-1.5 py-0.5">
              <GitBranch size={10} />
              {p.branch}
            </span>
          )}
        </>
      }
      trailing={
        <span className="shrink-0 text-[11px] text-ink-mute tabular-nums">
          {lastActive ? relativeTime(lastActive) : 'no runs'}
        </span>
      }
      metrics={[
        { label: 'runs', value: p.run_count ? String(p.run_count) : null },
        { label: 'tokens', value: p.usage ? formatTokens(p.usage.total_tokens) : null },
        { label: 'cost', value: p.usage ? formatCost(p.usage.cost_usd) : null },
      ]}
    />
  );
}

function ProjectsEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <FolderGit2 size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No projects yet</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        No projects yet — run an agent against a directory to register it as a project.
      </p>
    </div>
  );
}
