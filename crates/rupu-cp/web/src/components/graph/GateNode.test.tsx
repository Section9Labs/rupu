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

function makeProps(node: GraphNode, ui?: 'classic' | 'next'): NodeProps<GateFlowNode> {
  const data: GateNodeData = { node, ui };
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
    expect(screen.getByText('awaiting')).toBeInTheDocument();
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

  it('next: renders a kind pill alongside the existing status treatment', () => {
    const node: GraphNode = { id: 'approve', kind: 'gate', state: 'awaiting_approval' };
    renderGate(makeProps(node, 'next'));
    expect(screen.getByTestId('rg-kindpill')).toBeInTheDocument();
    expect(screen.getByText('awaiting')).toBeInTheDocument();
  });
});
