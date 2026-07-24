// Projects registry — lists all workspaces tracked by this control plane.
// Each row is a project card that links to /projects/:wsId for the overview.

import { useEffect, useState } from 'react';
import { GitBranch, GitFork, Github, Gitlab, HardDrive, Server } from 'lucide-react';
import { api, type ProjectRow } from '../lib/api';
import { projectProvider, providerLabel, type ProjectProvider } from '../lib/projectProvider';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import UsageBarChart from '../components/charts/UsageBarChart';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
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
    <div className="p-8">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Projects</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Workspaces registered with this control plane — each with its own runs, sessions, and
          coverage.
        </p>
      </header>

      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {projects === null && !error && (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading projects…" />
        </div>
      )}

      {projects !== null && projects.length === 0 && (
        <EmptyState
          title="No projects yet"
          hint="No projects yet — run an agent against a directory to register it as a project."
        />
      )}

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
          <SortableTable<ProjectRow>
            columns={PROJECT_COLUMNS}
            rows={shown}
            rowKey={(p) => p.ws_id}
            rowHref={(p) => `/projects/${encodeURIComponent(p.ws_id)}`}
            initialSort={{ key: 'last_active', dir: 'desc' }}
          />
          {all.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
              scroll for more
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** Effective "last active" timestamp for a project (falls back to last run). */
function projectLastActive(p: ProjectRow): string | null | undefined {
  return p.last_active ?? p.last_run_at;
}

/** Leading-icon component for a project's SCM provider. Muted to fit the
 *  instrument look; carries a `title`/`aria-label` so the row is identifiable
 *  without relying on the glyph alone. */
export function ProviderIcon({ remote }: { remote?: string | null }) {
  const provider: ProjectProvider = projectProvider(remote);
  const label = providerLabel(remote);
  const Icon =
    provider === 'github'
      ? Github
      : provider === 'gitlab'
        ? Gitlab
        : provider === 'local'
          ? HardDrive
          : Server;
  return (
    <span
      className="inline-flex text-ink-dim"
      title={label}
      role="img"
      aria-label={label}
    >
      <Icon size={30} />
    </span>
  );
}

// Sort ordering for the provider column (groups same-host projects together).
const PROVIDER_ORDER: Record<ProjectProvider, number> = {
  github: 0,
  gitlab: 1,
  remote: 2,
  local: 3,
};

const PROJECT_COLUMNS: Column<ProjectRow>[] = [
  {
    key: 'provider',
    header: 'Source',
    fit: true,
    sortable: true,
    sortValue: (p) => PROVIDER_ORDER[projectProvider(p.repo_remote)],
    render: (p) => <ProviderIcon remote={p.repo_remote} />,
  },
  {
    key: 'name',
    header: 'Name',
    subject: true,
    sortable: true,
    sortValue: (p) => p.name,
    titleValue: (p) => p.name,
    render: (p) => <span className="text-sm font-semibold text-ink">{p.name}</span>,
  },
  {
    key: 'path',
    header: 'Path',
    fit: true,
    sortable: true,
    sortValue: (p) => p.path,
    render: (p) => (
      <span className="text-note text-ink-mute font-mono truncate block max-w-xs">{p.path}</span>
    ),
  },
  {
    key: 'branch',
    header: 'Branch',
    fit: true,
    sortable: true,
    sortValue: (p) => p.branch ?? null,
    render: (p) => (
      <div className="flex items-center gap-1.5 flex-wrap">
        {p.repo_remote && (
          <span className="inline-flex items-center gap-1 text-meta text-ink bg-surface rounded px-1.5 py-0.5">
            <GitFork size={10} />
            {p.repo_remote}
          </span>
        )}
        {p.branch ? (
          <span className="inline-flex items-center gap-1 text-meta text-ink bg-surface rounded px-1.5 py-0.5">
            <GitBranch size={10} />
            {p.branch}
          </span>
        ) : (
          !p.repo_remote && <span className="text-ink-mute">—</span>
        )}
      </div>
    ),
  },
  {
    key: 'runs',
    header: 'Runs',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (p) => p.run_count,
    render: (p) => <span className="text-ink">{p.run_count ? String(p.run_count) : '—'}</span>,
  },
  {
    key: 'tokens',
    header: 'Tokens',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (p) => p.usage?.total_tokens ?? null,
    render: (p) => (
      <span className="text-ink-dim">{p.usage ? formatTokens(p.usage.total_tokens) : '—'}</span>
    ),
  },
  {
    key: 'cost',
    header: 'Cost',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (p) => p.usage?.cost_usd ?? null,
    render: (p) => (
      <span className="text-ink font-medium">{p.usage ? formatCost(p.usage.cost_usd) : '—'}</span>
    ),
  },
  {
    key: 'last_active',
    header: 'Last active',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (p) => {
      const t = projectLastActive(p);
      return t ? Date.parse(t) : null;
    },
    render: (p) => {
      const t = projectLastActive(p);
      return <span className="text-ink-mute">{t ? relativeTime(t) : 'no runs'}</span>;
    },
  },
];
