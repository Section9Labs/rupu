// @vitest-environment jsdom
// WorkflowEditorGraph — palette wiring (rendered) + the exported pure mutation
// helpers (applyConnect / applyDelete / applyAddNode).
//
// @xyflow/react is mocked: the real canvas depends on ResizeObserver and layout
// APIs jsdom lacks, and its drag/connect behavior isn't what we're verifying
// here — the editor's mutation logic lives in the exported pure helpers, which we
// test directly. The mock is a thin stub (ReactFlow renders its children, the
// chrome components render nothing) so the palette/toolbar DOM still mounts.

import '@testing-library/jest-dom/vitest';
import type { ReactNode } from 'react';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, act } from '@testing-library/react';

// Captures the `onEdgesChange` callback ReactFlow's mock last received so
// tests can invoke it directly to simulate xyflow's own select/remove
// EdgeChange events (real @xyflow/react can't mount under jsdom — no
// ResizeObserver/layout APIs — so this is the only way to exercise the
// component's onEdgesChange handler). `vi.hoisted` so the same object is
// visible both inside the mock factory (hoisted above imports by vi.mock)
// and in the test bodies below.
const rfCapture = vi.hoisted(() => ({
  onEdgesChange: undefined as ((changes: unknown[]) => void) | undefined,
}));

// ReactFlow's mock also serializes the `edges` prop it received into a data
// attribute (JSON) so tests can assert on label/color without mounting the
// real canvas — the mutation/derivation logic under test lives in the
// component's `edges` useMemo, not in @xyflow/react itself.
vi.mock('@xyflow/react', () => ({
  ReactFlow: ({
    children,
    edges,
    nodes,
    onEdgesChange,
  }: {
    children?: ReactNode;
    edges?: unknown[];
    nodes?: unknown[];
    onEdgesChange?: (changes: unknown[]) => void;
  }) => {
    rfCapture.onEdgesChange = onEdgesChange;
    return (
      <div data-testid="rf" data-edges={JSON.stringify(edges ?? [])} data-nodes={JSON.stringify(nodes ?? [])}>
        {children}
      </div>
    );
  },
  ReactFlowProvider: ({ children }: { children?: ReactNode }) => <>{children}</>,
  // Serializes the props Background received into data attributes so tests can
  // assert on variant/gap without mounting the real canvas-pattern renderer.
  Background: ({
    variant,
    gap,
    lineWidth,
    size,
    color,
  }: {
    variant?: string;
    gap?: number;
    lineWidth?: number;
    size?: number;
    color?: string;
  }) => (
    <div
      data-testid="bg"
      data-variant={variant}
      data-gap={gap}
      data-linewidth={lineWidth}
      data-size={size}
      data-color={color}
    />
  ),
  Controls: () => null,
  MiniMap: () => null,
  Handle: () => null,
  Position: { Top: 'top', Bottom: 'bottom', Left: 'left', Right: 'right' },
  MarkerType: { ArrowClosed: 'arrowclosed' },
  BackgroundVariant: { Dots: 'dots', Lines: 'lines', Cross: 'cross' },
  applyNodeChanges: (_changes: unknown, nodes: unknown) => nodes,
  useReactFlow: () => ({
    screenToFlowPosition: (p: { x: number; y: number }) => p,
    fitView: () => {},
  }),
}));

// useThemeColors reads CSS custom properties via getComputedStyle, which
// resolves to nothing under jsdom (no stylesheet loaded) — every token would
// collapse to the same fallback and the true/false edge colors would be
// indistinguishable. Mock it with fixed, distinct RGB strings so the branch
// edge-color assertions below are meaningful. `get`/`alpha` are KEY-AWARE
// (echo the key into the string) so kind-accent-driven styling (Task 1 round
// 2) is assertable per StepKind without needing real CSS.
vi.mock('../../lib/useThemeColors', () => ({
  useThemeColors: () => ({
    bg: 'rgb(0 0 0)',
    panel: 'rgb(0 0 0)',
    surface: 'rgb(0 0 0)',
    surfaceHover: 'rgb(0 0 0)',
    border: 'rgb(0 0 0)',
    ink: 'rgb(0 0 0)',
    inkDim: 'rgb(0 0 0)',
    inkMute: 'rgb(0 0 0)',
    brand: { 500: 'rgb(0 0 0)', 600: 'rgb(0 0 0)', 700: 'rgb(0 0 0)' },
    status: {
      running: 'rgb(0 0 0)',
      done: 'rgb(34 197 94)',
      completed: 'rgb(0 0 0)',
      failed: 'rgb(239 68 68)',
      awaiting: 'rgb(0 0 0)',
      paused: 'rgb(0 0 0)',
      pending: 'rgb(0 0 0)',
      skipped: 'rgb(0 0 0)',
      cancelled: 'rgb(0 0 0)',
      rejected: 'rgb(0 0 0)',
    },
    sev: { critical: 'rgb(0 0 0)', high: 'rgb(0 0 0)', medium: 'rgb(0 0 0)', low: 'rgb(0 0 0)', info: 'rgb(0 0 0)' },
    info: 'rgb(0 0 0)',
    get: (key: string) => `rgb(${key})`,
    alpha: (key: string, a: number) => `rgb(${key} / ${a})`,
  }),
}));

import WorkflowEditorGraph, {
  applyConnect,
  applyDelete,
  applyRemoveEdges,
  applyAddNode,
  applyAddNodeAt,
  applyAddConnectedNext,
  applyInsertOnEdge,
  asStepKind,
} from './WorkflowEditorGraph';
import { deriveEdges, hasExplicitEdges, yamlToGraph } from '../../lib/workflowGraph';
import type { WorkflowGraph } from '../../lib/workflowGraph';
import { KIND_ACCENT } from './kindVisuals';

afterEach(cleanup);

function makeGraph(): WorkflowGraph {
  return {
    nodes: [
      { id: 'a', data: { id: 'a', kind: 'step', agent: 'x', prompt: 'p' }, position: { x: 0, y: 0 } },
      { id: 'b', data: { id: 'b', kind: 'step', agent: 'y', prompt: 'q' }, position: { x: 0, y: 100 } },
    ],
    edges: [{ id: 'a->b', source: 'a', target: 'b' }],
    meta: { name: 'wf', rest: {} },
  };
}

describe('palette', () => {
  it('clicking a palette card adds one node and selects the new id', () => {
    const onChange = vi.fn();
    const onSelect = vi.fn();
    render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={onChange}
        selectedId={null}
        onSelect={onSelect}
        problemsById={{}}
        onInvalidConnection={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Add step node' }));

    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0][0] as WorkflowGraph;
    expect(next.nodes).toHaveLength(3);
    const newId = next.nodes[2].id;
    expect(onSelect).toHaveBeenCalledWith(newId);
  });

  it('⊕ next is disabled with no selection and enabled with one', () => {
    const { rerender } = render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
      />,
    );
    expect(screen.getByRole('button', { name: '⊕ next' })).toBeDisabled();

    rerender(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId="a"
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
      />,
    );
    expect(screen.getByRole('button', { name: '⊕ next' })).toBeEnabled();
  });
});

describe('node projection', () => {
  function projectedNodes(workflowEditorUi?: 'classic' | 'next') {
    render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi={workflowEditorUi}
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-nodes');
    expect(raw).toBeTruthy();
    return JSON.parse(raw!) as Array<{ data: { workflowEditorUi?: string } }>;
  }

  it('threads workflowEditorUi="next" onto every projected node', () => {
    const nodes = projectedNodes('next');
    expect(nodes.length).toBeGreaterThan(0);
    for (const n of nodes) expect(n.data.workflowEditorUi).toBe('next');
  });

  it('defaults to workflowEditorUi="classic" when the prop is unset', () => {
    const nodes = projectedNodes(undefined);
    expect(nodes.length).toBeGreaterThan(0);
    for (const n of nodes) expect(n.data.workflowEditorUi).toBe('classic');
  });
});

describe('canvas backdrop (Task 3)', () => {
  function bgAttrs(workflowEditorUi?: 'classic' | 'next') {
    render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi={workflowEditorUi}
      />,
    );
    const bg = screen.getByTestId('bg');
    return { variant: bg.getAttribute('data-variant'), gap: bg.getAttribute('data-gap') };
  }

  it('classic renders a Dots background with gap 16 (unchanged)', () => {
    const { variant, gap } = bgAttrs('classic');
    expect(variant).toBe('dots');
    expect(gap).toBe('16');
  });

  it('defaults to Dots/gap 16 when the flag is unset', () => {
    const { variant, gap } = bgAttrs(undefined);
    expect(variant).toBe('dots');
    expect(gap).toBe('16');
  });

  it('next renders a Lines background with gap 28', () => {
    const { variant, gap } = bgAttrs('next');
    expect(variant).toBe('lines');
    expect(gap).toBe('28');
  });

  it('applies the wfx-canvas radial-wash class to the canvas container only when next', () => {
    const { container, rerender } = render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="classic"
      />,
    );
    expect(container.querySelector('.wfx-canvas')).not.toBeInTheDocument();

    rerender(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="next"
      />,
    );
    expect(container.querySelector('.wfx-canvas')).toBeInTheDocument();
  });
});

describe('palette container / portal (Task 1: inspector-rail dock)', () => {
  it('classic ignores paletteContainer — floating dock renders inline, container stays empty', () => {
    const portalHost = document.createElement('div');
    document.body.appendChild(portalHost);
    const { container } = render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="classic"
        paletteContainer={portalHost}
      />,
    );
    expect(screen.getByRole('button', { name: 'Add step node' })).toBeInTheDocument();
    expect(container.querySelector('.wfx-palette-rail')).not.toBeInTheDocument();
    expect(portalHost.children.length).toBe(0);
    document.body.removeChild(portalHost);
  });

  it('next + paletteContainer set: the palette portals into the container as variant="rail", not the canvas', () => {
    const portalHost = document.createElement('div');
    document.body.appendChild(portalHost);
    const { container } = render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="next"
        paletteContainer={portalHost}
      />,
    );
    expect(portalHost.querySelector('.wfx-palette-rail')).toBeInTheDocument();
    expect(container.querySelector('.wfx-palette-rail')).not.toBeInTheDocument();
    expect(container.querySelector('.wfx-palette')).not.toBeInTheDocument();
    document.body.removeChild(portalHost);
  });

  it('next + no paletteContainer: renders nothing for the palette that frame (no flash of the floating dock)', () => {
    const { container } = render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="next"
      />,
    );
    expect(container.querySelector('.wfx-palette')).not.toBeInTheDocument();
    expect(container.querySelector('.wfx-palette-rail')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Add step node' })).not.toBeInTheDocument();
  });
});

describe('applyConnect', () => {
  it('valid connection emits onChange with the new edge, not onInvalid', () => {
    const onChange = vi.fn();
    const onInvalid = vi.fn();
    const g = makeGraph();
    g.nodes.push({ id: 'c', data: { id: 'c', kind: 'step' }, position: { x: 0, y: 200 } });

    applyConnect(g, { source: 'b', target: 'c' }, onChange, onInvalid);

    expect(onInvalid).not.toHaveBeenCalled();
    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0][0] as WorkflowGraph;
    expect(next.edges).toContainEqual({ id: 'b->c', source: 'b', target: 'c' });
  });

  it('self-loop is rejected with a reason, no onChange', () => {
    const onChange = vi.fn();
    const onInvalid = vi.fn();
    applyConnect(makeGraph(), { source: 'a', target: 'a' }, onChange, onInvalid);
    expect(onChange).not.toHaveBeenCalled();
    expect(onInvalid).toHaveBeenCalledWith(expect.stringContaining('itself'));
  });

  it('cycle is rejected with a reason, no onChange', () => {
    const onChange = vi.fn();
    const onInvalid = vi.fn();
    // a->b already exists; b->a would close a cycle.
    applyConnect(makeGraph(), { source: 'b', target: 'a' }, onChange, onInvalid);
    expect(onChange).not.toHaveBeenCalled();
    expect(onInvalid).toHaveBeenCalledWith(expect.stringContaining('cycle'));
  });

  // Was "duplicate edge is rejected with a reason, no onChange" under the old
  // reorder model, where redrawing an already-adjacent pair was a pointless
  // no-op worth flagging. Task 5: a plain connect that only fails as a
  // "duplicate" is let through (`plainConnectAllowed`) — under the
  // derived-edges model an already-adjacent pair already carries an
  // unlabeled chain edge, indistinguishable from an explicit `next` at the
  // `edges`-array level, so redrawing it is exactly how you turn an implicit
  // chain edge into an explicit one. `setNext` is idempotent for the same
  // target either way.
  it('redrawing an already-connected pair sets an explicit `next` instead of being rejected as a duplicate', () => {
    const onChange = vi.fn();
    const onInvalid = vi.fn();
    applyConnect(makeGraph(), { source: 'a', target: 'b' }, onChange, onInvalid);
    expect(onInvalid).not.toHaveBeenCalled();
    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0][0] as WorkflowGraph;
    expect(next.nodes.find((n) => n.id === 'a')!.data.next).toEqual(['b']);
  });

  it('missing endpoint is a no-op', () => {
    const onChange = vi.fn();
    const onInvalid = vi.fn();
    applyConnect(makeGraph(), { source: null, target: 'b' }, onChange, onInvalid);
    expect(onChange).not.toHaveBeenCalled();
    expect(onInvalid).not.toHaveBeenCalled();
  });

  describe('drawn from a branch node arm handle', () => {
    function graphWithBranch(): WorkflowGraph {
      const g = makeGraph();
      g.nodes.push({
        id: 'br',
        data: { id: 'br', kind: 'branch', condition: 'inputs.ok' },
        position: { x: 0, y: 0 },
      });
      return g;
    }

    it('sourceHandle "else" from a branch node: edge carries branch/label + else target appended', () => {
      const onChange = vi.fn();
      const onInvalid = vi.fn();
      const g = graphWithBranch();

      applyConnect(g, { source: 'br', target: 'a', sourceHandle: 'else' }, onChange, onInvalid);

      expect(onInvalid).not.toHaveBeenCalled();
      expect(onChange).toHaveBeenCalledTimes(1);
      const next = onChange.mock.calls[0][0] as WorkflowGraph;
      expect(next.edges).toContainEqual({
        id: 'br->a:else',
        source: 'br',
        target: 'a',
        branch: 'else',
        label: 'false',
      });
      const br = next.nodes.find((n) => n.id === 'br');
      expect(br?.data.elseTargets).toEqual(['a']);
      expect(br?.data.thenTargets ?? []).toEqual([]);
    });

    it('sourceHandle "then" from a branch node: edge carries branch/label + then target appended', () => {
      const onChange = vi.fn();
      const onInvalid = vi.fn();
      const g = graphWithBranch();

      applyConnect(g, { source: 'br', target: 'b', sourceHandle: 'then' }, onChange, onInvalid);

      expect(onChange).toHaveBeenCalledTimes(1);
      const next = onChange.mock.calls[0][0] as WorkflowGraph;
      expect(next.edges).toContainEqual({
        id: 'br->b:then',
        source: 'br',
        target: 'b',
        branch: 'then',
        label: 'true',
      });
      const br = next.nodes.find((n) => n.id === 'br');
      expect(br?.data.thenTargets).toEqual(['b']);
    });

    it('does not duplicate the target if the arm list already contains it', () => {
      const onChange = vi.fn();
      const onInvalid = vi.fn();
      const g = graphWithBranch();
      const br = g.nodes.find((n) => n.id === 'br')!;
      br.data.thenTargets = ['b'];

      applyConnect(g, { source: 'br', target: 'b', sourceHandle: 'then' }, onChange, onInvalid);

      const next = onChange.mock.calls[0][0] as WorkflowGraph;
      expect(next.nodes.find((n) => n.id === 'br')?.data.thenTargets).toEqual(['b']);
    });

    it('a non-branch source with a "then"/"else"-shaped handle id is NOT treated as an arm (plain edge)', () => {
      const onChange = vi.fn();
      const onInvalid = vi.fn();
      const g = makeGraph(); // 'a' is a plain `step` node, not `branch`

      applyConnect(g, { source: 'a', target: 'b', sourceHandle: 'then' }, onChange, onInvalid);

      // Proof the arm branch was NOT taken: the result sets 'a'.data.next
      // (the plain-connect path), NOT a `br->b:then`-shaped edge/thenTargets
      // entry (which the arm path would have produced instead).
      expect(onInvalid).not.toHaveBeenCalled();
      expect(onChange).toHaveBeenCalledTimes(1);
      const next = onChange.mock.calls[0][0] as WorkflowGraph;
      expect(next.nodes.find((n) => n.id === 'a')!.data.next).toEqual(['b']);
      expect(next.edges).toContainEqual({ id: 'a->b', source: 'a', target: 'b' });
    });

    it('a branch source with no sourceHandle (or null) falls back to a plain edge, unchanged behavior', () => {
      const onChange = vi.fn();
      const onInvalid = vi.fn();
      const g = graphWithBranch();

      applyConnect(g, { source: 'br', target: 'a', sourceHandle: null }, onChange, onInvalid);

      expect(onChange).toHaveBeenCalledTimes(1);
      const next = onChange.mock.calls[0][0] as WorkflowGraph;
      expect(next.edges).toContainEqual({ id: 'br->a', source: 'br', target: 'a' });
      expect(next.nodes.find((n) => n.id === 'br')?.data.thenTargets ?? []).toEqual([]);
      expect(next.nodes.find((n) => n.id === 'br')?.data.elseTargets ?? []).toEqual([]);
    });

    it('end-to-end: the graph applyConnect produces, re-rendered, anchors at the drawn arm handle', () => {
      // Proves the two fixes compose: applyConnect tags the new edge with
      // `branch: 'else'`, and the edges memo derives `sourceHandle` from
      // `branch` alone — so a hand-drawn arm connection anchors correctly on
      // the very next render, with no extra field needed on GraphEdge.
      const onChange = vi.fn();
      const g = graphWithBranch();
      applyConnect(g, { source: 'br', target: 'a', sourceHandle: 'else' }, onChange, () => {});
      const next = onChange.mock.calls[0][0] as WorkflowGraph;

      render(
        <WorkflowEditorGraph
          graph={next}
          onChange={() => {}}
          selectedId={null}
          onSelect={() => {}}
          problemsById={{}}
          onInvalidConnection={() => {}}
        />,
      );
      const raw = screen.getByTestId('rf').getAttribute('data-edges');
      const edges = JSON.parse(raw!) as Array<{ id: string; sourceHandle?: string }>;
      expect(edges.find((e) => e.id === 'br->a:else')?.sourceHandle).toBe('else');
    });
  });
});

describe('applyDelete', () => {
  it('removes the node and every edge touching it', () => {
    const next = applyDelete(makeGraph(), 'a');
    expect(next.nodes.map((n) => n.id)).toEqual(['b']);
    expect(next.edges).toHaveLength(0);
  });
});

describe('applyRemoveEdges', () => {
  it('removing a plain chain edge is a no-op — chain order derives from node-array position, which an edge-id delete cannot target (Task 3: single-source)', () => {
    const g = makeGraph();
    const next = applyRemoveEdges(g, new Set(['a->b']));
    expect(next.edges).toEqual(g.edges);
    expect(next.nodes).toEqual(g.nodes);
  });

  it('removing an id that names no edge is a no-op copy', () => {
    const g = makeGraph();
    const next = applyRemoveEdges(g, new Set(['no->such']));
    expect(next.edges).toEqual(g.edges);
  });

  it('removing a branch-arm edge also drops its target from the branch node\'s arm list', () => {
    const g = makeGraph();
    g.nodes.push({
      id: 'br',
      data: { id: 'br', kind: 'branch', condition: 'inputs.ok', thenTargets: ['a'], elseTargets: ['b'] },
      position: { x: 0, y: 0 },
    });
    g.edges.push(
      { id: 'br->a:then', source: 'br', target: 'a', label: 'true', branch: 'then' },
      { id: 'br->b:else', source: 'br', target: 'b', label: 'false', branch: 'else' },
    );

    const next = applyRemoveEdges(g, new Set(['br->a:then']));

    expect(next.edges.some((e) => e.id === 'br->a:then')).toBe(false);
    expect(next.edges.some((e) => e.id === 'br->b:else')).toBe(true);
    const br = next.nodes.find((n) => n.id === 'br');
    expect(br?.data.thenTargets).toEqual([]);
    expect(br?.data.elseTargets).toEqual(['b']);
  });

  it('removing both branch arms in one call clears both lists', () => {
    const g = makeGraph();
    g.nodes.push({
      id: 'br',
      data: { id: 'br', kind: 'branch', condition: 'inputs.ok', thenTargets: ['a'], elseTargets: ['b'] },
      position: { x: 0, y: 0 },
    });
    g.edges.push(
      { id: 'br->a:then', source: 'br', target: 'a', label: 'true', branch: 'then' },
      { id: 'br->b:else', source: 'br', target: 'b', label: 'false', branch: 'else' },
    );

    const next = applyRemoveEdges(g, new Set(['br->a:then', 'br->b:else']));

    // Under the derived-edges model EVERY consecutive node-array pair carries
    // a chain edge — nodes are [a, b, br], so both a->b and b->br remain;
    // only the two branch-arm edges (which derived from the now-empty
    // then/else lists) are gone.
    expect(next.edges).toHaveLength(2);
    expect(next.edges).toContainEqual({ id: 'a->b', source: 'a', target: 'b' });
    expect(next.edges).toContainEqual({ id: 'b->br', source: 'b', target: 'br' });
    const br = next.nodes.find((n) => n.id === 'br');
    expect(br?.data.thenTargets).toEqual([]);
    expect(br?.data.elseTargets).toEqual([]);
  });
});

// Critical fix: drawing ONE edge (or branch arm) on a legacy multi-step
// workflow used to flip `hasExplicitEdges` for the WHOLE graph, so every
// OTHER node — still with no explicit `next` of its own — lost the implicit
// chain edge it used to derive from list order. `applyConnect` now runs the
// nodes through `materializeLegacyChain` before applying the new edge, so the
// pre-existing chain survives as explicit `next` entries.
describe('legacy->graph migration (materialize before the first explicit edge)', () => {
  it('the exact repro: legacy chain a->b->c->d, drawing b->d preserves a->b and c->d instead of collapsing to [b->d]', () => {
    let g = yamlToGraph({
      name: 'w',
      steps: [
        { id: 'a', agent: 'x', prompt: 'p' },
        { id: 'b', agent: 'x', prompt: 'p' },
        { id: 'c', agent: 'x', prompt: 'p' },
        { id: 'd', agent: 'x', prompt: 'p' },
      ],
    });
    expect(hasExplicitEdges(g.nodes)).toBe(false); // confirms this starts legacy

    applyConnect(g, { source: 'b', target: 'd', sourceHandle: null }, (ng) => (g = ng), () => {});

    // Nothing orphaned: a and c both still reach a successor, and the new
    // edge is there too — exactly {a->b, c->d, b->d}, in any order.
    const bySourceTarget = (e: { source: string; target: string }) => `${e.source}->${e.target}`;
    expect(new Set(g.edges.map(bySourceTarget))).toEqual(new Set(['a->b', 'c->d', 'b->d']));
    expect(g.edges).toHaveLength(3);
  });

  it('applying the SAME drawn edge to the raw (unmaterialized) nodes reproduces the bug — the collapse this test guards against', () => {
    // Same starting graph as above, but skipping materialization (the bug's
    // exact mechanism): only `b` gets an explicit `next`, everyone else's
    // implicit chain edge is gone once graph mode kicks in.
    const g = yamlToGraph({
      name: 'w',
      steps: [
        { id: 'a', agent: 'x', prompt: 'p' },
        { id: 'b', agent: 'x', prompt: 'p' },
        { id: 'c', agent: 'x', prompt: 'p' },
        { id: 'd', agent: 'x', prompt: 'p' },
      ],
    });
    const buggyNodes = g.nodes.map((n) => (n.id === 'b' ? { ...n, data: { ...n.data, next: ['d'] } } : n));
    const edges = deriveEdges(buggyNodes);
    expect(edges).toHaveLength(1);
    expect(edges[0]).toMatchObject({ source: 'b', target: 'd' });
  });

  it('a first branch-arm draw on a legacy chain preserves the other chain edges', () => {
    // `br` leads the array, then a->b->c is the legacy chain; drawing br's
    // "then" arm forward to `c` (not backward, which would close a cycle
    // through the existing implicit chain) is the first explicit connection.
    let g = yamlToGraph({
      name: 'w',
      steps: [
        { id: 'br', branch: { condition: 'x' } },
        { id: 'a', agent: 'x', prompt: 'p' },
        { id: 'b', agent: 'x', prompt: 'p' },
        { id: 'c', agent: 'x', prompt: 'p' },
      ],
    });
    expect(hasExplicitEdges(g.nodes)).toBe(false);

    applyConnect(g, { source: 'br', target: 'c', sourceHandle: 'then' }, (ng) => (g = ng), () => {});

    expect(g.edges).toContainEqual({ id: 'a->b', source: 'a', target: 'b' });
    expect(g.edges).toContainEqual({ id: 'b->c', source: 'b', target: 'c' });
    expect(g.edges).toContainEqual({
      id: 'br->c:then',
      source: 'br',
      target: 'c',
      branch: 'then',
      label: 'true',
    });
  });
});

// Task 5: drawing a line sets an explicit `next` edge (replacing the old one
// on a regular node, accumulating on a `split`); dropping a node leaves it
// unconnected; deleting a line clears the edge it came from. Supersedes the
// earlier "plain connect = reorder" model (see the `applyConnect`/
// `applyAddConnectedNext` rewrites below).
describe('explicit connect/drop', () => {
  it('drawing a plain edge sets next; a second draw from a regular node REPLACES it', () => {
    let g = yamlToGraph({
      name: 'w',
      steps: [
        { id: 'a', agent: 'x', prompt: 'p', next: [] },
        { id: 'b', agent: 'x', prompt: 'p' },
        { id: 'c', agent: 'x', prompt: 'p' },
      ],
    });
    applyConnect(g, { source: 'a', target: 'b', sourceHandle: null }, (ng) => (g = ng), () => {});
    expect(g.nodes.find((n) => n.id === 'a')!.data.next).toEqual(['b']);
    applyConnect(g, { source: 'a', target: 'c', sourceHandle: null }, (ng) => (g = ng), () => {});
    expect(g.nodes.find((n) => n.id === 'a')!.data.next).toEqual(['c']); // replaced, not [b, c]
  });

  it('a split node accumulates edges instead of replacing', () => {
    let g = yamlToGraph({
      name: 'w',
      steps: [{ id: 's', split: [] }, { id: 'a', agent: 'x', prompt: 'p' }, { id: 'b', agent: 'x', prompt: 'p' }],
    });
    applyConnect(g, { source: 's', target: 'a', sourceHandle: null }, (ng) => (g = ng), () => {});
    applyConnect(g, { source: 's', target: 'b', sourceHandle: null }, (ng) => (g = ng), () => {});
    expect(g.nodes.find((n) => n.id === 's')!.data.split).toEqual(['a', 'b']);
  });

  it('a freshly added node is disconnected (no next)', () => {
    const r = applyAddNode(yamlToGraph({ name: 'w', steps: [{ id: 'a', agent: 'x', prompt: 'p', next: [] }] }), 'step');
    expect(r.graph.nodes.find((n) => n.id === r.id)!.data.next ?? []).toEqual([]);
  });

  it('applyDelete scrubs the deleted id from a surviving node\'s `next`', () => {
    let g = yamlToGraph({
      name: 'w',
      steps: [
        { id: 'a', agent: 'x', prompt: 'p', next: ['b'] },
        { id: 'b', agent: 'x', prompt: 'p' },
      ],
    });
    g = applyDelete(g, 'b');
    expect(g.nodes.find((n) => n.id === 'a')!.data.next ?? []).toEqual([]);
    expect(invariantHolds(g)).toBe(true);
  });

  it('applyDelete scrubs the deleted id from a surviving `split` node\'s fan-out list', () => {
    let g = yamlToGraph({
      name: 'w',
      steps: [{ id: 's', split: ['a', 'b'] }, { id: 'a', agent: 'x', prompt: 'p' }, { id: 'b', agent: 'x', prompt: 'p' }],
    });
    g = applyDelete(g, 'a');
    expect(g.nodes.find((n) => n.id === 's')!.data.split).toEqual(['b']);
    expect(invariantHolds(g)).toBe(true);
  });

  it('applyRemoveEdges clears the `next` entry a drawn edge set (not a reorder)', () => {
    // A third `split` node (kind alone keeps the workflow in graph mode
    // regardless of its own empty fan-out) isolates "clear the entry" from
    // the separate legacy/graph-mode-boundary behavior a 2-node all-next
    // workflow would hit: clearing the ONLY non-empty `next` in a graph with
    // no split/join anywhere reverts `hasExplicitEdges` to false, and a
    // legacy chain edge would derive right back between the two remaining
    // array-adjacent nodes — see `hasExplicitEdges`'s doc comment. That's a
    // real, accepted boundary of the derived-edges model (Task 4), not
    // something this op should paper over.
    let g = yamlToGraph({
      name: 'w',
      steps: [
        { id: 'a', agent: 'x', prompt: 'p', next: ['b'] },
        { id: 'b', agent: 'x', prompt: 'p' },
        { id: 'z', split: [] },
      ],
    });
    g = applyRemoveEdges(g, new Set(['a->b']));
    expect(g.nodes.find((n) => n.id === 'a')!.data.next).toEqual([]);
    expect(g.edges).toEqual([]);
    expect(invariantHolds(g)).toBe(true);
  });

  it('applyRemoveEdges clears the target from a `split` node\'s fan-out list', () => {
    let g = yamlToGraph({
      name: 'w',
      steps: [{ id: 's', split: ['a', 'b'] }, { id: 'a', agent: 'x', prompt: 'p' }, { id: 'b', agent: 'x', prompt: 'p' }],
    });
    g = applyRemoveEdges(g, new Set(['s->a']));
    expect(g.nodes.find((n) => n.id === 's')!.data.split).toEqual(['b']);
    expect(invariantHolds(g)).toBe(true);
  });
});

describe('applyAddNode', () => {
  it('parallel default carries an empty parallel array', () => {
    const { graph, id } = applyAddNode(makeGraph(), 'parallel');
    const added = graph.nodes.find((n) => n.id === id);
    expect(added?.data.kind).toBe('parallel');
    expect(added?.data.parallel).toEqual([]);
  });

  it('panel default carries an empty panel config', () => {
    const { graph, id } = applyAddNode(makeGraph(), 'panel');
    const added = graph.nodes.find((n) => n.id === id);
    expect(added?.data.kind).toBe('panel');
    expect(added?.data.panel).toEqual({ panelists: [], subject: '' });
  });

  it('branch default seeds an empty condition + empty then/else target lists', () => {
    const { graph, id } = applyAddNode(makeGraph(), 'branch');
    const added = graph.nodes.find((n) => n.id === id);
    expect(added?.data.kind).toBe('branch');
    expect(added?.data.condition).toBe('');
    expect(added?.data.thenTargets).toEqual([]);
    expect(added?.data.elseTargets).toEqual([]);
  });

  it('generates an id that does not collide with existing nodes', () => {
    const g = makeGraph();
    g.nodes.push({ id: 'step-1', data: { id: 'step-1', kind: 'step' }, position: { x: 0, y: 0 } });
    const { id } = applyAddNode(g, 'step');
    expect(id).toBe('step-2');
  });
});

describe('asStepKind', () => {
  it('accepts branch alongside the other known kinds', () => {
    expect(asStepKind('branch')).toBe('branch');
    expect(asStepKind('step')).toBe('step');
    expect(asStepKind('nonsense')).toBeNull();
  });
});

describe('branch edge rendering', () => {
  it('a labeled branch-arm edge passes its label + a color into the xyflow edge', () => {
    const g = makeGraph();
    g.nodes.push({
      id: 'br',
      data: { id: 'br', kind: 'branch', condition: 'inputs.ok', thenTargets: ['a'], elseTargets: ['b'] },
      position: { x: 0, y: 0 },
    });
    g.edges.push(
      { id: 'br->a:then', source: 'br', target: 'a', label: 'true', branch: 'then' },
      { id: 'br->b:else', source: 'br', target: 'b', label: 'false', branch: 'else' },
    );
    render(
      <WorkflowEditorGraph
        graph={g}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    expect(raw).toBeTruthy();
    const edges = JSON.parse(raw!) as Array<{ id: string; label?: string; style?: { stroke?: string } }>;
    const thenEdge = edges.find((e) => e.id === 'br->a:then');
    const elseEdge = edges.find((e) => e.id === 'br->b:else');
    expect(thenEdge?.label).toBe('true');
    expect(elseEdge?.label).toBe('false');
    // Distinct colors for the two arms (both defined, and not equal to each other).
    expect(thenEdge?.style?.stroke).toBeTruthy();
    expect(elseEdge?.style?.stroke).toBeTruthy();
    expect(thenEdge?.style?.stroke).not.toBe(elseEdge?.style?.stroke);
    // The plain chain edge (a->b) carries no label.
    const plain = edges.find((e) => e.id === 'a->b');
    expect(plain?.label).toBeUndefined();
  });

  it('anchors each branch-arm edge at its OWN handle — else at "else", then at "then" (not both at the default first handle)', () => {
    const g = makeGraph();
    g.nodes.push({
      id: 'br',
      data: { id: 'br', kind: 'branch', condition: 'inputs.ok', thenTargets: ['a'], elseTargets: ['b'] },
      position: { x: 0, y: 0 },
    });
    g.edges.push(
      { id: 'br->a:then', source: 'br', target: 'a', label: 'true', branch: 'then' },
      { id: 'br->b:else', source: 'br', target: 'b', label: 'false', branch: 'else' },
    );
    render(
      <WorkflowEditorGraph
        graph={g}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const edges = JSON.parse(raw!) as Array<{ id: string; sourceHandle?: string }>;
    expect(edges.find((e) => e.id === 'br->a:then')?.sourceHandle).toBe('then');
    expect(edges.find((e) => e.id === 'br->b:else')?.sourceHandle).toBe('else');
    // A plain chain edge (no branch arm — every non-branch kind has a single,
    // unlabeled default source handle) carries no sourceHandle at all.
    expect(edges.find((e) => e.id === 'a->b')?.sourceHandle).toBeUndefined();
  });

  it('next mode bumps the branch-arm stroke width and themes the plain edge too', () => {
    const g = makeGraph();
    g.nodes.push({
      id: 'br',
      data: { id: 'br', kind: 'branch', condition: 'inputs.ok', thenTargets: ['a'], elseTargets: ['b'] },
      position: { x: 0, y: 0 },
    });
    g.edges.push({ id: 'br->a:then', source: 'br', target: 'a', label: 'true', branch: 'then' });
    render(
      <WorkflowEditorGraph
        graph={g}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="next"
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const edges = JSON.parse(raw!) as Array<{
      id: string;
      style?: { stroke?: string; strokeWidth?: number };
    }>;
    const thenEdge = edges.find((e) => e.id === 'br->a:then');
    expect(thenEdge?.style?.strokeWidth).toBeGreaterThan(1.6);
    // The plain chain edge (a->b, no branch) also gets a themed, bolder stroke in next mode.
    const plain = edges.find((e) => e.id === 'a->b');
    expect(plain?.style?.stroke).toBeTruthy();
    expect(plain?.style?.strokeWidth).toBeTruthy();
  });

  it('next mode swaps branch-arm labels for ✓ then / ✕ else chips with a filled bg (Task 3)', () => {
    const g = makeGraph();
    g.nodes.push({
      id: 'br',
      data: { id: 'br', kind: 'branch', condition: 'inputs.ok', thenTargets: ['a'], elseTargets: ['b'] },
      position: { x: 0, y: 0 },
    });
    g.edges.push(
      { id: 'br->a:then', source: 'br', target: 'a', label: 'true', branch: 'then' },
      { id: 'br->b:else', source: 'br', target: 'b', label: 'false', branch: 'else' },
    );
    render(
      <WorkflowEditorGraph
        graph={g}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="next"
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const edges = JSON.parse(raw!) as Array<{
      id: string;
      label?: string;
      labelBgStyle?: { fill?: string; fillOpacity?: number };
      labelBgPadding?: number[];
      labelBgBorderRadius?: number;
    }>;
    const thenEdge = edges.find((e) => e.id === 'br->a:then');
    const elseEdge = edges.find((e) => e.id === 'br->b:else');
    expect(thenEdge?.label).toBe('✓ then');
    expect(elseEdge?.label).toBe('✕ else');
    expect(thenEdge?.labelBgStyle?.fill).toBeTruthy();
    expect(elseEdge?.labelBgStyle?.fill).toBeTruthy();
    expect(thenEdge?.labelBgPadding).toEqual([6, 3]);
    expect(thenEdge?.labelBgBorderRadius).toBe(6);
  });

  it('classic mode keeps the raw true/false label and a transparent labelBgStyle (unchanged)', () => {
    const g = makeGraph();
    g.nodes.push({
      id: 'br',
      data: { id: 'br', kind: 'branch', condition: 'inputs.ok', thenTargets: ['a'], elseTargets: ['b'] },
      position: { x: 0, y: 0 },
    });
    g.edges.push({ id: 'br->a:then', source: 'br', target: 'a', label: 'true', branch: 'then' });
    render(
      <WorkflowEditorGraph
        graph={g}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="classic"
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const edges = JSON.parse(raw!) as Array<{
      id: string;
      label?: string;
      labelBgStyle?: { fill?: string; fillOpacity?: number };
    }>;
    const thenEdge = edges.find((e) => e.id === 'br->a:then');
    expect(thenEdge?.label).toBe('true');
    expect(thenEdge?.labelBgStyle).toEqual({ fillOpacity: 0 });
  });

  it('next mode tints the plain edge arrow marker (classic leaves it undefined)', () => {
    const classicRender = () => {
      const { unmount } = render(
        <WorkflowEditorGraph
          graph={makeGraph()}
          onChange={() => {}}
          selectedId={null}
          onSelect={() => {}}
          problemsById={{}}
          onInvalidConnection={() => {}}
          workflowEditorUi="classic"
        />,
      );
      const raw = screen.getByTestId('rf').getAttribute('data-edges');
      unmount();
      return JSON.parse(raw!) as Array<{ id: string; markerEnd?: { color?: string } }>;
    };
    const classicEdges = classicRender();
    const classicPlain = classicEdges.find((e) => e.id === 'a->b');
    expect(classicPlain?.markerEnd?.color).toBeUndefined();

    render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="next"
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const nextEdges = JSON.parse(raw!) as Array<{ id: string; markerEnd?: { color?: string } }>;
    const nextPlain = nextEdges.find((e) => e.id === 'a->b');
    expect(nextPlain?.markerEnd?.color).toBeTruthy();
  });

  it('next mode gives a non-branch labeled edge a neutral brand-tinted chip', () => {
    const g = makeGraph();
    g.edges.push({ id: 'a->b:extra', source: 'a', target: 'b', label: 'data-ref' });
    render(
      <WorkflowEditorGraph
        graph={g}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="next"
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const edges = JSON.parse(raw!) as Array<{
      id: string;
      label?: string;
      labelBgStyle?: { fill?: string };
    }>;
    const labeled = edges.find((e) => e.id === 'a->b:extra');
    expect(labeled?.label).toBe('data-ref');
    expect(labeled?.labelBgStyle?.fill).toBeTruthy();
  });
});

describe('kind-colored + animated edges (Task 1 round 2)', () => {
  it('next plain edge: stroke + arrow marker both carry the SOURCE node kind accent, strokeWidth 2', () => {
    const g = makeGraph();
    // Source 'a' is a `step` node in makeGraph(); retype it `for_each` so the
    // assertion actually exercises the per-kind lookup (not a coincidence with
    // the default kind).
    g.nodes[0] = { ...g.nodes[0], data: { ...g.nodes[0].data, kind: 'for_each' } };
    render(
      <WorkflowEditorGraph
        graph={g}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="next"
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const edges = JSON.parse(raw!) as Array<{
      id: string;
      style?: { stroke?: string; strokeWidth?: number };
      markerEnd?: { color?: string };
      animated?: boolean;
    }>;
    const plain = edges.find((e) => e.id === 'a->b');
    const accentKey = KIND_ACCENT.for_each;
    expect(plain?.style?.stroke).toBe(`rgb(${accentKey} / 0.55)`);
    expect(plain?.style?.strokeWidth).toBe(2);
    expect(plain?.markerEnd?.color).toBe(`rgb(${accentKey} / 0.55)`);
    expect(plain?.animated).toBe(true);
  });

  it('next plain edge from a `step`-kind source uses the step accent, distinct from a `branch` source', () => {
    const g = makeGraph(); // a, b both `step`
    render(
      <WorkflowEditorGraph
        graph={g}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="next"
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const edges = JSON.parse(raw!) as Array<{ id: string; style?: { stroke?: string } }>;
    const plain = edges.find((e) => e.id === 'a->b');
    expect(plain?.style?.stroke).toBe(`rgb(${KIND_ACCENT.step} / 0.55)`);
  });

  it('next branch-arm edges keep their existing chip/color/width styling and additionally gain animated: true', () => {
    const g = makeGraph();
    g.nodes.push({
      id: 'br',
      data: { id: 'br', kind: 'branch', condition: 'inputs.ok', thenTargets: ['a'], elseTargets: ['b'] },
      position: { x: 0, y: 0 },
    });
    g.edges.push(
      { id: 'br->a:then', source: 'br', target: 'a', label: 'true', branch: 'then' },
      { id: 'br->b:else', source: 'br', target: 'b', label: 'false', branch: 'else' },
    );
    render(
      <WorkflowEditorGraph
        graph={g}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="next"
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const edges = JSON.parse(raw!) as Array<{
      id: string;
      label?: string;
      style?: { stroke?: string; strokeWidth?: number };
      animated?: boolean;
    }>;
    const thenEdge = edges.find((e) => e.id === 'br->a:then');
    const elseEdge = edges.find((e) => e.id === 'br->b:else');
    // Unchanged from current `next` shape: label chip + full-strength status
    // colors at 2.5 stroke width.
    expect(thenEdge?.label).toBe('✓ then');
    expect(elseEdge?.label).toBe('✕ else');
    expect(thenEdge?.style?.stroke).toBe('rgb(34 197 94)');
    expect(elseEdge?.style?.stroke).toBe('rgb(239 68 68)');
    expect(thenEdge?.style?.strokeWidth).toBe(2.5);
    // New: both arms are now animated too.
    expect(thenEdge?.animated).toBe(true);
    expect(elseEdge?.animated).toBe(true);
  });

  it('classic edges never carry an `animated` key, regardless of branch/plain', () => {
    const g = makeGraph();
    g.nodes.push({
      id: 'br',
      data: { id: 'br', kind: 'branch', condition: 'inputs.ok', thenTargets: ['a'], elseTargets: ['b'] },
      position: { x: 0, y: 0 },
    });
    g.edges.push({ id: 'br->a:then', source: 'br', target: 'a', label: 'true', branch: 'then' });
    render(
      <WorkflowEditorGraph
        graph={g}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="classic"
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const edges = JSON.parse(raw!) as Array<{ id: string; animated?: boolean }>;
    for (const e of edges) expect(e.animated).toBeUndefined();
  });

  it('classic plain edge styling is exactly today\'s shape (no kind-accent stroke, undefined marker color)', () => {
    render(
      <WorkflowEditorGraph
        graph={makeGraph()}
        onChange={() => {}}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi="classic"
      />,
    );
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    const edges = JSON.parse(raw!) as Array<{
      id: string;
      style?: { stroke?: string; strokeWidth?: number };
      markerEnd?: { color?: string };
    }>;
    const plain = edges.find((e) => e.id === 'a->b');
    expect(plain?.style).toBeUndefined();
    expect(plain?.markerEnd?.color).toBeUndefined();
  });
});

describe('edge selection + deletion (onEdgesChange)', () => {
  function renderGraph(opts: { graph?: WorkflowGraph; workflowEditorUi?: 'classic' | 'next' } = {}) {
    const onChange = vi.fn();
    const graph = opts.graph ?? makeGraph();
    render(
      <WorkflowEditorGraph
        graph={graph}
        onChange={onChange}
        selectedId={null}
        onSelect={() => {}}
        problemsById={{}}
        onInvalidConnection={() => {}}
        workflowEditorUi={opts.workflowEditorUi}
      />,
    );
    return { onChange, graph };
  }

  function currentEdges() {
    const raw = screen.getByTestId('rf').getAttribute('data-edges');
    expect(raw).toBeTruthy();
    return JSON.parse(raw!) as Array<{
      id: string;
      selected?: boolean;
      style?: { stroke?: string; strokeWidth?: number };
    }>;
  }

  function graphWithBranch(): WorkflowGraph {
    const g = makeGraph();
    g.nodes.push({
      id: 'br',
      data: { id: 'br', kind: 'branch', condition: 'inputs.ok', thenTargets: ['a'], elseTargets: ['b'] },
      position: { x: 0, y: 0 },
    });
    g.edges.push(
      { id: 'br->a:then', source: 'br', target: 'a', label: 'true', branch: 'then' },
      { id: 'br->b:else', source: 'br', target: 'b', label: 'false', branch: 'else' },
    );
    return g;
  }

  it('a select change marks the edge selected; a matching deselect clears it', () => {
    renderGraph();
    expect(currentEdges().find((e) => e.id === 'a->b')?.selected).toBeUndefined();

    act(() => {
      rfCapture.onEdgesChange?.([{ type: 'select', id: 'a->b', selected: true }]);
    });
    expect(currentEdges().find((e) => e.id === 'a->b')?.selected).toBe(true);

    act(() => {
      rfCapture.onEdgesChange?.([{ type: 'select', id: 'a->b', selected: false }]);
    });
    expect(currentEdges().find((e) => e.id === 'a->b')?.selected).toBeUndefined();
  });

  it('unselected edges never carry a `selected` key (classic shape preserved)', () => {
    renderGraph({ workflowEditorUi: 'classic' });
    for (const e of currentEdges()) expect(e.selected).toBeUndefined();
  });

  it('a remove change for a chain edge is a no-op (chain order derives from node-array position, not a stored edge) — onChange still fires with the edge intact (Task 3: single-source)', () => {
    const { onChange, graph } = renderGraph();
    act(() => {
      rfCapture.onEdgesChange?.([{ type: 'remove', id: 'a->b' }]);
    });
    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0][0] as WorkflowGraph;
    expect(next.edges.find((e) => e.id === 'a->b')).toBeDefined();
    expect(next).toEqual(graph);
  });

  it('removing a selected edge prunes it from the selection set', () => {
    renderGraph();
    act(() => {
      rfCapture.onEdgesChange?.([{ type: 'select', id: 'a->b', selected: true }]);
    });
    expect(currentEdges().find((e) => e.id === 'a->b')?.selected).toBe(true);

    act(() => {
      rfCapture.onEdgesChange?.([{ type: 'remove', id: 'a->b' }]);
    });
    // The (unchanged, mocked) `graph` prop still carries the edge, but its
    // selection was pruned — proves the internal selectedEdgeIds state, not
    // just the parent's graph, dropped the removed id.
    expect(currentEdges().find((e) => e.id === 'a->b')?.selected).toBeUndefined();
  });

  it('removing a branch-arm edge drops the target from the branch node\'s arm list AND the edge itself', () => {
    const { onChange } = renderGraph({ graph: graphWithBranch() });
    act(() => {
      rfCapture.onEdgesChange?.([{ type: 'remove', id: 'br->a:then' }]);
    });
    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0][0] as WorkflowGraph;
    expect(next.edges.find((e) => e.id === 'br->a:then')).toBeUndefined();
    expect(next.nodes.find((n) => n.id === 'br')?.data.thenTargets).toEqual([]);
  });

  it('next mode: selecting a plain edge bumps its inline stroke width and alpha', () => {
    renderGraph({ workflowEditorUi: 'next' });
    const before = currentEdges().find((e) => e.id === 'a->b');
    act(() => {
      rfCapture.onEdgesChange?.([{ type: 'select', id: 'a->b', selected: true }]);
    });
    const after = currentEdges().find((e) => e.id === 'a->b');
    expect(after?.selected).toBe(true);
    expect(after?.style?.strokeWidth ?? 0).toBeGreaterThan(before?.style?.strokeWidth ?? 0);
    expect(after?.style?.stroke).not.toBe(before?.style?.stroke);
  });

  it('next mode: selecting a branch-arm edge bumps its stroke width beyond the default 2.5', () => {
    renderGraph({ graph: graphWithBranch(), workflowEditorUi: 'next' });
    act(() => {
      rfCapture.onEdgesChange?.([{ type: 'select', id: 'br->a:then', selected: true }]);
    });
    const edge = currentEdges().find((e) => e.id === 'br->a:then');
    expect(edge?.selected).toBe(true);
    expect(edge?.style?.strokeWidth).toBeGreaterThan(2.5);
  });

  it('classic mode: a branch-arm edge carries no inline strokeWidth bump on select (byte-identical style object shape otherwise)', () => {
    renderGraph({ graph: graphWithBranch(), workflowEditorUi: 'classic' });
    act(() => {
      rfCapture.onEdgesChange?.([{ type: 'select', id: 'br->a:then', selected: true }]);
    });
    const edge = currentEdges().find((e) => e.id === 'br->a:then');
    expect(edge?.selected).toBe(true);
    expect(edge?.style?.strokeWidth).toBeUndefined();
  });
});

describe('applyAddNodeAt', () => {
  it('places the new node at the given position', () => {
    const { graph, id } = applyAddNodeAt(makeGraph(), 'step', { x: 321, y: 654 });
    const added = graph.nodes.find((n) => n.id === id);
    expect(added?.position).toEqual({ x: 321, y: 654 });
    expect(graph.nodes).toHaveLength(3);
  });

  it('seeds container shapes for parallel/panel kinds', () => {
    const par = applyAddNodeAt(makeGraph(), 'parallel', { x: 0, y: 0 });
    expect(par.graph.nodes.find((n) => n.id === par.id)?.data.parallel).toEqual([]);
    const pan = applyAddNodeAt(makeGraph(), 'panel', { x: 0, y: 0 });
    expect(pan.graph.nodes.find((n) => n.id === pan.id)?.data.panel).toEqual({
      panelists: [],
      subject: '',
    });
  });
});

describe('applyAddConnectedNext', () => {
  it('adds a step node + an edge source->new and places it to the right', () => {
    const g = makeGraph(); // a at {0,0}, b at {0,100}
    const { graph, id } = applyAddConnectedNext(g, 'a');
    const added = graph.nodes.find((n) => n.id === id);
    expect(added?.data.kind).toBe('step');
    expect(added?.position.x).toBeGreaterThan(g.nodes[0].position.x);
    expect(added?.position.y).toBe(g.nodes[0].position.y);
    expect(graph.edges).toContainEqual({ id: `a->${id}`, source: 'a', target: id });
  });

  it('honors an explicit kind', () => {
    const { graph, id } = applyAddConnectedNext(makeGraph(), 'b', 'panel');
    expect(graph.nodes.find((n) => n.id === id)?.data.kind).toBe('panel');
  });

  it('appends the node at the end of the array when the source is unknown — under the derived-edges model that still chains it from whatever was previously last (no more "disconnected append")', () => {
    const g = makeGraph(); // a, b (b is last)
    const { graph, id } = applyAddConnectedNext(g, 'does-not-exist');
    expect(graph.nodes).toHaveLength(3);
    expect(graph.nodes[graph.nodes.length - 1]?.id).toBe(id);
    expect(graph.edges).toContainEqual({ id: `b->${id}`, source: 'b', target: id });
  });

  it('"+ next" from the middle of a legacy 3-chain a->b->c preserves a->b and reroutes b->c through the new node instead of orphaning c (Critical-bug fix)', () => {
    const g = yamlToGraph({
      name: 'w',
      steps: [
        { id: 'a', agent: 'x', prompt: 'p' },
        { id: 'b', agent: 'x', prompt: 'p' },
        { id: 'c', agent: 'x', prompt: 'p' },
      ],
    });
    expect(hasExplicitEdges(g.nodes)).toBe(false);

    const { graph, id } = applyAddConnectedNext(g, 'b');

    // a->b: untouched, still there. b->new: the "+ next" edge this op draws.
    // new->c: the old (implicit) b->c successor, rerouted through the
    // inserted node rather than dropped — c is never left unreachable.
    expect(graph.edges).toContainEqual({ id: 'a->b', source: 'a', target: 'b' });
    expect(graph.edges).toContainEqual({ id: `b->${id}`, source: 'b', target: id });
    expect(graph.edges).toContainEqual({ id: `${id}->c`, source: id, target: 'c' });
    expect(graph.edges).toHaveLength(3);
  });
});

describe('applyAddConnectedNext placement', () => {
  it('offsets by the SOURCE node width, so a wide container never overlaps its next node', () => {
    const graph = {
      nodes: [
        {
          id: 'fan',
          data: {
            id: 'fan',
            kind: 'parallel' as const,
            parallel: [{ id: 'a', agent: 'x', prompt: 'p' }],
          },
          position: { x: 100, y: 40 },
        },
      ],
      edges: [],
      settings: {},
    } as unknown as Parameters<typeof applyAddConnectedNext>[0];

    const { graph: next, id } = applyAddConnectedNext(graph, 'fan');
    const added = next.nodes.find((n) => n.id === id)!;
    // parallel is PARALLEL_W (220) wide, + the 64px gap — NOT NODE_W (210).
    expect(added.position).toEqual({ x: 100 + 220 + 64, y: 40 });
  });

  it('still offsets a plain step by the step width', () => {
    const graph = {
      nodes: [
        { id: 's', data: { id: 's', kind: 'step' as const, agent: 'a' }, position: { x: 0, y: 0 } },
      ],
      edges: [],
      settings: {},
    } as unknown as Parameters<typeof applyAddConnectedNext>[0];

    const { graph: next, id } = applyAddConnectedNext(graph, 's');
    expect(next.nodes.find((n) => n.id === id)!.position).toEqual({ x: 210 + 64, y: 0 });
  });
});

describe('applyInsertOnEdge', () => {
  it('splits A->B into A->new and new->B', () => {
    const { graph, id } = applyInsertOnEdge(makeGraph(), 'a->b', 'step', { x: 5, y: 5 });
    expect(graph.edges.some((e) => e.id === 'a->b')).toBe(false);
    expect(graph.edges).toContainEqual({ id: `a->${id}`, source: 'a', target: id });
    expect(graph.edges).toContainEqual({ id: `${id}->b`, source: id, target: 'b' });
    expect(graph.nodes.find((n) => n.id === id)?.position).toEqual({ x: 5, y: 5 });
  });

  it('falls back to a plain add (appended to the end) when the edge is unknown — the derived-edges model chains it from whatever was previously last', () => {
    const g = makeGraph(); // a, b (b is last)
    const { graph, id } = applyInsertOnEdge(g, 'no->such', 'step', { x: 1, y: 2 });
    expect(graph.nodes).toHaveLength(g.nodes.length + 1);
    expect(graph.edges).toContainEqual({ id: `b->${id}`, source: 'b', target: id });
    expect(graph.nodes.find((n) => n.id === id)?.position).toEqual({ x: 1, y: 2 });
  });
});

function invariantHolds(g: { nodes: any[]; edges: any[] }): boolean {
  return JSON.stringify(g.edges) === JSON.stringify(deriveEdges(g.nodes));
}

describe('single-source graph ops', () => {
  const base = () =>
    yamlToGraph({
      name: 'w',
      steps: [
        { id: 'b', branch: { condition: 'x', then: [], else: [] } },
        { id: 't', agent: 'a', prompt: 'p' },
      ],
    });

  it('a branch then-target set via applyConnect appears as a derived edge (P0.2)', () => {
    let g = base();
    applyConnect(g, { source: 'b', target: 't', sourceHandle: 'then' }, (ng) => (g = ng), () => {});
    expect(g.nodes.find((n) => n.id === 'b')!.data.thenTargets).toContain('t');
    expect(invariantHolds(g)).toBe(true);
    expect(deriveEdges(g.nodes)).toContainEqual(expect.objectContaining({ source: 'b', target: 't', branch: 'then' }));
  });

  it('applyDelete scrubs the deleted id from surviving branch targets (P1)', () => {
    let g = base();
    applyConnect(g, { source: 'b', target: 't', sourceHandle: 'then' }, (ng) => (g = ng), () => {});
    g = applyDelete(g, 't');
    expect(g.nodes.find((n) => n.id === 'b')!.data.thenTargets ?? []).not.toContain('t');
    expect(invariantHolds(g)).toBe(true);
  });

  it('every op leaves edges === deriveEdges(nodes)', () => {
    let g = base();
    expect(invariantHolds(g)).toBe(true);
    const r = applyAddNode(g, 'step');
    expect(invariantHolds(r.graph)).toBe(true);
  });
});
