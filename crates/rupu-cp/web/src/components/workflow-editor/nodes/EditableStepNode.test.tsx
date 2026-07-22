// @vitest-environment jsdom
// EditableStepNode — the editor card mirrors the Runs cards (LR handles, per-kind
// body). @xyflow/react is mocked because jsdom lacks the layout/ResizeObserver
// APIs the real handles need; the mock stubs Handle/Position so the card DOM
// mounts and we can assert the per-kind body content.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import type { Node, NodeProps } from '@xyflow/react';

vi.mock('@xyflow/react', () => ({
  Handle: () => null,
  Position: { Top: 'top', Bottom: 'bottom', Left: 'left', Right: 'right' },
}));

import EditableStepNode, { type NodeData } from './EditableStepNode';
import type { GraphNode, StepNodeData } from '../../../lib/workflowGraph';
import type { WorkflowEditorUi } from '../../../hooks/useWorkflowEditorUi';

afterEach(cleanup);

function renderNode(
  data: StepNodeData,
  problems: string[] = [],
  selected = false,
  workflowEditorUi?: WorkflowEditorUi,
) {
  const node: GraphNode = { id: data.id, data, position: { x: 0, y: 0 } };
  const props = { data: { node, problems, workflowEditorUi }, selected } as unknown as NodeProps<
    Node<NodeData, 'editable'>
  >;
  return render(<EditableStepNode {...props} />);
}

describe('EditableStepNode', () => {
  it('a step shows its id, kind chip, and agent', () => {
    renderNode({ id: 'build', kind: 'step', agent: 'coder' });
    expect(screen.getByText('build')).toBeInTheDocument();
    expect(screen.getByText('step')).toBeInTheDocument();
    expect(screen.getByText('coder')).toBeInTheDocument();
  });

  it('a step with no agent reads "(no agent)"', () => {
    renderNode({ id: 'x', kind: 'step' });
    expect(screen.getByText('(no agent)')).toBeInTheDocument();
  });

  it('a parallel node renders a stacked row per sub-step', () => {
    renderNode({
      id: 'fan',
      kind: 'parallel',
      parallel: [
        { id: 'lint', agent: 'a', prompt: 'p' },
        { id: 'test', agent: 'b', prompt: 'q' },
      ],
    });
    expect(screen.getByText('parallel')).toBeInTheDocument();
    expect(screen.getByText('lint')).toBeInTheDocument();
    expect(screen.getByText('test')).toBeInTheDocument();
  });

  it('a parallel node with no sub-steps shows the empty placeholder', () => {
    renderNode({ id: 'fan', kind: 'parallel', parallel: [] });
    expect(screen.getByText('no sub-steps')).toBeInTheDocument();
  });

  it('a panel node shows a gate block when a gate is set', () => {
    renderNode({
      id: 'rev',
      kind: 'panel',
      panel: {
        panelists: ['a', 'b'],
        subject: 's',
        gate: { until_no_findings_at_severity_or_above: 'high' },
      },
    });
    expect(screen.getByText('panel')).toBeInTheDocument();
    expect(screen.getByText('· 2 panelists')).toBeInTheDocument();
    expect(screen.getByText(/gate ≥ high/)).toBeInTheDocument();
  });

  it('renders the problem dot when problems are present', () => {
    renderNode({ id: 'x', kind: 'step' }, ['needs an agent']);
    expect(screen.getByLabelText('has problems')).toBeInTheDocument();
  });

  it('a for_each node shows the for_each expression', () => {
    renderNode({ id: 'each', kind: 'for_each', agent: 'a', for_each: 'inputs.files' });
    expect(screen.getByText(/for_each: inputs.files/)).toBeInTheDocument();
  });

  it('carries data-ui="next" on the outer node when workflowEditorUi is "next"', () => {
    const { container } = renderNode({ id: 'x', kind: 'step' }, [], false, 'next');
    expect(container.querySelector('[data-ui="next"]')).toBeInTheDocument();
  });

  it('defaults to data-ui="classic" when workflowEditorUi is unset', () => {
    const { container } = renderNode({ id: 'x', kind: 'step' });
    expect(container.querySelector('[data-ui="classic"]')).toBeInTheDocument();
  });
});
