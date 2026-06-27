import { it, expect } from 'vitest';
import { autoLayout, NODE_W, NODE_H } from './workflowLayout';
import type { GraphNode, GraphEdge } from './workflowGraph';

function node(id: string): GraphNode {
  return { id, data: { id, kind: 'step' }, position: { x: 0, y: 0 } };
}

function edge(source: string, target: string): GraphEdge {
  return { id: `${source}->${target}`, source, target };
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
