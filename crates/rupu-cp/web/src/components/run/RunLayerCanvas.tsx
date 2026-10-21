// The run graph's message and board layer on its canvas (RunGraph's
// `overlay`, spec `2026-10-21-rupu-agentiflow-flow-design.md` goal 6): the
// board rail right of the cards, the operator's and the off-canvas entries
// left of them, and the messages as arcs between the cards, routed by
// `flowEdges` over a run router. The rail, entries and arcs are the
// agentiflow map's own components (flowmap/).

import { useCallback, useMemo, useState } from 'react';
import { Handle, Position, type Edge, type Node, type NodeMouseHandler } from '@xyflow/react';
import type { RunGraphModel } from '../../lib/runGraphModel';
import type { Pos } from '../../lib/graphLayout';
import type { Size } from '../../lib/graph/measured';
import { KIND_FILTERS, OPERATOR, type KindFilter } from '../../lib/messages';
import { BOARD, OFFCANVAS, type BoardSource, type FlowMessage } from '../../lib/flow/flowModel';
import {
  RAIL,
  boardAnchorKeys,
  boardKey,
  claimMessages,
  messageArcs,
  type Anchor,
  type Router,
} from '../../lib/flow/flowEdges';
import { boardAt, RUN_SPACE } from '../../lib/flow/runLayer';
import { useArrivals } from '../../lib/flow/useArrivals';
import { useStableList } from '../../lib/flow/useStableList';
import { RAIL_GAP } from '../../lib/flow/flowLayout';
import type { MessageFlowEdge } from '../flowmap/MessageEdge';

const sameId = (a: FlowMessage, b: FlowMessage) => a.id === b.id;
const ALL_KINDS: ReadonlySet<KindFilter> = new Set(KIND_FILTERS.map((k) => k.key));
/** The entries sit this far left of the leftmost card. */
const ENTRY_GAP = 170;

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
 *  step not yet revealed is not drawn), the operator's or the off-canvas
 *  entry, or its item on the board rail. */
function runRouter(visible: ReadonlySet<string>): Router {
  const anchorOf = (endpoint: string, m: FlowMessage): Anchor | null => {
    if (endpoint === OPERATOR) return { node: 'operator' };
    if (endpoint === OFFCANVAS) return { node: 'offcanvas' };
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
  const anchors = useMemo(() => new Set(boardAnchorKeys(RAIL, railView, posts)), [railView, posts]);
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
    const v = new Set<string>([RAIL, 'operator', 'offcanvas']);
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

  // The rail right of every card, the entries left of them.
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

  // Every card an arc can leave from or land on: the messages' ends and the
  // board's writers (a claim's arc leaves from its owner's).
  const ends = useMemo(() => {
    const e = new Set((messages ?? []).flatMap((m) => [m.from, ...m.to]));
    for (const w of writers?.values() ?? []) e.add(w);
    return e;
  }, [messages, writers]);
  const on = !!overlay;
  const nodes = useMemo<Node[]>(() => {
    if (!on) return [];
    const out: Node[] = [
      {
        id: RAIL,
        type: 'rail',
        position: { x: box.maxX + RAIL_GAP, y: box.minY },
        draggable: false,
        selectable: false,
        measured: measured.get(RAIL),
        data: { view: railView, posts, minHeight: Math.max(0, box.maxY - box.minY), onSelect: select, anchor },
      },
    ];
    const entry = (id: string, label: string, y: number): Node => ({
      id,
      type: 'entry',
      position: { x: box.minX - ENTRY_GAP, y },
      draggable: false,
      selectable: false,
      measured: measured.get(id),
      data: { label },
    });
    if (ends.has(OPERATOR)) out.push(entry('operator', 'operator', box.minY - 56));
    if (ends.has(OFFCANVAS)) out.push(entry('offcanvas', 'off map', box.maxY + 24));
    return out;
  }, [on, box, measured, railView, posts, select, anchor, ends]);

  const arcs = useMemo(
    () =>
      messageArcs([...shown, ...claims], router, {
        kinds: ALL_KINDS,
        participant: null,
        focus,
        hover,
        selected,
        anchors,
      }),
    [shown, claims, router, focus, hover, selected, anchors],
  );
  const claimIds = useMemo(() => new Set(claims.map((c) => c.id)), [claims]);
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
            zIndex: a.highlighted ? 20 : 10,
            data: {
              message: a.message,
              highlighted: a.highlighted,
              arriving: arrivals.has(a.message.id),
              onSelect: claimIds.has(a.message.id) ? undefined : select,
            },
          }) satisfies MessageFlowEdge,
      ),
    [arcs, arrivals, claimIds, select],
  );

  return useMemo(() => (on ? { nodes, edges, onEnter, onLeave } : null), [on, nodes, edges, onEnter, onLeave]);
}
