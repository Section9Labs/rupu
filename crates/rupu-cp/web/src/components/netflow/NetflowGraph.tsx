// NetflowGraph — bipartite topology: sources (runs, or `system` for
// unattributed egress) on the left, endpoints (`host:port`) on the right.
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
// GraphEdge.bytes is a VISUAL WEIGHT (`ledger::graph_view` sums
// bytes_in/out with `unwrap_or(0)`, so a Coarse flow contributes 0), never
// an honest total — this component only ever feeds it through
// `layoutBipartite` as a stroke-width scale. It is never rendered as a
// label, tooltip, or byte count.

import { useMemo } from 'react';
import type { GraphView } from '../../lib/netflow';
import { useThemeColors } from '../../lib/useThemeColors';
import { EmptyState } from '../ui/EmptyState';
import { layoutBipartite, type LayoutOpts } from './layout';

export interface NetflowGraphProps {
  graph: GraphView;
  /** Defaults to `calls` — see `LayoutOpts.weightBy` doc for why. */
  weightBy?: LayoutOpts['weightBy'];
  width?: number;
}

const ROW_HEIGHT = 34;
const NODE_R = 4;

export function NetflowGraph({ graph, weightBy = 'calls', width = 880 }: NetflowGraphProps) {
  const colors = useThemeColors();
  const laid = useMemo(
    () => layoutBipartite(graph, { width, rowHeight: ROW_HEIGHT, weightBy }),
    [graph, width, weightBy],
  );

  if (graph.nodes.length === 0) {
    return (
      <EmptyState
        title="No flows to graph for this scope"
        hint="Sources — runs, or system for unattributed egress (updater, ASN refresh, CP fleet traffic) — connect to the host:port endpoints they reached."
      />
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
