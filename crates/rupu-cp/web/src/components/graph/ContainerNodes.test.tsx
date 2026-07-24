// @vitest-environment jsdom
// Container run nodes take their KIND accent in next; the per-sub-step /
// per-unit chips stay state-colored. We assert the container tint CHANGES
// between classic and next rather than pinning a hex (the token resolves
// through CSS vars that jsdom does not compute) — the point is that kind now
// drives it, and that the status channel survives.
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { ReactFlowProvider, type NodeProps } from '@xyflow/react';
import ParallelNode from './ParallelNode';
import PanelLoopNode from './PanelLoopNode';
import FanoutNode from './FanoutNode';
import type { GraphNode } from '../../lib/runGraphModel';

afterEach(cleanup);

// RunGraph paints these nodes via `useThemeColors`, which reads real CSS
// custom properties off `document.documentElement` (see
// `RunGraph.edges.test.tsx`'s established fixture pattern). jsdom has no
// stylesheet loaded in this test file, so every token would otherwise
// collapse to the SAME '0 0 0' fallback — making the "must differ"
// assertions below meaningless. Seed the real light-theme values the three
// containers' tint tokens resolve through.
const CSS_VARS: Record<string, string> = {
  '--c-brand-500': '124 58 237',
  '--c-sev-critical': '147 51 234',
  '--c-status-running': '59 130 246',
  '--c-status-awaiting': '245 158 11',
  '--c-status-done': '34 197 94',
  '--c-status-failed': '239 68 68',
  '--c-status-pending': '148 163 184',
  '--c-status-paused': '6 182 212',
  '--c-status-skipped': '203 213 225',
};

for (const [k, v] of Object.entries(CSS_VARS)) {
  document.documentElement.style.setProperty(k, v);
}

// Same harness shape as GateNode.test.tsx: `Handle` needs a provider ancestor.
function renderContainer(
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  Component: React.ComponentType<NodeProps<any>>,
  type: string,
  node: GraphNode,
  ui?: 'classic' | 'next',
) {
  const props = {
    id: node.id,
    data: { node, ui },
    type,
    dragging: false,
    zIndex: 0,
    selectable: true,
    deletable: true,
    selected: false,
    draggable: false,
    isConnectable: true,
    positionAbsoluteX: 0,
    positionAbsoluteY: 0,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  } as unknown as NodeProps<any>;
  return render(
    <ReactFlowProvider>
      <Component {...props} />
    </ReactFlowProvider>,
  );
}

function renderParallel(node: GraphNode, ui?: 'classic' | 'next') {
  return renderContainer(ParallelNode, 'parallel', node, ui);
}

function renderPanel(node: GraphNode, ui?: 'classic' | 'next') {
  return renderContainer(PanelLoopNode, 'panel', node, ui);
}

function renderFanout(node: GraphNode, ui?: 'classic' | 'next') {
  return renderContainer(FanoutNode, 'fanout', node, ui);
}

function containerBorderColor(container: HTMLElement): string {
  return (container.querySelector('[data-testid="rg-container"]') as HTMLElement).style.borderColor;
}

const PARALLEL: GraphNode = {
  id: 'fanwork',
  kind: 'parallel',
  state: 'running',
  parallel: [
    { id: 'lint', state: 'done' },
    { id: 'test', state: 'running' },
  ],
} as unknown as GraphNode;

describe('ParallelNode', () => {
  it('next: keeps the status channel — every sub-step chip still renders', () => {
    renderParallel(PARALLEL, 'next');
    expect(screen.getByText('lint')).toBeInTheDocument();
    expect(screen.getByText('test')).toBeInTheDocument();
    expect(screen.getByText(/parallel · fanwork/)).toBeInTheDocument();
  });

  it('next: the container tint switches from the legacy brand violet to the parallel kind accent', () => {
    const classic = renderParallel(PARALLEL, 'classic');
    const classicTint = containerBorderColor(classic.container);
    cleanup();
    const next = renderParallel(PARALLEL, 'next');
    const nextTint = containerBorderColor(next.container);
    expect(nextTint).not.toBe(classicTint);
  });
});

const PANEL: GraphNode = {
  id: 'review',
  kind: 'panel',
  state: 'running',
  round: { current: 2, max: 5 },
  gate: { until_severity: 'high', max_iterations: 5 },
  fanout: {
    total: 2,
    byState: { pending: 0, running: 1, awaiting_approval: 0, paused: 0, done: 1, failed: 0, skipped: 0 },
    units: [
      { index: 0, key: 'alice', state: 'done' },
      { index: 1, key: 'bob', state: 'running' },
    ],
  },
} as unknown as GraphNode;

describe('PanelLoopNode', () => {
  it('next: keeps the status channel — panelist chips and the loop cue still render', () => {
    renderPanel(PANEL, 'next');
    expect(screen.getByText('alice')).toBeInTheDocument();
    expect(screen.getByText('bob')).toBeInTheDocument();
    expect(screen.getByLabelText('looping')).toBeInTheDocument();
  });

  it('next: the container tint switches from the legacy brand violet to the panel kind accent', () => {
    const classic = renderPanel(PANEL, 'classic');
    const classicTint = containerBorderColor(classic.container);
    cleanup();
    const next = renderPanel(PANEL, 'next');
    const nextTint = containerBorderColor(next.container);
    expect(nextTint).not.toBe(classicTint);
  });
});

const FANOUT: GraphNode = {
  id: 'shard',
  kind: 'for_each',
  state: 'running',
  fanout: {
    total: 3,
    byState: { pending: 0, running: 1, awaiting_approval: 0, paused: 0, done: 2, failed: 0, skipped: 0 },
    units: [
      { index: 0, key: 'a', state: 'done' },
      { index: 1, key: 'b', state: 'done' },
      { index: 2, key: 'c', state: 'running' },
    ],
  },
} as unknown as GraphNode;

describe('FanoutNode', () => {
  it('next: keeps the status channel — every unit square still renders', () => {
    renderFanout(FANOUT, 'next');
    expect(screen.getByText(/for_each · shard · 3/)).toBeInTheDocument();
    expect(screen.getByText('2', { exact: false })).toBeInTheDocument();
  });

  it('next: the container tint switches from the legacy running-blue to the for_each kind accent', () => {
    const classic = renderFanout(FANOUT, 'classic');
    const classicTint = containerBorderColor(classic.container);
    cleanup();
    const next = renderFanout(FANOUT, 'next');
    const nextTint = containerBorderColor(next.container);
    expect(nextTint).not.toBe(classicTint);
  });
});

// total > FANOUT_INLINE_THRESHOLD (12) takes the LARGE collapsed-card branch
// instead of the inline unit grid.
const FANOUT_LARGE: GraphNode = {
  id: 'bigshard',
  kind: 'for_each',
  state: 'running',
  fanout: {
    total: 15,
    byState: { pending: 2, running: 3, awaiting_approval: 0, paused: 0, done: 10, failed: 0, skipped: 0 },
    units: Array.from({ length: 15 }, (_, i) => ({
      index: i,
      key: `u${i}`,
      state: i < 10 ? 'done' : i < 13 ? 'running' : 'pending',
    })),
  },
} as unknown as GraphNode;

describe('FanoutNode (large card, total > 12)', () => {
  it('next: the header label, the %, and the expand-all button take the for_each kind accent, not the legacy running-blue', () => {
    renderFanout(FANOUT_LARGE, 'classic');
    const classicHeader = screen.getByText(/for_each · bigshard/);
    const classicHeaderColor = getComputedStyle(classicHeader).color;
    const classicPct = screen.getByText('67%');
    const classicPctColor = getComputedStyle(classicPct).color;
    const classicButton = screen.getByRole('button', { name: /expand all/ });
    const classicButtonColor = getComputedStyle(classicButton).color;
    cleanup();

    renderFanout(FANOUT_LARGE, 'next');
    const nextHeader = screen.getByText(/for_each · bigshard/);
    const nextHeaderColor = getComputedStyle(nextHeader).color;
    const nextPct = screen.getByText('67%');
    const nextPctColor = getComputedStyle(nextPct).color;
    const nextButton = screen.getByRole('button', { name: /expand all/ });
    const nextButtonColor = getComputedStyle(nextButton).color;

    expect(nextHeaderColor).not.toBe(classicHeaderColor);
    expect(nextPctColor).not.toBe(classicPctColor);
    expect(nextButtonColor).not.toBe(classicButtonColor);
  });
});
