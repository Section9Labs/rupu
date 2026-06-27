import { it, expect } from 'vitest';
import { autoLayout, reconcileGraph, reconcileFromYaml, NODE_W, NODE_H } from './workflowLayout';
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

it('lays out a linear chain A→B→C with strictly increasing y', () => {
  const nodes = [node('a'), node('b'), node('c')];
  const edges = [edge('a', 'b'), edge('b', 'c')];
  const out = autoLayout(nodes, edges);
  const by = new Map(out.map((n) => [n.id, n]));
  expect(by.get('b')!.position.y).toBeGreaterThan(by.get('a')!.position.y);
  expect(by.get('c')!.position.y).toBeGreaterThan(by.get('b')!.position.y);
});

it('lays out a diamond so D is below B and C, both below A', () => {
  const nodes = [node('a'), node('b'), node('c'), node('d')];
  const edges = [edge('a', 'b'), edge('a', 'c'), edge('b', 'd'), edge('c', 'd')];
  const out = autoLayout(nodes, edges);
  const by = new Map(out.map((n) => [n.id, n]));
  const a = by.get('a')!.position.y;
  const b = by.get('b')!.position.y;
  const c = by.get('c')!.position.y;
  const d = by.get('d')!.position.y;
  expect(b).toBeGreaterThan(a);
  expect(c).toBeGreaterThan(a);
  expect(d).toBeGreaterThan(b);
  expect(d).toBeGreaterThan(c);
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
