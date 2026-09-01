// Flow detail — the right-hand slide-over a table row click opens: the
// full record, with the fidelity badge + its explanation line (fidelity
// moved here from the old table column) and the em-dash note for coarse
// rows ("not observable, never zero").
//
// Every optional field renders an em dash when absent, through the same
// helpers the table uses (`formatBytes` for bytes; explicit dashes for
// timings/IPs) — this panel is where a curious reader lands, so it is
// exactly where "we could not see it" must never read as "it was zero".

import { formatBytes, type FlowView } from '../../../lib/netflow';
import { FidelityBadge, FIDELITY_TITLE } from '../FidelityBadge';
import { Badge } from '../../ui/Badge';
import type { NetflowScope } from '../ScopeDisclosure';

export interface FlowDetailPanelProps {
  flow: FlowView | null;
  scope: NetflowScope;
  onClose: () => void;
}

const COARSE_NOTE =
  'Em dashes mean "not observable", never zero — this connector cannot report byte counts or peer IPs. Host, outcome and timing are real.';
const OBSERVED_NOTE = 'All fields observed directly by the instrumented HTTP client.';

export function FlowDetailPanel({ flow, scope, onClose }: FlowDetailPanelProps) {
  if (!flow) return null;
  const coarse = flow.fidelity === 'coarse';
  const rows: { k: string; v: string }[] = [
    { k: 'Time', v: new Date(flow.ts).toLocaleString() },
    // Attribution rows only where they mean something: at run scope every
    // flow belongs to the run already on screen.
    ...(scope !== 'run'
      ? [
          { k: 'Run', v: flow.run_id ?? '—' },
          { k: 'Workflow', v: flow.workflow ?? '—' },
        ]
      : []),
    { k: 'Origin', v: flow.ctx.origin.name ?? flow.ctx.origin.kind },
    { k: 'Status', v: flow.status != null ? String(flow.status) : '—' },
    { k: 'Bytes in', v: formatBytes(flow.bytes_in) },
    { k: 'Bytes out', v: formatBytes(flow.bytes_out) },
    { k: 'TTFB', v: flow.ttfb_ms != null ? `${flow.ttfb_ms} ms` : '—' },
    { k: 'Duration', v: flow.duration_ms != null ? `${flow.duration_ms} ms` : '—' },
    { k: 'Peer IP', v: flow.peer_ip ?? '—' },
    { k: 'Network', v: flow.asn ? `AS${flow.asn.asn} ${flow.asn.org}` : '—' },
    ...(flow.error ? [{ k: 'Error', v: flow.error }] : []),
  ];

  return (
    <aside
      role="dialog"
      aria-label="Flow detail"
      className="fixed inset-y-0 right-0 z-50 w-[400px] max-w-[92vw] overflow-y-auto border-l border-border bg-panel shadow-2xl"
    >
      <div className="flex items-center justify-between border-b border-border px-5 py-4">
        <p className="text-meta font-semibold uppercase tracking-wider text-ink-mute">
          Flow detail
        </p>
        <button
          type="button"
          aria-label="Close flow detail"
          onClick={onClose}
          className="text-base leading-none text-ink-mute hover:text-ink"
        >
          ×
        </button>
      </div>
      <div className="p-5">
        <p className="font-mono text-ui font-semibold text-ink">
          {flow.host}:{flow.port}
        </p>
        <p className="mb-3.5 break-all font-mono text-note text-ink-dim">
          {flow.method} {flow.path}
        </p>
        <div className="mb-4 flex items-center gap-2">
          <Badge tone={flow.outcome === 'ok' ? 'green' : 'red'} size="md">
            {flow.outcome}
            {flow.status != null ? ` · ${flow.status}` : ''}
          </Badge>
          <FidelityBadge fidelity={flow.fidelity} />
        </div>
        <p className="mb-3 text-note leading-relaxed text-ink-dim">
          {FIDELITY_TITLE[flow.fidelity]}
        </p>
        {rows.map((r) => (
          <div
            key={r.k}
            className="flex justify-between gap-3 border-b border-border py-1.5 text-ui"
          >
            <span className="flex-none text-ink-mute">{r.k}</span>
            <span className="break-all text-right font-mono text-note text-ink">{r.v}</span>
          </div>
        ))}
        <p className="mt-3.5 text-note leading-relaxed text-ink-mute">
          {coarse ? COARSE_NOTE : OBSERVED_NOTE}
        </p>
      </div>
    </aside>
  );
}

export default FlowDetailPanel;
