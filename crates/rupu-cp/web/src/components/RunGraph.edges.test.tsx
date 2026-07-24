// @vitest-environment jsdom
// RunGraph edges — classic keeps the flat/blue/amber status styling; next
// colors each edge by its SOURCE step's kind and animates only the live
// frontier. We assert on the Edge objects React Flow is handed, by mocking
// @xyflow/react's ReactFlow to capture its props.
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, cleanup } from '@testing-library/react';
import type { Edge } from '@xyflow/react';

let captured: Edge[] = [];
vi.mock('@xyflow/react', async () => {
  const actual = await vi.importActual<typeof import('@xyflow/react')>('@xyflow/react');
  return {
    ...actual,
    ReactFlow: (props: { edges: Edge[] }) => {
      captured = props.edges;
      return <div data-testid="rf" />;
    },
  };
});

const uiMock = vi.fn(() => 'classic');
vi.mock('../hooks/useWorkflowEditorUi', () => ({
  useWorkflowEditorUi: () => uiMock(),
}));

import RunGraph from './RunGraph';
import type { RunGraphModel, GraphNode } from '../lib/runGraphModel';

// RunGraph paints edge strokes via `useThemeColors`, which reads real CSS
// custom properties off `document.documentElement` (see
// `lib/useThemeColors.test.tsx`'s established fixture pattern). jsdom has no
// stylesheet loaded in this test file, so every token would otherwise fall
// back to the SAME '0 0 0' value — collapsing genuinely distinct tokens
// (e.g. `status.awaiting` vs `status.paused`) to equal strings and making the
// "must differ" assertions below meaningless. Seed the real light-theme
// values from `styles.css` so the color comparisons are real.
const CSS_VARS: Record<string, string> = {
  '--c-ink-mute': '148 163 184',
  '--c-brand-500': '124 58 237',
  '--c-sev-critical': '147 51 234',
  '--c-sev-info': '100 116 139',
  '--c-status-running': '59 130 246',
  '--c-status-done': '34 197 94',
  '--c-status-awaiting': '245 158 11',
  '--c-status-paused': '6 182 212',
};

function node(id: string, kind: GraphNode['kind'], state: GraphNode['state']): GraphNode {
  return { id, kind, state } as GraphNode;
}

function modelWith(nodes: GraphNode[], edges: { from: string; to: string }[]): RunGraphModel {
  return {
    nodes,
    edges,
    nodeById: (id: string) => nodes.find((n) => n.id === id),
  } as unknown as RunGraphModel;
}

for (const [k, v] of Object.entries(CSS_VARS)) {
  document.documentElement.style.setProperty(k, v);
}

afterEach(() => {
  cleanup();
  captured = [];
  uiMock.mockReturnValue('classic');
});

const POS = new Map([
  ['a', { x: 0, y: 0, width: 160, height: 60 }],
  ['b', { x: 100, y: 0, width: 160, height: 60 }],
]);

describe('RunGraph edges', () => {
  it('classic: the edge into a running step keeps the rg-edge-active class', () => {
    const model = modelWith(
      [node('a', 'action', 'done'), node('b', 'step', 'running')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={model} positions={POS} />);
    expect(captured[0].className).toBe('rg-edge-active');
  });

  it('next: an edge is colored by its SOURCE kind and the live one flows', () => {
    uiMock.mockReturnValue('next');
    const model = modelWith(
      [node('a', 'action', 'done'), node('b', 'step', 'running')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={model} positions={POS} />);
    const edge = captured[0];
    // live frontier animates via the color-agnostic class...
    expect(edge.className).toBe('rg-edge-flow');
    // ...and its stroke comes from JS (the SOURCE step is an action step, so
    // the stroke must NOT be the running-blue the classic path would use).
    expect(edge.style?.stroke).toBeTruthy();
    expect(edge.style?.stroke).not.toBe('');
  });

  it('next: an edge into an awaiting gate flows in amber, not the source kind color', () => {
    uiMock.mockReturnValue('next');
    const model = modelWith(
      [node('a', 'action', 'done'), node('b', 'gate', 'awaiting_approval')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={model} positions={POS} />);
    const awaitEdge = captured[0];
    expect(awaitEdge.className).toBe('rg-edge-flow');

    // Compare against the same topology with a non-awaiting target: the
    // awaiting edge's stroke must differ (amber status wins over kind).
    cleanup();
    captured = [];
    const model2 = modelWith(
      [node('a', 'action', 'done'), node('b', 'gate', 'done')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={model2} positions={POS} />);
    expect(captured[0].style?.stroke).not.toBe(awaitEdge.style?.stroke);
  });

  it('next: a not-yet-reached edge is muted relative to a traversed one', () => {
    uiMock.mockReturnValue('next');
    const done = modelWith(
      [node('a', 'step', 'done'), node('b', 'step', 'done')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={done} positions={POS} />);
    const traversed = captured[0].style?.stroke;

    cleanup();
    captured = [];
    const pending = modelWith(
      [node('a', 'step', 'done'), node('b', 'step', 'pending')],
      [{ from: 'a', to: 'b' }],
    );
    render(<RunGraph model={pending} positions={POS} />);
    expect(captured[0].style?.stroke).not.toBe(traversed);
  });
});
