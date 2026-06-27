// WorkflowEditorGraph — the editable @xyflow/react canvas for the workflow editor.
//
// Mirrors RunGraph's idioms (ReactFlowProvider wrapper + inner component,
// module-scope memoized NODE_TYPES, Background/Controls/MiniMap) but is fully
// editable: a palette adds nodes, edges are drawn with validated connections
// (rejected with a reason via onInvalidConnection), nodes/edges are deleted, and
// nodes are dragged. All mutation flows through `onChange(fullGraph)` — the parent
// owns the WorkflowGraph; this component is a controlled view over it.
//
// The mutation logic lives in small exported pure helpers (applyConnect /
// applyDelete / applyAddNode) so it is unit-testable without mounting the canvas.

import { useCallback, useMemo } from 'react';
import {
  Background,
  BackgroundVariant,
  Controls,
  MarkerType,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  applyNodeChanges,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
  type NodeMouseHandler,
  type NodeTypes,
} from '@xyflow/react';
import {
  canConnect,
  type GraphNode,
  type StepKind,
  type StepNodeData,
  type WorkflowGraph,
} from '../../lib/workflowGraph';
import { autoLayout } from '../../lib/workflowLayout';
import EditableStepNode, { type NodeData } from './nodes/EditableStepNode';

import '@xyflow/react/dist/style.css';

// Memoized once at module scope so React Flow doesn't see a fresh object each
// render (mirrors RunGraph).
const NODE_TYPES: NodeTypes = { editable: EditableStepNode };

// ── Pure mutation helpers (exported for tests) ───────────────────────────────

/** Validate + apply a drawn connection. Valid → onChange with the new edge
 *  appended; invalid → onInvalid(reason) and NO onChange. Missing endpoints are
 *  ignored. */
export function applyConnect(
  graph: WorkflowGraph,
  conn: { source: string | null; target: string | null },
  onChange: (g: WorkflowGraph) => void,
  onInvalid: (reason: string) => void,
): void {
  const { source, target } = conn;
  if (!source || !target) return;
  const res = canConnect(source, target, { edges: graph.edges });
  if (!res.ok) {
    onInvalid(res.reason);
    return;
  }
  const id = `${source}->${target}`;
  onChange({ ...graph, edges: [...graph.edges, { id, source, target }] });
}

/** Remove a node AND every edge that touches it. */
export function applyDelete(graph: WorkflowGraph, id: string): WorkflowGraph {
  return {
    ...graph,
    nodes: graph.nodes.filter((n) => n.id !== id),
    edges: graph.edges.filter((e) => e.source !== id && e.target !== id),
  };
}

/** Smallest free `step-N` id (N ≥ 1) not already used by a node. */
export function nextNodeId(nodes: GraphNode[]): string {
  const ids = new Set(nodes.map((n) => n.id));
  let i = 1;
  while (ids.has(`step-${i}`)) i++;
  return `step-${i}`;
}

/** Append a new node of the given kind near canvas center; returns the updated
 *  graph and the new id (so the caller can select it to open the form). */
export function applyAddNode(graph: WorkflowGraph, kind: StepKind): { graph: WorkflowGraph; id: string } {
  const id = nextNodeId(graph.nodes);
  const data: StepNodeData = { id, kind };
  if (kind === 'parallel') data.parallel = [];
  if (kind === 'panel') data.panel = { panelists: [], subject: '' };
  const count = graph.nodes.length;
  const node: GraphNode = { id, data, position: { x: 60, y: 60 + 100 * count } };
  return { graph: { ...graph, nodes: [...graph.nodes, node] }, id };
}

// ── Component ────────────────────────────────────────────────────────────────

interface Props {
  graph: WorkflowGraph;
  onChange: (g: WorkflowGraph) => void;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  problemsById: Record<string, string[]>;
  onInvalidConnection: (reason: string) => void;
  /** When true the YAML is currently unparseable: keep the last good graph on
   *  screen but dim it, freeze interactions, and show a "graph paused" chip. */
  paused?: boolean;
}

export default function WorkflowEditorGraph(props: Props) {
  return (
    <ReactFlowProvider>
      <WorkflowEditorGraphInner {...props} />
    </ReactFlowProvider>
  );
}

function WorkflowEditorGraphInner({
  graph,
  onChange,
  selectedId,
  onSelect,
  problemsById,
  onInvalidConnection,
  paused = false,
}: Props) {
  const nodes = useMemo<Node<NodeData>[]>(
    () =>
      graph.nodes.map((node) => ({
        id: node.id,
        type: 'editable',
        position: node.position,
        data: { node, problems: problemsById[node.id] ?? [] },
        selected: node.id === selectedId,
      })),
    [graph.nodes, problemsById, selectedId],
  );

  const edges = useMemo<Edge[]>(
    () =>
      graph.edges.map((e) => ({
        id: e.id,
        source: e.source,
        target: e.target,
        markerEnd: { type: MarkerType.ArrowClosed },
      })),
    [graph.edges],
  );

  // Move (drag) + delete (Backspace/Delete) both arrive as node changes.
  const onNodesChange = useCallback(
    (changes: NodeChange<Node<NodeData>>[]) => {
      const removed = changes.flatMap((c) => (c.type === 'remove' ? [c.id] : []));
      if (removed.length > 0) {
        let g = graph;
        for (const id of removed) g = applyDelete(g, id);
        onChange(g);
        if (selectedId !== null && removed.includes(selectedId)) onSelect(null);
        return;
      }
      const moves = changes.filter((c) => c.type === 'position');
      if (moves.length === 0) return;
      const next = applyNodeChanges(moves, nodes);
      const posById = new Map(next.map((n) => [n.id, n.position]));
      onChange({
        ...graph,
        nodes: graph.nodes.map((n) => {
          const p = posById.get(n.id);
          return p ? { ...n, position: { x: p.x, y: p.y } } : n;
        }),
      });
    },
    [graph, nodes, onChange, selectedId, onSelect],
  );

  const onEdgesChange = useCallback(
    (changes: EdgeChange<Edge>[]) => {
      const removed = new Set(changes.flatMap((c) => (c.type === 'remove' ? [c.id] : [])));
      if (removed.size === 0) return;
      onChange({ ...graph, edges: graph.edges.filter((e) => !removed.has(e.id)) });
    },
    [graph, onChange],
  );

  const onConnect = useCallback(
    (conn: Connection) => applyConnect(graph, conn, onChange, onInvalidConnection),
    [graph, onChange, onInvalidConnection],
  );

  // Block the visual drop for invalid targets before onConnect even fires.
  const isValidConnection = useCallback(
    (c: Edge | Connection) =>
      !!c.source && !!c.target && canConnect(c.source, c.target, { edges: graph.edges }).ok,
    [graph.edges],
  );

  const onNodeClick = useCallback<NodeMouseHandler<Node<NodeData>>>(
    (_evt, node) => onSelect(node.id),
    [onSelect],
  );

  const onPaneClick = useCallback(() => onSelect(null), [onSelect]);

  const addNode = useCallback(
    (kind: StepKind) => {
      const { graph: g, id } = applyAddNode(graph, kind);
      onChange(g);
      onSelect(id);
    },
    [graph, onChange, onSelect],
  );

  const relayout = useCallback(() => {
    onChange({ ...graph, nodes: autoLayout(graph.nodes, graph.edges) });
  }, [graph, onChange]);

  return (
    <div className="relative h-full min-h-[16rem] w-full overflow-hidden rounded-xl border border-border shadow-card">
      {/* "graph paused" chip — top-center, shown while YAML is unparseable */}
      {paused && (
        <div
          role="status"
          className="pointer-events-none absolute left-1/2 top-3 z-20 -translate-x-1/2 rounded-full border border-amber-300 bg-amber-50 px-3 py-1 text-[11px] font-medium text-amber-800 shadow-card"
        >
          YAML not parseable — graph paused
        </div>
      )}

      {/* palette + toolbar — static DOM floated over the canvas */}
      <div className="absolute left-3 top-3 z-10 flex flex-wrap items-center gap-1.5 rounded-lg border border-border bg-white/95 px-2 py-1.5 shadow-card">
        <span className="pr-1 text-[10px] font-semibold uppercase tracking-wide text-ink-mute">Add</span>
        <button
          type="button"
          disabled={paused}
          onClick={() => addNode('step')}
          className="rounded-md border border-blue-200 bg-blue-50 px-2 py-1 text-[11px] font-medium text-blue-700 hover:bg-blue-100 disabled:cursor-not-allowed disabled:opacity-50"
        >
          Step
        </button>
        <button
          type="button"
          disabled={paused}
          onClick={() => addNode('for_each')}
          className="rounded-md border border-violet-200 bg-violet-50 px-2 py-1 text-[11px] font-medium text-violet-700 hover:bg-violet-100 disabled:cursor-not-allowed disabled:opacity-50"
        >
          For-each
        </button>
        <button
          type="button"
          disabled={paused}
          onClick={() => addNode('parallel')}
          className="rounded-md border border-purple-200 bg-purple-50 px-2 py-1 text-[11px] font-medium text-purple-700 hover:bg-purple-100 disabled:cursor-not-allowed disabled:opacity-50"
        >
          Parallel
        </button>
        <button
          type="button"
          disabled={paused}
          onClick={() => addNode('panel')}
          className="rounded-md border border-amber-200 bg-amber-50 px-2 py-1 text-[11px] font-medium text-amber-700 hover:bg-amber-100 disabled:cursor-not-allowed disabled:opacity-50"
        >
          Panel
        </button>
        <span className="mx-1 h-4 w-px bg-border" aria-hidden />
        <button
          type="button"
          disabled={paused}
          onClick={relayout}
          className="rounded-md border border-border bg-white px-2 py-1 text-[11px] font-medium text-ink-dim hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50"
        >
          Re-layout
        </button>
      </div>

      <div className={paused ? 'h-full w-full opacity-60 pointer-events-none' : 'h-full w-full'}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        isValidConnection={isValidConnection}
        onNodeClick={onNodeClick}
        onPaneClick={onPaneClick}
        nodesDraggable={!paused}
        nodesConnectable={!paused}
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
    </div>
  );
}
