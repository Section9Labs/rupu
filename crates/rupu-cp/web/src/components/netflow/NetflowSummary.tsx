// Host rollup for the Netflow tab — one row per (host, port), entirely
// server-computed. Uses the same `SortableTable` primitive as
// `NetflowTable`/`FindingsTable` for markup + sorting affordance, but does
// NO arithmetic beyond ordering: percentile math and the unknown-bytes rule
// live exactly once, in `rupu_netflow::ledger::host_rollup`. Re-deriving any
// of it here would risk a second, drifting implementation.

import { formatBytes, type HostRollup } from '../../lib/netflow';
import SortableTable, { type Column } from '../lists/SortableTable';

/** `p50_ms`/`p95_ms` are typed `number | undefined`: serde omits the key
 *  entirely (never `null`) when a percentile is unobservable. Accept
 *  `null` too so this stays correct even if a caller widens the type or
 *  passes a raw `HostRollup.bytes_in`-style field through by mistake —
 *  either way, "no data" renders the same, never `0 ms`. */
function formatMs(n: number | null | undefined): string {
  return n != null ? `${n} ms` : '—';
}

export function NetflowSummary({ hosts }: { hosts: HostRollup[] }) {
  if (hosts.length === 0) return null;

  const columns: Column<HostRollup>[] = [
    {
      key: 'host',
      header: 'Host',
      subject: true,
      sortable: true,
      sortValue: (h) => h.host,
      titleValue: (h) => `${h.host}:${h.port}`,
      render: (h) => <span className="font-mono text-note">{h.host}</span>,
    },
    {
      key: 'calls',
      header: 'Calls',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (h) => h.calls,
      render: (h) => h.calls,
    },
    {
      key: 'errors',
      header: 'Errors',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (h) => h.errors,
      render: (h) => (h.errors > 0 ? <span className="text-err">{h.errors}</span> : h.errors),
    },
    {
      key: 'bytes_in',
      header: 'In',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (h) => h.bytes_in,
      render: (h) => formatBytes(h.bytes_in),
    },
    {
      key: 'bytes_out',
      header: 'Out',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (h) => h.bytes_out,
      render: (h) => formatBytes(h.bytes_out),
    },
    {
      key: 'p50_ms',
      header: 'p50',
      fit: true,
      align: 'right',
      sortable: true,
      // Column['sortValue'] is `string | number | null` (no `undefined`);
      // `p50_ms` is `number | undefined` (key omitted, not nulled, when
      // unobservable — see `HostRollup`'s doc comment) so coerce for the
      // sort comparator only. `formatMs` above is what actually renders.
      sortValue: (h) => h.p50_ms ?? null,
      render: (h) => formatMs(h.p50_ms),
    },
    {
      key: 'p95_ms',
      header: 'p95',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (h) => h.p95_ms ?? null,
      render: (h) => formatMs(h.p95_ms),
    },
  ];

  return (
    <SortableTable<HostRollup>
      columns={columns}
      rows={hosts}
      rowKey={(h) => `${h.host}:${h.port}`}
      initialSort={{ key: 'calls', dir: 'desc' }}
    />
  );
}

export default NetflowSummary;
