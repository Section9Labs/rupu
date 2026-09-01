// Pure layout + hover-connectivity math for the explorer's Topology view
// (three fixed-x columns of HTML node rows over an SVG ribbon underlay).
// Deliberately NOT the retired bipartite `layout.ts` shape and not the DAG
// engine in `components/graph` — a workflows→origins→networks flow is a
// two-stage sankey, drawn as cubic béziers between row midpoints.
//
// Kept free of React/DOM imports so it's testable without rendering
// (`topologyLayout.test.ts`), mirroring the old `layout.ts` convention.
//
// Ribbon width scales by CALLS only — never bytes (Fix 4 rationale in the
// retired NetflowGraph.tsx applies to every visual weight in the redesign:
// Coarse flows' bytes sum to 0 by construction, so a byte weight renders
// "unobservable" as "tiny"). `calls` has no such gap.

import type { LinkAgg, SankeyView } from '../../../lib/netflow';

/** Fixed geometry, matching the approved v3 mockup: three 240px columns
 *  at x = 0 / 400 / 800 in a 1040px canvas; 34px rows on a 40px pitch
 *  under a 22px column header. */
export const TOPOLOGY_WIDTH = 1040;
export const COLUMN_WIDTH = 240;
export const COLUMN_X = [0, 400, 800] as const;
const HEADER_H = 22;
const ROW_PITCH = 40;
const ROW_MID = 17;

/** Which topology column a node (or a hover) belongs to. */
export type TopoDim = 'wf' | 'or' | 'org';

export interface PositionedLink {
  /** Stable render key. */
  key: string;
  from: string;
  to: string;
  /** `wf` for a workflows→origins ribbon, `or` for origins→networks. */
  fromDim: 'wf' | 'or';
  /** SVG path — cubic bézier between the two row midpoints. */
  d: string;
  /** Stroke width: `2 + 24·calls/max` across ALL links (call-weighted). */
  width: number;
  /** Error rate above 5% — rendered in the failed tone. */
  failed: boolean;
}

export interface TopologyLayout {
  /** SVG/canvas height: tall enough for the deepest column. */
  height: number;
  links: PositionedLink[];
}

function rowY(index: number): number {
  return HEADER_H + index * ROW_PITCH + ROW_MID;
}

function positionLinks(
  links: LinkAgg[],
  fromDim: 'wf' | 'or',
  x1: number,
  x2: number,
  fromIndex: Map<string, number>,
  toIndex: Map<string, number>,
  maxCalls: number,
): PositionedLink[] {
  return links.flatMap((l) => {
    const ia = fromIndex.get(l.from);
    const ib = toIndex.get(l.to);
    // A link whose endpoint isn't in the node universe can't be drawn —
    // shouldn't happen (the server builds links from the same flows the
    // universe covers) but a malformed response must not crash the view.
    if (ia === undefined || ib === undefined) return [];
    const y1 = rowY(ia);
    const y2 = rowY(ib);
    return [
      {
        key: `${fromDim}:${l.from}->${l.to}`,
        from: l.from,
        to: l.to,
        fromDim,
        d: `M ${x1} ${y1} C ${x1 + 70} ${y1}, ${x2 - 70} ${y2}, ${x2} ${y2}`,
        width: 2 + (24 * l.calls) / maxCalls,
        failed: l.calls > 0 && l.errors / l.calls > 0.05,
      },
    ];
  });
}

export function layoutTopology(sankey: SankeyView): TopologyLayout {
  const index = (ids: { id: string }[]) => new Map(ids.map((n, i) => [n.id, i]));
  const wfIndex = index(sankey.workflows);
  const orIndex = index(sankey.origins);
  const orgIndex = index(sankey.orgs);

  const maxCalls = Math.max(
    1,
    ...[...sankey.wf_origin, ...sankey.origin_org].map((l) => l.calls),
  );

  const rows = Math.max(
    sankey.workflows.length,
    sankey.origins.length,
    sankey.orgs.length,
    1,
  );

  return {
    height: HEADER_H + rows * ROW_PITCH + 6,
    links: [
      ...positionLinks(
        sankey.wf_origin,
        'wf',
        COLUMN_X[0] + COLUMN_WIDTH,
        COLUMN_X[1],
        wfIndex,
        orIndex,
        maxCalls,
      ),
      ...positionLinks(
        sankey.origin_org,
        'or',
        COLUMN_X[1] + COLUMN_WIDTH,
        COLUMN_X[2],
        orIndex,
        orgIndex,
        maxCalls,
      ),
    ],
  };
}

// ── hover connectivity ──────────────────────────────────────────────────
// Hovering a node highlights every ribbon and node on a path through it
// and fades the rest. Membership is tested against the two adjacency sets
// below; keys are joined with a control character no dimension key can
// contain (origin keys carry `:`, so `:`-joining would be ambiguous).

const SEP = '\u0000';

export interface PairSets {
  /** workflow→origin adjacency, from `sankey.wf_origin`. */
  ab: Set<string>;
  /** origin→org adjacency, from `sankey.origin_org`. */
  bc: Set<string>;
}

export function buildPairSets(sankey: SankeyView): PairSets {
  return {
    ab: new Set(sankey.wf_origin.map((l) => l.from + SEP + l.to)),
    bc: new Set(sankey.origin_org.map((l) => l.from + SEP + l.to)),
  };
}

export interface HoverKey {
  dim: TopoDim;
  key: string;
}

/** Whether the node `(dim, key)` sits on a path through the hovered node. */
export function isConnected(
  hover: HoverKey,
  dim: TopoDim,
  key: string,
  pairs: PairSets,
): boolean {
  if (hover.dim === dim) return hover.key === key;
  const has = (set: Set<string>, a: string, b: string) => set.has(a + SEP + b);
  if (hover.dim === 'wf' && dim === 'or') return has(pairs.ab, hover.key, key);
  if (hover.dim === 'or' && dim === 'wf') return has(pairs.ab, key, hover.key);
  if (hover.dim === 'or' && dim === 'org') return has(pairs.bc, hover.key, key);
  if (hover.dim === 'org' && dim === 'or') return has(pairs.bc, key, hover.key);
  if (hover.dim === 'wf' && dim === 'org') {
    // Two hops: any origin adjacent to BOTH the hovered workflow and this org.
    return [...pairs.ab]
      .filter((p) => p.startsWith(hover.key + SEP))
      .some((p) => has(pairs.bc, p.split(SEP)[1], key));
  }
  if (hover.dim === 'org' && dim === 'wf') {
    return [...pairs.bc]
      .filter((p) => p.endsWith(SEP + hover.key))
      .some((p) => has(pairs.ab, key, p.split(SEP)[0]));
  }
  return true;
}

/** Whether a ribbon touches the hovered node (drawn at full emphasis). */
export function linkTouchesHover(link: PositionedLink, hover: HoverKey | null): boolean {
  if (!hover) return false;
  return link.from === hover.key || link.to === hover.key;
}
