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
// is drawn (density), `layoutFlow` where; this file wires them to xyflow and
// to the timeline (what is revealed at `t`), the filters and the selection.

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
  type NodeProps,
  type NodeTypes,
} from '@xyflow/react';
import { CheckCircle2, Circle } from 'lucide-react';
import { api, apiErrorMessage, type AgentiflowDetail, type RunChild } from '../../lib/api';
import { buildFlowModel, BOARD, FLOW_SPACE, OFFCANVAS, type FlowMessage, type FlowNodeRef } from '../../lib/flow/flowModel';
import { FRAME_HEAD, FRAME_PAD, RAIL_W, layoutFlow } from '../../lib/flow/flowLayout';
import { useFlowData } from '../../lib/flow/useFlowData';
import { useMessageSpace, type MessageSpace } from '../../lib/useMessageSpace';
import { useTimeline } from '../../lib/timeline/useTimeline';
import { revealed, timeBounds } from '../../lib/timeline/timeline';
import { KIND_FILTERS, LEAD, OPERATOR, type KindFilter } from '../../lib/messages';
import { useThemeColors, type ThemeColors } from '../../lib/useThemeColors';
import { edgeTone, useEntranceSlots } from '../../lib/graph/runMotion';
import { mergeMeasured, type Size } from '../../lib/graph/measured';
import type { StepState } from '../../lib/runGraphModel';
import { crewTint, parseCodename } from '../../lib/codename';
import { formatSpendUsd, formatSpendTokens, goalViews } from '../../lib/agentiflow';
import { formatDuration } from '../../lib/duration';
import { stateStyle } from '../graph/stepStyle';
import { runKindAccent, type RunKind } from '../graph/kindBridge';
import RunEdge, { type RunFlowEdge } from '../graph/RunEdge';
import { NODE_TYPES, flowKind, runEdges } from '../RunGraph';
import { useThemeMode } from '../codename/useThemeMode';
import { DialogFrame } from '../customers/DialogFrame';
import { TimeAxis } from '../timeline/TimeAxis';
import MessageEdge, { type MessageFlowEdge } from './MessageEdge';
import BoardRail from './BoardRail';
import MessageListPanel from './MessageListPanel';
import FlowToolbar from './FlowToolbar';
import RoundNode from './RoundNode';
import ChildNode, { CHILD_KIND, childState } from './ChildNode';
import WorkflowFrameNode from './WorkflowFrameNode';
import MoreNode from './MoreNode';

import '@xyflow/react/dist/style.css';

/** The run graph's minimap rule: only past this many cards. */
const MINIMAP_MIN_NODES = 6;
/** The spine sits this far left of the round cards. */
const SPINE_X = -28;
/** How long a newly revealed message's packet travels (and a bit). */
const ARRIVE_MS = 1500;
const ALL_KINDS: ReadonlySet<KindFilter> = new Set(KIND_FILTERS.map((k) => k.key));
const FIT = { padding: 0.12, maxZoom: 1.0, minZoom: 0.15 };

// ---------------------------------------------------------------------------
// The map's own small nodes: the spine, the lead's start, the rail, the
// operator's (and an unknown address's) entry.
// ---------------------------------------------------------------------------

interface SpineData extends Record<string, unknown> {
  height: number;
  dots: number[];
  live: boolean;
  tint: string;
}

function SpineNode({ data }: NodeProps<Node<SpineData, 'spine'>>) {
  const top = data.dots[0] ?? 0;
  const end = data.dots[data.dots.length - 1] ?? top;
  return (
    <svg className="afx-spine-svg" width={24} height={Math.max(end + 24, 1)} aria-hidden style={{ color: data.tint }}>
      <line className="afx-spine-line" x1={12} y1={0} x2={12} y2={end} pathLength={1} />
      {data.dots.map((y, i) => (
        <circle key={i} className="afx-spine-dot" cx={12} cy={y} r={4} style={{ ['--i' as string]: Math.min(i, 10) }} />
      ))}
      {data.live && (
        <>
          <circle className="afx-spine-ring" cx={12} cy={end} r={5} />
          <circle className="afx-spine-dot" cx={12} cy={end} r={5} />
        </>
      )}
    </svg>
  );
}

interface LeadData extends Record<string, unknown> {
  codename: string;
  agent: string | null;
  startedAt: string;
}

function LeadNode({ data }: NodeProps<Node<LeadData, 'lead'>>) {
  const t = Date.parse(data.startedAt);
  return (
    <div className="afx-lead" data-testid="flow-lead">
      <b>Engagement started</b>
      <span>
        lead {data.codename}
        {data.agent ? ` · ${data.agent}` : ''}
        {Number.isFinite(t) ? ` · ${new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : ''}
      </span>
    </div>
  );
}

interface RailData extends Record<string, unknown> {
  view: Parameters<typeof BoardRail>[0]['board'];
  posts: FlowMessage[];
  minHeight: number;
  onSelect: (id: string) => void;
  anchor: (key: string) => React.ReactNode;
}

function RailNode({ data }: NodeProps<Node<RailData, 'rail'>>) {
  return (
    <div className="afx-rail-node nodrag nowheel" style={{ width: RAIL_W, minHeight: data.minHeight }}>
      <Handle type="target" position={Position.Left} className="gx-port" />
      <div className="afx-rail-title">Board</div>
      <BoardRail board={data.view} posts={data.posts} onSelect={data.onSelect} anchor={data.anchor} />
    </div>
  );
}

interface EntryData extends Record<string, unknown> {
  label: string;
}

function EntryNode({ data }: NodeProps<Node<EntryData, 'entry'>>) {
  return (
    <div className="afx-entry" data-testid={`flow-entry-${data.label}`}>
      {data.label}
      <Handle type="source" position={Position.Right} className="gx-port" />
      <Handle type="target" position={Position.Right} className="gx-port" />
    </div>
  );
}

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
  onSpace: (runId: string, space: { id: string; participants: MessageSpace['participants']; messages: MessageSpace['log']['messages'] } | null) => void;
}) {
  const s = useMessageSpace({ route: 'runs', id: runId }, { live, load: true, reading: false });
  const present = s.space === 'present';
  useEffect(() => {
    onSpace(runId, present ? { id: runId, participants: s.participants, messages: s.log.messages } : null);
  }, [onSpace, runId, present, s.participants, s.log]);
  return null;
}

const isUnfinished = (c: RunChild | undefined) => c?.status === 'running' || c?.status === 'pending';

/** Whether a message passes the toolbar's filters. */
function passes(m: FlowMessage, kinds: ReadonlySet<KindFilter>, participant: string | null): boolean {
  const k = KIND_FILTERS.find((f) => f.kinds.includes(m.message.kind));
  if (k && !kinds.has(k.key)) return false;
  if (participant === null) return true;
  const msg = m.message;
  return (
    msg.from === participant ||
    msg.to === participant ||
    (msg.delivery?.resolved ?? []).includes(participant) ||
    (msg.delivery?.undelivered ?? []).some((u) => u.to === participant)
  );
}

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

function FlowMapBody({ detail, running, flowSpace }: FlowMapProps & { flowSpace: MessageSpace }) {
  const colors = useThemeColors();
  const mode = useThemeMode();
  const rf = useReactFlow();
  const { record } = detail;

  const data = useFlowData(detail, { running, flowSpace });
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const [kinds, setKinds] = useState<ReadonlySet<KindFilter>>(ALL_KINDS);
  const [participant, setParticipant] = useState<string | null>(null);
  const [selectedMsg, setSelectedMsg] = useState<string | null>(null);
  const [focusNode, setFocusNode] = useState<string | null>(null);
  const [hoverNode, setHoverNode] = useState<string | null>(null);
  const [measured, setMeasured] = useState<ReadonlyMap<string, Size>>(() => new Map());
  const [definition, setDefinition] = useState<
    { child: RunChild; yaml: string | null; error: string | null } | null
  >(null);

  const { children, spaces, boards, childGraphs } = data;
  const model = useMemo(
    () => buildFlowModel({ detail, children, spaces, boards, childGraphs }, { expanded }),
    [detail, children, spaces, boards, childGraphs, expanded],
  );
  const layout = useMemo(() => layoutFlow(model, measured), [model, measured]);

  // --- the timeline --------------------------------------------------------
  const { items, bands } = model.timeline;
  const nowMs = useMemo(() => (running ? Date.now() : null), [running, detail]);
  const tl = useTimeline({ items, bands, nowMs });
  const bounds = useMemo(() => timeBounds(items, bands, nowMs), [items, bands, nowMs]);
  const itemKeys = useMemo(() => new Set(items.map((i) => i.key)), [items]);
  const shownSet = useMemo(() => (tl.t === null ? null : revealed(items, tl.t)), [items, tl.t]);
  const isShown = useCallback(
    (key: string) => shownSet === null || !itemKeys.has(key) || shownSet.has(key),
    [shownSet, itemKeys],
  );

  // --- what is on the canvas ----------------------------------------------
  const refById = useMemo(() => new Map(model.nodes.map((n) => [n.id, n])), [model]);
  const roundOfChild = useMemo(() => {
    const m = new Map<string, number>();
    for (const r of model.rounds) for (const id of r.childIds) m.set(id, r.round);
    return m;
  }, [model]);
  const liveRound = running ? (model.rounds.find((r) => r.endedAt === null)?.round ?? null) : null;
  const visible = useMemo(() => {
    const v = new Set<string>(['lead', 'spine', 'rail']);
    for (const n of model.nodes) {
      if (n.kind === 'lead') continue;
      const roundKey = `round:${n.round}`;
      if (!isShown(roundKey)) continue;
      if ((n.kind === 'child' || n.kind === 'frame') && !isShown(n.id)) continue;
      v.add(n.id);
      if (n.kind === 'frame' && n.graph) for (const s of n.graph.nodes) v.add(`${n.id}/${s.id}`);
    }
    for (const r of model.rounds) if (r.collapsed && isShown(`round:${r.round}`)) v.add(`summary:${r.round}`);
    return v;
  }, [model, isShown]);

  /** The card a message endpoint lands on, as drawn now (null: not drawn). */
  const anchorOf = useCallback(
    (endpoint: string, m: FlowMessage): { node: string; handle?: string } | null => {
      if (endpoint === LEAD) {
        const r = model.rounds.find((x) => (x.endedAt ?? Infinity) >= m.at) ?? model.rounds[model.rounds.length - 1];
        return r && visible.has(`round:${r.round}`) ? { node: `round:${r.round}` } : null;
      }
      if (endpoint === OPERATOR) return { node: 'operator' };
      if (endpoint === OFFCANVAS) return { node: 'offcanvas' };
      if (endpoint === BOARD) {
        const key = m.kind === 'retract' ? (m.message.refs?.retracts ?? m.id) : m.id;
        return m.space === FLOW_SPACE ? { node: 'rail', handle: key } : visible.has(m.space) ? { node: m.space, handle: key } : null;
      }
      if (visible.has(endpoint)) return { node: endpoint };
      const slash = endpoint.indexOf('/');
      if (slash > 0) return anchorOf(endpoint.slice(0, slash), m);
      const round = refById.get(endpoint)?.round ?? roundOfChild.get(endpoint);
      if (round === undefined) return null;
      const r = model.rounds.find((x) => x.round === round);
      const id = r?.collapsed ? `summary:${round}` : r && r.hidden > 0 ? `more:${round}` : `round:${round}`;
      return visible.has(id) ? { node: id } : null;
    },
    [model, visible, refById, roundOfChild],
  );

  // Messages revealed at `t` (filters aside), and the ones that just landed.
  const shownMessages = useMemo(() => model.messages.filter((m) => isShown(m.id)), [model, isShown]);
  const seen = useRef<Set<string> | null>(null);
  const arrivedAt = useRef(new Map<string, number>());
  const arriving = useMemo(() => {
    const now = Date.now();
    const ids = new Set(shownMessages.map((m) => m.id));
    if (seen.current !== null) {
      for (const id of ids) if (!seen.current.has(id)) arrivedAt.current.set(id, now);
    }
    seen.current = ids;
    return (id: string) => now - (arrivedAt.current.get(id) ?? -Infinity) < ARRIVE_MS;
  }, [shownMessages]);

  const listed = useMemo(() => shownMessages.filter((m) => passes(m, kinds, participant)), [shownMessages, kinds, participant]);

  // Each card's message count, over what is revealed.
  const counts = useMemo(() => {
    const c = new Map<string, number>();
    for (const m of shownMessages) {
      const at = new Set<string>();
      for (const e of [m.from, ...m.to]) {
        const a = anchorOf(e, m);
        if (a && a.node !== 'rail') at.add(a.node);
      }
      for (const id of at) c.set(id, (c.get(id) ?? 0) + 1);
    }
    return c;
  }, [shownMessages, anchorOf]);

  // --- the board rail and frame strips, as of `t` -------------------------
  const boardAsOf = useCallback(
    (spaceId: string, view: (typeof model.board)['view']) => {
      if (!view || 'kind' in view) return view;
      return {
        ...view,
        claims: view.claims.filter((c) => isShown(`claim:${spaceId}:${c.key}:${c.acquired_at}`)),
        directives: view.directives.filter((d) => isShown(`directive:${spaceId}:${d.id}`)),
      };
    },
    [isShown],
  );
  const railView = boardAsOf(FLOW_SPACE, model.board.view);
  const railPosts = model.board.posts.filter((p) => isShown(p.id));
  /** The anchors each board shows, `<node>|<key>`. */
  const anchors = useMemo(() => {
    const s = new Set<string>();
    const add = (node: string, view: typeof railView, posts: FlowMessage[]) => {
      if (view && !('kind' in view)) {
        for (const c of view.claims) s.add(`${node}|claim:${c.key}:${c.acquired_at}`);
        for (const d of view.directives) s.add(`${node}|${d.id}`);
      }
      for (const p of posts) s.add(`${node}|${p.id}`);
    };
    add('rail', railView, railPosts);
    for (const [f, b] of model.board.frames) {
      if (visible.has(f)) add(f, boardAsOf(f, b.view), b.posts.filter((p) => isShown(p.id)));
    }
    return s;
  }, [railView, railPosts, model, visible, boardAsOf, isShown]);
  const anchor = useCallback(
    (key: string) => <Handle key={`a:${key}`} id={key} type="target" position={Position.Left} className="afx-anchor" />,
    [],
  );

  // --- selection -----------------------------------------------------------
  const [listSelected, setListSelected] = useState(0);
  const selectMessage = useCallback((id: string | null) => setSelectedMsg(id), []);
  const selectFromList = useCallback((id: string | null) => {
    setSelectedMsg(id);
    if (id) setListSelected((n) => n + 1);
  }, []);

  /** The cards a message touches, as drawn. */
  const endsOf = useCallback(
    (m: FlowMessage) => [m.from, ...m.to].map((e) => anchorOf(e, m)?.node).filter((x): x is string => !!x),
    [anchorOf],
  );
  const roundOfNode = useCallback(
    (id: string): number | undefined => {
      if (id.startsWith('round:') || id.startsWith('more:') || id.startsWith('summary:')) return Number(id.split(':')[1]);
      const base = id.includes('/') ? id.slice(0, id.indexOf('/')) : id;
      return refById.get(base)?.round ?? roundOfChild.get(base);
    },
    [refById, roundOfChild],
  );

  // --- nodes ---------------------------------------------------------------
  const slots = useEntranceSlots(layout.positions);
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
  const childById = useMemo(() => new Map(children.map((c) => [c.run_id, c])), [children]);
  const leadTint = crewTint(parseCodename(record.codename).crew, mode) ?? colors.get('brand.500');

  const nodes = useMemo<Node[]>(() => {
    const out: Node[] = [];
    const pos = (id: string) => {
      const p = layout.positions.get(id);
      return p ? { x: p.x, y: p.y } : { x: 0, y: 0 };
    };
    const common = (id: string) => ({
      id,
      position: pos(id),
      draggable: false,
      measured: measured.get(id),
      hidden: !visible.has(id),
    });
    const roundYs = model.rounds.map((r) => {
      const p = layout.positions.get(`round:${r.round}`);
      return p ? p.y + Math.min(p.height, 60) / 2 : 0;
    });
    out.push({
      id: 'spine',
      type: 'spine',
      position: { x: SPINE_X - 12, y: 0 },
      draggable: false,
      selectable: false,
      zIndex: -1,
      data: {
        height: layout.height,
        dots: model.rounds.filter((r) => visible.has(`round:${r.round}`)).map((r) => roundYs[model.rounds.indexOf(r)]),
        live: running && tl.t === null,
        tint: leadTint,
      },
    });
    out.push({
      ...common('lead'),
      type: 'lead',
      selectable: false,
      data: { codename: record.codename, agent: detail.def?.lead ?? null, startedAt: record.started_at },
    });
    for (const r of model.rounds) {
      const id = `round:${r.round}`;
      out.push({
        ...common(id),
        type: 'round',
        data: {
          round: r,
          live: r.round === liveRound,
          launched: r.childIds.length,
          messages: counts.get(id) ?? 0,
          slot: slots.get(id) ?? 0,
        },
      });
      if (r.collapsed) {
        const sid = `summary:${r.round}`;
        out.push({
          ...common(sid),
          type: 'summary',
          data: {
            variant: 'summary',
            round: r.round,
            children: r.childIds.map((c) => childById.get(c)).filter((c): c is RunChild => !!c),
            onExpand,
            slot: slots.get(sid) ?? 0,
          },
        });
      }
    }
    for (const n of model.nodes) {
      if (n.kind === 'child' && n.child) {
        out.push({
          ...common(n.id),
          type: 'child',
          data: { child: n.child, inferred: n.inferred, messages: counts.get(n.id) ?? 0, slot: slots.get(n.id) ?? 0 },
        });
      } else if (n.kind === 'more') {
        const r = model.rounds.find((x) => x.round === n.round)!;
        const inRound = new Set(r.childIds);
        out.push({
          ...common(n.id),
          type: 'more',
          data: {
            variant: 'more',
            round: n.round,
            // The round's top-level children not drawn as cards.
            children: r.childIds
              .filter((c) => !refById.has(c))
              .map((c) => childById.get(c))
              .filter((c): c is RunChild => !!c && !inRound.has(c.parent_run_id)),
            onExpand,
            slot: slots.get(n.id) ?? 0,
          },
        });
      } else if (n.kind === 'frame' && n.child) {
        const box = layout.frames.get(n.id);
        let graphHeight = (box?.height ?? 220) - FRAME_HEAD;
        if (n.graph) {
          let bottom = 0;
          for (const s of n.graph.nodes) {
            const p = layout.positions.get(`${n.id}/${s.id}`);
            if (p) bottom = Math.max(bottom, p.y + p.height);
          }
          graphHeight = bottom - (box?.y ?? 0) - FRAME_HEAD + FRAME_PAD;
        } else {
          graphHeight = Math.max(80, graphHeight - 40);
        }
        const frameBoard = model.board.frames.get(n.id) ?? null;
        out.push({
          ...common(n.id),
          type: 'frame',
          zIndex: 0,
          data: {
            child: n.child,
            round: n.round,
            inferred: n.inferred,
            width: box?.width ?? 480,
            graphHeight,
            loaded: !!n.graph,
            error: data.frameErrors.get(n.id) ?? null,
            board: frameBoard && {
              ...frameBoard,
              view: boardAsOf(n.id, frameBoard.view),
              posts: frameBoard.posts.filter((p) => isShown(p.id)),
            },
            messages: counts.get(n.id) ?? 0,
            onLoad: data.loadFrame,
            onDefinition,
            onSelectMessage: selectMessage,
            anchor,
            slot: slots.get(n.id) ?? 0,
          },
        });
        if (n.graph) {
          for (const s of n.graph.nodes) {
            const id = `${n.id}/${s.id}`;
            out.push({
              ...common(id),
              type: flowKind(s.kind),
              zIndex: 1,
              data: { node: s, slot: slots.get(id) ?? 0 },
            });
          }
        }
      }
    }
    out.push({
      id: 'rail',
      type: 'rail',
      position: { x: layout.railX, y: 0 },
      draggable: false,
      selectable: false,
      measured: measured.get('rail'),
      data: { view: railView, posts: railPosts, minHeight: layout.height, onSelect: selectMessage, anchor },
    });
    const ends = new Set(model.messages.flatMap((m) => [m.from, ...m.to]));
    if (ends.has(OPERATOR)) {
      out.push({
        id: 'operator',
        type: 'entry',
        position: { x: layout.operator.x, y: layout.operator.y },
        draggable: false,
        selectable: false,
        data: { label: 'operator' },
      });
    }
    if (ends.has(OFFCANVAS)) {
      out.push({
        id: 'offcanvas',
        type: 'entry',
        position: { x: layout.operator.x, y: layout.operator.y + 40 },
        draggable: false,
        selectable: false,
        data: { label: 'off map' },
      });
    }
    return out.map((n) => (n.id === focusNode ? { ...n, selected: true } : n));
  }, [
    model,
    layout,
    measured,
    visible,
    slots,
    counts,
    liveRound,
    running,
    tl.t,
    leadTint,
    record,
    detail.def,
    childById,
    refById,
    roundOfChild,
    onExpand,
    onDefinition,
    selectMessage,
    anchor,
    boardAsOf,
    isShown,
    railView,
    railPosts,
    data.frameErrors,
    data.loadFrame,
    focusNode,
  ]);

  // --- edges ---------------------------------------------------------------
  const edges = useMemo<Edge[]>(() => {
    const out: Edge[] = [];
    // The tree: a round into what it launched, a child into its children.
    const tree = (source: string, target: string, sRef: FlowNodeRef | undefined, tRef: FlowNodeRef | undefined) => {
      if (!visible.has(source) || !visible.has(target)) return;
      const tone = edgeTone(stateOfRef(sRef, liveRound), tRef ? stateOfRef(tRef, liveRound) : 'pending');
      const fromKey = runKindAccent(runKindOfRef(sRef));
      const toKey = runKindAccent(runKindOfRef(tRef));
      out.push({
        id: `t:${source}->${target}`,
        source,
        target,
        type: 'run',
        className: tone === 'live' ? 'rg-edge-flow' : undefined,
        data: {
          tone,
          from: colors.get(fromKey),
          to: colors.get(toKey),
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
    // The messages.
    const focus = [focusNode, hoverNode].filter((x): x is string => !!x);
    const focusRound = focusNode?.startsWith('round:') ? Number(focusNode.slice(6)) : null;
    const msg = (m: FlowMessage, targets: string[], onSelect?: (id: string) => void) => {
      if (!passes(m, kinds, participant)) return;
      const from = anchorOf(m.from, m);
      if (!from) return;
      const ends = endsOf(m);
      const touched = focus.some((f) => ends.includes(f));
      const inRound = focusRound !== null && ends.some((e) => roundOfNode(e) === focusRound);
      const highlighted = m.id === selectedMsg || touched;
      if (!m.atRest && !highlighted && !inRound) return;
      targets.forEach((t, i) => {
        const to = anchorOf(t, m);
        if (!to) return;
        const handle = to.handle && anchors.has(`${to.node}|${to.handle}`) ? to.handle : undefined;
        out.push({
          id: `m:${m.space}:${m.id}:${to.node}:${i}`,
          source: from.node,
          target: to.node,
          targetHandle: handle,
          type: 'message',
          zIndex: highlighted ? 20 : 10,
          data: { message: m, highlighted, arriving: arriving(m.id), onSelect },
        } satisfies MessageFlowEdge);
      });
    };
    for (const m of shownMessages) msg(m, m.to, selectMessage);
    // Each claim, from the card that holds it to its place on the board.
    for (const [spaceId, b] of [[FLOW_SPACE, model.board] as const, ...model.board.frames]) {
      const view = boardAsOf(spaceId, b.view);
      if (!view || 'kind' in view) continue;
      const atRest = model.messages.filter((m) => m.space === spaceId).every((m) => m.atRest);
      for (const c of view.claims) {
        const writer = b.writers.get(c.owner);
        if (!writer) continue;
        const at = Date.parse(c.acquired_at);
        const claim: FlowMessage = {
          id: `claim:${c.key}:${c.acquired_at}`,
          kind: 'post',
          from: writer,
          to: [BOARD],
          undelivered: [],
          atRest,
          space: spaceId,
          at: Number.isFinite(at) ? at : 0,
          atKnown: Number.isFinite(at),
          body: `claim: ${c.key_known ? c.key : '(key not recorded)'} · ${c.state}`,
          message: {
            id: `claim:${c.key}:${c.acquired_at}`,
            ts: c.acquired_at,
            kind: 'post',
            post_kind: 'claim',
            from: c.owner,
            to: null,
            body: c.key,
            delivery: { resolved: [] },
            seen: [],
          },
        };
        msg(claim, [BOARD]);
      }
    }
    return out;
  }, [
    model,
    visible,
    liveRound,
    colors,
    slots,
    refById,
    focusNode,
    hoverNode,
    kinds,
    participant,
    anchorOf,
    endsOf,
    roundOfNode,
    selectedMsg,
    anchors,
    arriving,
    shownMessages,
    selectMessage,
    boardAsOf,
  ]);

  // --- interaction ---------------------------------------------------------
  const onNodesChange = useCallback((changes: NodeChange[]) => {
    setMeasured((prev) => mergeMeasured(prev, changes));
  }, []);
  const onNodeClick = useCallback<NodeMouseHandler>((_e, n) => {
    if (n.type === 'spine' || n.type === 'rail' || n.type === 'lead') return;
    setFocusNode((cur) => (cur === n.id ? null : n.id));
  }, []);
  const onNodeEnter = useCallback<NodeMouseHandler>((_e, n) => setHoverNode(n.id), []);
  const onNodeLeave = useCallback<NodeMouseHandler>(() => setHoverNode(null), []);

  // The viewport: fit the map's width at first, from its top (a running flow
  // from its newest round); follow the newest round while running unless
  // the operator has panned away. A poll never moves it otherwise.
  const pane = useRef<HTMLDivElement>(null);
  const panned = useRef(false);
  const placed = useRef(false);
  const lastRound = model.rounds[model.rounds.length - 1];
  const lastRoundPos = lastRound ? layout.positions.get(`round:${lastRound.round}`) : undefined;
  const followKey = running && lastRound ? `round:${lastRound.round}` : null;
  useEffect(() => {
    const el = pane.current;
    if (!el || !lastRoundPos) return;
    const w = el.clientWidth;
    const h = el.clientHeight;
    if (w === 0 || h === 0) return;
    const left = layout.operator.x - 24;
    const zoom = Math.min(1, Math.max(FIT.minZoom, w / (layout.width - left + 48)));
    const x = -left * zoom + 24;
    if (!placed.current) {
      placed.current = true;
      if (layout.height * zoom < h) {
        void rf.fitView(FIT);
        return;
      }
      const y = running ? -(lastRoundPos.y * zoom) + h * 0.35 : 24;
      void rf.setViewport({ x, y: Math.min(24, y), zoom });
      return;
    }
    if (!followKey || panned.current) return;
    const vp = rf.getViewport();
    void rf.setViewport({ x: vp.x, y: Math.min(24, -(lastRoundPos.y * vp.zoom) + h * 0.35), zoom: vp.zoom }, { duration: 320 });
    // Only a new newest round (or the first placement) moves the viewport.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [followKey, lastRoundPos !== undefined]);

  // A row picked in the Messages list finds its arc: the map centres on it.
  useEffect(() => {
    if (!selectedMsg || listSelected === 0) return;
    const m = model.messages.find((x) => x.id === selectedMsg);
    if (!m) return;
    const ends = endsOf(m).filter((e) => layout.positions.has(e));
    if (ends.length === 0) return;
    const ps = ends.map((e) => layout.positions.get(e)!);
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
            setFocusNode(null);
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

/** Over the canvas's top-right corner: the children at a glance. */
function MapSummary({ list, messages }: { list: RunChild[]; messages: number }) {
  const count = (s: StepState) => list.filter((c) => childState(c) === s).length;
  const items: [StepState, number, string][] = [
    ['done', count('done'), 'done'],
    ['running', count('running'), 'running'],
    ['failed', count('failed'), 'failed'],
  ];
  return (
    <div className="gx-summary" data-testid="flow-summary">
      {items
        .filter(([, n]) => n > 0)
        .map(([s, n, w]) => (
          <span key={s} data-state={s} className="gx-summary-item gx-s">
            <i className="afx-dot" aria-hidden />
            <b>{n}</b> {w}
          </span>
        ))}
      <span className="gx-summary-item">
        <b>{messages}</b> messages
      </span>
    </div>
  );
}

/** Goals met after the latest assessed round (the record's once the flow
 *  completes), and the run's spend. */
function latestGoals(detail: AgentiflowDetail, model: ReturnType<typeof buildFlowModel>) {
  if (detail.record.status === 'completed' && detail.record.goals.length > 0) return detail.record.goals;
  for (let i = model.rounds.length - 1; i >= 0; i--) if (model.rounds[i].goalsAfter) return model.rounds[i].goalsAfter!;
  return detail.record.goals;
}

function Meter({ value, tone }: { value: number; tone: 'ok' | 'brand' }) {
  return (
    <span className="afx-meter" data-tone={tone} aria-hidden>
      <i style={{ transform: `scaleX(${Math.min(1, Math.max(0, value))})` }} />
    </span>
  );
}

function HeaderStrip({
  detail,
  model,
  running,
}: {
  detail: AgentiflowDetail;
  model: ReturnType<typeof buildFlowModel>;
  running: boolean;
}) {
  const { record, def } = detail;
  const goals = latestGoals(detail, model);
  const total = def?.goals.length ?? record.goals_total;
  const met = goals.filter((g) => g.met).length;
  const capUsd = def?.budget?.usd ?? null;
  const capRounds = def?.budget?.rounds ?? null;
  const started = Date.parse(record.started_at);
  const ended = record.ended_at ? Date.parse(record.ended_at) : Date.now();
  return (
    <div className="afx-head" data-testid="flow-head">
      <span>
        <span className="k">round</span> <b>{model.rounds.length}</b>
        {capRounds ? ` / ${capRounds}` : ''}
      </span>
      <span>
        <span className="k">goals</span> {total > 0 && <Meter value={met / total} tone="ok" />}
        <b>{met}</b> / {total} met
      </span>
      <span>
        <span className="k">spend</span> {capUsd ? <Meter value={(record.spent_usd ?? 0) / capUsd} tone="brand" /> : null}
        <b>{formatSpendUsd(record.spent_usd)}</b>
        {capUsd ? ` / ${formatSpendUsd(capUsd)}` : ''}
      </span>
      <span>
        <b>{formatSpendTokens(record.spent_tokens)}</b>
      </span>
      {Number.isFinite(started) && (
        <span>
          <span className="k">{running ? 'running' : 'ran'}</span> {formatDuration(Math.max(0, ended - started))}
        </span>
      )}
    </div>
  );
}

function GoalsPanel({
  detail,
  model,
  list,
  spaces,
}: {
  detail: AgentiflowDetail;
  model: ReturnType<typeof buildFlowModel>;
  list: RunChild[];
  spaces: { participants: { live: boolean }[] }[];
}) {
  const after = new Map(latestGoals(detail, model).map((g) => [g.id, g]));
  const views = goalViews(detail.record, detail.def);
  const count = (s: StepState) => list.filter((c) => childState(c) === s).length;
  const participants = spaces.flatMap((s) => s.participants);
  return (
    <section className="afx-panel" aria-label="Goals">
      <h3 className="afx-panel-h">
        Goals {detail.def?.description && <span className="afx-panel-sub">{detail.def.description}</span>}
      </h3>
      <ul className="afx-goals">
        {views.map((g) => {
          const now = after.get(g.id);
          const met = now?.met ?? g.met;
          return (
            <li key={g.id}>
              {met ? (
                <CheckCircle2 size={14} className="text-ok" aria-label="met" />
              ) : (
                <Circle size={14} className="text-ink-mute" aria-label="not met" />
              )}
              <span className="o">{g.objective}</span>
              <span className="c">
                {now?.current ?? g.current}/{g.target}
              </span>
            </li>
          );
        })}
        {views.length === 0 && <li className="text-ink-mute">No goals recorded.</li>}
      </ul>
      <dl className="afx-kv">
        <dt>children</dt>
        <dd>
          {count('done')} done · {count('running')} running · {count('failed')} failed
        </dd>
        <dt>participants</dt>
        <dd>
          {participants.filter((p) => p.live).length} live of {participants.length}
        </dd>
      </dl>
    </section>
  );
}
