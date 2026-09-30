// @vitest-environment jsdom
// Container run nodes take their KIND accent (parallel/panel/for_each each
// have a distinct token — see kindVisuals.KIND_ACCENT); the per-sub-step /
// per-unit chips stay state-colored. The border-color assertions pin the
// exact color jsdom's CSSOM normalizes the seeded CSS_VARS below to
// (`rgb(<channels> / <alpha>)` inline style reads back as
// `rgba(<r>, <g>, <b>, <alpha>)`), since `useThemeColors` reads the raw
// channels directly off `document.documentElement`'s inline style (no
// var()-resolution involved).
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
) {
  const props = {
    id: node.id,
    data: { node },
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

function renderParallel(node: GraphNode) {
  return renderContainer(ParallelNode, 'parallel', node);
}

function renderPanel(node: GraphNode) {
  return renderContainer(PanelLoopNode, 'panel', node);
}

function renderFanout(node: GraphNode) {
  return renderContainer(FanoutNode, 'fanout', node);
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
  it('keeps the status channel — every sub-step chip still renders', () => {
    renderParallel(PARALLEL);
    expect(screen.getByText('lint')).toBeInTheDocument();
    expect(screen.getByText('test')).toBeInTheDocument();
    expect(screen.getByText(/parallel · fanwork/)).toBeInTheDocument();
  });

  it('the container tint uses the parallel kind accent (sev.critical)', () => {
    const { container } = renderParallel(PARALLEL);
    expect(containerBorderColor(container)).toBe('rgba(147, 51, 234, 0.4)');
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
  it('keeps the status channel — panelist chips and the loop cue still render', () => {
    renderPanel(PANEL);
    expect(screen.getByText('alice')).toBeInTheDocument();
    expect(screen.getByText('bob')).toBeInTheDocument();
    expect(screen.getByLabelText('looping')).toBeInTheDocument();
  });

  it('the container tint uses the panel kind accent (status.awaiting)', () => {
    const { container } = renderPanel(PANEL);
    expect(containerBorderColor(container)).toBe('rgba(245, 158, 11, 0.4)');
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
  it('keeps the status channel — every unit square still renders', () => {
    renderFanout(FANOUT);
    expect(screen.getByText(/for_each · shard · 3/)).toBeInTheDocument();
    expect(screen.getByText('2', { exact: false })).toBeInTheDocument();
  });

  it('the container tint uses the for_each kind accent (brand.500)', () => {
    const { container } = renderFanout(FANOUT);
    expect(containerBorderColor(container)).toBe('rgba(124, 58, 237, 0.4)');
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
  it('the header label, the %, and the expand-all button all take the same for_each kind accent', () => {
    renderFanout(FANOUT_LARGE);
    const header = screen.getByText(/for_each · bigshard/);
    const headerColor = getComputedStyle(header).color;
    const pct = screen.getByText('67%');
    const pctColor = getComputedStyle(pct).color;
    const button = screen.getByRole('button', { name: /expand all/ });
    const buttonColor = getComputedStyle(button).color;

    expect(headerColor).toBe(pctColor);
    expect(pctColor).toBe(buttonColor);
  });
});

// ---------------------------------------------------------------------------
// Codenames (agent codenames Plan 2, Task 5)
// ---------------------------------------------------------------------------

describe('codenames on container nodes', () => {
  it('FanoutNode (inline) header shows role badge + role word + ×N; unit titles carry the leaf · agent · provider/model', () => {
    const node = {
      ...FANOUT,
      agent: 'scanner',
      fanout: {
        ...FANOUT.fanout!,
        units: FANOUT.fanout!.units.map((u) => ({
          ...u,
          codename: `jade-reef/heron#${u.index}`,
          ...(u.index === 0 ? { provider: 'anthropic', model: 'claude-opus-5-5' } : {}),
        })),
      },
    } as GraphNode;
    const { container } = renderFanout(node);
    const role = screen.getByTestId('rg-fanout-role');
    expect(role).toHaveTextContent('heron ×3');
    expect(role.querySelector('svg[data-shape]')).not.toBeNull();
    expect(container.querySelector('button[title^="heron#0 · scanner · anthropic/claude-opus-5-5"]')).not.toBeNull();
    // placed unit (no provider/model) → just leaf · agent
    expect(container.querySelector('button[title^="heron#1 · scanner ·"]')).not.toBeNull();
  });

  it('FanoutNode (large) header shows role + ×N and no per-unit names', () => {
    const node = {
      ...FANOUT_LARGE,
      fanout: {
        ...FANOUT_LARGE.fanout!,
        units: FANOUT_LARGE.fanout!.units.map((u) => ({ ...u, codename: `jade-reef/heron#${u.index}` })),
      },
    } as GraphNode;
    renderFanout(node);
    expect(screen.getByTestId('rg-fanout-role')).toHaveTextContent('heron ×15');
    expect(screen.queryByText(/heron#3/)).toBeNull();
  });

  it('FanoutNode without codenames renders no role header', () => {
    renderFanout(FANOUT);
    expect(screen.queryByTestId('rg-fanout-role')).toBeNull();
  });

  it('ParallelNode rows show the sub-step codename · agent · provider/model', () => {
    const node = {
      ...PARALLEL,
      parallel: [
        { id: 'lint', state: 'done', agent: 'linter', codename: 'jade-reef/heron.a', provider: 'anthropic', model: 'claude-opus-5-5' },
        { id: 'test', state: 'running', agent: 'tester' },
      ],
    } as GraphNode;
    renderParallel(node);
    expect(screen.getByText('lint')).toBeInTheDocument();
    expect(screen.getByText('heron.a · linter · anthropic/claude-opus-5-5')).toBeInTheDocument();
    // no codename → sub id row still renders, no identity
    expect(screen.getByText('test')).toBeInTheDocument();
  });

  it('PanelLoopNode unit title uses the unit agent, never the unit key', () => {
    const node = {
      ...PANEL,
      fanout: {
        ...PANEL.fanout!,
        units: [
          { index: 0, key: 'alice', state: 'done', codename: 'jade-reef/lynx1', agent: 'sec-reviewer', provider: 'openai', model: 'gpt-5' },
          { index: 1, key: 'bob', state: 'running', codename: 'jade-reef/lynx2' },
        ],
      },
    } as GraphNode;
    const { container } = renderPanel(node);
    expect(container.querySelector('button[title^="lynx1 · sec-reviewer · openai/gpt-5"]')).not.toBeNull();
    expect(container.querySelector('button[title^="lynx2 · running"]')).not.toBeNull();
    expect(container.querySelector('button[title*="bob"]')).toBeNull();
  });

  it('PanelLoopNode unit chips use the codename leaf when present', () => {
    const node = {
      ...PANEL,
      fanout: {
        ...PANEL.fanout!,
        units: PANEL.fanout!.units.map((u, i) => ({ ...u, codename: `jade-reef/lynx${i + 1}` })),
      },
    } as GraphNode;
    renderPanel(node);
    expect(screen.getByText('lynx1')).toBeInTheDocument();
  });
});
