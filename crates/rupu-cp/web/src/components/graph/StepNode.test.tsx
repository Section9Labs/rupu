// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { ReactFlowProvider } from '@xyflow/react';
import StepNode from './StepNode';
import type { GraphNode } from '../../lib/runGraphModel';
import { nodeSize, STEP_AGENT_H, STEP_H } from '../../lib/nodeSize';

function renderNode(node: Partial<GraphNode>) {
  const data = { node: { id: 'build', kind: 'step', state: 'running', ...node } as GraphNode };
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
  it('keeps the status overlay legible on a FAILED step', () => {
    renderNode({ state: 'failed' });
    // the status glyph + label must survive the kind repaint
    expect(screen.getByText('✕')).toBeInTheDocument();
    expect(screen.getByText('failed')).toBeInTheDocument();
    // and the kind identity is present
    expect(screen.getByText('step')).toBeInTheDocument();
  });

  it('renders a kind pill for the step kind', () => {
    renderNode({ kind: 'step' });
    expect(screen.getByTestId('rg-kindpill')).toHaveTextContent('step');
  });

  it('shows codename · agent, with provider/model on its own line', () => {
    renderNode({ agent: 'security-reviewer', codename: 'jade-reef/heron', provider: 'anthropic', model: 'claude-opus-5-5' });
    expect(screen.getByTestId('agent-head')).toHaveTextContent(/^heron · security-reviewer$/);
    // provider/model is its OWN element (not a clipped tail of the name line)
    const pm = screen.getByTestId('agent-pm');
    expect(pm).toHaveTextContent(/^anthropic\/claude-opus-5-5$/);
    expect(pm.contains(screen.getByTestId('agent-head'))).toBe(false);
    // hover shows the full identity
    expect(screen.getByTestId('agent-identity')).toHaveAttribute(
      'title',
      'jade-reef/heron · security-reviewer · anthropic/claude-opus-5-5',
    );
  });

  it('reserves the taller agent box so the identity rows fit', () => {
    const { container } = renderNode({ agent: 'security-reviewer', codename: 'jade-reef/heron' });
    const root = container.querySelector('.shadow-card') as HTMLElement;
    expect(root.style.minHeight).toBe(`${STEP_AGENT_H}px`);
    expect(nodeSize({ id: 'x', kind: 'step', state: 'pending', agent: 'a' } as GraphNode).height).toBe(STEP_AGENT_H);
    expect(nodeSize({ id: 'x', kind: 'step', state: 'pending' } as GraphNode).height).toBe(STEP_H);
  });

  it('keeps the plain agent name without a codename', () => {
    renderNode({ agent: 'security-reviewer' });
    expect(screen.getByText('security-reviewer')).toBeInTheDocument();
    expect(screen.queryByTestId('agent-pm')).toBeNull();
  });

  it('renders a derived (legacy) codename muted, with the derived tooltip', () => {
    renderNode({ agent: 'security-reviewer', codename: 'jade-reef/heron', codenameDerived: true });
    const ident = screen.getByTestId('agent-identity');
    expect(ident).toHaveClass('opacity-60');
    expect(ident.getAttribute('title')).toMatch(/derived for a run recorded before codenames/);
  });

  it('renders a stored codename at full strength', () => {
    renderNode({ agent: 'security-reviewer', codename: 'jade-reef/heron' });
    expect(screen.getByTestId('agent-identity')).not.toHaveClass('opacity-60');
  });
});
