// FlowMap — the agentiflow's Flow tab, the unfolding map (spec
// `2026-10-21-rupu-agentiflow-flow-design.md`): a header strip (goals met,
// spend, tokens), the time axis with Replay, the message filters, then the
// canvas: the lead's spine down the left with a card per round, each round
// branching right into the children it launched (a workflow child as a frame
// holding its run graph), the messages as arcs between the cards, and the
// flow's board as a rail on the right. The Messages list and a Goals panel
// sit under it.
//
// The canvas is the run graph's (RunGraph): the same xyflow setup, cards,
// edges, controls, background and minimap rule. `buildFlowModel` decides what
// is drawn (density), `layoutFlow` where, `flowEdges` which arcs land on
// which cards; this file wires them to xyflow and to the timeline (what is
// revealed at `t`), the filters and the selection.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Edge,
  type EdgeTypes,
  type Node,
  type NodeChange,
  type NodeMouseHandler,
  type NodeTypes,
} from '@xyflow/react';
import { useNavigate } from 'react-router-dom';
import { api, apiErrorMessage, type AgentiflowDetail, type RunChild } from '../../lib/api';
import {
  buildFlowModel,
  FLOW_SPACE,
  OFFCANVAS,
  type BoardSource,
  type FlowBoardSpace,
  type FlowNodeRef,
  type FlowSpace,
} from '../../lib/flow/flowModel';
import {
  RAIL,
  boardAnchorKeys,
  claimMessages,
  makeRouter,
  messageArcs,
  messageCounts,
  passesFilters,
} from '../../lib/flow/flowEdges';
import { FRAME_HEAD, FRAME_PAD, layoutFlow } from '../../lib/flow/flowLayout';
import { useFlowData } from '../../lib/flow/useFlowData';
import { useArrivals } from '../../lib/flow/useArrivals';
import { useMessageSpace, type MessageSpace } from '../../lib/useMessageSpace';
import { useTimeline } from '../../lib/timeline/useTimeline';
import { countAtOrBefore, timeBounds } from '../../lib/timeline/timeline';
import { KIND_FILTERS, OPERATOR, type KindFilter } from '../../lib/messages';
import { useThemeColors, type ThemeColors } from '../../lib/useThemeColors';
import { edgeTone, useEntranceSlots } from '../../lib/graph/runMotion';
import { mergeMeasured, type Size } from '../../lib/graph/measured';
import type { StepState } from '../../lib/runGraphModel';
import { crewTint, parseCodename } from '../../lib/codename';
import { stateStyle } from '../graph/stepStyle';
import { runKindAccent, type RunKind } from '../graph/kindBridge';
import RunEdge, { type RunFlowEdge } from '../graph/RunEdge';
import { NODE_TYPES, flowKind, runEdges } from '../RunGraph';
import { useThemeMode } from '../codename/useThemeMode';
import { DialogFrame } from '../customers/DialogFrame';
import { TimeAxis } from '../timeline/TimeAxis';
import MessageEdge, { type MessageFlowEdge } from './MessageEdge';
import MessageListPanel from './MessageListPanel';
import FlowToolbar from './FlowToolbar';
import RoundNode from './RoundNode';
import ChildNode, { CHILD_KIND, childState } from './ChildNode';
import WorkflowFrameNode from './WorkflowFrameNode';
import { EntryNode, LeadNode, RailNode, SpineNode } from './MapNodes';
import { GoalsPanel, HeaderStrip, MapSummary } from './FlowPanels';
import MoreNode from './MoreNode';

import '@xyflow/react/dist/style.css';

/** The run graph's minimap rule: only past this many cards. */
const MINIMAP_MIN_NODES = 6;
/** The spine sits this far left of the round cards. */
const SPINE_X = -28;
/** The rail's top: clear of the summary chip at the canvas's top right. */
const RAIL_TOP = 56;
const ALL_KINDS: ReadonlySet<KindFilter> = new Set(KIND_FILTERS.map((k) => k.key));
const FIT = { padding: 0.12, maxZoom: 1.0, minZoom: 0.15 };

const MAP_NODE_TYPES: NodeTypes = {
  ...NODE_TYPES,
  spine: SpineNode,
  lead: LeadNode,
  round: RoundNode,
  child: ChildNode,
  frame: WorkflowFrameNode,
  more: MoreNode,
  summary: MoreNode,
  rail: RailNode,
  entry: EntryNode,
};
const MAP_EDGE_TYPES: EdgeTypes = { run: RunEdge, message: MessageEdge };

// ---------------------------------------------------------------------------

export interface FlowMapProps {
  detail: AgentiflowDetail;
  running: boolean;
  /** The page's hold on the flow's message space; the map opens its own
   *  when none is given. */
  flowSpace?: MessageSpace;
}

export default function FlowMap(props: FlowMapProps) {
  return (
    <ReactFlowProvider>
      {props.flowSpace ? <FlowMapBody {...props} flowSpace={props.flowSpace} /> : <OwnSpace {...props} />}
    </ReactFlowProvider>
  );
}

function OwnSpace(props: FlowMapProps) {
  const space = useMessageSpace(
    { route: 'agentiflows', id: props.detail.record.id },
    { live: props.running, load: true, reading: false },
  );
  return <FlowMapBody {...props} flowSpace={space} />;
}

/** A loaded workflow child's own message space, handed up to the map. */
function FrameSpace({
  runId,
  live,
  onSpace,
}: {
  runId: string;
  live: boolean;
  onSpace: (runId: string, space: FlowSpace | null) => void;
}) {
  const s = useMessageSpace({ route: 'runs', id: runId }, { live, load: true, reading: false });
  const present = s.space === 'present';
  useEffect(() => {
    onSpace(runId, present ? { id: runId, participants: s.participants, messages: s.log.messages, caughtUp: s.caughtUp } : null);
  }, [onSpace, runId, present, s.participants, s.log, s.caughtUp]);
  return null;
}

const isUnfinished = (c: RunChild | undefined) => c?.status === 'running' || c?.status === 'pending';

function stateOfRef(n: FlowNodeRef | undefined, liveRound: number | null): StepState {
  if (!n) return 'pending';
  if (n.child) return childState(n.child);
  if (n.kind === 'round') return n.round === liveRound ? 'running' : 'done';
  return 'pending';
}

function runKindOfRef(n: FlowNodeRef | undefined): RunKind {
  if (n?.child) return CHILD_KIND[n.child.kind].run;
  if (n?.kind === 'more') return 'for_each';
  return 'step';
}

function shallowEqual(a: Record<string, unknown>, b: Record<string, unknown>): boolean {
  const ka = Object.keys(a);
  if (ka.length !== Object.keys(b).length) return false;
  return ka.every((k) => Object.is(a[k], b[k]));
}

function FlowMapBody({ detail, running, flowSpace }: FlowMapProps & { flowSpace: MessageSpace }) {
  const colors = useThemeColors();
  const mode = useThemeMode();
  const rf = useReactFlow();
  const navigate = useNavigate();
  const { record } = detail;

  const data = useFlowData(detail, { running, flowSpace });
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const [kinds, setKinds] = useState<ReadonlySet<KindFilter>>(ALL_KINDS);
  const [participant, setParticipant] = useState<string | null>(null);
  const [selectedMsg, setSelectedMsg] = useState<string | null>(null);
  /** The clicked card, and the round it is in (kept open while focused). */
  const [focus, setFocus] = useState<{ id: string; round: number | undefined } | null>(null);
  const [hoverNode, setHoverNode] = useState<string | null>(null);
  const [measured, setMeasured] = useState<ReadonlyMap<string, Size>>(() => new Map());
  const [definition, setDefinition] = useState<
    { child: RunChild; yaml: string | null; error: string | null } | null
  >(null);

  // A focused card's round stays expanded through polls that would fold it.
  const focusRound = focus?.round;
  const open = useMemo(
    () => (focusRound === undefined ? expanded : new Set(expanded).add(`round:${focusRound}`)),
    [expanded, focusRound],
  );
  const { children, spaces, boards, childGraphs } = data;
  const model = useMemo(
    () => buildFlowModel({ detail, children, spaces, boards, childGraphs }, { expanded: open }),
    [detail, children, spaces, boards, childGraphs, open],
  );
  const layout = useMemo(() => layoutFlow(model, measured), [model, measured]);

  // --- the timeline --------------------------------------------------------
  // The reveal is keyed on how many items are at or before `t`, so a replay
  // frame that reveals nothing new changes nothing below it.
  const { items, bands } = model.timeline;
  const nowMs = useMemo(() => (running ? Date.now() : null), [running, detail]);
  const tl = useTimeline({ items, bands, nowMs });
  const bounds = useMemo(() => timeBounds(items, bands, nowMs), [items, bands, nowMs]);
  const sortedItems = useMemo(() => [...items].sort((a, b) => a.at - b.at), [items]);
  const itemKeys = useMemo(() => new Set(items.map((i) => i.key)), [items]);
  const revealCount = tl.t === null ? null : countAtOrBefore(sortedItems, tl.t);
  const replaying = tl.t !== null;
  const shownSet = useMemo(
    () => (revealCount === null ? null : new Set(sortedItems.slice(0, revealCount).map((i) => i.key))),
    [sortedItems, revealCount],
  );
  const isShown = useCallback(
    (key: string) => shownSet === null || !itemKeys.has(key) || shownSet.has(key),
    [shownSet, itemKeys],
  );

  // --- what is on the canvas ----------------------------------------------
  const refById = useMemo(() => new Map(model.nodes.map((n) => [n.id, n])), [model]);
  const childById = useMemo(() => new Map(children.map((c) => [c.run_id, c])), [children]);
  const liveRound = running ? (model.rounds.find((r) => r.endedAt === null)?.round ?? null) : null;
  const visible = useMemo(() => {
    const v = new Set<string>(['lead', 'spine', RAIL]);
    for (const n of model.nodes) {
      if (n.kind === 'lead') continue;
      if (!isShown(`round:${n.round}`)) continue;
      if ((n.kind === 'child' || n.kind === 'frame') && !isShown(n.id)) continue;
      v.add(n.id);
      if (n.kind === 'frame' && n.graph) for (const s of n.graph.nodes) v.add(`${n.id}/${s.id}`);
    }
    for (const r of model.rounds) if (r.collapsed && isShown(`round:${r.round}`)) v.add(`summary:${r.round}`);
    return v;
  }, [model, isShown]);
  const router = useMemo(() => makeRouter(model, visible), [model, visible]);
  const routerRef = useRef(router);
  routerRef.current = router;

  // Messages revealed at `t` (filters aside).
  const shownMessages = useMemo(() => model.messages.filter((m) => isShown(m.id)), [model, isShown]);
  const listed = useMemo(
    () => shownMessages.filter((m) => passesFilters(m, kinds, participant)),
    [shownMessages, kinds, participant],
  );
  const counts = useMemo(() => messageCounts(shownMessages, router), [shownMessages, router]);

  // The packets (useArrivals): a message landing after its space caught up
  // travels once.
  const flowCaughtUp =
    flowSpace.caughtUp || flowSpace.status === 'unavailable' || flowSpace.status === 'error' || flowSpace.space === 'absent';
  const caughtUpKey = useMemo(
    () => [...(flowCaughtUp ? [FLOW_SPACE] : []), ...spaces.filter((s) => s.id !== FLOW_SPACE && s.caughtUp).map((s) => s.id)].join(','),
    [flowCaughtUp, spaces],
  );
  const arrivals = useArrivals(shownMessages, caughtUpKey);

  // --- the board rail and frame strips, as of `t` -------------------------
  const boardAsOf = useCallback(
    (spaceId: string, view: BoardSource | null): BoardSource | null => {
      if (!view || 'kind' in view || shownSet === null) return view;
      return {
        ...view,
        claims: view.claims.filter((c) => isShown(`claim:${spaceId}:${c.key}:${c.acquired_at}`)),
        directives: view.directives.filter((d) => isShown(`directive:${spaceId}:${d.id}`)),
      };
    },
    [isShown, shownSet],
  );
  const railView = useMemo(() => boardAsOf(FLOW_SPACE, model.board.view), [boardAsOf, model]);
  const railPosts = useMemo(
    () => (shownSet === null ? model.board.posts : model.board.posts.filter((p) => isShown(p.id))),
    [model, isShown, shownSet],
  );
  const frameBoards = useMemo(() => {
    const m = new Map<string, FlowBoardSpace>();
    for (const [f, b] of model.board.frames) {
      m.set(f, {
        ...b,
        view: boardAsOf(f, b.view),
        posts: shownSet === null ? b.posts : b.posts.filter((p) => isShown(p.id)),
      });
    }
    return m;
  }, [model, boardAsOf, isShown, shownSet]);
  const anchors = useMemo(() => {
    const s = new Set(boardAnchorKeys(RAIL, railView, railPosts));
    for (const [f, b] of frameBoards) if (visible.has(f)) for (const k of boardAnchorKeys(f, b.view, b.posts)) s.add(k);
    return s;
  }, [railView, railPosts, frameBoards, visible]);
  const anchor = useCallback(
    (key: string) => <Handle key={`a:${key}`} id={key} type="target" position={Position.Left} className="afx-anchor" />,
    [],
  );
  const claims = useMemo(() => {
    const atRest = (spaceId: string) => model.messages.filter((m) => m.space === spaceId).every((m) => m.atRest);
    return [
      ...claimMessages(FLOW_SPACE, railView, model.board.writers, atRest(FLOW_SPACE)),
      ...[...frameBoards].flatMap(([f, b]) => claimMessages(f, b.view, b.writers, atRest(f))),
    ];
  }, [model, railView, frameBoards]);

  // --- selection -----------------------------------------------------------
  const [listSelected, setListSelected] = useState(0);
  const selectMessage = useCallback((id: string | null) => setSelectedMsg(id), []);
  const selectFromList = useCallback((id: string | null) => {
    setSelectedMsg(id);
    if (id) setListSelected((n) => n + 1);
  }, []);

  // --- callbacks the cards hold (stable, so the cards' memos hold) --------
  const onExpand = useCallback((round: number) => {
    setExpanded((s) => new Set(s).add(`round:${round}`));
  }, []);
  const onDefinition = useCallback(
    (child: RunChild) => {
      setDefinition({ child, yaml: null, error: null });
      api.getAgentiflowGenerated(record.id, child.handle ?? '').then(
        (d) => setDefinition((cur) => (cur?.child.run_id === child.run_id ? { ...cur, yaml: d.yaml } : cur)),
        (e: unknown) =>
          setDefinition((cur) => (cur?.child.run_id === child.run_id ? { ...cur, error: apiErrorMessage(e) } : cur)),
      );
    },
    [record.id],
  );
  // A frame's fan-out and panel controls: a unit opens its transcript (the
  // run's page when it has none yet); "expand all" opens the run's page,
  // where the whole fan-out is browsed.
  const graphsRef = useRef(childGraphs);
  graphsRef.current = childGraphs;
  const frameHandlers = useRef(
    new Map<string, { open: (step: string, index: number) => void; expand: (step: string) => void }>(),
  );
  const handlersOf = useCallback(
    (frame: string) => {
      let h = frameHandlers.current.get(frame);
      if (!h) {
        const runPage = `/runs/${encodeURIComponent(frame)}`;
        h = {
          open: (step, index) => {
            const node = graphsRef.current.get(frame)?.nodeById(step);
            const unit = node?.fanout?.units.find((u) => u.index === index);
            if (unit?.transcriptPath) {
              navigate(`/transcript?path=${encodeURIComponent(unit.transcriptPath)}&live=${unit.state === 'running' ? 1 : 0}`);
            } else navigate(runPage);
          },
          expand: () => navigate(runPage),
        };
        frameHandlers.current.set(frame, h);
      }
      return h;
    },
    [navigate],
  );
  const leadTint = crewTint(parseCodename(record.codename).crew, mode) ?? colors.get('brand.500');

  // --- nodes ---------------------------------------------------------------
  const slots = useEntranceSlots(layout.positions);
  // A card keeps its previous `data` while nothing in it changed, so its
  // memo skips the render (a pure cache: the same input gives the same data).
  const dataCache = useRef(new Map<string, Record<string, unknown>>());
  const nodes = useMemo<Node[]>(() => {
    const cache = dataCache.current;
    const stable = (id: string, d: Record<string, unknown>) => {
      const prev = cache.get(id);
      if (prev && shallowEqual(prev, d)) return prev;
      cache.set(id, d);
      return d;
    };
    const out: Node[] = [];
    const node = (id: string, type: string, d: Record<string, unknown>, extra: Partial<Node> = {}): Node => {
      const p = layout.positions.get(id);
      return {
        id,
        type,
        position: p ? { x: p.x, y: p.y } : { x: 0, y: 0 },
        draggable: false,
        measured: measured.get(id),
        hidden: !visible.has(id),
        selected: id === focus?.id,
        data: stable(id, d),
        ...extra,
      };
    };
    const dots: number[] = [];
    for (const r of model.rounds) {
      const p = layout.positions.get(`round:${r.round}`);
      if (p && visible.has(`round:${r.round}`)) dots.push(p.y + Math.min(p.height, 60) / 2);
    }
    out.push({
      id: 'spine',
      type: 'spine',
      position: { x: SPINE_X - 12, y: 0 },
      draggable: false,
      selectable: false,
      zIndex: -1,
      data: stable('spine', { height: layout.height, dots, live: running && !replaying, tint: leadTint }),
    });
    out.push(
      node('lead', 'lead', { codename: record.codename, agent: detail.def?.lead ?? null, startedAt: record.started_at }, { selectable: false }),
    );
    for (const r of model.rounds) {
      const id = `round:${r.round}`;
      out.push(
        node(id, 'round', {
          round: r,
          live: r.round === liveRound,
          launched: r.childIds.length,
          messages: counts.get(id) ?? 0,
          slot: slots.get(id) ?? 0,
        }),
      );
      if (r.collapsed) {
        const sid = `summary:${r.round}`;
        out.push(
          node(sid, 'summary', {
            variant: 'summary',
            round: r.round,
            children: r.childIds.map((c) => childById.get(c)).filter((c): c is RunChild => !!c),
            onExpand,
            slot: slots.get(sid) ?? 0,
          }),
        );
      }
    }
    for (const n of model.nodes) {
      if (n.kind === 'child' && n.child) {
        out.push(
          node(n.id, 'child', { child: n.child, inferred: n.inferred, messages: counts.get(n.id) ?? 0, slot: slots.get(n.id) ?? 0 }),
        );
      } else if (n.kind === 'more') {
        const r = model.rounds.find((x) => x.round === n.round)!;
        const inRound = new Set(r.childIds);
        out.push(
          node(n.id, 'more', {
            variant: 'more',
            round: n.round,
            // The round's top-level children not drawn as cards.
            children: r.childIds
              .filter((c) => !refById.has(c))
              .map((c) => childById.get(c))
              .filter((c): c is RunChild => !!c && !inRound.has(c.parent_run_id)),
            onExpand,
            slot: slots.get(n.id) ?? 0,
          }),
        );
      } else if (n.kind === 'frame' && n.child) {
        const box = layout.frames.get(n.id);
        let graphHeight = Math.max(80, (box?.height ?? 220) - FRAME_HEAD - 40);
        if (n.graph) {
          let bottom = 0;
          for (const s of n.graph.nodes) {
            const p = layout.positions.get(`${n.id}/${s.id}`);
            if (p) bottom = Math.max(bottom, p.y + p.height);
          }
          graphHeight = bottom - (box?.y ?? 0) - FRAME_HEAD + FRAME_PAD;
        }
        out.push(
          node(
            n.id,
            'frame',
            {
              child: n.child,
              round: n.round,
              inferred: n.inferred,
              width: box?.width ?? 480,
              graphHeight,
              loaded: !!n.graph,
              error: data.frameErrors.get(n.id) ?? null,
              board: frameBoards.get(n.id) ?? null,
              messages: counts.get(n.id) ?? 0,
              onLoad: data.loadFrame,
              onDefinition,
              onSelectMessage: selectMessage,
              anchor,
              slot: slots.get(n.id) ?? 0,
            },
            { zIndex: 0 },
          ),
        );
        if (n.graph) {
          const h = handlersOf(n.id);
          for (const s of n.graph.nodes) {
            const id = `${n.id}/${s.id}`;
            out.push(
              node(
                id,
                flowKind(s.kind),
                { node: s, slot: slots.get(id) ?? 0, onOpenUnit: h.open, onExpandFanout: h.expand },
                { zIndex: 1 },
              ),
            );
          }
        }
      }
    }
    out.push(
      node(
        RAIL,
        'rail',
        { view: railView, posts: railPosts, minHeight: Math.max(0, layout.height - RAIL_TOP), onSelect: selectMessage, anchor },
        // Below the canvas's summary chip, which sits over the top-right.
        { position: { x: layout.railX, y: RAIL_TOP }, selectable: false, hidden: false, selected: false },
      ),
    );
    const ends = new Set(model.messages.flatMap((m) => [m.from, ...m.to]));
    const entry = (id: string, label: string, y: number) =>
      node(id, 'entry', { label }, { position: { x: layout.operator.x, y }, selectable: false, hidden: false, selected: false });
    if (ends.has(OPERATOR)) out.push(entry('operator', 'operator', layout.operator.y));
    if (ends.has(OFFCANVAS)) out.push(entry('offcanvas', 'off map', layout.operator.y + 40));
    return out;
  }, [
    model,
    layout,
    measured,
    visible,
    slots,
    counts,
    liveRound,
    running,
    replaying,
    leadTint,
    record,
    detail.def,
    childById,
    refById,
    onExpand,
    onDefinition,
    selectMessage,
    anchor,
    railView,
    railPosts,
    frameBoards,
    handlersOf,
    data.frameErrors,
    data.loadFrame,
    focus?.id,
  ]);

  // --- edges ---------------------------------------------------------------
  const treeEdges = useMemo<Edge[]>(() => {
    const out: Edge[] = [];
    // The tree: a round into what it launched, a child into its children.
    const tree = (source: string, target: string, sRef: FlowNodeRef | undefined, tRef: FlowNodeRef | undefined) => {
      if (!visible.has(source) || !visible.has(target)) return;
      const tone = edgeTone(stateOfRef(sRef, liveRound), tRef ? stateOfRef(tRef, liveRound) : 'pending');
      out.push({
        id: `t:${source}->${target}`,
        source,
        target,
        type: 'run',
        className: tone === 'live' ? 'rg-edge-flow' : undefined,
        data: {
          tone,
          from: colors.get(runKindAccent(runKindOfRef(sRef))),
          to: colors.get(runKindAccent(runKindOfRef(tRef))),
          light: colors.status.running,
          slot: slots.get(source) ?? 0,
        },
      } satisfies RunFlowEdge);
    };
    for (const n of model.nodes) {
      if (n.kind === 'child' || n.kind === 'frame' || n.kind === 'more') tree(n.parentId, n.id, refById.get(n.parentId), n);
    }
    for (const r of model.rounds) {
      if (r.collapsed) tree(`round:${r.round}`, `summary:${r.round}`, refById.get(`round:${r.round}`), undefined);
    }
    // Each loaded frame's own run graph.
    for (const n of model.nodes) {
      if (n.kind === 'frame' && n.graph && visible.has(n.id)) out.push(...runEdges(n.graph, colors, slots, `${n.id}/`));
    }
    return out;
  }, [model, visible, liveRound, colors, slots, refById]);

  const arcs = useMemo(
    () =>
      messageArcs([...shownMessages, ...claims], router, {
        kinds,
        participant,
        focus: focus?.id ?? null,
        hover: hoverNode,
        selected: selectedMsg,
        anchors,
      }),
    [shownMessages, claims, router, kinds, participant, focus?.id, hoverNode, selectedMsg, anchors],
  );
  const claimIds = useMemo(() => new Set(claims.map((c) => c.id)), [claims]);
  const edges = useMemo<Edge[]>(
    () => [
      ...treeEdges,
      ...arcs.map(
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
              onSelect: claimIds.has(a.message.id) ? undefined : selectMessage,
            },
          }) satisfies MessageFlowEdge,
      ),
    ],
    [treeEdges, arcs, arrivals, claimIds, selectMessage],
  );

  // --- interaction ---------------------------------------------------------
  const onNodesChange = useCallback((changes: NodeChange[]) => {
    setMeasured((prev) => mergeMeasured(prev, changes));
  }, []);
  const onNodeClick = useCallback<NodeMouseHandler>((_e, n) => {
    if (n.type === 'spine' || n.type === 'rail' || n.type === 'lead') return;
    setFocus((cur) => (cur?.id === n.id ? null : { id: n.id, round: routerRef.current.roundOfNode(n.id) }));
  }, []);
  const onNodeEnter = useCallback<NodeMouseHandler>((_e, n) => setHoverNode(n.id), []);
  const onNodeLeave = useCallback<NodeMouseHandler>(() => setHoverNode(null), []);

  // The viewport: fit the map's width at first, from its top (a running flow
  // from its newest round); follow the newest round while running unless
  // the operator has panned away. A poll never moves it otherwise.
  const pane = useRef<HTMLDivElement>(null);
  const panned = useRef(false);
  const placed = useRef(false);
  const placedWidth = useRef(0);
  const lastRound = model.rounds[model.rounds.length - 1];
  const lastRoundPos = lastRound ? layout.positions.get(`round:${lastRound.round}`) : undefined;
  const followKey = running && lastRound ? `round:${lastRound.round}` : null;
  useEffect(() => {
    const el = pane.current;
    if (!el || !lastRoundPos || !rf.viewportInitialized || !data.childrenLoaded) return;
    const w = el.clientWidth;
    const h = el.clientHeight;
    if (w === 0 || h === 0) return;
    const left = layout.operator.x - 24;
    const fitZoom = Math.min(1, Math.max(FIT.minZoom, w / (layout.width - left + 48)));
    if (!placed.current) {
      placed.current = true;
      placedWidth.current = layout.width;
      const y = running ? -(lastRoundPos.y * fitZoom) + h * 0.35 : 24;
      void rf.setViewport({ x: -left * fitZoom + 24, y: Math.min(24, y), zoom: fitZoom });
      return;
    }
    if (panned.current) return;
    // The map grew wider (a frame's graph loaded): keep its width in view.
    if (layout.width > placedWidth.current) {
      placedWidth.current = layout.width;
      const vp = rf.getViewport();
      if (fitZoom < vp.zoom) {
        void rf.setViewport({ x: -left * fitZoom + 24, y: vp.y * (fitZoom / vp.zoom), zoom: fitZoom }, { duration: 320 });
        return;
      }
    }
    if (!followKey) return;
    const vp = rf.getViewport();
    void rf.setViewport({ x: vp.x, y: Math.min(24, -(lastRoundPos.y * vp.zoom) + h * 0.35), zoom: vp.zoom }, { duration: 320 });
    // Only a new newest round, a wider map or the first placement moves it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [followKey, lastRoundPos !== undefined, rf.viewportInitialized, data.childrenLoaded, layout.width]);

  // A row picked in the Messages list finds its arc: the map centres on it.
  useEffect(() => {
    if (!selectedMsg || listSelected === 0) return;
    const m = model.messages.find((x) => x.id === selectedMsg);
    if (!m) return;
    const ps = router
      .endsOf(m)
      .map((e) => layout.positions.get(e))
      .filter((p): p is NonNullable<typeof p> => !!p);
    if (ps.length === 0) return;
    const cx = ps.reduce((a, p) => a + p.x + p.width / 2, 0) / ps.length;
    const cy = ps.reduce((a, p) => a + p.y + p.height / 2, 0) / ps.length;
    panned.current = true;
    void rf.setCenter(cx, cy, { zoom: rf.getViewport().zoom, duration: 320 });
    // Only on a pick from the list.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listSelected]);

  // --- the panels ----------------------------------------------------------
  const participantNames = useMemo(() => {
    const s = new Set<string>();
    for (const sp of spaces) for (const p of sp.participants) s.add(p.address);
    for (const m of model.messages) s.add(m.message.from);
    return [...s].sort();
  }, [spaces, model]);
  const spaceLabels = useMemo(() => {
    const m = new Map<string, string>([[FLOW_SPACE, 'Flow']]);
    for (const sp of spaces) if (sp.id !== FLOW_SPACE) m.set(sp.id, childById.get(sp.id)?.address ?? sp.id);
    return m;
  }, [spaces, childById]);
  const cardCount = model.nodes.filter((n) => n.kind === 'child' || n.kind === 'frame' || n.kind === 'round').length;

  return (
    <div className="afx-map space-y-3" data-testid="flow-map">
      <HeaderStrip detail={detail} model={model} running={running} />
      <TimeAxis
        bands={bands}
        items={items}
        bounds={bounds}
        t={tl.t}
        live={running}
        onScrub={tl.scrub}
        onReplay={tl.replay}
        onRelease={tl.release}
      />
      <FlowToolbar
        kinds={kinds}
        participant={participant}
        participants={participantNames}
        onKinds={setKinds}
        onParticipant={setParticipant}
      />
      {data.childrenError && (
        <div className="afx-note" role="status">
          couldn't read this flow's children: {data.childrenError}
        </div>
      )}
      <div ref={pane} className="gx-canvas afx-canvas relative w-full overflow-hidden rounded-xl border border-border shadow-card">
        <MapSummary list={children} messages={shownMessages.length} />
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={MAP_NODE_TYPES}
          edgeTypes={MAP_EDGE_TYPES}
          onNodesChange={onNodesChange}
          onNodeClick={onNodeClick}
          onNodeMouseEnter={onNodeEnter}
          onNodeMouseLeave={onNodeLeave}
          onPaneClick={() => {
            setFocus(null);
            setSelectedMsg(null);
          }}
          onMoveStart={(e) => {
            if (e) panned.current = true;
          }}
          nodesDraggable={false}
          nodesConnectable={false}
          elementsSelectable
          onlyRenderVisibleElements
          minZoom={FIT.minZoom}
          maxZoom={1.5}
          proOptions={{ hideAttribution: true }}
          style={{ background: 'transparent' }}
        >
          <Background variant={BackgroundVariant.Dots} gap={20} size={1.1} color={colors.alpha('ink', 0.09)} />
          {cardCount > MINIMAP_MIN_NODES && (
            <MiniMap
              pannable
              zoomable
              style={{ width: 150, height: 96 }}
              maskColor={colors.alpha('bg', 0.55)}
              nodeColor={(n) => miniColor(colors, n)}
              nodeBorderRadius={4}
            />
          )}
          <Controls
            showInteractive={false}
            fitViewOptions={{ ...FIT, duration: 320 }}
            onFitView={() => {
              panned.current = false;
            }}
          />
        </ReactFlow>
      </div>
      {data.loadedFrames.map((f) => (
        <FrameSpace key={f} runId={f} live={isUnfinished(childById.get(f))} onSpace={data.setFrameSpace} />
      ))}
      <div className="grid gap-3 lg:grid-cols-[3fr_2fr]">
        <section className="afx-panel" aria-label="Messages">
          <h3 className="afx-panel-h">
            Messages <span>{listed.length}</span>
          </h3>
          <MessageListPanel messages={listed} spaces={spaceLabels} selected={selectedMsg} onSelect={selectFromList} />
        </section>
        <GoalsPanel detail={detail} model={model} list={children} spaces={spaces} />
      </div>
      {definition && (
        <DialogFrame
          title={`${definition.child.name}: the definition the lead wrote`}
          onRequestClose={() => setDefinition(null)}
          className="max-w-3xl"
        >
          {definition.error ? (
            <p className="mt-3 text-sm text-err">{definition.error}</p>
          ) : definition.yaml === null ? (
            <p className="mt-3 text-sm text-ink-mute">Loading…</p>
          ) : (
            <pre className="mt-3 max-h-[60vh] overflow-auto rounded-md border border-border bg-bg p-3 font-mono text-note text-ink">
              {definition.yaml}
            </pre>
          )}
          <div className="mt-3 flex justify-end">
            <button type="button" className="tl-btn" onClick={() => setDefinition(null)}>
              Close
            </button>
          </div>
        </DialogFrame>
      )}
    </div>
  );
}

function miniColor(colors: ThemeColors, n: Node): string {
  const d = n.data as { child?: RunChild; node?: { state: StepState }; live?: boolean };
  const state: StepState | null = d.child
    ? childState(d.child)
    : d.node
      ? d.node.state
      : n.type === 'round'
        ? d.live
          ? 'running'
          : 'done'
        : null;
  if (!state || state === 'pending' || state === 'skipped') return colors.alpha('inkMute', 0.45);
  return stateStyle(colors, state).color;
}
