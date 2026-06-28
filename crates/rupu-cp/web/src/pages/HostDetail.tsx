// Host detail — basic info/health for a single host + that host's workflow runs.
// Route: /hosts/:id

import { useEffect, useState } from 'react';
import { useParams, Link } from 'react-router-dom';
import { ChevronLeft } from 'lucide-react';
import { api, type HostView, type RunListRow } from '../lib/api';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { SectionHeader } from '../components/lists/SectionHeader';
import { StatusPill } from '../components/StatusPill';
import { relativeTime } from '../lib/time';
import { HostStatusBadge } from '../components/ui/HostStatusBadge';
import type { HostTransportKind } from '../lib/api';

// ---------------------------------------------------------------------------
// Visual tokens
// ---------------------------------------------------------------------------

const TRANSPORT_LABEL: Record<HostTransportKind, string> = {
  local: 'local',
  http_cp: 'http-cp',
};

// ---------------------------------------------------------------------------
// Run columns (minimal — matches WorkflowRuns column shape)
// ---------------------------------------------------------------------------

const RUN_COLUMNS: Column<RunListRow>[] = [
  {
    key: 'id',
    header: 'Run',
    render: (r) => (
      <Link
        to={`/runs/${encodeURIComponent(r.id)}`}
        className="text-sm font-mono text-brand-600 hover:text-brand-700 hover:underline"
      >
        {r.id.slice(0, 10)}…
      </Link>
    ),
  },
  {
    key: 'workflow',
    header: 'Workflow',
    sortable: true,
    sortValue: (r) => r.workflow_name,
    render: (r) => <span className="text-sm text-ink">{r.workflow_name}</span>,
  },
  {
    key: 'status',
    header: 'Status',
    sortable: true,
    sortValue: (r) => r.status,
    render: (r) => <StatusPill status={r.status} size="xs" />,
  },
  {
    key: 'started',
    header: 'Started',
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
      {/* Breadcrumb */}
      <nav className="mb-4">
        <Link
          to="/hosts"
          className="inline-flex items-center gap-1 text-sm text-ink-dim hover:text-ink"
        >
          <ChevronLeft size={14} />
          Hosts
        </Link>
      </nav>

      {/* Host info */}
      {hostError ? (
        <div className="mb-6 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {hostError}
        </div>
      ) : host === null ? (
        <div className="mb-6 text-sm text-ink-dim">Loading host…</div>
      ) : (
        <div className="mb-8">
          <header className="mb-4 flex items-start gap-3">
            <div>
              <h1 className="text-2xl font-semibold text-ink">{host.name}</h1>
              <p className="mt-0.5 text-sm font-mono text-ink-mute">{host.id}</p>
            </div>
            <HostStatusBadge status={host.status} />
          </header>

          <dl className="grid grid-cols-2 gap-x-8 gap-y-3 sm:grid-cols-4 rounded-xl border border-border bg-panel px-5 py-4 text-sm">
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

        {runsError && (
          <div className="mb-4 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
            {runsError}
          </div>
        )}

        {runs === null ? (
          <div className="text-sm text-ink-dim">Loading runs…</div>
        ) : runs.length === 0 ? (
          <div className="rounded-xl border border-dashed border-border bg-panel/50 py-10 flex items-center justify-center">
            <p className="text-sm text-ink-dim">No runs found for this host.</p>
          </div>
        ) : (
          <SortableTable<RunListRow>
            columns={RUN_COLUMNS}
            rows={runs}
            rowKey={(r) => r.id}
            initialSort={{ key: 'started', dir: 'desc' }}
          />
        )}
      </section>
    </div>
  );
}
