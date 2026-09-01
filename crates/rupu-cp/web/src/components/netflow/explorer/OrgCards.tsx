// Per-org endpoint cards (Topology view only): one card per ASN org,
// listing its endpoints with calls / errors / p95; a row toggles that
// endpoint's filter.
//
// Data comes straight from `ExplorerResponse.timeline.lanes` — the lanes
// already carry the org grouping, per-endpoint stats, and the exact
// facet semantics these cards need (window + every filter EXCEPT the
// endpoint dimension, so a selected endpoint's siblings stay visible),
// and the server sorts them busiest-org-first with lanes contiguous per
// org. Re-aggregating here would just be a second implementation.

import type { LaneAgg } from '../../../lib/netflow';
import { cn } from '../../../lib/cn';

export interface OrgCardsProps {
  lanes: LaneAgg[];
  selectedHosts: string[];
  onToggleHost: (endpointKey: string) => void;
}

interface OrgGroup {
  orgId: string;
  org: string;
  asn?: number;
  calls: number;
  lanes: LaneAgg[];
}

function groupByOrg(lanes: LaneAgg[]): OrgGroup[] {
  const groups: OrgGroup[] = [];
  for (const lane of lanes) {
    const last = groups[groups.length - 1];
    if (last && last.orgId === lane.org_id) {
      last.lanes.push(lane);
      last.calls += lane.calls;
    } else {
      groups.push({
        orgId: lane.org_id,
        org: lane.org,
        asn: lane.asn,
        calls: lane.calls,
        lanes: [lane],
      });
    }
  }
  return groups;
}

export function OrgCards({ lanes, selectedHosts, onToggleHost }: OrgCardsProps) {
  const groups = groupByOrg(lanes);
  if (groups.length === 0) return null;

  return (
    <div className="grid gap-3 [grid-template-columns:repeat(auto-fill,minmax(300px,1fr))]">
      {groups.map((g) => (
        <div key={g.orgId} className="overflow-hidden rounded-xl border border-border bg-panel">
          <div className="flex items-baseline justify-between gap-2 border-b border-border bg-surface/60 px-3.5 py-2.5">
            <span className="text-note font-semibold text-ink">{g.org}</span>
            <span className="font-mono text-meta text-ink-mute">
              {g.asn != null ? `AS${g.asn} · ` : ''}
              {g.calls} calls
            </span>
          </div>
          {g.lanes.map((lane) => {
            const key = `${lane.host}:${lane.port}`;
            const selected = selectedHosts.includes(key);
            return (
                <button
                  key={key}
                  type="button"
                  onClick={() => onToggleHost(key)}
                  aria-pressed={selected}
                  title="Filter everything to this endpoint"
                  className={cn(
                    'flex w-full items-center gap-2 border-t border-border px-3.5 py-2 text-left text-note first:border-t-0 hover:bg-surface-hover/50',
                    selected && 'bg-brand-50',
                  )}
                >
                  <span className="min-w-0 flex-1 truncate font-mono text-ink">{key}</span>
                  <span className="flex-none tabular-nums text-err">
                    {lane.errors > 0 ? `${lane.errors} err` : ''}
                  </span>
                  <span className="w-14 flex-none text-right tabular-nums text-ink-mute">
                    {lane.calls} calls
                  </span>
                  <span className="w-14 flex-none text-right tabular-nums text-ink-mute">
                    {lane.p95_ms != null ? `${lane.p95_ms} ms` : '—'}
                  </span>
                </button>
            );
          })}
        </div>
      ))}
    </div>
  );
}

export default OrgCards;
