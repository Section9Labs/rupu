// Host detail — basic info/health for a single host + that host's workflow runs.
// Route: /hosts/:id

import { useEffect, useState } from 'react';
import { useParams } from 'react-router-dom';
import { api, type HostView, type RunListRow } from '../lib/api';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { SectionHeader } from '../components/lists/SectionHeader';
import { ListSkeleton } from '../components/lists/ListSkeleton';
import { StatusPill } from '../components/StatusPill';
import { relativeTime } from '../lib/time';
import { runHref } from '../lib/runs';
import { HostStatusBadge } from '../components/ui/HostStatusBadge';
import { PageHeader } from '../components/ui/PageHeader';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import type { HostTransportKind } from '../lib/api';

const BACK = { label: 'Hosts', to: '/hosts' };

// ---------------------------------------------------------------------------
// Visual tokens
// ---------------------------------------------------------------------------

const TRANSPORT_LABEL: Record<HostTransportKind, string> = {
  local: 'local',
  http_cp: 'http-cp',
  tunnel: 'tunnel',
  ssh: 'SSH',
  bucket: 'Bucket',
};

// ---------------------------------------------------------------------------
// Run columns (minimal — matches WorkflowRuns column shape)
// ---------------------------------------------------------------------------

const RUN_COLUMNS: Column<RunListRow>[] = [
  {
    key: 'status',
    header: 'Status',
    fit: true,
    sortable: true,
    sortValue: (r) => r.status,
    render: (r) => <StatusPill status={r.status} size="xs" />,
  },
  {
    key: 'workflow',
    header: 'Workflow',
    subject: true,
    sortable: true,
    sortValue: (r) => r.workflow_name,
    titleValue: (r) => r.workflow_name,
    render: (r) => <span className="text-sm text-ink">{r.workflow_name}</span>,
  },
  {
    key: 'id',
    header: 'Run',
    fit: true,
    // Plain content — row-level navigation is `rowHref` (SortableTable
    // link-wraps the whole row); an inline <Link> here would nest an <a>
    // inside SortableTable's own <a>.
    render: (r) => (
      <span className="text-sm font-mono text-ink-dim">{r.id.slice(0, 10)}…</span>
    ),
  },
  {
    key: 'started',
    header: 'Started',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => {
      const t = Date.parse(r.started_at);
      return Number.isNaN(t) ? null : t;
    },
    render: (r) => <span className="text-ui text-ink-dim">{relativeTime(r.started_at)}</span>,
  },
];

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function HostDetail() {
  const { id } = useParams<{ id: string }>();
  const [host, setHost] = useState<HostView | null>(null);
  const [runs, setRuns] = useState<RunListRow[] | null>(null);
  const [hostError, setHostError] = useState<string | null>(null);
  const [runsError, setRunsError] = useState<string | null>(null);

  useEffect(() => {
    if (!id) return;
    let cancelled = false;

    // Fetch host info: filter the getHosts list to this id
    api.getHosts().then((data) => {
      if (cancelled) return;
      const found = data.find((h) => h.id === id);
      if (!found) {
        setHostError(`Host "${id}" not found`);
      } else {
        setHost(found);
      }
    }).catch((e: unknown) => {
      if (cancelled) return;
      setHostError(e instanceof Error ? e.message : 'Failed to load host');
    });

    // Fetch runs scoped to this host
    api.getWorkflowRuns({ host: id }).then((data) => {
      if (cancelled) return;
      setRuns(data);
    }).catch((e: unknown) => {
      if (cancelled) return;
      setRunsError(e instanceof Error ? e.message : 'Failed to load runs');
    });

    return () => { cancelled = true; };
  }, [id]);

  return (
    <div className="p-8">
      {/* Host info */}
      {hostError ? (
        <>
          <PageHeader back={BACK} title="Host" />
          <ErrorBanner className="mt-4">{hostError}</ErrorBanner>
        </>
      ) : host === null ? (
        <>
          <PageHeader back={BACK} title="Host" />
          <div className="mt-6"><Spinner label="Loading host…" /></div>
        </>
      ) : (
        <div className="mb-8">
          <PageHeader
            back={BACK}
            title={host.name}
            subtitle={<span className="font-mono">{host.id}</span>}
            actions={<HostStatusBadge status={host.status} />}
          />

          <dl className="mt-6 grid grid-cols-2 gap-x-8 gap-y-3 sm:grid-cols-4 rounded-xl border border-border bg-panel px-5 py-4 text-sm">
            <div>
              <dt className="text-note font-medium text-ink-mute">Transport</dt>
              <dd className="mt-0.5 text-ink">{TRANSPORT_LABEL[host.transport_kind]}</dd>
            </div>
            {host.base_url && (
              <div>
                <dt className="text-note font-medium text-ink-mute">Base URL</dt>
                <dd className="mt-0.5 text-ink font-mono truncate">{host.base_url}</dd>
              </div>
            )}
            <div>
              <dt className="text-note font-medium text-ink-mute">Version</dt>
              <dd className="mt-0.5 text-ink font-mono">{host.version ?? '—'}</dd>
            </div>
            <div>
              <dt className="text-note font-medium text-ink-mute">Active runs</dt>
              <dd className="mt-0.5 text-ink tabular-nums">{host.active_run_count}</dd>
            </div>
            <div>
              <dt className="text-note font-medium text-ink-mute">Last seen</dt>
              <dd className="mt-0.5 text-ink">{relativeTime(host.last_seen_at)}</dd>
            </div>
            {host.capabilities && (
              <div className="col-span-2 sm:col-span-3">
                <dt className="text-note font-medium text-ink-mute">Backends</dt>
                <dd className="mt-0.5 text-ink">
                  {host.capabilities.backends.join(', ') || '—'}
                </dd>
              </div>
            )}
          </dl>
        </div>
      )}

      {/* Runs */}
      <section>
        <SectionHeader
          tone="muted"
          label="Runs"
          count={runs?.length ?? 0}
          hint={host ? `on ${host.name}` : undefined}
        />

        {runsError && <ErrorBanner className="mb-4">{runsError}</ErrorBanner>}

        {runs === null ? (
          <ListSkeleton label="Loading runs…" />
        ) : runs.length === 0 ? (
          <EmptyState title="No runs found for this host." />
        ) : (
          <SortableTable<RunListRow>
            columns={RUN_COLUMNS}
            rows={runs}
            rowKey={(r) => r.id}
            rowHref={runHref}
            initialSort={{ key: 'started', dir: 'desc' }}
          />
        )}
      </section>
    </div>
  );
}
