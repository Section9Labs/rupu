// @vitest-environment jsdom
// LoopGroupNode — the dashed loop group-boundary card (Phase 3 Task 5).
// Same jsdom/@xyflow/react-mock posture as EditableStepNode.test.tsx: the
// real canvas can't mount under jsdom, so this renders the card component
// directly with hand-built NodeProps.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import type { Node, NodeProps } from '@xyflow/react';

vi.mock('@xyflow/react', () => ({
  Handle: () => null,
  Position: { Top: 'top', Bottom: 'bottom', Left: 'left', Right: 'right' },
}));

import LoopGroupNode, { loopGroupBox, type LoopGroupNodeData } from './LoopGroupNode';
import type { GraphNode, WorkflowLoop } from '../../../lib/workflowGraph';

afterEach(cleanup);

function renderGroup(data: LoopGroupNodeData) {
  const props = { data } as unknown as NodeProps<Node<LoopGroupNodeData, 'loopGroup'>>;
  return render(<LoopGroupNode {...props} />);
}

const REFINE: WorkflowLoop = {
  name: 'refine',
  nodes: ['gen', 'test', 'critique'],
  until: '{{ steps.critique.approved }}',
  maxIterations: 5,
  onMax: 'fail',
};

describe('LoopGroupNode', () => {
  it('shows the loop name and a truncated until/max badge', () => {
    renderGroup({ loop: REFINE, width: 400, height: 200, problems: [] });
    expect(screen.getByText('refine')).toBeInTheDocument();
    expect(screen.getByText((t) => t.includes('≤5'))).toBeInTheDocument();
  });

  it('renders an error indicator when the loop has inline validation problems', () => {
    renderGroup({ loop: REFINE, width: 400, height: 200, problems: ["step 'gen' also belongs to loop 'x'"] });
    expect(screen.getByLabelText('loop has problems')).toBeInTheDocument();
  });

  it('has no error indicator when clean', () => {
    renderGroup({ loop: REFINE, width: 400, height: 200, problems: [] });
    expect(screen.queryByLabelText('loop has problems')).not.toBeInTheDocument();
  });

  it('renders the runtime overlay (iteration/max + converged) when provided', () => {
    renderGroup({
      loop: REFINE,
      width: 400,
      height: 200,
      problems: [],
      runtime: { iteration: 2, converged: true, failed: false },
    });
    expect(screen.getByText((t) => t.includes('iteration 2/5') && t.includes('converged'))).toBeInTheDocument();
  });

  it('renders no runtime overlay when absent (the static-only, no-run-open case)', () => {
    renderGroup({ loop: REFINE, width: 400, height: 200, problems: [] });
    expect(screen.queryByText(/iteration/)).not.toBeInTheDocument();
  });

  it('clicking the label invokes onEdit', () => {
    const onEdit = vi.fn();
    renderGroup({ loop: REFINE, width: 400, height: 200, problems: [], onEdit });
    screen.getByText('refine').click();
    expect(onEdit).toHaveBeenCalledTimes(1);
  });
});

describe('loopGroupBox', () => {
  const node = (id: string, x: number, y: number): GraphNode => ({
    id,
    data: { id, kind: 'step', agent: 'a', prompt: 'p' },
    position: { x, y },
  });

  it('returns null when no member resolves to a real node', () => {
    expect(loopGroupBox([node('a', 0, 0)], { ...REFINE, nodes: ['ghost'] })).toBeNull();
  });

  it('covers every member node with padding', () => {
    const nodes = [node('gen', 0, 0), node('test', 300, 0), node('critique', 150, 200)];
    const box = loopGroupBox(nodes, REFINE);
    expect(box).not.toBeNull();
    if (!box) return;
    // Every member's box must be fully inside the loop's bounding box.
    for (const n of nodes) {
      expect(n.position.x).toBeGreaterThanOrEqual(box.x);
      expect(n.position.y).toBeGreaterThanOrEqual(box.y);
    }
  });
});
