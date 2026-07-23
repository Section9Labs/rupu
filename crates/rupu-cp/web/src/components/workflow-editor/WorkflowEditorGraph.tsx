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

import { useCallback, useMemo, useState, type DragEvent, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import {
  Background,
  BackgroundVariant,
  Controls,
  MarkerType,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  applyNodeChanges,
  useReactFlow,
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
  type GraphEdge,
  type GraphNode,
  type StepKind,
  type StepNodeData,
  type WorkflowGraph,
} from '../../lib/workflowGraph';
import { autoLayout, NODE_W } from '../../lib/workflowLayout';
import { useThemeColors, type ThemeColors } from '../../lib/useThemeColors';
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';
import EditableStepNode, { type NodeData } from './nodes/EditableStepNode';
import NodePalette, { NODE_KIND_MIME } from './NodePalette';

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

/** Build the StepNodeData for a fresh node of `kind`, seeding the container
 *  shapes (parallel array, panel config) so the node is immediately valid to
 *  render and round-trip. */
function newNodeData(id: string, kind: StepKind): StepNodeData {
  const data: StepNodeData = { id, kind };
  if (kind === 'parallel') data.parallel = [];
  if (kind === 'panel') data.panel = { panelists: [], subject: '' };
  if (kind === 'branch') {
    data.condition = '';
    data.thenTargets = [];
    data.elseTargets = [];
  }
  return data;
}

/** Append a new node of `kind` at an EXPLICIT canvas position; returns the
 *  updated graph and the new id. The drop-on-canvas path (palette → drag)
 *  computes the position via `screenToFlowPosition`. */
export function applyAddNodeAt(
  graph: WorkflowGraph,
  kind: StepKind,
  position: { x: number; y: number },
): { graph: WorkflowGraph; id: string } {
  const id = nextNodeId(graph.nodes);
  const node: GraphNode = { id, data: newNodeData(id, kind), position: { x: position.x, y: position.y } };
  return { graph: { ...graph, nodes: [...graph.nodes, node] }, id };
}

/** Append a new node of the given kind near canvas center; returns the updated
 *  graph and the new id (so the caller can select it to open the form). The
 *  click-to-add (no-drag) path. */
export function applyAddNode(graph: WorkflowGraph, kind: StepKind): { graph: WorkflowGraph; id: string } {
  const count = graph.nodes.length;
  return applyAddNodeAt(graph, kind, { x: 60, y: 60 + 100 * count });
}

/** Add a new node (default kind `step`) AND an edge `sourceId -> newId`,
 *  positioned to the right of the source so the linear chain reads L→R. Powers
 *  the inline "⊕ next" fast path. Returns the updated graph and the new id. If
 *  `sourceId` is unknown the node is still added (placed at a sane default) but
 *  no edge is created. */
export function applyAddConnectedNext(
  graph: WorkflowGraph,
  sourceId: string,
  kind: StepKind = 'step',
): { graph: WorkflowGraph; id: string } {
  const source = graph.nodes.find((n) => n.id === sourceId);
  const id = nextNodeId(graph.nodes);
  const gap = 64;
  const base = source ? source.position : { x: 60, y: 60 };
  const node: GraphNode = {
    id,
    data: newNodeData(id, kind),
    position: { x: base.x + NODE_W + gap, y: base.y },
  };
  const nodes = [...graph.nodes, node];
  const edges = source
    ? [...graph.edges, { id: `${sourceId}->${id}`, source: sourceId, target: id }]
    : graph.edges;
  return { graph: { ...graph, nodes, edges }, id };
}

/** Insert a new node onto an existing edge A→B: remove A→B, add the node at
 *  `position`, and wire A→new + new→B. Stays a DAG by construction (a linear
 *  insert never closes a cycle). If `edgeId` is unknown, falls back to a plain
 *  add at `position`. Returns the updated graph and the new id. */
export function applyInsertOnEdge(
  graph: WorkflowGraph,
  edgeId: string,
  kind: StepKind,
  position: { x: number; y: number },
): { graph: WorkflowGraph; id: string } {
  const edge = graph.edges.find((e) => e.id === edgeId);
  if (!edge) return applyAddNodeAt(graph, kind, position);
  const id = nextNodeId(graph.nodes);
  const node: GraphNode = { id, data: newNodeData(id, kind), position: { x: position.x, y: position.y } };
  const edges = graph.edges.filter((e) => e.id !== edgeId);
  edges.push({ id: `${edge.source}->${id}`, source: edge.source, target: id });
  edges.push({ id: `${id}->${edge.target}`, source: id, target: edge.target });
  return { graph: { ...graph, nodes: [...graph.nodes, node], edges }, id };
}

/** Narrow an arbitrary dataTransfer string to a StepKind. Exported for tests. */
export function asStepKind(v: string): StepKind | null {
  return v === 'step' || v === 'for_each' || v === 'parallel' || v === 'panel' || v === 'branch' ? v : null;
}

/** Edge accent color for a branch arm — green for the "then" (true) arm, red
 *  for the "else" (false) arm; undefined (default edge styling) for plain
 *  chain/data-ref edges. */
function branchEdgeColor(branch: GraphEdge['branch'], colors: ThemeColors): string | undefined {
  if (branch === 'then') return colors.status.done;
  if (branch === 'else') return colors.status.failed;
  return undefined;
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
  /** Workflow-editor-UI flag — gates the branch palette card in NodePalette
   *  ('next' only). Rendering an EXISTING branch node is always on regardless
   *  of this flag; only the ability to ADD a new one from the palette is gated. */
  workflowEditorUi?: WorkflowEditorUi;
  /** Inspector-rail slot to portal the palette into (Task 1), owned by
   *  WorkflowEditor. Only consulted when `workflowEditorUi === 'next'`:
   *  - set → the palette portals in as `variant="rail"`, no floating dock.
   *  - null/undefined (rail slot not mounted yet) → palette renders nothing
   *    for that frame, rather than flashing the floating dock.
   *  Classic ALWAYS renders today's floating dock and ignores this prop. */
  paletteContainer?: HTMLElement | null;
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
  workflowEditorUi = 'classic',
  paletteContainer,
}: Props) {
  const colors = useThemeColors();
  const nodes = useMemo<Node<NodeData>[]>(
    () =>
      graph.nodes.map((node) => ({
        id: node.id,
        type: 'editable',
        position: node.position,
        data: { node, problems: problemsById[node.id] ?? [], workflowEditorUi },
        selected: node.id === selectedId,
      })),
    [graph.nodes, problemsById, selectedId, workflowEditorUi],
  );

  const edges = useMemo<Edge[]>(
    () =>
      graph.edges.map((e) => {
        const color = branchEdgeColor(e.branch, colors);
        const next = workflowEditorUi === 'next';
        const edge: Edge = {
          id: e.id,
          source: e.source,
          target: e.target,
          type: 'smoothstep',
          markerEnd: { type: MarkerType.ArrowClosed, color },
        };
        if (e.label !== undefined) edge.label = e.label;
        if (color !== undefined) {
          // Branch (true/false) arm — colored stroke, a touch bolder for `next`
          // (mirrors the mockup's `.edge.t-true`/`.edge.t-false`).
          edge.style = next ? { stroke: color, strokeWidth: 2.5 } : { stroke: color };
          edge.labelStyle = { fill: color, fontWeight: 600 };
          edge.labelBgStyle = { fillOpacity: 0 };
        } else if (next) {
          // Plain chain/data-ref edge — themed muted stroke for `next` (mockup's
          // `.edge`); classic leaves this edge with xyflow's default styling.
          edge.style = { stroke: colors.alpha('inkMute', 0.5), strokeWidth: 1.6 };
        }
        return edge;
      }),
    [graph.edges, colors, workflowEditorUi],
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

  const rf = useReactFlow();

  // Click-to-add (palette card click / keyboard): drop near center.
  const addNode = useCallback(
    (kind: StepKind) => {
      if (paused) return;
      const { graph: g, id } = applyAddNode(graph, kind);
      onChange(g);
      onSelect(id);
    },
    [graph, onChange, onSelect, paused],
  );

  // Inline "⊕ next": add a step connected from the selected node and select it.
  const addConnectedNext = useCallback(() => {
    if (paused || !selectedId) return;
    const { graph: g, id } = applyAddConnectedNext(graph, selectedId);
    onChange(g);
    onSelect(id);
  }, [graph, selectedId, onChange, onSelect, paused]);

  // Palette → canvas drag-and-drop. onDragOver must preventDefault to allow the
  // drop; onDrop projects the pointer to flow coords and adds the node there.
  const onDragOver = useCallback(
    (e: DragEvent<HTMLDivElement>) => {
      if (paused) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
    },
    [paused],
  );

  const onDrop = useCallback(
    (e: DragEvent<HTMLDivElement>) => {
      if (paused) return;
      e.preventDefault();
      const kind = asStepKind(e.dataTransfer.getData(NODE_KIND_MIME));
      if (!kind) return;
      const position = rf.screenToFlowPosition({ x: e.clientX, y: e.clientY });
      const { graph: g, id } = applyAddNodeAt(graph, kind, position);
      onChange(g);
      onSelect(id);
    },
    [graph, onChange, onSelect, paused, rf],
  );

  const relayout = useCallback(() => {
    onChange({ ...graph, nodes: autoLayout(graph.nodes, graph.edges) });
  }, [graph, onChange]);

  // Find-step: filter node ids and select+center a match.
  const [query, setQuery] = useState('');
  const q = query.trim().toLowerCase();
  const matches = useMemo(
    () => (q ? graph.nodes.filter((n) => n.id.toLowerCase().includes(q)) : []),
    [q, graph.nodes],
  );
  const locate = useCallback(
    (id: string) => {
      onSelect(id);
      rf.fitView({ nodes: [{ id }], duration: 300, maxZoom: 1.2 });
      setQuery('');
    },
    [onSelect, rf],
  );
  const onFindKeyDown = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter' && matches.length > 0) {
        e.preventDefault();
        locate(matches[0].id);
      } else if (e.key === 'Escape') {
        setQuery('');
      }
    },
    [matches, locate],
  );

  return (
    <div
      className={[
        'relative h-full min-h-[16rem] w-full overflow-hidden rounded-xl border border-border shadow-card',
        workflowEditorUi === 'next' ? 'wfx-canvas' : '',
      ]
        .join(' ')
        .trim()}
    >
      {/* "graph paused" chip — top-center, shown while YAML is unparseable */}
      {paused && (
        <div
          role="status"
          className="pointer-events-none absolute left-1/2 top-3 z-20 -translate-x-1/2 rounded-full border border-warn/30 bg-warn-bg px-3 py-1 text-note font-medium text-warn shadow-card"
        >
          YAML not parseable — graph paused
        </div>
      )}

      {/* toolbar — Re-layout / + next / find. The "Add" palette is now the
          graphical NodePalette dock (bottom-left). */}
      <div className="absolute left-3 top-3 z-10 flex flex-wrap items-center gap-1.5 rounded-lg border border-border bg-panel/95 px-2 py-1.5 shadow-card">
        <button
          type="button"
          disabled={paused}
          onClick={relayout}
          className="rounded-md border border-border bg-panel px-2 py-1 text-note font-medium text-ink-dim hover:bg-surface-hover disabled:cursor-not-allowed disabled:opacity-50"
        >
          Re-layout
        </button>
        <button
          type="button"
          disabled={paused || !selectedId}
          onClick={addConnectedNext}
          title={selectedId ? `Add a step connected from ${selectedId}` : 'Select a node first'}
          className="rounded-md border border-info/30 bg-info-bg px-2 py-1 text-note font-medium text-info hover:bg-info-bg disabled:cursor-not-allowed disabled:opacity-50"
        >
          ⊕ next
        </button>
        <span className="mx-1 h-4 w-px bg-border" aria-hidden />
        <div className="relative">
          <input
            type="search"
            value={query}
            disabled={paused}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onFindKeyDown}
            placeholder="Find step…"
            aria-label="Find step by id"
            className="w-28 rounded-md border border-border bg-panel px-2 py-1 text-note text-ink placeholder:text-ink-mute focus:outline-none focus:ring-1 focus:ring-brand-100 disabled:cursor-not-allowed disabled:opacity-50"
          />
          {matches.length > 0 && (
            <ul className="absolute left-0 top-full z-20 mt-1 max-h-40 w-40 overflow-auto rounded-md border border-border bg-panel py-1 shadow-card">
              {matches.slice(0, 8).map((n) => (
                <li key={n.id}>
                  <button
                    type="button"
                    onClick={() => locate(n.id)}
                    className="block w-full truncate px-2 py-1 text-left text-note text-ink hover:bg-surface-hover"
                  >
                    {n.id}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      {workflowEditorUi === 'next'
        ? paletteContainer &&
          createPortal(
            <NodePalette
              onAdd={addNode}
              onDragStartKind={() => {}}
              disabled={paused}
              workflowEditorUi={workflowEditorUi}
              variant="rail"
            />,
            paletteContainer,
          )
        : (
          <NodePalette
            onAdd={addNode}
            onDragStartKind={() => {}}
            disabled={paused}
            workflowEditorUi={workflowEditorUi}
          />
        )}

      <div
        className={paused ? 'h-full w-full opacity-60 pointer-events-none' : 'h-full w-full'}
        onDragOver={onDragOver}
        onDrop={onDrop}
      >
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
        // `next` leaves this transparent so the `.wfx-canvas` radial wash on the
        // outer container (set above) shows through behind the grid pattern.
        style={{ background: workflowEditorUi === 'next' ? 'transparent' : colors.bg }}
      >
        {workflowEditorUi === 'next' ? (
          <Background
            variant={BackgroundVariant.Lines}
            gap={28}
            lineWidth={1}
            color={colors.alpha('inkMute', 0.12)}
          />
        ) : (
          <Background variant={BackgroundVariant.Dots} gap={16} size={1} color={colors.alpha('inkMute', 0.25)} />
        )}
        <MiniMap
          pannable
          zoomable
          className="!border-border !bg-panel"
          maskColor={colors.alpha('ink', 0.08)}
          nodeColor={colors.inkMute}
          nodeStrokeColor={colors.border}
        />
        <Controls className="!border-border !bg-panel !shadow-card" showInteractive={false} />
      </ReactFlow>
      </div>
    </div>
  );
}
