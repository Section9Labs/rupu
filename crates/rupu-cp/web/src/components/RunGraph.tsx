// RunGraph (v2) — a read-only @xyflow/react canvas for a run's step DAG.
//
// Four custom node types (step / parallel / fanout / panel) painted from a
// `RunGraphModel`, positioned by the pre-computed dagre layout. The active
// frontier animates: the edge INTO a running node gets blue marching-ants,
// the edge into an awaiting node an amber dashed flow; all others are static.
//
// Faithful to the approved graph-pro / fanout-loop mockups. Rendering is
// validated by a human — the bar here is a correct, strict-typed structure.

import { useCallback, useMemo } from 'react';
import {
  Background,
  BackgroundVariant,
  Controls,
  MarkerType,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  type Edge,
  type Node,
  type NodeMouseHandler,
  type NodeTypes,
} from '@xyflow/react';
import type { GraphNode, RunGraphModel } from '../lib/runGraphModel';
import type { StepNodeDto } from '../lib/api';
import type { Pos } from '../lib/graphLayout';
import StepNode from './graph/StepNode';
import ParallelNode from './graph/ParallelNode';
import FanoutNode from './graph/FanoutNode';
import PanelLoopNode from './graph/PanelLoopNode';

import '@xyflow/react/dist/style.css';

// Map the model's step kind → a registered React Flow node type.
type FlowKind = 'step' | 'parallel' | 'fanout' | 'panel';

function flowKind(kind: StepNodeDto['kind']): FlowKind {
  switch (kind) {
    case 'parallel':
      return 'parallel';
    case 'for_each':
      return 'fanout';
    case 'panel':
      return 'panel';
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
};

interface NodeData extends Record<string, unknown> {
  node: GraphNode;
  onOpenUnit?: (stepId: string, index: number) => void;
  onExpandFanout?: (stepId: string) => void;
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
}

export default function RunGraph(props: Props) {
  return (
    <ReactFlowProvider>
      <RunGraphInner {...props} />
    </ReactFlowProvider>
  );
}

function RunGraphInner({ model, positions, onOpenUnit, onExpandFanout, onSelectNode }: Props) {
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
            label: unit.key,
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
      // for_each nodes select via their unit squares (handleOpenUnit), not the
      // node body — skip to avoid clobbering a unit selection.
      if (node.kind === 'for_each') return;
      onSelectNode({
        path: node.transcriptPath ?? null,
        live: node.state === 'running',
        label: node.id,
      });
    },
    [onSelectNode],
  );

  const nodes = useMemo<Node<NodeData>[]>(() => {
    return model.nodes.map((node) => {
      const pos = positions.get(node.id);
      return {
        id: node.id,
        type: flowKind(node.kind),
        position: pos ? { x: pos.x, y: pos.y } : { x: 0, y: 0 },
        data: { node, onOpenUnit: handleOpenUnit, onExpandFanout },
        draggable: false,
        selectable: true,
      };
    });
  }, [model, positions, handleOpenUnit, onExpandFanout]);

  const edges = useMemo<Edge[]>(() => {
    return model.edges.map((e) => {
      const target = model.nodeById(e.to);
      const targetState = target?.state;
      const active = targetState === 'running';
      const awaiting = targetState === 'awaiting_approval';

      const stroke = active ? '#1860f2' : awaiting ? '#f59e0b' : '#cbd5e1';
      return {
        id: `${e.from}->${e.to}`,
        source: e.from,
        target: e.to,
        type: 'smoothstep',
        // Animation: marching-ants is driven by the CSS class on the edge
        // group (rg-edge-active / rg-edge-await) so the dashes march along the
        // rendered curve. `animated` stays off to avoid the default dash anim
        // doubling up.
        className: active ? 'rg-edge-active' : awaiting ? 'rg-edge-await' : undefined,
        markerEnd: { type: MarkerType.ArrowClosed, color: stroke },
        style: active || awaiting ? undefined : { stroke, strokeWidth: 2 },
      };
    });
  }, [model]);

  if (model.nodes.length === 0) {
    return (
      <div className="flex h-[420px] w-full items-center justify-center rounded-xl border border-border bg-panel text-sm text-ink-dim shadow-card">
        No steps recorded for this run yet.
      </div>
    );
  }

  return (
    <div className="h-[420px] w-full overflow-hidden rounded-xl border border-brand-100 shadow-card">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        onNodeClick={handleNodeClick}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable
        fitView
        fitViewOptions={{ padding: 0.2, maxZoom: 1.0 }}
        proOptions={{ hideAttribution: true }}
        style={{ background: '#fafafa' }}
      >
        <Background variant={BackgroundVariant.Dots} gap={16} size={1} color="#e2e8f0" />
        <MiniMap pannable zoomable className="!border-border !bg-panel" />
        <Controls className="!border-border !bg-panel !shadow-card" showInteractive={false} />
      </ReactFlow>
    </div>
  );
}
