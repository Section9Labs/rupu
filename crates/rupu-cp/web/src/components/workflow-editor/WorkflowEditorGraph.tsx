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

import { useCallback, useEffect, useMemo, useState, type DragEvent, type KeyboardEvent } from 'react';
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
  deriveEdges,
  withDerivedEdges,
  type GraphEdge,
  type GraphNode,
  type StepKind,
  type StepNodeData,
  type WorkflowGraph,
} from '../../lib/workflowGraph';
import { autoLayout, editorNodeSize, NODE_W } from '../../lib/workflowLayout';
import { useThemeColors, type ThemeColors } from '../../lib/useThemeColors';
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';
import type { ToolSpec } from '../../lib/api';
import EditableStepNode, { type NodeData } from './nodes/EditableStepNode';
import { KIND_ACCENT } from './kindVisuals';
import NodePalette, { NODE_KIND_MIME, NODE_SEED_MIME } from './NodePalette';

import '@xyflow/react/dist/style.css';

// Memoized once at module scope so React Flow doesn't see a fresh object each
// render (mirrors RunGraph).
const NODE_TYPES: NodeTypes = { editable: EditableStepNode };

// ── Pure mutation helpers (exported for tests) ───────────────────────────────

/** Whether `sourceHandle` names a real branch arm on `source`: the handle id
 *  is `"then"`/`"else"` AND `source` really is a `branch`-kind node. Shared by
 *  `applyConnect` and the live-drag `isValidConnection` check so both agree
 *  on what counts as an arm connection. */
function armForConnection(
  nodes: GraphNode[],
  source: string,
  sourceHandle: string | null | undefined,
): 'then' | 'else' | undefined {
  if (sourceHandle !== 'then' && sourceHandle !== 'else') return undefined;
  const sourceNode = nodes.find((n) => n.id === source);
  return sourceNode?.data.kind === 'branch' ? sourceHandle : undefined;
}

/** Validate + apply a drawn connection. Valid → onChange with the new edge
 *  appended; invalid → onInvalid(reason) and NO onChange. Missing endpoints are
 *  ignored.
 *
 *  Branch arms: `EditableStepNode` gives every `branch`-kind node TWO source
 *  handles, `id="then"` / `id="else"` (both the `next` and classic render
 *  paths). When the drawn connection originates from one of those handles AND
 *  the source node really is a `branch` (`armForConnection`), `target` is
 *  appended to the source node's `thenTargets`/`elseTargets` — those arrays,
 *  not a stored edges array, are what `graphToWorkflowObject` reads to emit
 *  `branch.then`/`branch.else`, AND what `deriveEdges` reads to draw the
 *  branch-arm edge on the very next render, so the append alone is enough for
 *  the connection to round-trip and to draw. Any other `sourceHandle`
 *  (including `null`, the every-other-kind default single source handle)
 *  falls back to today's plain-edge behavior.
 *
 *  `canConnect` is called with the resolved `arm` (not `undefined`) so its
 *  duplicate check compares like with like: under the derived-edges model
 *  EVERY array-adjacent node pair already carries an (unlabeled) chain edge,
 *  so a branch node drawing an arm to its own array-adjacent successor must
 *  not be rejected as "already connected" against that unrelated chain edge. */
export function applyConnect(
  graph: WorkflowGraph,
  conn: { source: string | null; target: string | null; sourceHandle?: string | null },
  onChange: (g: WorkflowGraph) => void,
  onInvalid: (reason: string) => void,
): void {
  const { source, target, sourceHandle } = conn;
  if (!source || !target) return;
  const arm = armForConnection(graph.nodes, source, sourceHandle);
  const res = canConnect(source, target, { edges: graph.edges }, arm);
  if (!res.ok) {
    onInvalid(res.reason);
    return;
  }

  if (arm) {
    const nodes = graph.nodes.map((n) => {
      if (n.id !== source) return n;
      const key = arm === 'then' ? 'thenTargets' : 'elseTargets';
      const list = (n.data[key] as string[] | undefined) ?? [];
      if (list.includes(target)) return n;
      return { ...n, data: { ...n.data, [key]: [...list, target] } };
    });
    onChange(withDerivedEdges(graph.meta, nodes));
    return;
  }
  // plain connect = reorder: move target to immediately after source (linear
  // flow is node-array order; a free-floating stored edge doesn't round-trip).
  const src = graph.nodes.findIndex((n) => n.id === source);
  const tgt = graph.nodes.findIndex((n) => n.id === target);
  if (src < 0 || tgt < 0) return;
  const nodes = [...graph.nodes];
  const [moved] = nodes.splice(tgt, 1);
  nodes.splice(nodes.findIndex((n) => n.id === source) + 1, 0, moved);
  onChange(withDerivedEdges(graph.meta, nodes));
}

/** Remove a node, scrubbing it from any surviving branch node's then/else
 *  target list (branch routing is single-source — the arrays, not a stored
 *  edges array, are what the deleted node's incoming/outgoing edges derive
 *  from). Chain/data-ref edges touching the deleted id simply stop existing
 *  once the id is gone from the node array — nothing else to clean up there. */
export function applyDelete(graph: WorkflowGraph, id: string): WorkflowGraph {
  const nodes = graph.nodes
    .filter((n) => n.id !== id)
    .map((n) => {
      const then = n.data.thenTargets?.filter((t) => t !== id);
      const els = n.data.elseTargets?.filter((t) => t !== id);
      if ((then?.length ?? 0) === (n.data.thenTargets?.length ?? 0) && (els?.length ?? 0) === (n.data.elseTargets?.length ?? 0)) return n;
      return { ...n, data: { ...n.data, ...(then ? { thenTargets: then } : {}), ...(els ? { elseTargets: els } : {}) } };
    });
  return withDerivedEdges(graph.meta, nodes);
}

/** Remove edges by id. A removed BRANCH-arm edge (`branch: 'then' | 'else'`)
 *  drops `edge.target` from the source branch node's corresponding arm list
 *  (`thenTargets` / `elseTargets`) — those arrays are the single source of
 *  truth the edge derives from, so without this the arm would re-derive the
 *  "deleted" edge on the very next render. A removed PLAIN chain/data-ref edge
 *  is a no-op: linear order is expressed by node-array position (or a
 *  `steps.X` template reference), neither of which "delete an edge" can
 *  target individually under the derived-edges model — see the file-header
 *  comment. */
export function applyRemoveEdges(graph: WorkflowGraph, ids: ReadonlySet<string>): WorkflowGraph {
  const removed = deriveEdges(graph.nodes).filter((e) => ids.has(e.id) && e.branch);
  if (removed.length === 0) return graph;
  const nodes = graph.nodes.map((n) => {
    let data = n.data;
    for (const e of removed) {
      if (e.source !== n.id) continue;
      if (e.branch === 'then') data = { ...data, thenTargets: (data.thenTargets ?? []).filter((t) => t !== e.target) };
      else if (e.branch === 'else') data = { ...data, elseTargets: (data.elseTargets ?? []).filter((t) => t !== e.target) };
    }
    return data === n.data ? n : { ...n, data };
  });
  return withDerivedEdges(graph.meta, nodes);
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
  if (kind === 'approval_gate') {
    data.approvalRequired = true;
    data.approvalOnReject = [];
  }
  if (kind === 'action') {
    data.action = '';
    data.with = {};
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
  seed?: Partial<StepNodeData>,
): { graph: WorkflowGraph; id: string } {
  const id = nextNodeId(graph.nodes);
  // `seed` pre-fills kind-specific fields (e.g. a connector palette card seeds
  // `action` with a tool name); id/kind always win over any seeded value.
  const data: StepNodeData = { ...newNodeData(id, kind), ...seed, id, kind };
  const node: GraphNode = { id, data, position: { x: position.x, y: position.y } };
  const nodes = [...graph.nodes, node];
  return { graph: withDerivedEdges(graph.meta, nodes), id };
}

/** Append a new node of the given kind near canvas center; returns the updated
 *  graph and the new id (so the caller can select it to open the form). The
 *  click-to-add (no-drag) path. `seed` pre-fills kind-specific fields. */
export function applyAddNode(
  graph: WorkflowGraph,
  kind: StepKind,
  seed?: Partial<StepNodeData>,
): { graph: WorkflowGraph; id: string } {
  const count = graph.nodes.length;
  return applyAddNodeAt(graph, kind, { x: 60, y: 60 + 100 * count }, seed);
}

/** Add a new node (default kind `step`), SPLICED into the node array
 *  immediately after `sourceId` so the chain edge `sourceId -> newId` derives
 *  from consecutive order, positioned to the right of the source so the
 *  linear chain reads L→R. Powers the inline "⊕ next" fast path. Returns the
 *  updated graph and the new id. If `sourceId` is unknown the node is still
 *  appended at the end (placed at a sane default position) — under the
 *  derived-edges model that still chains it from whatever was previously
 *  last in the array, since EVERY consecutive array pair is a chain edge. */
export function applyAddConnectedNext(
  graph: WorkflowGraph,
  sourceId: string,
  kind: StepKind = 'step',
): { graph: WorkflowGraph; id: string } {
  const sourceIdx = graph.nodes.findIndex((n) => n.id === sourceId);
  const source = sourceIdx >= 0 ? graph.nodes[sourceIdx] : undefined;
  const id = nextNodeId(graph.nodes);
  const gap = 64;
  const base = source ? source.position : { x: 60, y: 60 };
  const node: GraphNode = {
    id,
    data: newNodeData(id, kind),
    position: { x: base.x + (source ? editorNodeSize(source.data).width : NODE_W) + gap, y: base.y },
  };
  const nodes = [...graph.nodes];
  nodes.splice(sourceIdx >= 0 ? sourceIdx + 1 : nodes.length, 0, node);
  return { graph: withDerivedEdges(graph.meta, nodes), id };
}

/** Insert a new node onto an existing edge A→B, keeping the derived edges
 *  honest:
 *   - PLAIN chain edge (A, B consecutive in the node array): splice the new
 *     node in between them, so A→new and new→B derive from the new
 *     consecutive order (A→B stops deriving since A and B are no longer
 *     adjacent).
 *   - BRANCH-arm edge (A is a `branch` node, B is one of its then/else
 *     targets): replace B with the new node in A's then/else array (so
 *     A→new becomes the derived branch-arm edge) AND splice the new node
 *     immediately before B in array order (so new→B derives as a plain
 *     chain edge) — the same "insert between the two nodes it splits"
 *     intent, expressed via arm-list + order rather than a stored edge.
 *  Stays a DAG by construction (a linear insert never closes a cycle). If
 *  `edgeId` is unknown, falls back to a plain add at `position`. Returns the
 *  updated graph and the new id. */
export function applyInsertOnEdge(
  graph: WorkflowGraph,
  edgeId: string,
  kind: StepKind,
  position: { x: number; y: number },
): { graph: WorkflowGraph; id: string } {
  const edge = deriveEdges(graph.nodes).find((e) => e.id === edgeId);
  if (!edge) return applyAddNodeAt(graph, kind, position);
  const id = nextNodeId(graph.nodes);
  const newNode: GraphNode = { id, data: newNodeData(id, kind), position: { x: position.x, y: position.y } };

  if (edge.branch) {
    const key = edge.branch === 'then' ? 'thenTargets' : 'elseTargets';
    const nodes = graph.nodes.map((n) => {
      if (n.id !== edge.source) return n;
      const list = (n.data[key] as string[] | undefined) ?? [];
      return { ...n, data: { ...n.data, [key]: list.map((t) => (t === edge.target ? id : t)) } };
    });
    const bIdx = nodes.findIndex((n) => n.id === edge.target);
    nodes.splice(bIdx >= 0 ? bIdx : nodes.length, 0, newNode);
    return { graph: withDerivedEdges(graph.meta, nodes), id };
  }

  const nodes = [...graph.nodes];
  const bIdx = nodes.findIndex((n) => n.id === edge.target);
  nodes.splice(bIdx >= 0 ? bIdx : nodes.length, 0, newNode);
  return { graph: withDerivedEdges(graph.meta, nodes), id };
}

/** Narrow an arbitrary dataTransfer string to a StepKind. Exported for tests. */
export function asStepKind(v: string): StepKind | null {
  return v === 'step' ||
    v === 'for_each' ||
    v === 'parallel' ||
    v === 'panel' ||
    v === 'branch' ||
    v === 'approval_gate' ||
    v === 'action'
    ? v
    : null;
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
  /** MCP tool catalog (from `api.getTools()`), owned by WorkflowEditor and
   *  threaded to NodePalette so the `next` look can offer connector ACTION
   *  cards grouped by prefix. Empty/omitted → no connector cards. */
  tools?: ToolSpec[];
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
  tools,
}: Props) {
  const colors = useThemeColors();

  // Edge selection lives here (not on GraphEdge) — it's transient UI state,
  // not something that round-trips to YAML. ReactFlow is fully controlled, so
  // without this an edge click's `EdgeChange {type:'select'}` has nowhere to
  // land: the edge never renders `selected`, and Backspace (which deletes
  // SELECTED elements) never sees a selected edge to remove.
  const [selectedEdgeIds, setSelectedEdgeIds] = useState<ReadonlySet<string>>(new Set());

  // Prune ids that no longer name an edge (e.g. a YAML reconcile dropped one
  // out from under the selection) so stale ids don't linger in state forever.
  useEffect(() => {
    setSelectedEdgeIds((prev) => {
      if (prev.size === 0) return prev;
      const live = new Set(graph.edges.map((e) => e.id));
      let changed = false;
      const next = new Set<string>();
      for (const id of prev) {
        if (live.has(id)) next.add(id);
        else changed = true;
      }
      return changed ? next : prev;
    });
  }, [graph.edges]);

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

  // Stable string key over (node id, kind) pairs only — NOT `graph.nodes`
  // itself, whose array identity (and therefore the memo below) changes on
  // every drag frame as positions update. Kind assignments change far less
  // often than positions, so this key stays referentially stable across
  // drags and lets the edges memo skip rebuilding all edge objects per frame.
  const kindKey = graph.nodes.map((n) => `${n.id}:${n.data.kind}`).join('|');
  const kindById = useMemo(
    () => new Map(graph.nodes.map((n) => [n.id, n.data.kind])),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- kindKey is the intentional stable proxy for graph.nodes' (id, kind) pairs
    [kindKey],
  );

  const edges = useMemo<Edge[]>(() => {
    // source-node-id → kind, so every plain edge can theme itself off the
    // node it flows FROM (KIND_ACCENT is the same accent EditableStepNode
    // paints on that node's top-bar — the edge just carries it downstream).
    return graph.edges.map((e) => {
      const branchColor = branchEdgeColor(e.branch, colors);
      const next = workflowEditorUi === 'next';
      const isSelected = selectedEdgeIds.has(e.id);
      const sourceKind = kindById.get(e.source);
      const accentKey = sourceKind ? KIND_ACCENT[sourceKind] : undefined;
      // Plain (non-branch) edges get a kind-tinted arrowhead in `next`,
      // matching the stroke below (classic leaves the marker color undefined
      // → xyflow's default).
      const markerColor = branchColor ?? (next ? colors.alpha(accentKey ?? 'inkMute', 0.55) : undefined);
      const edge: Edge = {
        id: e.id,
        source: e.source,
        target: e.target,
        type: 'smoothstep',
        markerEnd: { type: MarkerType.ArrowClosed, color: markerColor },
      };
      // Anchor the edge at the branch node's matching arm handle.
      // `EditableStepNode` gives every `branch`-kind node TWO source handles,
      // `id="then"` (green, top) / `id="else"` (red, below) — both the `next`
      // and classic render paths use those exact ids. Without this, xyflow
      // falls back to the FIRST source handle it finds for every edge from
      // that node (always "then"/green), so a correctly-derived else-arm
      // edge still visually draws from the green dot. `branch` and the
      // handle id are the same two literal strings by construction, so this
      // is a pure derivation, not a new field to keep in sync.
      if (e.branch) edge.sourceHandle = e.branch;
      // Only set `selected` when true, so an unselected edge's emitted shape
      // stays byte-identical to before edge selection existed (classic tests
      // assert exact edge shapes).
      if (isSelected) edge.selected = true;
      // xyflow's built-in dashed flow animation — every `next` edge gets it
      // (branch arm or plain); classic never sets the key at all, so its
      // emission stays byte-identical to before this change.
      if (next) edge.animated = true;
      if (e.label !== undefined) edge.label = e.label;
      if (branchColor !== undefined) {
        // Branch (true/false) arm — colored stroke, a touch bolder for `next`
        // (mirrors the mockup's `.edge.t-true`/`.edge.t-false`), bolder still
        // when selected. An inline `style.stroke` always wins over xyflow's
        // default `.selected` CSS treatment, so `next` earns its selected
        // emphasis here rather than through the stylesheet; classic (no
        // inline strokeWidth here) still falls back to xyflow's default
        // selected-edge CSS.
        edge.style = next ? { stroke: branchColor, strokeWidth: isSelected ? 3.5 : 2.5 } : { stroke: branchColor };
        edge.labelStyle = { fill: branchColor, fontWeight: 600 };
        if (next) {
          // Semantic label chip: ✓ then / ✕ else, filled with a soft tint of
          // the arm's own accent (status.done / status.failed).
          const isThen = e.branch === 'then';
          edge.label = isThen ? '✓ then' : '✕ else';
          edge.labelBgStyle = {
            fill: colors.alpha(isThen ? 'status.done' : 'status.failed', 0.12),
          };
          edge.labelBgPadding = [6, 3];
          edge.labelBgBorderRadius = 6;
        } else {
          edge.labelBgStyle = { fillOpacity: 0 };
        }
      } else if (next) {
        // Plain chain/data-ref edge — themed off the SOURCE node's kind
        // accent (mockup's `.edge`, upgraded from a flat muted stroke so
        // every edge — not just branch arms — reads as meaningful); bolder,
        // full-strength when selected. Classic leaves this edge with
        // xyflow's default styling (including its default selected look).
        edge.style = {
          stroke: colors.alpha(accentKey ?? 'inkMute', isSelected ? 0.9 : 0.55),
          strokeWidth: isSelected ? 3 : 2,
        };
        if (e.label !== undefined) {
          // Non-branch edges that carry a label (none produced by
          // yamlToGraph today, but the renderer stays generic) get a
          // neutral brand-tinted chip instead of the branch-arm colors.
          edge.labelStyle = { fill: colors.inkDim, fontWeight: 600 };
          edge.labelBgStyle = { fill: colors.alpha('brand.500', 0.12) };
          edge.labelBgPadding = [6, 3];
          edge.labelBgBorderRadius = 6;
        }
      }
      return edge;
    });
  }, [graph.edges, kindById, colors, workflowEditorUi, selectedEdgeIds]);

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

  // Select (click) + remove (Backspace/Delete on a selected edge) both arrive
  // as edge changes. In controlled mode xyflow only APPLIES a change once we
  // mirror it back into the edges we hand it next render — select changes go
  // into `selectedEdgeIds` (which the edges memo reads back as `selected`),
  // remove changes go through `applyRemoveEdges` (which also detaches a
  // deleted branch-arm target from the source branch node's arm list).
  const onEdgesChange = useCallback(
    (changes: EdgeChange<Edge>[]) => {
      const selects = changes.filter(
        (c): c is Extract<EdgeChange<Edge>, { type: 'select' }> => c.type === 'select',
      );
      if (selects.length > 0) {
        setSelectedEdgeIds((prev) => {
          let next: Set<string> | undefined;
          for (const c of selects) {
            const has = prev.has(c.id) || (next?.has(c.id) ?? false);
            if (c.selected === has) continue;
            if (!next) next = new Set(prev);
            if (c.selected) next.add(c.id);
            else next.delete(c.id);
          }
          return next ?? prev;
        });
      }

      const removed = new Set(changes.flatMap((c) => (c.type === 'remove' ? [c.id] : [])));
      if (removed.size > 0) {
        onChange(applyRemoveEdges(graph, removed));
        setSelectedEdgeIds((prev) => {
          if (![...removed].some((id) => prev.has(id))) return prev;
          const next = new Set(prev);
          for (const id of removed) next.delete(id);
          return next;
        });
      }
    },
    [graph, onChange],
  );

  const onConnect = useCallback(
    (conn: Connection) => applyConnect(graph, conn, onChange, onInvalidConnection),
    [graph, onChange, onInvalidConnection],
  );

  // Block the visual drop for invalid targets before onConnect even fires.
  // Resolves the same `arm` as `applyConnect` (via `sourceHandle`) so a
  // branch-arm drag preview isn't rejected against an unrelated chain edge to
  // an array-adjacent node — see `armForConnection`'s docstring.
  const isValidConnection = useCallback(
    (c: Edge | Connection) => {
      if (!c.source || !c.target) return false;
      const arm = armForConnection(graph.nodes, c.source, c.sourceHandle);
      return canConnect(c.source, c.target, { edges: graph.edges }, arm).ok;
    },
    [graph.edges, graph.nodes],
  );

  const onNodeClick = useCallback<NodeMouseHandler<Node<NodeData>>>(
    (_evt, node) => onSelect(node.id),
    [onSelect],
  );

  const onPaneClick = useCallback(() => onSelect(null), [onSelect]);

  const rf = useReactFlow();

  // Click-to-add (palette card click / keyboard): drop near center. `seed`
  // carries a connector card's pre-filled tool name (kind 'action').
  const addNode = useCallback(
    (kind: StepKind, seed?: Partial<StepNodeData>) => {
      if (paused) return;
      const { graph: g, id } = applyAddNode(graph, kind, seed);
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
      // A connector card also stashes a JSON seed (e.g. `{ action: '<tool>' }`).
      let seed: Partial<StepNodeData> | undefined;
      const seedRaw = e.dataTransfer.getData(NODE_SEED_MIME);
      if (seedRaw) {
        try {
          const parsed = JSON.parse(seedRaw);
          if (parsed && typeof parsed === 'object') seed = parsed as Partial<StepNodeData>;
        } catch {
          /* malformed seed — fall back to a bare node of `kind` */
        }
      }
      const position = rf.screenToFlowPosition({ x: e.clientX, y: e.clientY });
      const { graph: g, id } = applyAddNodeAt(graph, kind, position, seed);
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
              tools={tools}
            />,
            paletteContainer,
          )
        : (
          <NodePalette
            onAdd={addNode}
            onDragStartKind={() => {}}
            disabled={paused}
            workflowEditorUi={workflowEditorUi}
            tools={tools}
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
