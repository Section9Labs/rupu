// @vitest-environment jsdom
// Container run nodes take their KIND accent (parallel/panel/for_each each
// have a distinct token — see kindVisuals.KIND_ACCENT), set on the card frame
// as `--k` (the stylesheet tints the kind tile and label with it); the
// per-sub-step / per-unit chips stay state-colored (`data-state`).
import { nodeSize, PARALLEL_HEADER_H, PARALLEL_SUBROW_ID_H, PARALLEL_ROW_GAP } from '../../lib/nodeSize';
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

/** The kind channels the card frame paints with (`--k`). */
function kindVar(container: HTMLElement): string {
  return (container.querySelector('.gx-node') as HTMLElement).style.getPropertyValue('--k');
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
    expect(screen.getByText('fanwork')).toBeInTheDocument();
    expect(screen.getByText('parallel')).toBeInTheDocument();
  });

  it('the container tint uses the parallel kind accent (sev.critical)', () => {
    const { container } = renderParallel(PARALLEL);
    expect(kindVar(container)).toBe('var(--c-sev-critical)');
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
    expect(kindVar(container)).toBe('var(--c-status-awaiting)');
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
    expect(screen.getByText('shard')).toBeInTheDocument();
    expect(screen.getByLabelText('2 of 3 done')).toHaveTextContent('2/3');
    // one cell per unit, each carrying its own state
    const cells = Array.from(document.querySelectorAll('button.gx-cell'));
    expect(cells.map((c) => c.getAttribute('data-state'))).toEqual(['done', 'done', 'running']);
  });

  it('the container tint uses the for_each kind accent (brand.500)', () => {
    const { container } = renderFanout(FANOUT);
    expect(kindVar(container)).toBe('var(--c-brand-500)');
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
  it('summarises done / total, the percentage, and expands into the unit browser', () => {
    const { container } = renderFanout(FANOUT_LARGE);
    expect(screen.getByText('bigshard')).toBeInTheDocument();
    expect(screen.getByText('67%')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /expand all 15 · running \(3\)/ })).toBeInTheDocument();
    // the density preview carries one cell per unit, by state
    expect(container.querySelectorAll('.gx-mini-cell[data-state="done"]')).toHaveLength(10);
    expect(kindVar(container)).toBe('var(--c-brand-500)');
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
    expect(screen.getByText('heron.a · linter')).toBeInTheDocument();
    // provider/model on its own line
    expect(screen.getByText('anthropic/claude-opus-5-5')).toHaveAttribute('data-testid', 'agent-pm');
    // no codename → sub id row still renders, no identity
    expect(screen.getByText('test')).toBeInTheDocument();
    // layout reserves the identity rows for every sub-step that runs an agent
    expect(nodeSize(node).height).toBe(PARALLEL_HEADER_H + 2 * PARALLEL_SUBROW_ID_H + PARALLEL_ROW_GAP);
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
    // the unit row shows provider/model in full, on its own line
    expect(screen.getByText('openai/gpt-5')).toHaveAttribute('data-testid', 'agent-pm');
    expect(screen.getByText('lynx1 · sec-reviewer')).toBeInTheDocument();
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

// ---------------------------------------------------------------------------
// Step warnings — a header marker with the message, plus the warned unit's own
// square / row. Never a state change.
// ---------------------------------------------------------------------------

describe('step warnings on container nodes', () => {
  const W = { index: 1, message: 'host gpu-9 sent no coverage stream' };

  it('FanoutNode (inline) marks the header and the warned unit square, leaving unit states alone', () => {
    const node = {
      ...FANOUT,
      warnings: [W],
      fanout: {
        ...FANOUT.fanout!,
        units: FANOUT.fanout!.units.map((u) => (u.index === 1 ? { ...u, warnings: [W.message] } : u)),
      },
    } as GraphNode;
    const { container } = renderFanout(node);
    const mark = screen.getByTestId('rg-warn');
    expect(mark.getAttribute('title')).toBe('unit 1: host gpu-9 sent no coverage stream');
    // only unit 1's cell carries the warning ring + message
    const squares = Array.from(container.querySelectorAll('button.gx-cell'));
    expect(squares).toHaveLength(3);
    expect(squares[1].className).toContain('ring-warn');
    expect(squares[1].getAttribute('title')).toMatch(/done · ⚠ host gpu-9 sent no coverage stream$/);
    expect(squares[0].className).not.toContain('ring-warn');
    expect(squares[0].getAttribute('title')).not.toMatch(/⚠/);
    // the header counts are still the lifecycle's: 2 done, none failed
    expect(screen.getByLabelText('2 of 3 done')).toBeInTheDocument();
    expect(container.querySelector('.gx-failcount')).toBeNull();
  });

  it('FanoutNode (large) marks the header', () => {
    renderFanout({ ...FANOUT_LARGE, warnings: [W] } as GraphNode);
    expect(screen.getByTestId('rg-warn')).toHaveAttribute('aria-label', '1 warning');
  });

  it('FanoutNode with no units yet (placeholder card) still marks a warning', () => {
    const node = { id: 'shard', kind: 'for_each', state: 'running', warnings: [W] } as unknown as GraphNode;
    renderFanout(node);
    expect(screen.getByText('starting units…')).toBeInTheDocument();
    expect(screen.getByTestId('rg-warn')).toHaveAttribute('aria-label', '1 warning');
  });

  it('FanoutNode with no warnings renders no marker', () => {
    renderFanout(FANOUT);
    expect(screen.queryByTestId('rg-warn')).toBeNull();
  });

  it('PanelLoopNode marks the header and the warned unit row', () => {
    const node = {
      ...PANEL,
      warnings: [W],
      fanout: {
        ...PANEL.fanout!,
        units: PANEL.fanout!.units.map((u) => (u.index === 1 ? { ...u, warnings: [W.message] } : u)),
      },
    } as GraphNode;
    const { container } = renderPanel(node);
    const marks = screen.getAllByTestId('rg-warn');
    // one on the container header + one on bob's row; alice's row has none
    expect(marks).toHaveLength(2);
    const rows = Array.from(container.querySelectorAll('button'));
    expect(rows[1].getAttribute('title')).toMatch(/⚠ host gpu-9 sent no coverage stream$/);
    expect(rows[0].getAttribute('title')).not.toMatch(/⚠/);
  });

  it('ParallelNode marks the header', () => {
    renderParallel({ ...PARALLEL, warnings: [{ message: 'one sub-step lost its coverage' }] } as GraphNode);
    expect(screen.getByTestId('rg-warn').getAttribute('title')).toBe('one sub-step lost its coverage');
  });
});
