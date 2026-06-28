// Workers list — workers registered with this control plane, with their
// declared capabilities, run-activity, and last-seen freshness. No detail
// route. Read-only.
//
// A "worker" is a LOCAL EXECUTION IDENTITY (per-machine/identity, not per-run):
// it registers/refreshes whenever you `rupu workflow run` or send a session
// turn. The explainer panel up top makes that explicit so the list isn't
// mistaken for a per-run process table.

import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Server } from 'lucide-react';
import { api, type WorkerView } from '../lib/api';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { SectionHeader } from '../components/lists/SectionHeader';
import { cn } from '../lib/cn';
import { relativeTime } from '../lib/time';

// A worker is considered stale if it hasn't been seen in this many ms.
const STALE_MS = 5 * 60 * 1000;

/** Local short-id truncation (a shared helper is being added separately). */
function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 10)}…` : id;
}

function isStale(lastSeen: string): boolean {
  const t = Date.parse(lastSeen);
  if (Number.isNaN(t)) return false;
  return Date.now() - t > STALE_MS;
}

export default function Workers() {
  const [workers, setWorkers] = useState<WorkerView[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getWorkers()
      .then((data) => {
        if (cancelled) return;
        setWorkers(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load workers');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="p-8">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Workers</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Local execution identities registered with this control plane.
        </p>
      </header>

      <Explainer />

      {error && (
        <div className="mb-4 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}

      {workers === null ? (
        <div className="text-sm text-ink-dim">Loading workers…</div>
      ) : workers.length === 0 ? (
        <EmptyState />
      ) : (
        <section>
          <SectionHeader tone="muted" label="Workers" count={workers.length} />
          <SortableTable<WorkerView>
            columns={COLUMNS}
            rows={workers}
            rowKey={(w) => w.worker_id}
            initialSort={{ key: 'name', dir: 'asc' }}
          />
        </section>
      )}
    </div>
  );
}

const COLUMNS: Column<WorkerView>[] = [
  {
    key: 'name',
    header: 'Name',
    sortable: true,
    sortValue: (w) => w.name,
    render: (w) => (
      <div className="min-w-0">
        <div className="text-sm font-medium text-ink truncate">{w.name}</div>
        <div className="text-note text-ink-mute font-mono truncate">{shortId(w.worker_id)}</div>
      </div>
    ),
  },
  {
    key: 'kind',
    header: 'Kind',
    sortable: true,
    sortValue: (w) => w.kind,
    render: (w) => (
      <Chip className="bg-surface text-ink-mute ring-border">{w.kind}</Chip>
    ),
  },
  {
    key: 'host',
    header: 'Host',
    render: (w) => <span className="text-note text-ink-mute font-mono">{w.host}</span>,
  },
  {
    key: 'active',
    header: 'Active runs',
    align: 'right',
    width: 'w-24',
    sortable: true,
    sortValue: (w) => w.active_run_count,
    render: (w) =>
      w.active_run_count > 0 ? (
        <Link
          to="/runs/workflows"
          className="font-medium text-brand-600 hover:text-brand-700 hover:underline tabular-nums"
        >
          {w.active_run_count}
        </Link>
      ) : (
        <span className="text-ink-mute tabular-nums">0</span>
      ),
  },
  {
    key: 'total',
    header: 'Total runs',
    align: 'right',
    width: 'w-24',
    sortable: true,
    sortValue: (w) => w.total_run_count,
    render: (w) => <span className="text-ink-dim tabular-nums">{w.total_run_count}</span>,
  },
  {
    key: 'last_run',
    header: 'Last run',
    sortable: true,
    sortValue: (w) => {
      if (!w.last_run_at) return null;
      const t = Date.parse(w.last_run_at);
      return Number.isNaN(t) ? null : t;
    },
    render: (w) =>
      w.last_run_at ? (
        <span className="text-ui text-ink-dim">{relativeTime(w.last_run_at)}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'last_seen',
    header: 'Last seen',
    sortable: true,
    sortValue: (w) => {
      const t = Date.parse(w.last_seen_at);
      return Number.isNaN(t) ? null : t;
    },
    render: (w) => {
      const stale = isStale(w.last_seen_at);
      return (
        <div className="flex items-center gap-2">
          <span className={cn('text-ui', stale ? 'text-warn' : 'text-ink-dim')}>
            {relativeTime(w.last_seen_at)}
          </span>
          {stale && <Chip className="bg-warn-bg text-warn ring-warn/30">stale</Chip>}
        </div>
      );
    },
  },
  {
    key: 'capabilities',
    header: 'Capabilities',
    render: (w) => <Capabilities worker={w} />,
  },
  {
    key: 'version',
    header: 'Version',
    align: 'right',
    width: 'w-16',
    render: (w) => <span className="text-note text-ink-mute tabular-nums">v{w.version}</span>,
  },
];

function Capabilities({ worker }: { worker: WorkerView }) {
  const caps = worker.capabilities ?? {};
  const backends = caps.backends ?? [];
  const scmHosts = caps.scm_hosts ?? [];
  const modes = caps.permission_modes ?? [];
  if (backends.length === 0 && scmHosts.length === 0 && modes.length === 0) {
    return <span className="text-note text-ink-mute">—</span>;
  }
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {backends.map((b) => (
        <Chip key={`b-${b}`} className="bg-info-bg text-info ring-info/30">
          {b}
        </Chip>
      ))}
      {scmHosts.map((h) => (
        <Chip key={`s-${h}`} className="bg-violet-50 text-violet-700 ring-violet-200">
          {h}
        </Chip>
      ))}
      {modes.map((m) => (
        <Chip key={`m-${m}`} className="bg-ok-bg text-ok ring-ok/30">
          {m}
        </Chip>
      ))}
    </div>
  );
}

function Chip({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-1.5 py-0.5 text-note font-medium ring-1',
        className,
      )}
    >
      {children}
    </span>
  );
}

function Explainer() {
  return (
    <div className="mb-6 rounded-xl border border-border bg-panel/50 px-4 py-3 text-sm text-ink-dim">
      <p>
        A <span className="font-medium text-ink">worker</span> is a local execution identity — it
        registers (or refreshes) whenever you run a workflow or send a session turn. It is
        per-machine, <span className="font-medium text-ink">not per-run</span>: launching work in
        the background refreshes an existing worker rather than spawning a new one here.
      </p>
      <ul className="mt-2 flex flex-col gap-1 text-lead">
        <li>
          <Chip className="mr-1.5 bg-surface text-ink-mute ring-border">cli</Chip>
          your machine&apos;s rupu CLI.
        </li>
        <li>
          <Chip className="mr-1.5 bg-surface text-ink-mute ring-border">autoflow_serve</Chip>
          the autoflow daemon.
        </li>
        <li>
          <Chip className="mr-1.5 bg-warn-bg text-warn ring-warn/30">stale</Chip>
          not seen in over 5 minutes.
        </li>
      </ul>
    </div>
  );
}

function EmptyState() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-surface flex items-center justify-center mb-3">
        <Server size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No workers registered</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Workers appear here once you run a workflow or send a session turn on a machine connected to
        this control plane.
      </p>
    </div>
  );
}
