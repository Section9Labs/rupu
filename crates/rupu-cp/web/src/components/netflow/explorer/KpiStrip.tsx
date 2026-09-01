// KPI strip — six cards over the currently windowed + filtered set,
// entirely server-computed (`ExplorerResponse.kpis`); the only arithmetic
// here is the error-rate percentage for DISPLAY (a division of two
// server-computed counts, not a re-aggregation).
//
// Honesty rules:
//   - `bytes_in`/`bytes_out` are OBSERVED-only sums: `null` renders as an
//     em dash via `formatBytes` (a sum of nothing observed is not `0 B`),
//     and `bytes_partial` marks the sums with `†` + tooltip whenever any
//     in-view flow's byte count was unobservable (coarse fidelity).
//   - `p95_ms` absent means "no flow contributed a duration" — an em
//     dash, never `0 ms`.

import { formatBytes, type KpiView } from '../../../lib/netflow';

const BYTES_PARTIAL_TIP =
  '† observed (HTTP-fidelity) flows only — coarse flows have unobservable byte counts, so the true total is at least this.';

interface Card {
  label: string;
  value: string;
  unit?: string;
  tip?: string;
  error?: boolean;
}

export function KpiStrip({ kpis }: { kpis: KpiView }) {
  const errorRate =
    kpis.flows > 0 ? `${((100 * kpis.errors) / kpis.flows).toFixed(1)}` : '—';
  const cards: Card[] = [
    { label: 'Flows', value: String(kpis.flows), tip: 'Recorded flows matching the current filters' },
    { label: 'Endpoints', value: String(kpis.endpoints), tip: 'Distinct host:port endpoints reached' },
    { label: 'Networks', value: String(kpis.orgs), tip: 'Distinct ASN organizations' },
    {
      label: 'Error rate',
      value: errorRate,
      unit: kpis.flows > 0 ? '%' : undefined,
      tip: `${kpis.errors} flows with a non-ok outcome`,
      error: kpis.errors > 0,
    },
    {
      label: 'Bytes in',
      value: formatBytes(kpis.bytes_in),
      unit: kpis.bytes_partial ? '†' : undefined,
      tip: kpis.bytes_partial ? BYTES_PARTIAL_TIP : 'Sum of observed response bytes',
    },
    {
      label: 'p95 latency',
      value: kpis.p95_ms != null ? String(kpis.p95_ms) : '—',
      unit: kpis.p95_ms != null ? 'ms' : undefined,
      tip: '95th percentile flow duration',
    },
  ];

  return (
    <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3 lg:grid-cols-6">
      {cards.map((c) => (
        <div
          key={c.label}
          title={c.tip}
          className="rounded-[10px] border border-border bg-panel px-3.5 py-2.5"
        >
          <p className="text-meta uppercase tracking-wider text-ink-mute">{c.label}</p>
          <p
            className={`mt-1 text-xl font-semibold leading-tight tabular-nums ${
              c.error ? 'text-err' : 'text-ink'
            }`}
          >
            {c.value}
            {c.unit && <span className="text-note font-normal text-ink-mute"> {c.unit}</span>}
          </p>
        </div>
      ))}
    </div>
  );
}

export default KpiStrip;
