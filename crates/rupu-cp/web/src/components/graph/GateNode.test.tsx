// @vitest-environment jsdom
import { afterEach, describe, it, expect } from 'vitest';
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { ReactFlowProvider, type NodeProps } from '@xyflow/react';
import GateNode, { type GateNodeData, type GateFlowNode } from './GateNode';
import type { GraphNode } from '../../lib/runGraphModel';

afterEach(() => {
  cleanup();
});

// `Handle` (used by GateNode) reads xyflow's zustand store off context, so
// every render needs a `ReactFlowProvider` ancestor — mirrors how the real
// canvas mounts each custom node type.
function renderGate(props: NodeProps<GateFlowNode>) {
  return render(
    <ReactFlowProvider>
      <GateNode {...props} />
    </ReactFlowProvider>,
  );
}

function makeProps(node: GraphNode): NodeProps<GateFlowNode> {
  const data: GateNodeData = { node };
  return {
    id: node.id,
    data,
    type: 'gate',
    dragging: false,
    zIndex: 0,
    selectable: true,
    deletable: true,
    selected: false,
    draggable: false,
    isConnectable: true,
    positionAbsoluteX: 0,
    positionAbsoluteY: 0,
  } as unknown as NodeProps<GateFlowNode>;
}

describe('GateNode', () => {
  it('renders the gate id and the "awaiting" label when awaiting_approval', () => {
    const node: GraphNode = { id: 'approve', kind: 'gate', state: 'awaiting_approval' };
    renderGate(makeProps(node));
    expect(screen.getByText('approve')).toBeInTheDocument();
    expect(screen.getByText('needs you')).toBeInTheDocument();
  });

  it('renders an "auto" tag when approval_gate.auto_approve is true', () => {
    const node: GraphNode = {
      id: 'approve',
      kind: 'gate',
      state: 'pending',
      approval_gate: { auto_approve: true, has_on_reject: false, timeout_seconds: null },
    };
    renderGate(makeProps(node));
    expect(screen.getByText('auto')).toBeInTheDocument();
  });

  it('does not render an "auto" tag when auto_approve is false', () => {
    const node: GraphNode = {
      id: 'approve',
      kind: 'gate',
      state: 'pending',
      approval_gate: { auto_approve: false, has_on_reject: false, timeout_seconds: null },
    };
    renderGate(makeProps(node));
    expect(screen.queryByText('auto')).not.toBeInTheDocument();
  });

  it('shows an on-reject affordance when has_on_reject is true', () => {
    const node: GraphNode = {
      id: 'approve',
      kind: 'gate',
      state: 'pending',
      approval_gate: { auto_approve: false, has_on_reject: true, timeout_seconds: null },
    };
    renderGate(makeProps(node));
    expect(screen.getByText(/on reject/)).toBeInTheDocument();
  });

  it('shows done / failed glyph labels', () => {
    const done: GraphNode = { id: 'approve', kind: 'gate', state: 'done' };
    const { unmount } = renderGate(makeProps(done));
    expect(screen.getByText('done')).toBeInTheDocument();
    unmount();

    const failed: GraphNode = { id: 'approve', kind: 'gate', state: 'failed' };
    renderGate(makeProps(failed));
    expect(screen.getByText('failed')).toBeInTheDocument();
  });

  it('renders a kind pill alongside the existing status treatment', () => {
    const node: GraphNode = { id: 'approve', kind: 'gate', state: 'awaiting_approval' };
    renderGate(makeProps(node));
    expect(screen.getByTestId('rg-kindpill')).toBeInTheDocument();
    expect(screen.getByText('needs you')).toBeInTheDocument();
  });

  it('carries Approve / Reject only while it waits and the page gave it a control', () => {
    const gate = { state: () => ({ pending: false, decision: null, error: null }), approve: () => {}, reject: () => {} };
    const waiting: GraphNode = { id: 'ship', kind: 'gate', state: 'awaiting_approval' };
    const props = makeProps(waiting);
    renderGate({ ...props, data: { ...props.data, gate } });
    expect(screen.getByRole('button', { name: 'Approve ship' })).toBeInTheDocument();
    cleanup();

    // a read-only graph (no control) and a gate that has moved on show none
    renderGate(makeProps(waiting));
    expect(screen.queryByRole('button', { name: 'Approve ship' })).toBeNull();
    cleanup();
    const done = makeProps({ ...waiting, state: 'done' });
    renderGate({ ...done, data: { ...done.data, gate } });
    expect(screen.queryByRole('button', { name: 'Approve ship' })).toBeNull();
  });
});
