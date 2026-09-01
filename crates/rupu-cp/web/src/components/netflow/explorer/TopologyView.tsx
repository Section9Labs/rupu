// Topology view — three fixed columns (Workflows → Origins → Networks) of
// HTML node rows over an SVG ribbon underlay (`topologyLayout.ts` owns the
// pure geometry/connectivity math).
//
// Interaction contract (v3 mockup):
//   - Ribbon width = calls (never bytes); failed tone above 5% error rate.
//   - Hover a node → connected ribbons/nodes at full emphasis (0.55),
//     everything else fades to 0.05; no hover → uniform 0.22.
//   - Click a node → toggle that dimension's filter (facet semantics live
//     server-side); selected nodes get the brand outline.
//   - A node with zero calls under the current window/filters renders
//     DIMMED, never removed — out-of-scope nodes are already absent from
//     the server response entirely.
//
// SVG strokes go through `useThemeColors` (inline SVG can't take Tailwind
// classes with alpha) — the same bridge the retired NetflowGraph used.

import { useMemo, useState } from 'react';
import type { NodeAgg, SankeyView } from '../../../lib/netflow';
import { useThemeColors } from '../../../lib/useThemeColors';
import { cn } from '../../../lib/cn';
import { EmptyState } from '../../ui/EmptyState';
import {
  netflowRangeEmptyHint,
  netflowWindowApplied,
  type NetflowWindowEcho,
} from '../ScopeDisclosure';
import {
  buildPairSets,
  isConnected,
  layoutTopology,
  linkTouchesHover,
  COLUMN_WIDTH,
  COLUMN_X,
  TOPOLOGY_WIDTH,
  type HoverKey,
  type TopoDim,
} from './topologyLayout';

export interface TopologyViewProps {
  sankey: SankeyView;
  /** Currently-selected keys per dimension (drives the selected outline). */
  selected: Record<TopoDim, string[]>;
  onToggle: (dim: TopoDim, key: string) => void;
  appliedWindow?: NetflowWindowEcho;
}

const COLUMNS: { dim: TopoDim; title: string; pick: (s: SankeyView) => NodeAgg[] }[] = [
  { dim: 'wf', title: 'Workflows', pick: (s) => s.workflows },
  { dim: 'or', title: 'Origins', pick: (s) => s.origins },
  { dim: 'org', title: 'Networks', pick: (s) => s.orgs },
];

export function TopologyView({ sankey, selected, onToggle, appliedWindow }: TopologyViewProps) {
  const colors = useThemeColors();
  const [hover, setHover] = useState<HoverKey | null>(null);
  const laid = useMemo(() => layoutTopology(sankey), [sankey]);
  const pairs = useMemo(() => buildPairSets(sankey), [sankey]);

  const empty =
    sankey.workflows.length === 0 && sankey.origins.length === 0 && sankey.orgs.length === 0;
  if (empty) {
    return netflowWindowApplied(appliedWindow) ? (
      <EmptyState title="No flows to graph in this range" hint={netflowRangeEmptyHint()} />
    ) : (
      <EmptyState
        title="No flows to graph for this scope"
        hint="Workflows connect to the origins they call through and the networks those calls reach."
      />
    );
  }

  return (
    <div className="overflow-x-auto">
      <div
        className="relative mx-auto"
        style={{ width: TOPOLOGY_WIDTH, maxWidth: '100%', height: laid.height }}
      >
        <svg
          width={TOPOLOGY_WIDTH}
          height={laid.height}
          className="pointer-events-none absolute inset-0"
          aria-hidden
        >
          {laid.links.map((l) => (
            <path
              key={l.key}
              d={l.d}
              fill="none"
              stroke={l.failed ? colors.get('status.failed') : colors.get('brand.500')}
              strokeOpacity={hover ? (linkTouchesHover(l, hover) ? 0.55 : 0.05) : 0.22}
              strokeWidth={l.width}
              strokeLinecap="round"
            />
          ))}
        </svg>
        {COLUMNS.map(({ dim, title, pick }, ci) => (
          <div
            key={dim}
            className="absolute top-0"
            style={{ left: COLUMN_X[ci], width: COLUMN_WIDTH }}
          >
            <p className="mb-1.5 text-meta font-semibold uppercase tracking-wider text-ink-mute">
              {title}
            </p>
            {pick(sankey).map((n) => {
              const on = selected[dim].includes(n.id);
              const dimmed = hover
                ? !isConnected(hover, dim, n.id, pairs)
                : n.calls === 0;
              return (
                <button
                  key={n.id}
                  type="button"
                  aria-pressed={on}
                  onClick={() => onToggle(dim, n.id)}
                  onMouseEnter={() => setHover({ dim, key: n.id })}
                  onMouseLeave={() => setHover(null)}
                  className={cn(
                    'mb-1.5 flex h-[34px] w-full items-center justify-between gap-2 rounded-lg border bg-panel px-2.5 text-left motion-safe:transition-[border-color,opacity] motion-safe:duration-100 hover:border-brand-500/50',
                    on ? 'border-brand-500/60 bg-brand-50' : 'border-border',
                    dimmed && 'opacity-35',
                  )}
                >
                  <span className="truncate font-mono text-note text-ink">{n.label}</span>
                  <span className="flex flex-none items-baseline gap-1.5 text-note tabular-nums text-ink-mute">
                    {n.errors > 0 && <span className="text-err">{n.errors} err</span>}
                    <span>{n.calls > 0 ? n.calls : '·'}</span>
                  </span>
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}

export default TopologyView;
