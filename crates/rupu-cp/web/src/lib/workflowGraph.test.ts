import { describe, it, expect } from 'vitest';
import {
  yamlToGraph,
  graphToWorkflowObject,
  extractStepRefs,
  topoSort,
  canConnect,
  validateGraph,
  type GraphNode,
  type GraphEdge,
  type StepNodeData,
} from './workflowGraph';

// ── Test helpers ──────────────────────────────────────────────────────────

/** Recursively sort object keys so deep-equal ignores key ordering. Arrays
 *  keep their order (order is meaningful for steps). */
function sortKeys(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(sortKeys);
  if (v && typeof v === 'object') {
    const o = v as Record<string, unknown>;
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(o).sort()) out[k] = sortKeys(o[k]);
    return out;
  }
  return v;
}

function expectRoundTrip(input: Record<string, unknown>): void {
  const res = graphToWorkflowObject(yamlToGraph(input));
  expect('obj' in res).toBe(true);
  if (!('obj' in res)) return;
  expect(sortKeys(res.obj)).toEqual(sortKeys(input));
}

function node(id: string, data: Partial<StepNodeData>, pos = { x: 0, y: 0 }): GraphNode {
  return { id, data: { id, kind: 'step', ...data }, position: pos };
}

function edge(source: string, target: string): GraphEdge {
  return { id: `${source}->${target}`, source, target };
}

// ── yamlToGraph ─────────────────────────────────────────────────────────────

describe('yamlToGraph', () => {
  it('maps a 3-step linear workflow to 3 nodes + 2 chain edges', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        { id: 'a', agent: 'x', prompt: 'p1' },
        { id: 'b', agent: 'x', prompt: 'p2' },
        { id: 'c', agent: 'x', prompt: 'p3' },
      ],
    });
    expect(g.nodes.map((n) => n.id)).toEqual(['a', 'b', 'c']);
    expect(g.edges.map((e) => e.id)).toEqual(['a->b', 'b->c']);
    expect(g.nodes[0].position).toEqual({ x: 0, y: 0 });
    expect(g.meta.name).toBe('wf');
  });

  it('adds a data-ref edge when a step references steps.X and does not duplicate a chain edge', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        { id: 'a', agent: 'x', prompt: 'start' },
        { id: 'b', agent: 'x', prompt: 'noref' },
        { id: 'c', agent: 'x', prompt: 'use steps.a.output here' },
      ],
    });
    // chain: a->b, b->c ; data-ref c references a -> a->c (new, not dup)
    expect(g.edges.map((e) => e.id).sort()).toEqual(['a->b', 'a->c', 'b->c']);
  });

  it('does not duplicate when a data-ref equals a chain edge', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        { id: 'a', agent: 'x', prompt: 'start' },
        { id: 'b', agent: 'x', prompt: 'use steps.a.output' },
      ],
    });
    expect(g.edges.map((e) => e.id)).toEqual(['a->b']);
  });

  it('classifies for_each / parallel / panel kinds with fields', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        { id: 'fe', agent: 'x', prompt: 'p', for_each: 'steps.s.items', max_parallel: 4 },
        {
          id: 'par',
          parallel: [
            { id: 'p1', agent: 'a', prompt: 'x' },
            { id: 'p2', agent: 'b', prompt: 'y' },
          ],
        },
        {
          id: 'pan',
          panel: {
            panelists: ['r1', 'r2'],
            subject: 'thing',
            gate: { until_severity: 'high', max_iterations: 2, fixer: 'f' },
          },
        },
      ],
    });
    const [fe, par, pan] = g.nodes;
    expect(fe.data.kind).toBe('for_each');
    expect(fe.data.for_each).toBe('steps.s.items');
    expect(fe.data.max_parallel).toBe(4);
    expect(par.data.kind).toBe('parallel');
    expect(par.data.parallel).toHaveLength(2);
    expect(par.data.parallel?.[0]).toEqual({ id: 'p1', agent: 'a', prompt: 'x' });
    expect(pan.data.kind).toBe('panel');
    expect(pan.data.panel?.panelists).toEqual(['r1', 'r2']);
    expect(pan.data.panel?.gate?.until_severity).toBe('high');
  });

  it('reads approvalRequired from approval.required', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [{ id: 'a', agent: 'x', prompt: 'p', approval: { required: true } }],
    });
    expect(g.nodes[0].data.approvalRequired).toBe(true);
  });

  it('preserves trigger/inputs/autoflow verbatim in meta.rest', () => {
    const input = {
      name: 'wf',
      description: 'd',
      trigger: { on: 'cron', cron: '0 0 * * *' },
      inputs: { repo: { type: 'string' } },
      autoflow: { enabled: true, wake_on: ['github.issue.opened'] },
      steps: [{ id: 'a', agent: 'x', prompt: 'p' }],
    };
    const g = yamlToGraph(input);
    expect(g.meta.name).toBe('wf');
    expect(g.meta.description).toBe('d');
    expect(g.meta.rest.trigger).toEqual(input.trigger);
    expect(g.meta.rest.inputs).toEqual(input.inputs);
    expect(g.meta.rest.autoflow).toEqual(input.autoflow);
    expect(g.meta.rest.name).toBeUndefined();
    expect(g.meta.rest.steps).toBeUndefined();
  });

  it('still yields a node for an unrecognized step shape', () => {
    const g = yamlToGraph({ name: 'wf', steps: [{ foo: 'bar' }] });
    expect(g.nodes).toHaveLength(1);
    expect(g.nodes[0].id).toBe('step-0');
    expect(g.nodes[0].data.kind).toBe('step');
  });

  it('falls back to empty name', () => {
    const g = yamlToGraph({ steps: [] });
    expect(g.meta.name).toBe('');
    expect(g.nodes).toEqual([]);
  });
});

// ── extractStepRefs ─────────────────────────────────────────────────────────

describe('extractStepRefs', () => {
  it('finds unique refs across prompt/when/for_each', () => {
    const data: StepNodeData = {
      id: 'x',
      kind: 'for_each',
      prompt: 'use steps.a and steps.b and steps.a again',
      when: 'steps.c.done',
      for_each: 'steps.a.items',
    };
    expect(extractStepRefs(data).sort()).toEqual(['a', 'b', 'c']);
  });

  it('scans sub-step prompts and panel subject/prompt', () => {
    const par: StepNodeData = {
      id: 'p',
      kind: 'parallel',
      parallel: [
        { id: 's1', agent: 'a', prompt: 'from steps.q' },
        { id: 's2', agent: 'b', prompt: 'from steps.r' },
      ],
    };
    expect(extractStepRefs(par).sort()).toEqual(['q', 'r']);

    const pan: StepNodeData = {
      id: 'pp',
      kind: 'panel',
      panel: { panelists: ['x'], subject: 'review steps.m', prompt: 'consider steps.n' },
    };
    expect(extractStepRefs(pan).sort()).toEqual(['m', 'n']);
  });

  it('returns empty when there are no refs', () => {
    expect(extractStepRefs({ id: 'a', kind: 'step', prompt: 'hello' })).toEqual([]);
  });
});

// ── topoSort ────────────────────────────────────────────────────────────────

describe('topoSort', () => {
  it('returns identity order for a linear chain', () => {
    const nodes = [node('a', {}), node('b', {}), node('c', {})];
    const edges = [edge('a', 'b'), edge('b', 'c')];
    const res = topoSort(nodes, edges);
    expect('order' in res).toBe(true);
    if ('order' in res) expect(res.order.map((n) => n.id)).toEqual(['a', 'b', 'c']);
  });

  it('orders a diamond A first, D last, B/C by y then x', () => {
    const nodes = [
      node('A', {}, { x: 0, y: 0 }),
      node('B', {}, { x: 0, y: 10 }),
      node('C', {}, { x: 0, y: 20 }),
      node('D', {}, { x: 0, y: 30 }),
    ];
    const edges = [edge('A', 'B'), edge('A', 'C'), edge('B', 'D'), edge('C', 'D')];
    const res = topoSort(nodes, edges);
    expect('order' in res).toBe(true);
    if ('order' in res) {
      const ids = res.order.map((n) => n.id);
      expect(ids[0]).toBe('A');
      expect(ids[3]).toBe('D');
      expect(ids).toEqual(['A', 'B', 'C', 'D']);
    }
  });

  it('breaks in-degree-0 ties by x when y is equal', () => {
    const nodes = [node('A', {}, { x: 0, y: 0 }), node('C', {}, { x: 100, y: 0 })];
    const res = topoSort(nodes, []);
    if ('order' in res) expect(res.order.map((n) => n.id)).toEqual(['A', 'C']);
  });

  it('reports a cycle', () => {
    const nodes = [node('x', {}), node('y', {})];
    const edges = [edge('x', 'y'), edge('y', 'x')];
    const res = topoSort(nodes, edges);
    expect('cycle' in res).toBe(true);
    if ('cycle' in res) expect(res.cycle.length).toBeGreaterThan(0);
  });
});

// ── graphToWorkflowObject + round-trips ─────────────────────────────────────

describe('graphToWorkflowObject', () => {
  it('errors on a cycle', () => {
    const g = {
      nodes: [node('x', {}), node('y', {})],
      edges: [edge('x', 'y'), edge('y', 'x')],
      meta: { name: 'wf', rest: {} },
    };
    const res = graphToWorkflowObject(g);
    expect('error' in res).toBe(true);
    if ('error' in res) expect(res.error).toContain('cycle through');
  });

  it('emits steps last, name first, omits empty fields', () => {
    const g = yamlToGraph({
      name: 'wf',
      trigger: { on: 'manual' },
      steps: [{ id: 'a', agent: 'x', prompt: 'p' }],
    });
    const res = graphToWorkflowObject(g);
    if ('obj' in res) {
      expect(Object.keys(res.obj)).toEqual(['name', 'trigger', 'steps']);
      const steps = res.obj.steps as Record<string, unknown>[];
      expect(steps[0]).toEqual({ id: 'a', agent: 'x', prompt: 'p' });
    }
  });

  it('parallel emits no top-level agent/prompt', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        {
          id: 'par',
          parallel: [{ id: 'p1', agent: 'a', prompt: 'x' }],
          max_parallel: 2,
        },
      ],
    });
    const res = graphToWorkflowObject(g);
    if ('obj' in res) {
      const steps = res.obj.steps as Record<string, unknown>[];
      expect(steps[0].agent).toBeUndefined();
      expect(steps[0].prompt).toBeUndefined();
      expect(steps[0]).toEqual({
        id: 'par',
        parallel: [{ id: 'p1', agent: 'a', prompt: 'x' }],
        max_parallel: 2,
      });
    }
  });

  it('round-trips a 3-step linear workflow', () => {
    expectRoundTrip({
      name: 'wf',
      steps: [
        { id: 'a', agent: 'x', prompt: 'p1' },
        { id: 'b', agent: 'x', prompt: 'p2' },
        { id: 'c', agent: 'x', prompt: 'p3' },
      ],
    });
  });

  it('round-trips a diamond (data-ref) workflow', () => {
    expectRoundTrip({
      name: 'wf',
      steps: [
        { id: 'A', agent: 'x', prompt: 'start' },
        { id: 'B', agent: 'x', prompt: 'use steps.A' },
        { id: 'C', agent: 'x', prompt: 'also steps.A' },
        { id: 'D', agent: 'x', prompt: 'merge steps.B and steps.C' },
      ],
    });
  });

  it('round-trips a for_each workflow', () => {
    expectRoundTrip({
      name: 'wf',
      steps: [
        { id: 'scan', agent: 'a', prompt: 'find' },
        { id: 'fix', agent: 'b', prompt: 'fix steps.scan', for_each: 'steps.scan.items', max_parallel: 3 },
      ],
    });
  });

  it('round-trips a parallel workflow', () => {
    expectRoundTrip({
      name: 'wf',
      steps: [
        {
          id: 'fan',
          parallel: [
            { id: 'p1', agent: 'a', prompt: 'x' },
            { id: 'p2', agent: 'b', prompt: 'y' },
          ],
          max_parallel: 2,
        },
      ],
    });
  });

  it('round-trips a panel-with-gate workflow', () => {
    expectRoundTrip({
      name: 'wf',
      steps: [
        {
          id: 'review',
          panel: {
            panelists: ['r1', 'r2'],
            subject: 'the code',
            prompt: 'review it',
            max_parallel: 2,
            gate: { until_severity: 'high', max_iterations: 3, fixer: 'f' },
          },
        },
      ],
    });
  });

  it('round-trips trigger + inputs + autoflow untouched', () => {
    const input = {
      name: 'wf',
      description: 'a workflow',
      trigger: { on: 'cron', cron: '0 0 * * *' },
      inputs: { repo: { type: 'string', required: true } },
      autoflow: { enabled: true, wake_on: ['github.issue.opened'] },
      steps: [
        { id: 'a', agent: 'x', prompt: 'p1' },
        { id: 'b', agent: 'x', prompt: 'use steps.a' },
      ],
    };
    expectRoundTrip(input);
    // Belt-and-suspenders: assert the top-level autoflow blocks specifically.
    const res = graphToWorkflowObject(yamlToGraph(input));
    if ('obj' in res) {
      expect(res.obj.trigger).toEqual(input.trigger);
      expect(res.obj.inputs).toEqual(input.inputs);
      expect(res.obj.autoflow).toEqual(input.autoflow);
    }
  });
});

// ── canConnect ──────────────────────────────────────────────────────────────

describe('canConnect', () => {
  it('rejects a self-loop', () => {
    const res = canConnect('a', 'a', { edges: [] });
    expect(res).toEqual({ ok: false, reason: "A step can't depend on itself." });
  });

  it('rejects a duplicate edge', () => {
    const res = canConnect('a', 'b', { edges: [edge('a', 'b')] });
    expect(res).toEqual({ ok: false, reason: 'These steps are already connected.' });
  });

  it('rejects a back-edge that closes a cycle', () => {
    const res = canConnect('b', 'a', { edges: [edge('a', 'b')] });
    expect(res.ok).toBe(false);
    if (!res.ok) expect(res.reason).toContain('cycle');
  });

  it('rejects a transitive cycle', () => {
    const res = canConnect('c', 'a', { edges: [edge('a', 'b'), edge('b', 'c')] });
    expect(res.ok).toBe(false);
  });

  it('allows valid fan-in', () => {
    const edges = [edge('a', 'c')];
    expect(canConnect('b', 'c', { edges })).toEqual({ ok: true });
  });
});

// ── validateGraph ───────────────────────────────────────────────────────────

describe('validateGraph', () => {
  it('returns an empty map for a clean graph', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        { id: 'a', agent: 'x', prompt: 'p1' },
        { id: 'b', agent: 'x', prompt: 'p2' },
      ],
    });
    expect(validateGraph(g)).toEqual({});
  });

  it('flags missing agent / prompt on a step', () => {
    const g = yamlToGraph({ name: 'wf', steps: [{ id: 'a' }] });
    expect(validateGraph(g).a).toEqual(expect.arrayContaining(['needs an agent', 'needs a prompt']));
  });

  it('flags a parallel node with no sub-steps', () => {
    const g: ReturnType<typeof yamlToGraph> = {
      nodes: [node('par', { kind: 'parallel', parallel: [] })],
      edges: [],
      meta: { name: 'wf', rest: {} },
    };
    expect(validateGraph(g).par.length).toBeGreaterThan(0);
  });

  it('flags a parallel sub-step missing agent/prompt', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [{ id: 'par', parallel: [{ id: 's1' }] }],
    });
    expect(validateGraph(g).par.length).toBeGreaterThan(0);
  });

  it('flags a panel with no panelists or subject', () => {
    const g = yamlToGraph({ name: 'wf', steps: [{ id: 'pan', panel: { panelists: [], subject: '' } }] });
    expect(validateGraph(g).pan.length).toBeGreaterThanOrEqual(2);
  });

  it('flags duplicate node ids on each duplicate', () => {
    const g: ReturnType<typeof yamlToGraph> = {
      nodes: [node('a', { agent: 'x', prompt: 'p' }), node('a', { agent: 'x', prompt: 'q' })],
      edges: [],
      meta: { name: 'wf', rest: {} },
    };
    const v = validateGraph(g);
    expect(v.a.some((m) => m.includes('duplicate'))).toBe(true);
  });

  it('flags a forward reference to a later step', () => {
    const g: ReturnType<typeof yamlToGraph> = {
      // A references steps.B, but with no edges A sorts before B (by id) — B runs later.
      nodes: [
        node('A', { agent: 'x', prompt: 'use steps.B' }),
        node('B', { agent: 'x', prompt: 'hi' }),
      ],
      edges: [],
      meta: { name: 'wf', rest: {} },
    };
    const v = validateGraph(g);
    expect(v.A.some((m) => m.includes('steps.B') && m.includes('later'))).toBe(true);
  });
});
