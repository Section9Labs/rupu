// Per-flow netflow table — the primary Netflow-tab view for a run, project,
// or the global scope. Built on the same `SortableTable` + `EmptyState`
// primitives as `findings/FindingsTable.tsx` rather than a hand-rolled
// `<table>`, so markup, sorting affordance and empty-state look match the
// rest of the CP.
//
// Every honesty rule this subsystem exists to enforce lives here:
//   - `formatBytes` (Task 4) renders `null`/`undefined` as an em dash, never
//     `0 B` — an unobserved byte count must never read as a real zero.
//   - The fidelity badge is unconditional: every row gets one, so a Coarse
//     row never LOOKS as complete as an HTTP row.
//   - `dropped > 0` gets a loud banner — the list is silently incomplete
//     without it, which is the exact defect this subsystem prevents.
//   - `asnLoaded === false` gets its own note: a blank Network column would
//     read as "this peer has no ASN", not "enrichment wasn't available".
//   - The empty state states netflow's scope limit (rupu's own egress, not
//     the agent's `bash` subprocess) so an empty table doesn't imply "no
//     network activity happened".

import { formatBytes, type FlowView } from '../../lib/netflow';
import SortableTable, { type Column } from '../lists/SortableTable';
import { EmptyState } from '../ui/EmptyState';
import { FidelityBadge } from './FidelityBadge';

export interface NetflowTableProps {
  flows: FlowView[];
  /** Records lost to writer overflow. Non-zero means this list is
   *  incomplete — surfaced as a banner, never silently absorbed. */
  dropped: number;
  /** `false` means ASN enrichment was unavailable, not that flows lack an
   *  ASN. Drives an explanatory note rather than a blank Network column. */
  asnLoaded: boolean;
}

function originLabel(f: FlowView): string {
  return f.ctx.origin.name ?? f.ctx.origin.kind;
}

export function NetflowTable({ flows, dropped, asnLoaded }: NetflowTableProps) {
  if (flows.length === 0) {
    return (
      <EmptyState
        title="No network flows recorded for this scope"
        hint="Netflow covers rupu's own egress — provider APIs, SCM connectors, MCP and webhooks. It does not cover traffic from the agent's bash subprocess."
      />
    );
  }

  const columns: Column<FlowView>[] = [
    {
      key: 'ts',
      header: 'Time',
      fit: true,
      sortable: true,
      sortValue: (f) => f.ts,
      render: (f) => new Date(f.ts).toLocaleTimeString(),
    },
    {
      key: 'origin',
      header: 'Origin',
      fit: true,
      render: (f) => <span className="text-ink-dim">{originLabel(f)}</span>,
    },
    {
      key: 'host',
      header: 'Host',
      fit: true,
      sortable: true,
      sortValue: (f) => f.host,
      render: (f) => <span className="font-mono text-note">{f.host}</span>,
    },
    {
      key: 'path',
      header: 'Path',
      subject: true,
      titleValue: (f) => `${f.method} ${f.path}`,
      render: (f) => <span className="font-mono text-note text-ink-dim">{f.path}</span>,
    },
    {
      key: 'network',
      header: 'Network',
      fit: true,
      render: (f) =>
        f.asn ? (
          <span className="font-mono text-note text-ink-mute">
            AS{f.asn.asn} {f.asn.org}
          </span>
        ) : (
          <span className="text-ink-mute">—</span>
        ),
    },
    {
      key: 'status',
      header: 'Status',
      fit: true,
      align: 'right',
      render: (f) => (
        <span className={f.outcome === 'ok' ? 'text-ink-dim' : 'text-err'}>
          {f.status ?? '—'}
        </span>
      ),
    },
    {
      key: 'bytes_in',
      header: 'In',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (f) => f.bytes_in ?? null,
      render: (f) => formatBytes(f.bytes_in),
    },
    {
      key: 'bytes_out',
      header: 'Out',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (f) => f.bytes_out ?? null,
      render: (f) => formatBytes(f.bytes_out),
    },
    {
      key: 'duration_ms',
      header: 'Duration',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (f) => f.duration_ms ?? null,
      render: (f) => (f.duration_ms != null ? `${f.duration_ms} ms` : '—'),
    },
    {
      key: 'fidelity',
      header: 'Fidelity',
      fit: true,
      render: (f) => <FidelityBadge fidelity={f.fidelity} />,
    },
  ];

  return (
    <div className="space-y-3">
      {dropped > 0 && (
        <div
          role="status"
          className="rounded-lg border border-warn/30 bg-warn-bg px-4 py-2 text-sm text-warn"
        >
          <span className="font-medium">{dropped} flows dropped</span> — the capture buffer
          overflowed, so this list is incomplete.
        </div>
      )}
      {!asnLoaded && (
        <p className="text-note text-ink-mute">
          ASN data not loaded — network enrichment will appear once the table has been fetched.
        </p>
      )}
      <SortableTable<FlowView>
        columns={columns}
        rows={flows}
        rowKey={(f) => f.id}
        initialSort={{ key: 'ts', dir: 'desc' }}
      />
    </div>
  );
}

export default NetflowTable;
