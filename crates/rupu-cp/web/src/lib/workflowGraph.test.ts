import { describe, it, expect } from 'vitest';
import {
  yamlToGraph,
  graphToWorkflowObject,
  extractStepRefs,
  topoSort,
  canConnect,
  validateGraph,
  hasInlineApproval,
  convertInlineApprovalToGate,
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
            gate: {
              until_no_findings_at_severity_or_above: 'high',
              fix_with: 'f',
              max_iterations: 2,
            },
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
    expect(pan.data.panel?.gate?.until_no_findings_at_severity_or_above).toBe('high');
    expect(pan.data.panel?.gate?.fix_with).toBe('f');
  });

  it('classifies a branch step and parses condition/then/else', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        { id: 'classify', agent: 'x', prompt: 'classify' },
        {
          id: 'route',
          branch: { condition: '{{ steps.classify.output }}', then: ['arm_a'], else: ['arm_b'] },
        },
        { id: 'arm_a', agent: 'x', prompt: 'do a' },
        { id: 'arm_b', agent: 'x', prompt: 'do b' },
      ],
    });
    const route = g.nodes.find((n) => n.id === 'route');
    expect(route?.data.kind).toBe('branch');
    expect(route?.data.condition).toBe('{{ steps.classify.output }}');
    expect(route?.data.thenTargets).toEqual(['arm_a']);
    expect(route?.data.elseTargets).toEqual(['arm_b']);
    expect(route?.data.agent).toBeUndefined();
    expect(route?.data.prompt).toBeUndefined();
  });

  it('emits labeled then/else edges from a branch node without collapsing onto the chain edge', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        { id: 'classify', agent: 'x', prompt: 'classify' },
        { id: 'route', branch: { condition: 'c', then: ['arm_a'], else: ['arm_b'] } },
        { id: 'arm_a', agent: 'x', prompt: 'do a' },
        { id: 'arm_b', agent: 'x', prompt: 'do b' },
      ],
    });
    // Chain edges (unlabeled): classify->route, route->arm_a, arm_a->arm_b.
    // Branch edges (labeled): route->arm_a (then, overlapping the chain edge
    // but NOT collapsed because the label is part of the dedupe key) and
    // route->arm_b (else, a brand-new edge not on the chain).
    const thenEdge = g.edges.find((e) => e.branch === 'then');
    const elseEdge = g.edges.find((e) => e.branch === 'else');
    expect(thenEdge).toMatchObject({ source: 'route', target: 'arm_a', label: 'true', branch: 'then' });
    expect(elseEdge).toMatchObject({ source: 'route', target: 'arm_b', label: 'false', branch: 'else' });
    // The plain chain edge route->arm_a still exists distinctly from the
    // labeled then-edge (different ids, both present).
    const plainRouteToArmA = g.edges.find((e) => e.source === 'route' && e.target === 'arm_a' && e.label === undefined);
    expect(plainRouteToArmA).toBeDefined();
    expect(thenEdge?.id).not.toBe(plainRouteToArmA?.id);
  });

  it('does not emit a branch-arm edge to a dangling target', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [{ id: 'route', branch: { condition: 'c', then: ['ghost'] } }],
    });
    expect(g.edges.some((e) => e.branch === 'then')).toBe(false);
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
    // graphToWorkflowObject now derives edges from nodes (deriveEdges), so a
    // cycle must be genuinely derivable: 'x' declared before 'y' gives a chain
    // edge x->y, and 'x' forward-referencing steps.y gives a data-ref edge
    // y->x — together a real cycle. Hand-supplied raw g.edges (as this test
    // used to do) are no longer consulted at all.
    const g = {
      nodes: [node('x', { prompt: 'steps.y.output' }, { x: 0, y: 0 }), node('y', {}, { x: 0, y: 1 })],
      edges: [],
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

  it('round-trips a panel-with-gate workflow using the REAL gate field names', () => {
    // The gate keys MUST match the orchestrator schema exactly
    // (until_no_findings_at_severity_or_above / fix_with / max_iterations) —
    // Workflow::parse uses deny_unknown_fields, so any other name 400s on save.
    const input = {
      name: 'wf',
      steps: [
        {
          id: 'review',
          panel: {
            panelists: ['r1', 'r2'],
            subject: 'the code',
            prompt: 'review it',
            max_parallel: 2,
            gate: {
              until_no_findings_at_severity_or_above: 'high',
              fix_with: 'finding-fixer',
              max_iterations: 4,
            },
          },
        },
      ],
    };
    expectRoundTrip(input);
    // Belt-and-suspenders: the serialized gate carries ONLY the real keys, so
    // the emitted YAML is parseable by Workflow::parse(deny_unknown_fields).
    const res = graphToWorkflowObject(yamlToGraph(input));
    if ('obj' in res) {
      const step = (res.obj.steps as Record<string, unknown>[])[0];
      const panel = step.panel as Record<string, unknown>;
      expect(panel.gate).toEqual({
        until_no_findings_at_severity_or_above: 'high',
        fix_with: 'finding-fixer',
        max_iterations: 4,
      });
    }
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

  it('round-trips a step-level contract: block (unmodeled key passthrough)', () => {
    const input = {
      name: 'wf',
      steps: [
        {
          id: 'a',
          agent: 'x',
          prompt: 'p',
          contract: { outputs: { items: { type: 'array' } }, description: 'emits items' },
        },
      ],
    };
    // The node captures `contract` into raw_passthrough on load…
    const g = yamlToGraph(input);
    expect(g.nodes[0].data.raw_passthrough?.contract).toEqual(input.steps[0].contract);
    // …and re-emits it untouched on save.
    expectRoundTrip(input);
  });

  it('round-trips a branch step with condition/then/else', () => {
    const input = {
      name: 'wf',
      steps: [
        { id: 'classify', agent: 'x', prompt: 'classify' },
        {
          id: 'route',
          branch: { condition: '{{ steps.classify.output }}', then: ['arm_a'], else: ['arm_b'] },
        },
        { id: 'arm_a', agent: 'x', prompt: 'do a' },
        { id: 'arm_b', agent: 'x', prompt: 'do b' },
      ],
    };
    expectRoundTrip(input);
    const g = yamlToGraph(input);
    const route = g.nodes.find((n) => n.id === 'route');
    expect(route?.data.kind).toBe('branch');
    expect(route?.data.condition).toBe('{{ steps.classify.output }}');
    expect(route?.data.thenTargets).toEqual(['arm_a']);
    expect(route?.data.elseTargets).toEqual(['arm_b']);
  });

  it('round-trips a branch step with only a then arm (else omitted, not emitted as empty array)', () => {
    const input = {
      name: 'wf',
      steps: [
        { id: 'route', branch: { condition: 'c', then: ['a'] } },
        { id: 'a', agent: 'x', prompt: 'p' },
      ],
    };
    expectRoundTrip(input);
  });

  it('round-trips a workflow with no branch step and preserves an unrelated unmodelled key in raw_passthrough', () => {
    const input = {
      name: 'wf',
      steps: [{ id: 'a', agent: 'x', prompt: 'p', some_future_key: { nested: true } }],
    };
    const g = yamlToGraph(input);
    expect(g.nodes[0].data.kind).toBe('step');
    expect(g.nodes[0].data.raw_passthrough?.some_future_key).toEqual({ nested: true });
    expectRoundTrip(input);
  });

  it('round-trips approval.prompt and approval.timeout_seconds', () => {
    const input = {
      name: 'wf',
      steps: [
        {
          id: 'a',
          agent: 'x',
          prompt: 'p',
          approval: { required: true, prompt: 'Ready for human review?', timeout_seconds: 3600 },
        },
      ],
    };
    const g = yamlToGraph(input);
    expect(g.nodes[0].data.approvalRequired).toBe(true);
    expect(g.nodes[0].data.approvalPrompt).toBe('Ready for human review?');
    expect(g.nodes[0].data.approvalTimeoutSeconds).toBe(3600);
    expectRoundTrip(input);
  });

  it('classifies a standalone approval GATE node and round-trips the full approval block', () => {
    const input = {
      name: 'wf',
      steps: [
        { id: 'work', agent: 'coder', prompt: 'do work' },
        {
          id: 'ship-gate',
          approval: {
            required: true,
            prompt: 'Ship {{ inputs.tag }}?',
            timeout_seconds: 3600,
            auto_approve: '{{ inputs.trusted }}',
            on_timeout: 'reject',
            notify: [{ action: 'scm.prs.comment', with: { body: 'awaiting approval' } }],
            on_reject: [{ id: 'cleanup', agent: 'coder', prompt: 'revert' }],
          },
        },
      ],
    };
    const g = yamlToGraph(input);
    const gate = g.nodes.find((n) => n.id === 'ship-gate');
    expect(gate?.data.kind).toBe('approval_gate');
    expect(gate?.data.agent).toBeUndefined();
    expect(gate?.data.approvalAutoApprove).toBe('{{ inputs.trusted }}');
    expect(gate?.data.approvalOnTimeout).toBe('reject');
    expect(gate?.data.approvalNotify).toHaveLength(1);
    expect(gate?.data.approvalOnReject).toHaveLength(1);
    expectRoundTrip(input);
  });

  it('keeps a legacy inline approval (agent + prompt + approval) as a plain step, not a gate', () => {
    const input = {
      name: 'wf',
      steps: [{ id: 'a', agent: 'x', prompt: 'p', approval: { required: true } }],
    };
    const g = yamlToGraph(input);
    expect(g.nodes[0].data.kind).toBe('step');
    expectRoundTrip(input);
  });

  it('classifies a connector ACTION step and round-trips action + with', () => {
    const input = {
      name: 'wf',
      steps: [
        { id: 'open-pr', action: 'scm.prs.create', with: { title: 'Fix {{ inputs.bug }}', base: 'main' } },
      ],
    };
    const g = yamlToGraph(input);
    const node = g.nodes.find((n) => n.id === 'open-pr');
    expect(node?.data.kind).toBe('action');
    expect(node?.data.action).toBe('scm.prs.create');
    expect(node?.data.with).toEqual({ title: 'Fix {{ inputs.bug }}', base: 'main' });
    expect(node?.data.agent).toBeUndefined();
    expectRoundTrip(input);
  });
});

describe('nested passthrough', () => {
  it.each([
    ['panel', { name: 'w', steps: [{ id: 'p', panel: { panelists: ['r'], subject: 's', future_key: 42 } }] }, (s: any) => s.panel.future_key],
    ['branch', { name: 'w', steps: [{ id: 'b', branch: { condition: 'x', future_key: 42 } }] }, (s: any) => s.branch.future_key],
    ['approval', { name: 'w', steps: [{ id: 'g', approval: { required: true, future_key: 42 } }] }, (s: any) => s.approval.future_key],
  ])('an unmodeled key under %s survives a round-trip', (_name, input, get) => {
    const g = yamlToGraph(input as Record<string, unknown>);
    const out = graphToWorkflowObject(g) as { obj: Record<string, unknown> };
    expect(get((out.obj.steps as Record<string, unknown>[])[0])).toBe(42);
  });
});

// ── canConnect ──────────────────────────────────────────────────────────────

describe('canConnect', () => {
  it('rejects a self-loop', () => {
    const res = canConnect('a', 'a', { edges: [] });
    expect(res).toEqual({ ok: false, reason: "A step can't depend on itself.", kind: 'self' });
  });

  it('rejects a duplicate edge', () => {
    const res = canConnect('a', 'b', { edges: [edge('a', 'b')] });
    expect(res).toEqual({ ok: false, reason: 'These steps are already connected.', kind: 'duplicate' });
  });

  it('rejects a back-edge that closes a cycle', () => {
    const res = canConnect('b', 'a', { edges: [edge('a', 'b')] });
    expect(res.ok).toBe(false);
    if (!res.ok) {
      expect(res.reason).toContain('cycle');
      expect(res.kind).toBe('cycle');
    }
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

  it('flags a branch step missing a condition', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        { id: 'route', branch: { then: ['a'] } },
        { id: 'a', agent: 'x', prompt: 'p' },
      ],
    });
    expect(validateGraph(g).route).toEqual(expect.arrayContaining(['branch needs a condition']));
  });

  it('flags a branch then/else target that is not a known step id', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [{ id: 'route', branch: { condition: 'c', then: ['ghost'], else: ['also-ghost'] } }],
    });
    const v = validateGraph(g);
    expect(v.route).toEqual(
      expect.arrayContaining([
        'branch target ghost is not a known step',
        'branch target also-ghost is not a known step',
      ]),
    );
  });

  it('returns no problems for a clean branch step', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        { id: 'route', branch: { condition: 'c', then: ['a'], else: ['b'] } },
        { id: 'a', agent: 'x', prompt: 'p' },
        { id: 'b', agent: 'x', prompt: 'p' },
      ],
    });
    expect(validateGraph(g).route).toBeUndefined();
  });

  it('flags a reference to an unknown step', () => {
    const g: ReturnType<typeof yamlToGraph> = {
      nodes: [node('a', { agent: 'x', prompt: 'use steps.ghost here' })],
      edges: [],
      meta: { name: 'wf', rest: {} },
    };
    const v = validateGraph(g);
    expect(v.a.some((m) => m.includes('unknown step ghost'))).toBe(true);
  });

  it('flags max_parallel below 1', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [{ id: 'p', parallel: [{ id: 's', agent: 'a', prompt: 'x' }], max_parallel: 0 }],
    });
    const problems = validateGraph(g);
    expect(Object.values(problems).flat().some((m) => /max.?parallel/i.test(m) && /1/.test(m))).toBe(true);
  });

  it('does not flag a valid positive max_parallel', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [{ id: 'p', parallel: [{ id: 's', agent: 'a', prompt: 'x' }], max_parallel: 2 }],
    });
    const problems = validateGraph(g);
    expect(Object.values(problems).flat().some((m) => /max.?parallel/i.test(m))).toBe(false);
  });

  it('flags an enabled gate with no severity/fix_with/max_iterations', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [{ id: 'p', panel: { panelists: ['r'], subject: 's', gate: {} } }],
    });
    expect(Object.values(validateGraph(g)).flat().some((m) => /gate/i.test(m))).toBe(true);
  });

  it('does not flag a fully-specified gate', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [
        {
          id: 'p',
          panel: {
            panelists: ['r'],
            subject: 's',
            gate: { until_no_findings_at_severity_or_above: 'medium', fix_with: 'r', max_iterations: 3 },
          },
        },
      ],
    });
    expect(Object.values(validateGraph(g)).flat().some((m) => /gate/i.test(m))).toBe(false);
  });

  it('flags a panel with max_parallel below 1', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [{ id: 'pan', panel: { panelists: ['r'], subject: 's', max_parallel: 0 } }],
    });
    const problems = validateGraph(g);
    expect(Object.values(problems).flat().some((m) => /max.*parallel.*1/.test(m))).toBe(true);
  });

  it('does not flag a panel with a valid positive max_parallel', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [{ id: 'pan', panel: { panelists: ['r'], subject: 's', max_parallel: 2 } }],
    });
    const problems = validateGraph(g);
    expect(Object.values(problems).flat().some((m) => /max.*parallel/i.test(m))).toBe(false);
  });

  it('flags a notify entry with no action', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [{ id: 'gate', approval: { required: true, notify: [{}] } }],
    });
    const problems = validateGraph(g);
    expect(problems.gate.some((m) => /notification/.test(m) && /action/.test(m))).toBe(true);
  });

  it('does not flag a complete notify entry', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [{ id: 'gate', approval: { required: true, notify: [{ action: 'x' }] } }],
    });
    const problems = validateGraph(g);
    expect((problems.gate ?? []).some((m) => /notification/.test(m))).toBe(false);
  });

  it('flags an action-shaped on_reject entry with an empty action', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [{ id: 'gate', approval: { required: true, on_reject: [{ action: '' }] } }],
    });
    const problems = validateGraph(g);
    expect(problems.gate.some((m) => /on_reject/.test(m) && /action/.test(m))).toBe(true);
  });

  it('does not flag an agent-shaped on_reject entry (pre-existing behavior)', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [{ id: 'gate', approval: { required: true, on_reject: [{ agent: '', prompt: '' }] } }],
    });
    const problems = validateGraph(g);
    expect((problems.gate ?? []).some((m) => /on_reject/.test(m))).toBe(false);
  });
});

// ── fold-in (review Minor): `with:` on a non-action step/for_each node ──────

describe('nodeToStepObject — with: on a non-action node (fold-in)', () => {
  it('round-trips a stray `with:` on a plain step instead of silently dropping it', () => {
    // Schema-invalid on save (workflow.rs rejects `with:` without `action:`),
    // but round-trip fidelity through the editor is the contract this module
    // holds everywhere else (raw_passthrough) — a silent drop would be worse.
    const input = {
      name: 'wf',
      steps: [{ id: 'a', agent: 'x', prompt: 'p', with: { stray: 'value' } }],
    };
    expectRoundTrip(input);
  });

  it('round-trips a non-empty `actions:` on an action node instead of silently dropping it', () => {
    // `actions` is in MODELLED_STEP_KEYS (so it's excluded from raw_passthrough)
    // and the action arm didn't emit it — an odd combo on hand-authored YAML,
    // but round-trip fidelity must not lose data either way.
    const input = {
      name: 'wf',
      steps: [
        { id: 'a', action: 'github.create_pr', with: { title: 't' }, actions: ['read', 'write'] },
      ],
    };
    expectRoundTrip(input);
  });
});

// ── hasInlineApproval / convertInlineApprovalToGate ─────────────────────────

describe('hasInlineApproval', () => {
  it('is true for a step with approval.required', () => {
    expect(hasInlineApproval({ id: 'a', kind: 'step', agent: 'x', prompt: 'p', approvalRequired: true })).toBe(
      true,
    );
  });

  it('is true for a for_each with approval.required', () => {
    expect(
      hasInlineApproval({ id: 'a', kind: 'for_each', agent: 'x', prompt: 'p', approvalRequired: true }),
    ).toBe(true);
  });

  it('is false without approval.required', () => {
    expect(hasInlineApproval({ id: 'a', kind: 'step', agent: 'x', prompt: 'p' })).toBe(false);
  });

  it('is false for a standalone approval_gate node (not a legacy inline approval)', () => {
    expect(hasInlineApproval({ id: 'a', kind: 'approval_gate', approvalRequired: true })).toBe(false);
  });
});

describe('convertInlineApprovalToGate', () => {
  it('inserts a new gate step before the agent step and strips the inline approval', () => {
    const input = {
      name: 'wf',
      steps: [
        {
          id: 'ship',
          agent: 'deployer',
          prompt: 'deploy it',
          approval: { required: true, prompt: 'ok to ship?', timeout_seconds: 300 },
        },
      ],
    };
    const g = yamlToGraph(input);
    const next = convertInlineApprovalToGate(g, 'ship');

    // the agent step no longer carries approval.
    const agent = next.nodes.find((n) => n.id === 'ship');
    expect(agent?.data.approvalRequired).toBeUndefined();
    expect(agent?.data.approvalPrompt).toBeUndefined();
    expect(agent?.data.approvalTimeoutSeconds).toBeUndefined();
    expect(agent?.data.agent).toBe('deployer'); // untouched
    expect(agent?.data.prompt).toBe('deploy it');

    // a new gate step exists, carrying the prompt/timeout, and runs BEFORE the
    // agent step in topo order.
    const sorted = topoSort(next.nodes, next.edges);
    expect('order' in sorted).toBe(true);
    if (!('order' in sorted)) return;
    const order = sorted.order.map((n) => n.id);
    const gateId = order.find((id) => id !== 'ship');
    expect(gateId).toBeDefined();
    const gate = next.nodes.find((n) => n.id === gateId);
    expect(gate?.data.kind).toBe('approval_gate');
    expect(gate?.data.approvalRequired).toBe(true);
    expect(gate?.data.approvalPrompt).toBe('ok to ship?');
    expect(gate?.data.approvalTimeoutSeconds).toBe(300);
    expect(order.indexOf(gateId!)).toBeLessThan(order.indexOf('ship'));

    // round-trips to YAML: a standalone gate step then the (approval-free)
    // agent step.
    const res = graphToWorkflowObject(next);
    expect('obj' in res).toBe(true);
    if (!('obj' in res)) return;
    const steps = (res.obj as { steps: Record<string, unknown>[] }).steps;
    expect(steps).toHaveLength(2);
    expect(steps[0]).toMatchObject({ id: gateId, approval: { required: true, prompt: 'ok to ship?' } });
    expect(steps[0].agent).toBeUndefined();
    expect(steps[1]).toMatchObject({ id: 'ship', agent: 'deployer', prompt: 'deploy it' });
    expect(steps[1].approval).toBeUndefined();
  });

  it('rewires a predecessor edge onto the new gate instead of the agent step', () => {
    const input = {
      name: 'wf',
      steps: [
        { id: 'build', agent: 'x', prompt: 'build' },
        { id: 'ship', agent: 'deployer', prompt: 'deploy {{ steps.build.output }}', approval: { required: true } },
      ],
    };
    const g = yamlToGraph(input);
    const next = convertInlineApprovalToGate(g, 'ship');
    const gateId = next.nodes.find((n) => n.data.kind === 'approval_gate')!.id;

    // Linear execution order is build -> gate -> ship (the chain edges derive
    // from the new node-array order: gate is spliced immediately before ship).
    expect(next.edges.some((e) => e.source === 'build' && e.target === gateId)).toBe(true);
    expect(next.edges.some((e) => e.source === gateId && e.target === 'ship')).toBe(true);
    // A build->ship DATA-REF edge also derives (honestly) because `ship`'s
    // prompt still reads `steps.build.output` — under the single-source
    // model edges are a pure derivation of node data/order, so this is now
    // expected: it represents ship's real data dependency on build's output,
    // independent of the gate the operator inserted into the linear chain
    // above (the chain edges alone already enforce gate-runs-between-them).
    expect(next.edges.some((e) => e.source === 'build' && e.target === 'ship')).toBe(true);
  });

  it('is a no-op for an unknown step id', () => {
    const g = yamlToGraph({ name: 'wf', steps: [{ id: 'a', agent: 'x', prompt: 'p' }] });
    expect(convertInlineApprovalToGate(g, 'ghost')).toBe(g);
  });

  it('is a no-op for a step without an inline approval', () => {
    const g = yamlToGraph({ name: 'wf', steps: [{ id: 'a', agent: 'x', prompt: 'p' }] });
    expect(convertInlineApprovalToGate(g, 'a')).toBe(g);
  });

  it('carries advanced approval fields (auto_approve/on_timeout/notify/on_reject) onto the new gate, leaving the agent step with no approval-derived fields', () => {
    const input = {
      name: 'wf',
      steps: [
        {
          id: 'ship',
          agent: 'deployer',
          prompt: 'deploy it',
          approval: {
            required: true,
            prompt: 'ok to ship?',
            timeout_seconds: 300,
            auto_approve: '{{ inputs.trusted }}',
            on_timeout: 'reject',
            notify: [{ slack: '#releases' }],
            on_reject: [{ action: 'notify', with: { channel: 'oncall' } }],
          },
        },
      ],
    };
    const g = yamlToGraph(input);
    const next = convertInlineApprovalToGate(g, 'ship');

    const agent = next.nodes.find((n) => n.id === 'ship');
    expect(agent?.data.approvalRequired).toBeUndefined();
    expect(agent?.data.approvalPrompt).toBeUndefined();
    expect(agent?.data.approvalTimeoutSeconds).toBeUndefined();
    expect(agent?.data.approvalAutoApprove).toBeUndefined();
    expect(agent?.data.approvalOnTimeout).toBeUndefined();
    expect(agent?.data.approvalNotify).toBeUndefined();
    expect(agent?.data.approvalOnReject).toBeUndefined();

    const gate = next.nodes.find((n) => n.data.kind === 'approval_gate');
    expect(gate?.data.approvalPrompt).toBe('ok to ship?');
    expect(gate?.data.approvalTimeoutSeconds).toBe(300);
    expect(gate?.data.approvalAutoApprove).toBe('{{ inputs.trusted }}');
    expect(gate?.data.approvalOnTimeout).toBe('reject');
    expect(gate?.data.approvalNotify).toEqual([{ slack: '#releases' }]);
    expect(gate?.data.approvalOnReject).toEqual([{ action: 'notify', with: { channel: 'oncall' } }]);

    // serializes: gate owns the full approval block, the agent step has none.
    const res = graphToWorkflowObject(next);
    expect('obj' in res).toBe(true);
    if (!('obj' in res)) return;
    const steps = (res.obj as { steps: Record<string, unknown>[] }).steps;
    const gateStep = steps.find((s) => s.id === gate?.id);
    const agentStep = steps.find((s) => s.id === 'ship');
    expect(gateStep?.approval).toEqual({
      required: true,
      prompt: 'ok to ship?',
      timeout_seconds: 300,
      auto_approve: '{{ inputs.trusted }}',
      on_timeout: 'reject',
      notify: [{ slack: '#releases' }],
      on_reject: [{ action: 'notify', with: { channel: 'oncall' } }],
    });
    expect(agentStep?.approval).toBeUndefined();
  });

  it('picks a non-colliding gate id if `<id>-gate` is already taken', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        { id: 'ship-gate', agent: 'x', prompt: 'unrelated' },
        { id: 'ship', agent: 'deployer', prompt: 'deploy', approval: { required: true } },
      ],
    });
    const next = convertInlineApprovalToGate(g, 'ship');
    const gate = next.nodes.find((n) => n.data.kind === 'approval_gate');
    expect(gate?.id).toBe('ship-gate-1');
  });

  it('shifts the agent step clear of the inserted gate, which is wider than a step', () => {
    const g = yamlToGraph({
      name: 'wf',
      steps: [
        {
          id: 'ship',
          agent: 'deployer',
          prompt: 'deploy it',
          approval: { required: true, prompt: 'ok to ship?' },
        },
      ],
    });
    const before = g.nodes.find((n) => n.id === 'ship')!.position.x;
    const next = convertInlineApprovalToGate(g, 'ship');
    const after = next.nodes.find((n) => n.id === 'ship')!.position.x;

    // a gate is GATE_W (214) wide, not NODE_W (210) — at the old 274 the step
    // landed 4px inside the gate it was making room for.
    expect(after - before).toBe(278);
  });
});

import { deriveEdges, withDerivedEdges, hasExplicitEdges } from './workflowGraph';

describe('deriveEdges', () => {
  // NOTE: as of this writing, no *.yaml under .rupu/workflows/ actually
  // contains a step-level `branch:` block (issue-supervisor-dispatch.yaml and
  // phase-delivery-cycle.yaml only have an unrelated `workspace.branch`
  // string field — verified by grep, not a branch *step*). So this test
  // hand-authors a small, realistic workflow object instead of loading one
  // from disk, in the same style as every other yamlToGraph({...}) call in
  // this file.
  //
  // Critically, the expectation below is a HARDCODED edge list computed by
  // hand from the steps — never `g.edges` or another `deriveEdges(...)` call
  // — so it actually discriminates: it fails if the chain loop or the
  // branch-arm loop in `deriveEdges` is broken or removed. (Verified by
  // temporarily deleting the branch-arm loop — see task-1-report.md.)
  it('derives the exact edge set for a workflow with a chain, a data-ref, and a branch', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [
        { id: 'triage', agent: 'a', prompt: 'look at the issue' },
        { id: 'route', branch: { condition: '{{ steps.triage.output }}', then: ['fix'], else: ['escalate'] } },
        { id: 'fix', agent: 'a', prompt: 'fix it based on {{ steps.triage.output }}' },
        { id: 'escalate', agent: 'a', prompt: 'escalate to a human' },
      ],
    });

    // Hand-derived from the steps above:
    //  (a) chain:     triage->route, route->fix, fix->escalate
    //  (b) data-ref:  `fix`'s prompt references steps.triage -> triage->fix
    //                 (route's condition also references steps.triage, but
    //                 that collapses onto the triage->route chain edge, so
    //                 it does NOT appear again here)
    //  (c) branch arm: route's then->fix and else->escalate
    const expected = [
      { id: 'triage->route', source: 'triage', target: 'route' },
      { id: 'route->fix', source: 'route', target: 'fix' },
      { id: 'fix->escalate', source: 'fix', target: 'escalate' },
      { id: 'triage->fix', source: 'triage', target: 'fix' },
      { id: 'route->fix:then', source: 'route', target: 'fix', label: 'true', branch: 'then' },
      { id: 'route->escalate:else', source: 'route', target: 'escalate', label: 'false', branch: 'else' },
    ];
    expect(deriveEdges(g.nodes)).toEqual(expected);
  });

  it('derives a branch arm edge from thenTargets/elseTargets', () => {
    const nodes = yamlToGraph({
      name: 'w',
      steps: [
        { id: 'b', branch: { condition: 'x', then: ['t'], else: ['e'] } },
        { id: 't', agent: 'a', prompt: 'p' },
        { id: 'e', agent: 'a', prompt: 'p' },
      ],
    }).nodes;
    const edges = deriveEdges(nodes);
    expect(edges).toContainEqual(expect.objectContaining({ source: 'b', target: 't', branch: 'then', label: 'true' }));
    expect(edges).toContainEqual(expect.objectContaining({ source: 'b', target: 'e', branch: 'else', label: 'false' }));
  });

  // This is NOT a deriveEdges correctness guard (that's the hardcoded test
  // above) — it documents a separate, real contract: `withDerivedEdges`
  // must always wire `edges` through `deriveEdges(nodes)` rather than letting
  // a caller pass/store edges independently. Later tasks (T2-T4) depend on
  // that wiring, not on deriveEdges's internal correctness.
  it('withDerivedEdges always wires edges through deriveEdges(nodes)', () => {
    const nodes = yamlToGraph({ name: 'w', steps: [{ id: 'a', agent: 'x', prompt: 'p' }] }).nodes;
    const g = withDerivedEdges({ name: 'w', rest: {} }, nodes);
    expect(g.edges).toEqual(deriveEdges(g.nodes));
  });
});

describe('serialization totality', () => {
  it('a panel step keeps its when/continue_on_error on round-trip (P0.4)', () => {
    const input = {
      name: 'w',
      steps: [{ id: 'p', when: 'inputs.go', continue_on_error: true, panel: { panelists: ['r'], subject: 's' } }],
    };
    const g = yamlToGraph(input);
    const out = graphToWorkflowObject(g) as { obj: Record<string, unknown> };
    const step = (out.obj.steps as Record<string, unknown>[])[0];
    expect(step.when).toBe('inputs.go');
    expect(step.continue_on_error).toBe(true);
  });

  it('graphToWorkflowObject orders by derived edges, not stored edges', () => {
    // Declaration order is 'b' then 'a' — lexicographically the reverse of id
    // order, and every node gets position {x:0,y:0} from yamlToGraph, so the
    // topoSort tiebreak (position, then id) can't accidentally reproduce the
    // correct order on its own. Only the derived chain edge b->a forces 'b'
    // first; trusting an emptied g.edges would fall back to the id tiebreak
    // and wrongly emit ['a', 'b'].
    const g = yamlToGraph({ name: 'w', steps: [{ id: 'b', agent: 'x', prompt: 'p' }, { id: 'a', agent: 'x', prompt: 'p' }] });
    // corrupt the stored edges; serialization must ignore them and use deriveEdges(nodes)
    const corrupted = { ...g, edges: [] };
    const out = graphToWorkflowObject(corrupted) as { obj: Record<string, unknown> };
    expect((out.obj.steps as Record<string, unknown>[]).map((s) => s.id)).toEqual(['b', 'a']);
  });
});

describe('explicit-edge model', () => {
  it('derives edges from next: not from list order', () => {
    // two steps, NO next → no chain edge (graph mode is off, but the key change:
    // when next IS present, order does not add edges). Use an explicit-next case:
    const g = yamlToGraph({ name: 'w', steps: [
      { id: 'a', agent: 'x', prompt: 'p', next: ['c'] },
      { id: 'b', agent: 'x', prompt: 'p' },     // no next → terminal in graph mode
      { id: 'c', agent: 'x', prompt: 'p' },
    ]});
    const e = deriveEdges(g.nodes);
    expect(e).toContainEqual(expect.objectContaining({ source: 'a', target: 'c' }));
    // a→b is NOT an edge just because b follows a in the list
    expect(e.some((x) => x.source === 'a' && x.target === 'b')).toBe(false);
  });

  it('a legacy workflow (no explicit edges) still shows the linear chain', () => {
    const g = yamlToGraph({ name: 'w', steps: [
      { id: 'a', agent: 'x', prompt: 'p' }, { id: 'b', agent: 'x', prompt: 'p' },
    ]});
    expect(deriveEdges(g.nodes)).toContainEqual(expect.objectContaining({ source: 'a', target: 'b' }));
  });

  it('a data reference infers an edge', () => {
    const g = yamlToGraph({ name: 'w', steps: [
      { id: 'a', agent: 'x', prompt: 'p', next: ['z'] },
      { id: 'b', agent: 'x', prompt: 'use {{ steps.a.output }}' },
      { id: 'z', agent: 'x', prompt: 'p' },
    ]});
    expect(deriveEdges(g.nodes)).toContainEqual(expect.objectContaining({ source: 'a', target: 'b' }));
  });

  it('round-trips next/split/join', () => {
    const input = { name: 'w', steps: [
      { id: 's', split: ['a', 'b'] },
      { id: 'a', agent: 'x', prompt: 'p', next: ['j'] },
      { id: 'b', agent: 'x', prompt: 'p', next: ['j'] },
      { id: 'j', join: { wait: 'all' } },
    ]};
    const out = graphToWorkflowObject(yamlToGraph(input)) as { obj: any };
    expect(out.obj.steps.find((s: any) => s.id === 's').split).toEqual(['a', 'b']);
    expect(out.obj.steps.find((s: any) => s.id === 'j').join).toEqual({ wait: 'all' });
  });

  it('hasExplicitEdges is false for a legacy workflow and true when any node has next/split/join', () => {
    const legacy = yamlToGraph({ name: 'w', steps: [{ id: 'a', agent: 'x', prompt: 'p' }, { id: 'b', agent: 'x', prompt: 'p' }] });
    expect(hasExplicitEdges(legacy.nodes)).toBe(false);

    const withNext = yamlToGraph({ name: 'w', steps: [{ id: 'a', agent: 'x', prompt: 'p', next: ['b'] }, { id: 'b', agent: 'x', prompt: 'p' }] });
    expect(hasExplicitEdges(withNext.nodes)).toBe(true);

    const withSplit = yamlToGraph({ name: 'w', steps: [{ id: 's', split: ['a'] }, { id: 'a', agent: 'x', prompt: 'p' }] });
    expect(hasExplicitEdges(withSplit.nodes)).toBe(true);

    const withJoin = yamlToGraph({ name: 'w', steps: [{ id: 'j', join: { wait: 'any' } }] });
    expect(hasExplicitEdges(withJoin.nodes)).toBe(true);
  });

  it('a split node fans out to its split targets as edges', () => {
    const g = yamlToGraph({ name: 'w', steps: [
      { id: 's', split: ['a', 'b'] },
      { id: 'a', agent: 'x', prompt: 'p', next: ['j'] },
      { id: 'b', agent: 'x', prompt: 'p', next: ['j'] },
      { id: 'j', join: { wait: 'all' } },
    ]});
    const e = deriveEdges(g.nodes);
    expect(e).toContainEqual(expect.objectContaining({ source: 's', target: 'a' }));
    expect(e).toContainEqual(expect.objectContaining({ source: 's', target: 'b' }));
    expect(e).toContainEqual(expect.objectContaining({ source: 'a', target: 'j' }));
    expect(e).toContainEqual(expect.objectContaining({ source: 'b', target: 'j' }));
  });

  it('parses next/split/join into StepNodeData and classifies kind', () => {
    const g = yamlToGraph({ name: 'w', steps: [
      { id: 's', split: ['a', 'b'] },
      { id: 'a', agent: 'x', prompt: 'p', next: ['j'] },
      { id: 'j', join: { wait: { count: 2 } } },
    ]});
    const s = g.nodes.find((n) => n.id === 's') as GraphNode;
    const a = g.nodes.find((n) => n.id === 'a') as GraphNode;
    const j = g.nodes.find((n) => n.id === 'j') as GraphNode;
    expect(s.data.kind).toBe('split');
    expect(s.data.split).toEqual(['a', 'b']);
    expect(a.data.next).toEqual(['j']);
    expect(j.data.kind).toBe('join');
    expect(j.data.joinWait).toEqual({ count: 2 });
  });

  it('a legacy workflow with next/split/join omitted round-trips without those keys', () => {
    const input = { name: 'w', steps: [{ id: 'a', agent: 'x', prompt: 'p' }, { id: 'b', agent: 'x', prompt: 'p' }] };
    expectRoundTrip(input);
    const out = graphToWorkflowObject(yamlToGraph(input)) as { obj: any };
    for (const s of out.obj.steps) {
      expect('next' in s).toBe(false);
      expect('split' in s).toBe(false);
      expect('join' in s).toBe(false);
    }
  });
});
