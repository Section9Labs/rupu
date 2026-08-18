// NetflowGraph — bipartite topology: sources (runs — one node per run id
// the read side attributed a flow to; see ScopeDisclosure.tsx's Finding-4
// note, there is no `system` fallback source any more) on the left,
// endpoints (`host:port`) on the right.
//
// This is the honest alternative to a geographic map: nearly every endpoint
// resolves to a CDN edge, so a country pin would show a PoP, not who was
// actually contacted. A topology answers the real question — who reached
// what — without pretending to know something it doesn't.
//
// Colours come from `useThemeColors()`, the SAME bridge `components/graph`
// (RunGraph, StepNode, ActionNode, ...) uses to paint inline SVG/canvas
// consumers that can't go through Tailwind class interpolation — see that
// hook's header comment. There are no `--graph-edge` / `--graph-node-*` CSS
// custom properties in this repo (verified against styles.css); reusing
// "that directory's colour tokens" means reusing this mechanism and its
// semantic tokens, not inventing new var names. `brand.500` (source) /
// `info` (endpoint) are picked deliberately: `info` is documented in
// useThemeColors as the generic semantic-state blue already used for
// network-flow-adjacent charts (ThroughputChart), and `status.failed` is
// the same red RunGraph already uses to mark a problem edge.
//
// Edge stroke width is always scaled by call count (`layoutBipartite`),
// never by `GraphEdge.bytes` (Fix 4, netflow Plan 3 review round 3): a
// former "weight by bytes" control drew Coarse-fidelity edges — whose bytes
// sum to 0 by construction — at the same minimum width as a genuinely tiny
// transfer, i.e. the one place in the UI where "unobservable" rendered as a
// quantity. `bytes` is never rendered as a label, tooltip, or byte count
// here, or fed into layout at all anymore.

import { useMemo } from 'react';
import type { GraphView } from '../../lib/netflow';
import { useThemeColors } from '../../lib/useThemeColors';
import { EmptyState } from '../ui/EmptyState';
import { layoutBipartite } from './layout';
import {
  netflowRangeEmptyHint,
  netflowSystemSourceHint,
  netflowWindowApplied,
  type NetflowScope,
  type NetflowWindowEcho,
} from './ScopeDisclosure';

export interface NetflowGraphProps {
  graph: GraphView;
  /** Which Network tab this graph renders on. Historically gated the
   *  empty-state hint's "system" source examples the same way
   *  NetflowScopeDisclosure gated the main disclosure (Fix 2, round 4: CP
   *  fleet traffic; round 5: ASN refresh, both global-scope-only) — both
   *  examples are gone now (netflow-per-run plan, Task 8), so there is
   *  currently nothing left for `scope` to gate here. Kept on the prop so
   *  a future genuinely scope-specific example doesn't need a signature
   *  change to land. See `netflowSystemSourceHint`'s doc comment. */
  scope: NetflowScope;
  width?: number;
  /** `NetflowResponse.window` — the server's OWN echo of the `?from=`/
   *  `?to=` it actually applied to produce `graph`, NOT the picker's local
   *  UI state (Task 4, Critical 2 fix-round). Same contract as
   *  `NetflowTable`'s `appliedWindow`: picks the empty-state wording
   *  ("in this range" vs "for this scope") so this graph can never
   *  disagree with the table rendered alongside it on the same screen.
   *  Optional and defaulting to "no window" so a caller that doesn't pass
   *  it keeps the exact prior wording. */
  appliedWindow?: NetflowWindowEcho;
}

const ROW_HEIGHT = 34;
const NODE_R = 4;

export function NetflowGraph({ graph, scope, width = 880, appliedWindow }: NetflowGraphProps) {
  const colors = useThemeColors();
  const laid = useMemo(
    () => layoutBipartite(graph, { width, rowHeight: ROW_HEIGHT }),
    [graph, width],
  );

  if (graph.nodes.length === 0) {
    return netflowWindowApplied(appliedWindow) ? (
      <EmptyState title="No flows to graph in this range" hint={netflowRangeEmptyHint()} />
    ) : (
      <EmptyState title="No flows to graph for this scope" hint={netflowSystemSourceHint(scope)} />
    );
  }

  return (
    <svg
      width={width}
      height={laid.height}
      role="img"
      aria-label="Network topology: sources on the left, endpoints reached on the right"
    >
      {laid.edges.map((e) => (
        <line
          key={`${e.from}->${e.to}`}
          x1={e.x1}
          y1={e.y1}
          x2={e.x2}
          y2={e.y2}
          strokeWidth={e.width}
          strokeLinecap="round"
          stroke={e.hasErrors ? colors.get('status.failed') : colors.alpha('inkMute', 0.45)}
        />
      ))}
      {laid.nodes.map((n) => (
        <g key={n.id} transform={`translate(${n.x}, ${n.y})`}>
          <circle
            r={NODE_R}
            fill={n.side === 'source' ? colors.get('brand.500') : colors.get('info')}
          />
          <text
            x={n.side === 'source' ? -(NODE_R + 6) : NODE_R + 6}
            textAnchor={n.side === 'source' ? 'end' : 'start'}
            dominantBaseline="middle"
            style={{ fill: colors.ink, fontSize: 12 }}
          >
            {n.label}
          </text>
        </g>
      ))}
    </svg>
  );
}

export default NetflowGraph;
