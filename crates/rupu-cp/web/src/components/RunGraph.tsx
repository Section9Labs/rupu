// RunGraph (v2) — a read-only @xyflow/react canvas for a run's step DAG.
//
// Six custom node types (step / parallel / fanout / panel / gate / action)
// painted from a `RunGraphModel`, positioned by the pre-computed dagre
// layout, joined by `RunEdge`s. The graph is alive the way the charts are
// (styles/graph-motion.css): it arrives column by column, traversed edges
// fill with their kinds' gradient, a light travels into a running step (amber
// into a gate that waits on you), and running / waiting / failed nodes glow in
// their state's color.

import { useCallback, useMemo, useState } from 'react';
import {
  Background,
  BackgroundVariant,
  Controls,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
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

import '@xyflow/react/dist/style.css';

// Map the model's step kind → a registered React Flow node type.
type FlowKind = 'step' | 'parallel' | 'fanout' | 'panel' | 'gate' | 'action';

function flowKind(kind: StepNodeDto['kind']): FlowKind {
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
// render (which it warns about and which defeats node memoization).
const NODE_TYPES: NodeTypes = {
  step: StepNode,
  parallel: ParallelNode,
  fanout: FanoutNode,
  panel: PanelLoopNode,
  gate: GateNode,
  action: ActionNode,
};
const EDGE_TYPES: EdgeTypes = { run: RunEdge };

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
}

export default function RunGraph(props: Props) {
  return (
    <ReactFlowProvider>
      <RunGraphInner {...props} />
    </ReactFlowProvider>
  );
}

function RunGraphInner({ model, positions, onOpenUnit, onExpandFanout, onSelectNode, gate }: Props) {
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
  const handleNodeClick = useCallback<NodeMouseHandler<Node<NodeData>>>(
    (_evt, flowNode) => {
      if (!onSelectNode) return;
      const node = flowNode.data.node;
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
  const onNodesChange = useCallback((changes: NodeChange<Node<NodeData>>[]) => {
    setMeasured((prev) => mergeMeasured(prev, changes));
    for (const c of changes) {
      if (c.type === 'select') setSelectedId((cur) => (c.selected ? c.id : cur === c.id ? null : cur));
    }
  }, []);

  const nodes = useMemo<Node<NodeData>[]>(() => {
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
      };
    });
  }, [model, positions, slots, measured, selectedId, handleOpenUnit, onExpandFanout, gate]);

  const edges = useMemo<RunFlowEdge[]>(() => {
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
        id: `${e.from}->${e.to}`,
        source: e.from,
        target: e.to,
        type: 'run',
        className: tone === 'live' || awaiting ? 'rg-edge-flow' : undefined,
        style: { stroke, strokeWidth: 2 },
        data: {
          tone,
          from: colors.get(fromKey),
          to: awaiting ? colors.status.awaiting : colors.get(toKey),
          light: awaiting ? colors.status.awaiting : colors.status.running,
          slot: slots.get(e.from) ?? 0,
        },
      };
    });
  }, [model, colors, slots]);

  if (model.nodes.length === 0) {
    return (
      <div className="flex h-[460px] w-full items-center justify-center rounded-xl border border-border bg-panel text-sm text-ink-dim shadow-card">
        No steps recorded for this run yet.
      </div>
    );
  }

  return (
    <div className="gx-canvas relative h-[460px] w-full overflow-hidden rounded-xl border border-border shadow-card">
      <GraphSummary nodes={model.nodes} />
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        edgeTypes={EDGE_TYPES}
        onNodeClick={handleNodeClick}
        onNodesChange={onNodesChange}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable
        fitView
        // Low enough that a narrow pane (a phone) still frames the whole run.
        minZoom={0.2}
        fitViewOptions={{ padding: 0.2, maxZoom: 1.0, minZoom: 0.2 }}
        proOptions={{ hideAttribution: true }}
        style={{ background: 'transparent' }}
      >
        <Background variant={BackgroundVariant.Dots} gap={20} size={1.1} color={colors.alpha('ink', 0.09)} />
        {nodes.length > MINIMAP_MIN_NODES && (
          <MiniMap
            pannable
            zoomable
            style={{ width: 150, height: 96 }}
            maskColor={colors.alpha('bg', 0.55)}
            nodeColor={(n) => miniColor(colors, (n.data as NodeData).node.state)}
            nodeBorderRadius={4}
          />
        )}
        <Controls showInteractive={false} fitViewOptions={{ padding: 0.2, maxZoom: 1.0, duration: 320 }} />
      </ReactFlow>
    </div>
  );
}
