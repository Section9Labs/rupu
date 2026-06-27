// @vitest-environment jsdom
// WorkflowEditorGraph — palette wiring (rendered) + the exported pure mutation
// helpers (applyConnect / applyDelete / applyAddNode).
//
// @xyflow/react is mocked: the real canvas depends on ResizeObserver and layout
// APIs jsdom lacks, and its drag/connect behavior isn't what we're verifying
// here — the editor's mutation logic lives in the exported pure helpers, which we
// test directly. The mock is a thin stub (ReactFlow renders its children, the
// chrome components render nothing) so the palette/toolbar DOM still mounts.

import '@testing-library/jest-dom/vitest';
import type { ReactNode } from 'react';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';

vi.mock('@xyflow/react', () => ({
  ReactFlow: ({ children }: { children?: ReactNode }) => <div data-testid="rf">{children}</div>,
  ReactFlowProvider: ({ children }: { children?: ReactNode }) => <>{children}</>,
  Background: () => null,
  Controls: () => null,
  MiniMap: () => null,
  Handle: () => null,
  Position: { Top: 'top', Bottom: 'bottom', Left: 'left', Right: 'right' },
  MarkerType: { ArrowClosed: 'arrowclosed' },
  BackgroundVariant: { Dots: 'dots' },
  applyNodeChanges: (_changes: unknown, nodes: unknown) => nodes,
}));

import WorkflowEditorGraph, { applyConnect, applyDelete, applyAddNode } from './WorkflowEditorGraph';
import type { WorkflowGraph } from '../../lib/workflowGraph';

afterEach(cleanup);

function makeGraph(): WorkflowGraph {
  return {
    nodes: [
      { id: 'a', data: { id: 'a', kind: 'step', agent: 'x', prompt: 'p' }, position: { x: 0, y: 0 } },
      { id: 'b', data: { id: 'b', kind: 'step', agent: 'y', prompt: 'q' }, position: { x: 0, y: 100 } },
    ],
    edges: [{ id: 'a->b', source: 'a', target: 'b' }],
    meta: { name: 'wf', rest: {} },
  };
}

describe('palette', () => {
  it('Step button adds one node and selects the new id', () => {
    const onChange = vi.fn();
    const onSelect = vi.fn();
    render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={onChange}
        selectedId={null}
        onSelect={onSelect}
        problemsById={{}}
        onInvalidConnection={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Step' }));

    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0][0] as WorkflowGraph;
    expect(next.nodes).toHaveLength(3);
    const newId = next.nodes[2].id;
    expect(onSelect).toHaveBeenCalledWith(newId);
  });
});

describe('applyConnect', () => {
  it('valid connection emits onChange with the new edge, not onInvalid', () => {
    const onChange = vi.fn();
    const onInvalid = vi.fn();
    const g = makeGraph();
    g.nodes.push({ id: 'c', data: { id: 'c', kind: 'step' }, position: { x: 0, y: 200 } });

    applyConnect(g, { source: 'b', target: 'c' }, onChange, onInvalid);

    expect(onInvalid).not.toHaveBeenCalled();
    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0][0] as WorkflowGraph;
    expect(next.edges).toContainEqual({ id: 'b->c', source: 'b', target: 'c' });
  });

  it('self-loop is rejected with a reason, no onChange', () => {
    const onChange = vi.fn();
    const onInvalid = vi.fn();
    applyConnect(makeGraph(), { source: 'a', target: 'a' }, onChange, onInvalid);
    expect(onChange).not.toHaveBeenCalled();
    expect(onInvalid).toHaveBeenCalledWith(expect.stringContaining('itself'));
  });

  it('cycle is rejected with a reason, no onChange', () => {
    const onChange = vi.fn();
    const onInvalid = vi.fn();
    // a->b already exists; b->a would close a cycle.
    applyConnect(makeGraph(), { source: 'b', target: 'a' }, onChange, onInvalid);
    expect(onChange).not.toHaveBeenCalled();
    expect(onInvalid).toHaveBeenCalledWith(expect.stringContaining('cycle'));
  });

  it('duplicate edge is rejected with a reason, no onChange', () => {
    const onChange = vi.fn();
    const onInvalid = vi.fn();
    applyConnect(makeGraph(), { source: 'a', target: 'b' }, onChange, onInvalid);
    expect(onChange).not.toHaveBeenCalled();
    expect(onInvalid).toHaveBeenCalledWith(expect.stringContaining('already connected'));
  });

  it('missing endpoint is a no-op', () => {
    const onChange = vi.fn();
    const onInvalid = vi.fn();
    applyConnect(makeGraph(), { source: null, target: 'b' }, onChange, onInvalid);
    expect(onChange).not.toHaveBeenCalled();
    expect(onInvalid).not.toHaveBeenCalled();
  });
});

describe('applyDelete', () => {
  it('removes the node and every edge touching it', () => {
    const next = applyDelete(makeGraph(), 'a');
    expect(next.nodes.map((n) => n.id)).toEqual(['b']);
    expect(next.edges).toHaveLength(0);
  });
});

describe('applyAddNode', () => {
  it('parallel default carries an empty parallel array', () => {
    const { graph, id } = applyAddNode(makeGraph(), 'parallel');
    const added = graph.nodes.find((n) => n.id === id);
    expect(added?.data.kind).toBe('parallel');
    expect(added?.data.parallel).toEqual([]);
  });

  it('panel default carries an empty panel config', () => {
    const { graph, id } = applyAddNode(makeGraph(), 'panel');
    const added = graph.nodes.find((n) => n.id === id);
    expect(added?.data.kind).toBe('panel');
    expect(added?.data.panel).toEqual({ panelists: [], subject: '' });
  });

  it('generates an id that does not collide with existing nodes', () => {
    const g = makeGraph();
    g.nodes.push({ id: 'step-1', data: { id: 'step-1', kind: 'step' }, position: { x: 0, y: 0 } });
    const { id } = applyAddNode(g, 'step');
    expect(id).toBe('step-2');
  });
});
