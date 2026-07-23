// @vitest-environment jsdom
import { afterEach, describe, it, expect } from 'vitest';
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { ReactFlowProvider, type NodeProps } from '@xyflow/react';
import ActionNode, { type ActionNodeData, type ActionFlowNode } from './ActionNode';
import type { GraphNode } from '../../lib/runGraphModel';

afterEach(() => {
  cleanup();
});

// `Handle` (used by ActionNode) reads xyflow's zustand store off context, so
// every render needs a `ReactFlowProvider` ancestor — mirrors how the real
// canvas mounts each custom node type.
function renderAction(props: NodeProps<ActionFlowNode>) {
  return render(
    <ReactFlowProvider>
      <ActionNode {...props} />
    </ReactFlowProvider>,
  );
}

function makeProps(node: GraphNode): NodeProps<ActionFlowNode> {
  const data: ActionNodeData = { node };
  return {
    id: node.id,
    data,
    type: 'action',
    dragging: false,
    zIndex: 0,
    selectable: true,
    deletable: true,
    selected: false,
    draggable: false,
    isConnectable: true,
    positionAbsoluteX: 0,
    positionAbsoluteY: 0,
  } as unknown as NodeProps<ActionFlowNode>;
}

describe('ActionNode', () => {
  it('renders the tool name', () => {
    const node: GraphNode = {
      id: 'create_pr',
      kind: 'action',
      state: 'running',
      action: 'scm.prs.create',
    };
    renderAction(makeProps(node));
    expect(screen.getByText('scm.prs.create')).toBeInTheDocument();
  });

  it('renders a "connector" tag', () => {
    const node: GraphNode = {
      id: 'create_pr',
      kind: 'action',
      state: 'pending',
      action: 'scm.prs.create',
    };
    renderAction(makeProps(node));
    expect(screen.getByText('connector')).toBeInTheDocument();
  });

  it('shows the done state label', () => {
    const node: GraphNode = {
      id: 'create_pr',
      kind: 'action',
      state: 'done',
      action: 'scm.prs.create',
    };
    renderAction(makeProps(node));
    expect(screen.getByText('done')).toBeInTheDocument();
  });
});
