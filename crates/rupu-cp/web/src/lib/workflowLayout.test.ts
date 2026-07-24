import { it, expect, describe } from 'vitest';
import {
  autoLayout,
  reconcileGraph,
  reconcileFromYaml,
  editorNodeSize,
  NODE_W,
  NODE_H,
} from './workflowLayout';
import type { GraphNode, GraphEdge, WorkflowGraph, StepNodeData } from './workflowGraph';

function node(id: string): GraphNode {
  return { id, data: { id, kind: 'step' }, position: { x: 0, y: 0 } };
}

function edge(source: string, target: string): GraphEdge {
  return { id: `${source}->${target}`, source, target };
}

function emptyMeta() {
  return { name: 'wf', rest: {} as Record<string, unknown> };
}

function nodeAt(id: string, x: number, y: number, data?: Partial<StepNodeData>): GraphNode {
  return { id, data: { id, kind: 'step', ...data }, position: { x, y } };
}

it('exports sensible node-size constants', () => {
  expect(NODE_W).toBeGreaterThan(0);
  expect(NODE_H).toBeGreaterThan(0);
});

it('lays out a linear chain A→B→C left-to-right (strictly increasing x)', () => {
  const nodes = [node('a'), node('b'), node('c')];
  const edges = [edge('a', 'b'), edge('b', 'c')];
  const out = autoLayout(nodes, edges);
  const by = new Map(out.map((n) => [n.id, n]));
  expect(by.get('b')!.position.x).toBeGreaterThan(by.get('a')!.position.x);
  expect(by.get('c')!.position.x).toBeGreaterThan(by.get('b')!.position.x);
});

it('lays out a diamond so D is right of B and C, both right of A (LR)', () => {
  const nodes = [node('a'), node('b'), node('c'), node('d')];
  const edges = [edge('a', 'b'), edge('a', 'c'), edge('b', 'd'), edge('c', 'd')];
  const out = autoLayout(nodes, edges);
  const by = new Map(out.map((n) => [n.id, n]));
  const a = by.get('a')!.position.x;
  const b = by.get('b')!.position.x;
  const c = by.get('c')!.position.x;
  const d = by.get('d')!.position.x;
  expect(b).toBeGreaterThan(a);
  expect(c).toBeGreaterThan(a);
  expect(d).toBeGreaterThan(b);
  expect(d).toBeGreaterThan(c);
});

it('editorNodeSize reserves a taller box for a parallel node per sub-step', () => {
  const one: StepNodeData = {
    id: 'p',
    kind: 'parallel',
    parallel: [{ id: 's1', agent: 'a', prompt: 'p' }],
  };
  const three: StepNodeData = {
    id: 'p',
    kind: 'parallel',
    parallel: [
      { id: 's1', agent: 'a', prompt: 'p' },
      { id: 's2', agent: 'a', prompt: 'p' },
      { id: 's3', agent: 'a', prompt: 'p' },
    ],
  };
  expect(editorNodeSize(three).height).toBeGreaterThan(editorNodeSize(one).height);
  // A plain step is the base box; a panel with a gate is taller than without.
  const step: StepNodeData = { id: 's', kind: 'step' };
  expect(editorNodeSize(step)).toEqual({ width: NODE_W, height: NODE_H });
  const panelNoGate: StepNodeData = { id: 'g', kind: 'panel', panel: { panelists: [], subject: '' } };
  const panelGate: StepNodeData = {
    id: 'g',
    kind: 'panel',
    panel: { panelists: [], subject: '', gate: { until_no_findings_at_severity_or_above: 'high' } },
  };
  expect(editorNodeSize(panelGate).height).toBeGreaterThan(editorNodeSize(panelNoGate).height);
  // panel scales with panelist count (F2): each panelist wraps to its own
  // port-pill row in the fixed-width safe rect, so a 3-panelist node must
  // reserve more height than a 1-panelist node, or the extra panelists get
  // silently clipped by `.wfx-safe`'s `overflow: hidden`.
  const panelOne: StepNodeData = {
    id: 'g',
    kind: 'panel',
    panel: { panelists: ['security-reviewer'], subject: '' },
  };
  const panelThree: StepNodeData = {
    id: 'g',
    kind: 'panel',
    panel: {
      panelists: ['security-reviewer', 'performance-reviewer', 'maintainability-reviewer'],
      subject: '',
    },
  };
  expect(editorNodeSize(panelThree).height).toBeGreaterThan(editorNodeSize(panelOne).height);
  // no panelists reserves the same as one (a `Math.max(rows, 1)` floor, same
  // discipline as the `parallel` case above).
  expect(editorNodeSize(panelNoGate).height).toBe(editorNodeSize(panelOne).height);
});

it('returns [] for an empty node list without throwing', () => {
  expect(autoLayout([], [])).toEqual([]);
});

it('does not mutate its inputs', () => {
  const nodes = [node('a'), node('b')];
  const edges = [edge('a', 'b')];
  const out = autoLayout(nodes, edges);
  // original positions untouched
  expect(nodes[0].position).toEqual({ x: 0, y: 0 });
  expect(nodes[1].position).toEqual({ x: 0, y: 0 });
  // returned nodes are different objects
  expect(out[0]).not.toBe(nodes[0]);
  expect(out[1]).not.toBe(nodes[1]);
});

// ── reconcileGraph ────────────────────────────────────────────────────────────

it('reconcileGraph keeps a surviving node position while updating its data', () => {
  const prev: WorkflowGraph = {
    meta: emptyMeta(),
    edges: [],
    nodes: [nodeAt('a', 123, 456, { agent: 'old' })],
  };
  // `next` comes from yamlToGraph: placeholder {0,0} positions, fresh data.
  const next: WorkflowGraph = {
    meta: { name: 'wf2', rest: {} },
    edges: [],
    nodes: [nodeAt('a', 0, 0, { agent: 'new', prompt: 'hi' })],
  };
  const out = reconcileGraph(prev, next);
  const a = out.nodes.find((n) => n.id === 'a')!;
  expect(a.position).toEqual({ x: 123, y: 456 }); // survivor keeps position
  expect(a.data.agent).toBe('new'); // data updated from next
  expect(a.data.prompt).toBe('hi');
});

it('reconcileGraph gives a new node a non-{0,0} dagre position without moving survivors', () => {
  const prev: WorkflowGraph = {
    meta: emptyMeta(),
    edges: [],
    nodes: [nodeAt('a', 50, 60)],
  };
  const next: WorkflowGraph = {
    meta: emptyMeta(),
    edges: [edge('a', 'b')],
    nodes: [nodeAt('a', 0, 0), nodeAt('b', 0, 0)],
  };
  const out = reconcileGraph(prev, next);
  const a = out.nodes.find((n) => n.id === 'a')!;
  const b = out.nodes.find((n) => n.id === 'b')!;
  expect(a.position).toEqual({ x: 50, y: 60 }); // survivor unmoved
  expect(b.position).not.toEqual({ x: 0, y: 0 }); // new node got laid out
});

it('reconcileGraph drops removed ids and takes edges/meta from next', () => {
  const prev: WorkflowGraph = {
    meta: { name: 'old', rest: { x: 1 } },
    edges: [edge('a', 'b')],
    nodes: [nodeAt('a', 1, 2), nodeAt('b', 3, 4)],
  };
  const next: WorkflowGraph = {
    meta: { name: 'new', rest: { y: 2 } },
    edges: [edge('a', 'c')],
    nodes: [nodeAt('a', 0, 0), nodeAt('c', 0, 0)],
  };
  const out = reconcileGraph(prev, next);
  expect(out.nodes.map((n) => n.id).sort()).toEqual(['a', 'c']); // b dropped
  expect(out.edges).toEqual([edge('a', 'c')]); // edges from next
  expect(out.meta).toEqual({ name: 'new', rest: { y: 2 } }); // meta from next
});

// ── reconcileFromYaml ─────────────────────────────────────────────────────────

it('reconcileFromYaml parses valid YAML and reconciles (not paused)', () => {
  const prev: WorkflowGraph = {
    meta: emptyMeta(),
    edges: [],
    nodes: [nodeAt('a', 77, 88, { agent: 'old' })],
  };
  const text = 'name: wf\nsteps:\n  - id: a\n    agent: new\n    prompt: hi\n';
  const res = reconcileFromYaml(prev, text);
  expect(res.paused).toBe(false);
  expect(res.graph).toBeDefined();
  const a = res.graph!.nodes.find((n) => n.id === 'a')!;
  expect(a.position).toEqual({ x: 77, y: 88 }); // survivor position kept
  expect(a.data.agent).toBe('new');
});

it('reconcileFromYaml pauses on a syntax error, returning no graph', () => {
  const prev: WorkflowGraph = { meta: emptyMeta(), edges: [], nodes: [nodeAt('a', 1, 2)] };
  const res = reconcileFromYaml(prev, 'name: [unclosed\n  bad: : :');
  expect(res.paused).toBe(true);
  expect(res.graph).toBeUndefined();
});

it('reconcileFromYaml pauses on a non-object document (scalar / list)', () => {
  const prev: WorkflowGraph = { meta: emptyMeta(), edges: [], nodes: [] };
  expect(reconcileFromYaml(prev, '42').paused).toBe(true);
  expect(reconcileFromYaml(prev, '- a\n- b').paused).toBe(true);
  expect(reconcileFromYaml(prev, '').paused).toBe(true); // empty → null
});

describe('editorNodeSize — per-kind shape boxes', () => {
  it('a branch reserves a wider box for its diamond (width over height — a diamond only uses a fraction of its box)', () => {
    expect(editorNodeSize({ id: 'b', kind: 'branch', condition: 'x' })).toEqual({
      width: 280,
      height: 200,
    });
  });

  it('action and approval_gate reserve extra width for their slanted sides', () => {
    expect(editorNodeSize({ id: 'a', kind: 'action', action: 'scm.prs.create' })).toEqual({
      width: 214,
      height: 80,
    });
    expect(editorNodeSize({ id: 'g', kind: 'approval_gate' })).toEqual({ width: 214, height: 80 });
  });

  it('for_each reserves extra width for its hexagon points, keeping its height', () => {
    expect(editorNodeSize({ id: 'f', kind: 'for_each', for_each: 'items' })).toEqual({
      width: 214,
      height: 100,
    });
  });

  it('a plain step is unchanged', () => {
    expect(editorNodeSize({ id: 's', kind: 'step', agent: 'a' })).toEqual({ width: 210, height: 80 });
  });
});
