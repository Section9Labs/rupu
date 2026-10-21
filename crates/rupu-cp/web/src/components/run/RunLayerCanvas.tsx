// The run graph's message and board layer on its canvas (RunGraph's
// `overlay`, spec `2026-10-21-rupu-agentiflow-flow-design.md` goal 6): the
// board rail a gutter right of the cards and the messages as arcs between
// the cards (the operator's, an off-map address's and a dead end as short
// stubs at the card they touch), routed by `flowEdges` over a run router.
// The rail and arcs are the agentiflow map's own components (flowmap/).

import { useCallback, useMemo, useState } from 'react';
import { Handle, Position, type Edge, type Node, type NodeMouseHandler } from '@xyflow/react';
import type { RunGraphModel } from '../../lib/runGraphModel';
import type { Pos } from '../../lib/graphLayout';
import type { Size } from '../../lib/graph/measured';
import { KIND_FILTERS, type KindFilter } from '../../lib/messages';
import { BOARD, isDeadEnd, isOutside, type BoardSource, type FlowMessage } from '../../lib/flow/flowModel';
import {
  RAIL,
  RAIL_LIMIT,
  boardAnchors,
  boardKey,
  claimMessages,
  messageArcs,
  railWindow,
  type Anchor,
  type RailSection,
  type Router,
} from '../../lib/flow/flowEdges';
import { boardAt, RUN_SPACE } from '../../lib/flow/runLayer';
import { useArrivals } from '../../lib/flow/useArrivals';
import { useStableList } from '../../lib/flow/useStableList';
import type { MessageFlowEdge } from '../flowmap/MessageEdge';

const sameId = (a: FlowMessage, b: FlowMessage) => a.id === b.id;
const ALL_KINDS: ReadonlySet<KindFilter> = new Set(KIND_FILTERS.map((k) => k.key));
/** The rail sits this far right of the rightmost card: one wide gutter, so
 *  board arcs stay short. */
export const RUN_RAIL_GAP = 64;

/** The run's message and board layer (RunDetail, when the run has a space). */
export interface RunGraphOverlay {
  /** The run's messages, endpoints on step ids (`buildRunLayer`). */
  messages: FlowMessage[];
  /** The run's board; the unavailable shape says why there is none. */
  board: BoardSource | null;
  /** Each claim owner's (and directive author's) card, by address. */
  writers?: ReadonlyMap<string, string>;
  /** What the replay has revealed (step ids, message ids, board item keys);
   *  `null` = the whole run, as today. */
  revealed: Set<string> | null;
  selected?: string | null;
  onSelect?(id: string | null): void;
  /** The space's log was read whole: messages landing after this travel. */
  caughtUp?: boolean;
}

/** The run graph's arc router: a message endpoint is its step's card (a
 *  step not yet revealed is not drawn) or its item on the board rail; the
 *  operator, an address off the map and a dead end are no card (stubs). */
function runRouter(visible: ReadonlySet<string>): Router {
  const anchorOf = (endpoint: string, m: FlowMessage): Anchor | null => {
    if (isOutside(endpoint) || isDeadEnd(endpoint)) return null;
    if (endpoint === BOARD) return { node: RAIL, handle: boardKey(m) };
    return visible.has(endpoint) ? { node: endpoint } : null;
  };
  return {
    anchorOf,
    endsOf: (m) => [m.from, ...m.to].map((e) => anchorOf(e, m)?.node).filter((x): x is string => !!x),
    roundOfNode: () => undefined,
  };
}

/** The layer on the canvas: the rail, the entries and the message arcs. */
export function useRunLayerCanvas(
  overlay: RunGraphOverlay | undefined,
  model: RunGraphModel,
  positions: Map<string, Pos>,
  measured: ReadonlyMap<string, Size>,
  focus: string | null,
): { nodes: Node[]; edges: Edge[]; onEnter: NodeMouseHandler; onLeave: NodeMouseHandler } | null {
  const [hover, setHover] = useState<string | null>(null);
  const onEnter = useCallback<NodeMouseHandler>((_e, n) => setHover(n.id), []);
  const onLeave = useCallback<NodeMouseHandler>(() => setHover(null), []);
  const revealed = overlay?.revealed ?? null;
  const isShown = useCallback((key: string) => revealed === null || revealed.has(key), [revealed]);

  const messages = overlay?.messages;
  const shown = useMemo(() => (messages ?? []).filter((m) => isShown(m.id)), [messages, isShown]);
  const arrivals = useArrivals(shown, overlay?.caughtUp ? RUN_SPACE : '');

  const board = overlay?.board ?? null;
  const railView = useMemo(() => boardAt(board, revealed), [board, revealed]);
  // Rebuilt with the shown messages; kept while its posts are the same ones.
  const posts = useStableList(
    useMemo(() => shown.filter((m) => m.kind === 'post' || m.kind === 'mention'), [shown]),
    sameId,
  );
  const [railOpen, setRailOpen] = useState<ReadonlySet<RailSection>>(() => new Set());
  const onOpen = useCallback((_node: string, section: RailSection, open: boolean) => {
    setRailOpen((cur) => {
      const next = new Set(cur);
      if (open) next.add(section);
      else next.delete(section);
      return next;
    });
  }, []);
  const anchors = useMemo(
    () => new Map(boardAnchors(RAIL, railWindow(railView, posts, railOpen, RAIL_LIMIT))),
    [railView, posts, railOpen],
  );
  const writers = overlay?.writers;
  const claims = useMemo(
    () =>
      claimMessages(
        RUN_SPACE,
        railView,
        writers ?? new Map(),
        (messages ?? []).every((m) => m.atRest),
      ),
    [railView, writers, messages],
  );

  const visible = useMemo(() => {
    const v = new Set<string>([RAIL]);
    for (const n of model.nodes) if (isShown(n.id)) v.add(n.id);
    return v;
  }, [model, isShown]);
  const router = useMemo(() => runRouter(visible), [visible]);

  const selected = overlay?.selected ?? null;
  const onSelect = overlay?.onSelect;
  const select = useCallback((id: string) => onSelect?.(id), [onSelect]);
  const anchor = useCallback(
    (key: string) => <Handle key={`a:${key}`} id={key} type="target" position={Position.Left} className="afx-anchor" />,
    [],
  );

  // The rail right of every card.
  const box = useMemo(() => {
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const p of positions.values()) {
      minX = Math.min(minX, p.x);
      minY = Math.min(minY, p.y);
      maxX = Math.max(maxX, p.x + p.width);
      maxY = Math.max(maxY, p.y + p.height);
    }
    return minX === Infinity ? { minX: 0, minY: 0, maxX: 0, maxY: 0 } : { minX, minY, maxX, maxY };
  }, [positions]);

  const on = !!overlay;
  const nodes = useMemo<Node[]>(() => {
    if (!on) return [];
    return [
      {
        id: RAIL,
        type: 'rail',
        position: { x: box.maxX + RUN_RAIL_GAP, y: box.minY },
        draggable: false,
        selectable: false,
        measured: measured.get(RAIL),
        data: { view: railView, posts, open: railOpen, onOpen, onSelect: select, anchor },
      },
    ];
  }, [on, box, measured, railView, posts, railOpen, onOpen, select, anchor]);

  const arcs = useMemo(
    () =>
      messageArcs([...shown, ...claims], router, {
        kinds: ALL_KINDS,
        participant: null,
        focus,
        hover,
        selected,
        anchors,
        arriving: arrivals,
      }),
    [shown, claims, router, focus, hover, selected, anchors, arrivals],
  );
  const claimIds = useMemo(() => new Set(claims.map((c) => c.id)), [claims]);
  // The step cards an arc keeps clear of.
  const obstacles = useMemo(() => [...positions.values()], [positions]);
  const edges = useMemo<Edge[]>(
    () =>
      arcs.map(
        (a) =>
          ({
            id: a.id,
            source: a.source,
            target: a.target,
            targetHandle: a.targetHandle,
            type: 'message',
            // Under the cards at rest; a lit arc lifts over them.
            zIndex: a.highlighted ? 20 : 0,
            data: {
              message: a.message,
              highlighted: a.highlighted,
              arriving: arrivals.has(a.message.id),
              onSelect: claimIds.has(a.message.id) ? undefined : select,
              from: positions.get(a.source),
              to: a.targetHandle ? undefined : positions.get(a.target),
              obstacles,
              stub: a.stub,
              label: a.label,
              dead: a.dead,
            },
          }) satisfies MessageFlowEdge,
      ),
    [arcs, arrivals, claimIds, select, positions, obstacles],
  );

  return useMemo(() => (on ? { nodes, edges, onEnter, onLeave } : null), [on, nodes, edges, onEnter, onLeave]);
}
