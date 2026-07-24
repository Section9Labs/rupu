// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { ReactFlowProvider } from '@xyflow/react';
import StepNode from './StepNode';
import type { GraphNode } from '../../lib/runGraphModel';

function renderNode(node: Partial<GraphNode>, ui?: 'classic' | 'next') {
  const data = { node: { id: 'build', kind: 'step', state: 'running', ...node } as GraphNode, ui };
  return render(
    <ReactFlowProvider>
      {/* React Flow node components are called with (props); the harness in
          GateNode.test.tsx shows the exact prop shape — mirror it. */}
      <StepNode data={data} id="build" type="step" selected={false} zIndex={0} isConnectable={false}
        positionAbsoluteX={0} positionAbsoluteY={0} dragging={false}
        draggable={false} selectable deletable />
    </ReactFlowProvider>,
  );
}

afterEach(cleanup);

describe('StepNode', () => {
  it('classic: renders the step id and the neutral kind chip', () => {
    renderNode({}, 'classic');
    expect(screen.getByText('build')).toBeInTheDocument();
    expect(screen.getByText('step')).toBeInTheDocument();
  });

  it('next: keeps the status overlay legible on a FAILED step', () => {
    renderNode({ state: 'failed' }, 'next');
    // the status glyph + label must survive the kind repaint
    expect(screen.getByText('✕')).toBeInTheDocument();
    expect(screen.getByText('failed')).toBeInTheDocument();
    // and the kind identity is present
    expect(screen.getByText('step')).toBeInTheDocument();
  });

  it('next: renders a kind pill for the step kind', () => {
    renderNode({ kind: 'step' }, 'next');
    expect(screen.getByTestId('rg-kindpill')).toHaveTextContent('step');
  });
});
