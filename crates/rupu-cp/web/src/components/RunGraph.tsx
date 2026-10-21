// RunGraph (v2) — a read-only @xyflow/react canvas for a run's step DAG.
//
// Six custom node types (step / parallel / fanout / panel / gate / action)
// painted from a `RunGraphModel`, positioned by the pre-computed dagre
// layout, joined by `RunEdge`s. The graph is alive the way the charts are
// (styles/graph-motion.css): it arrives column by column, traversed edges
// fill with their kinds' gradient, a light travels into a running step (amber
// into a gate that waits on you), and running / waiting / failed nodes glow in
// their state's color.
//
// With an `overlay` (a run with a message space, spec
// `2026-10-21-rupu-agentiflow-flow-design.md` goal 6) the graph also carries
// the run's messages as arcs between its step cards (the operator's as a
// short entry at the card it reached), the board as a rail on the right, and
// the replay's reveal: a step, edge, arc or board item not yet revealed is
// hidden. The arcs and rail are the agentiflow map's (flowmap/), routed by
// `flowEdges`.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Background,
  type FitViewOptions,
  BackgroundVariant,
  Controls,
  MiniMap,
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
import type { GraphNode, RunGraphModel } from '../lib/runGraphModel';
import type { StepNodeDto } from '../lib/api';
import type { Pos } from '../lib/graphLayout';
import { useThemeColors, type ThemeColors } from '../lib/useThemeColors';
import { stateStyle } from './graph/stepStyle';
import { STATE_WORD, StatusGlyph } from './graph/NodeFrame';
import type { GateControl } from './graph/GateActions';
import { runKindAccent } from './graph/kindBridge';
import StepNode from './graph/StepNode';
import ParallelNode from './graph/ParallelNode';
import FanoutNode from './graph/FanoutNode';
import PanelLoopNode from './graph/PanelLoopNode';
import GateNode from './graph/GateNode';
import ActionNode from './graph/ActionNode';
import RunEdge, { type RunFlowEdge } from './graph/RunEdge';
import { parseCodename } from '../lib/codename';
import { edgeTone, useEntranceSlots } from '../lib/graph/runMotion';
import { mergeMeasured, type Size } from '../lib/graph/measured';
import MessageEdge from './flowmap/MessageEdge';
import { RailNode } from './flowmap/MapNodes';
import { MINIMAP_MIN_PANE, usePaneWidth } from '../lib/graph/usePaneWidth';
import { useCamera } from '../lib/graph/useCamera';
import { bringIntoView } from '../lib/flow/camera';
import { useRunLayerCanvas, type RunGraphOverlay } from './run/RunLayerCanvas';

export type { RunGraphOverlay } from './run/RunLayerCanvas';

import '@xyflow/react/dist/style.css';

// Map the model's step kind → a registered React Flow node type.
export type FlowKind = 'step' | 'parallel' | 'fanout' | 'panel' | 'gate' | 'action';

export function flowKind(kind: StepNodeDto['kind']): FlowKind {
  switch (kind) {
    case 'parallel':
      return 'parallel';
    case 'for_each':
      return 'fanout';
    case 'panel':
      return 'panel';
    case 'gate':
      return 'gate';
    case 'action':
      return 'action';
    default:
      return 'step';
  }
}

// Memoized once at module scope so React Flow doesn't see a fresh object each
// render (which it warns about and which defeats node memoization). The
// agentiflow map draws a workflow child's steps with these same components.
export const NODE_TYPES: NodeTypes = {
  step: StepNode,
  parallel: ParallelNode,
  fanout: FanoutNode,
  panel: PanelLoopNode,
  gate: GateNode,
  action: ActionNode,
};
const EDGE_TYPES: EdgeTypes = { run: RunEdge, message: MessageEdge };
/** The layer's own node: the board rail. */
const LAYER_NODE_TYPES: NodeTypes = { ...NODE_TYPES, rail: RailNode };
const FIT_OPTIONS = { padding: 0.2, maxZoom: 1.0, minZoom: 0.2 };
/** With the minimap showing, the fit keeps the graph's right edge (the
 *  board rail's, with a layer) clear of it. */
const FIT_WITH_MINIMAP = {
  ...FIT_OPTIONS,
  padding: { top: '10%', bottom: '10%', left: '10%', right: '180px' } as FitViewOptions['padding'],
};

/** A graph this small fits the canvas; the minimap only earns its place past it. */
const MINIMAP_MIN_NODES = 6;

/** The summary's order: what needs a look first. */
const SUMMARY_ORDER: GraphNode['state'][] = ['running', 'awaiting_approval', 'failed', 'paused', 'done', 'pending', 'skipped'];

/** `2 running · 1 needs you · 1 failed …` over the canvas: the run at a glance. */
function GraphSummary({ nodes }: { nodes: readonly GraphNode[] }) {
  const counts = new Map<GraphNode['state'], number>();
  for (const n of nodes) counts.set(n.state, (counts.get(n.state) ?? 0) + 1);
  const items = SUMMARY_ORDER.filter((s) => counts.has(s));
  return (
    <div className="gx-summary" data-testid="rg-summary">
      {items.map((s) => (
        <span key={s} data-state={s} className="gx-summary-item gx-s">
          <StatusGlyph state={s} />
          <b>{counts.get(s)}</b> {STATE_WORD[s]}
        </span>
      ))}
    </div>
  );
}

/** The minimap paints each node in its state's color: a live status map. */
function miniColor(colors: ThemeColors, state: GraphNode['state']): string {
  return state === 'pending' || state === 'skipped' ? colors.alpha('inkMute', 0.45) : stateStyle(colors, state).color;
}

/** The run graph's connectors for `model`: each edge colored from its source
 *  step's kind into its target's, toned by their states. `prefix` namespaces
 *  the node ids (the agentiflow map keys a frame's steps `<frame>/<step>`). */
export function runEdges(
  model: RunGraphModel,
  colors: ThemeColors,
  slots: ReadonlyMap<string, number>,
  prefix = '',
): RunFlowEdge[] {
  return model.edges.map((e) => {
    const source = model.nodeById(e.from);
    const target = model.nodeById(e.to);
    const tone = edgeTone(source?.state, target?.state);

    // The connector takes its kinds' colors (from the SOURCE step's kind
    // into the TARGET's); the live frontier carries a glow in the running
    // color. An awaiting gate is the one place status wins over kind: the
    // edge arrives in amber, "needs you", whatever kind of step precedes it.
    const fromKey = source ? runKindAccent(source.kind) : 'inkMute';
    const toKey = target ? runKindAccent(target.kind) : fromKey;
    const awaiting = tone === 'await';
    const stroke = awaiting
      ? colors.status.awaiting
      : tone === 'idle' || tone === 'dead'
        ? colors.alpha(fromKey, 0.35)
        : colors.get(fromKey);
    return {
      id: `${prefix}${e.from}->${prefix}${e.to}`,
      source: `${prefix}${e.from}`,
      target: `${prefix}${e.to}`,
      type: 'run',
      className: tone === 'live' || awaiting ? 'rg-edge-flow' : undefined,
      style: { stroke, strokeWidth: 2 },
      data: {
        tone,
        from: colors.get(fromKey),
        to: awaiting ? colors.status.awaiting : colors.get(toKey),
        light: awaiting ? colors.status.awaiting : colors.status.running,
        slot: slots.get(`${prefix}${e.from}`) ?? 0,
      },
    };
  });
}

interface NodeData extends Record<string, unknown> {
  node: GraphNode;
  onOpenUnit?: (stepId: string, index: number) => void;
  onExpandFanout?: (stepId: string) => void;
  slot: number;
  gate?: GateControl;
}

/** A transcript selection emitted by clicking a step node or a fan-out unit. */
export interface NodeSelection {
  path: string | null;
  live: boolean;
  label: string;
}

interface Props {
  model: RunGraphModel;
  positions: Map<string, Pos>;
  onOpenUnit?: (stepId: string, index: number) => void;
  onExpandFanout?: (stepId: string) => void;
  /** Emitted on click of a step node or a fan-out unit square. */
  onSelectNode?: (sel: NodeSelection) => void;
  /** Approve / reject on a gate that waits (RunDetail's gate handlers). */
  gate?: GateControl;
  /** The run's messages, board and replay (only a run with a message space). */
  overlay?: RunGraphOverlay;
}

export default function RunGraph(props: Props) {
  return (
    <ReactFlowProvider>
      <RunGraphInner {...props} />
    </ReactFlowProvider>
  );
}

function RunGraphInner({ model, positions, onOpenUnit, onExpandFanout, onSelectNode, gate, overlay }: Props) {
  const colors = useThemeColors();
  // Clicking a fan-out unit square selects that unit's transcript AND keeps the
  // existing drill behavior. Wrap onOpenUnit so the unit click does both.
  const handleOpenUnit = useCallback(
    (stepId: string, index: number) => {
      onOpenUnit?.(stepId, index);
      if (onSelectNode) {
        const unit = model.nodeById(stepId)?.fanout?.units.find((u) => u.index === index);
        if (unit) {
          onSelectNode({
            path: unit.transcriptPath ?? null,
            live: unit.state === 'running',
            // Prefer the unit's codename leaf; RunDetail routes units via
            // onOpenUnit, so this label is display-only.
            label: unit.codename ? parseCodename(unit.codename).leaf : unit.key,
          });
        }
      }
    },
    [onOpenUnit, onSelectNode, model],
  );

  // Clicking a step (non-fanout) node selects its transcript. Fan-out unit
  // selection is handled via handleOpenUnit on the inner unit squares.
  const handleNodeClick = useCallback<NodeMouseHandler>(
    (_evt, flowNode) => {
      if (!onSelectNode) return;
      const node = (flowNode.data as Partial<NodeData>).node;
      // The layer's rail and entries are not steps.
      if (!node) return;
      // for_each and panel nodes select via their unit squares / chips
      // (handleOpenUnit), not the node body — skip so a chip click isn't
      // clobbered by the bubbled node-body selection (and panel nodes carry
      // no own transcript).
      if (node.kind === 'for_each' || node.kind === 'panel') return;
      onSelectNode({
        path: node.transcriptPath ?? null,
        live: node.state === 'running',
        label: node.id,
      });
    },
    [onSelectNode],
  );

  const slots = useEntranceSlots(positions);

  // The nodes are rebuilt from the model on every live event. Hand xyflow back
  // the sizes it measured (and the node the operator selected): a node object
  // without `measured` makes xyflow drop its handles until it re-measures, so
  // every edge would unmount and remount on each event (a flicker, and every
  // edge's animation restarting).
  const [measured, setMeasured] = useState<ReadonlyMap<string, Size>>(() => new Map());
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const onNodesChange = useCallback((changes: NodeChange[]) => {
    setMeasured((prev) => mergeMeasured(prev, changes));
    for (const c of changes) {
      if (c.type === 'select') setSelectedId((cur) => (c.selected ? c.id : cur === c.id ? null : cur));
    }
  }, []);

  const revealed = overlay?.revealed ?? null;
  const stepNodes = useMemo<Node<NodeData>[]>(() => {
    return model.nodes.map((node) => {
      const pos = positions.get(node.id);
      return {
        id: node.id,
        type: flowKind(node.kind),
        position: pos ? { x: pos.x, y: pos.y } : { x: 0, y: 0 },
        data: { node, onOpenUnit: handleOpenUnit, onExpandFanout, slot: slots.get(node.id) ?? 0, gate },
        draggable: false,
        selectable: true,
        selected: node.id === selectedId,
        measured: measured.get(node.id),
        ...(revealed && !revealed.has(node.id) ? { hidden: true } : {}),
      };
    });
  }, [model, positions, slots, measured, selectedId, handleOpenUnit, onExpandFanout, gate, revealed]);

  const runFlowEdges = useMemo(() => {
    const edges = runEdges(model, colors, slots);
    if (!revealed) return edges;
    return edges.map((e) => (revealed.has(e.source) && revealed.has(e.target) ? e : { ...e, hidden: true }));
  }, [model, colors, slots, revealed]);

  const layer = useRunLayerCanvas(overlay, model, positions, measured, selectedId);
  const pane = useRef<HTMLDivElement>(null);
  const paneWidth = usePaneWidth(pane);
  const showMinimap = model.nodes.length > MINIMAP_MIN_NODES && paneWidth >= MINIMAP_MIN_PANE;
  const fitOptions = showMinimap ? FIT_WITH_MINIMAP : FIT_OPTIONS;
  const nodes = useMemo<Node[]>(() => (layer ? [...stepNodes, ...layer.nodes] : stepNodes), [stepNodes, layer]);
  const edges = useMemo<Edge[]>(() => (layer ? [...runFlowEdges, ...layer.edges] : runFlowEdges), [runFlowEdges, layer]);

  // A replay (or a scrub) follows the newest revealed step, unless the
  // operator moved the camera since it began; "Follow" takes it back.
  const camera = useCamera();
  const replayActive = !!overlay && overlay.revealed !== null;
  const [follow, setFollow] = useState(true);
  useEffect(() => {
    if (replayActive) setFollow(true);
  }, [replayActive]);
  const followStep = replayActive ? (overlay?.follow ?? null) : null;
  useEffect(() => {
    const el = pane.current;
    if (!followStep || !follow || !el) return;
    const box = positions.get(followStep);
    if (!box) return;
    const next = bringIntoView(box, rf.getViewport(), { width: el.clientWidth, height: el.clientHeight }, {
      reserveRight: showMinimap ? 180 : 0,
      anchor: { x: 0.6, y: 0.5 },
    });
    if (next) camera(next);
    // A new step to follow, or follow taken back, moves it; nothing else.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [followStep, follow]);

  // The layer arrives after the graph (its space loads after it): frame it
  // once it is on the canvas, so the rail and the entries are in view.
  const rf = useReactFlow();
  const layerOn = !!layer;
  useEffect(() => {
    if (!layerOn || !rf.viewportInitialized) return;
    const raf = requestAnimationFrame(() => void rf.fitView(fitOptions));
    return () => cancelAnimationFrame(raf);
    // Only when the layer first appears (or the canvas first initialises).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layerOn, rf.viewportInitialized]);

  if (model.nodes.length === 0) {
    return (
      <div className="flex h-[460px] w-full items-center justify-center rounded-xl border border-border bg-panel text-sm text-ink-dim shadow-card">
        No steps recorded for this run yet.
      </div>
    );
  }

  return (
    <div
      ref={pane}
      className={`gx-canvas relative ${overlay ? 'h-[540px]' : 'h-[460px]'} w-full overflow-hidden rounded-xl border border-border shadow-card`}
    >
      <GraphSummary nodes={revealed ? model.nodes.filter((n) => revealed.has(n.id)) : model.nodes} />
      {replayActive && !follow && (
        <button type="button" className="afx-follow" onClick={() => setFollow(true)}>
          Follow
        </button>
      )}
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={overlay ? LAYER_NODE_TYPES : NODE_TYPES}
        edgeTypes={EDGE_TYPES}
        onNodeClick={handleNodeClick}
        onNodesChange={onNodesChange}
        onMoveStart={(e) => {
          if (e && replayActive) setFollow(false);
        }}
        onNodeMouseEnter={layer ? layer.onEnter : undefined}
        onNodeMouseLeave={layer ? layer.onLeave : undefined}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable
        fitView
        // Low enough that a narrow pane (a phone) still frames the whole run.
        minZoom={0.2}
        fitViewOptions={fitOptions}
        proOptions={{ hideAttribution: true }}
        style={{ background: 'transparent' }}
      >
        <Background variant={BackgroundVariant.Dots} gap={20} size={1.1} color={colors.alpha('ink', 0.09)} />
        {showMinimap && (
          <MiniMap
            pannable
            zoomable
            style={{ width: 150, height: 96 }}
            maskColor={colors.alpha('bg', 0.55)}
            nodeColor={(n) => {
              const node = (n.data as Partial<NodeData>).node;
              return node ? miniColor(colors, node.state) : colors.alpha('inkMute', 0.45);
            }}
            nodeBorderRadius={4}
          />
        )}
        <Controls showInteractive={false} fitViewOptions={{ ...fitOptions, duration: 320 }} />
      </ReactFlow>
    </div>
  );
}
