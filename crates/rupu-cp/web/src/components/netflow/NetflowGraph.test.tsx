// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import NetflowGraph from './NetflowGraph';
import type { GraphView } from '../../lib/netflow';

afterEach(() => {
  cleanup();
});

const graph: GraphView = {
  nodes: [
    { id: 'run-1', label: 'run-1', side: 'source' },
    { id: 'api.anthropic.com:443', label: 'api.anthropic.com', side: 'endpoint' },
  ],
  edges: [{ from: 'run-1', to: 'api.anthropic.com:443', calls: 3, bytes: 300, errors: 0 }],
};

describe('NetflowGraph', () => {
  it('labels both sides of the topology', () => {
    render(<NetflowGraph graph={graph} />);
    expect(screen.getByText('run-1')).toBeInTheDocument();
    expect(screen.getByText('api.anthropic.com')).toBeInTheDocument();
  });

  it('draws one line per edge', () => {
    const { container } = render(<NetflowGraph graph={graph} />);
    expect(container.querySelectorAll('line')).toHaveLength(1);
  });

  it('renders an explicit empty state rather than a blank canvas', () => {
    render(<NetflowGraph graph={{ nodes: [], edges: [] }} />);
    expect(screen.getByText(/No flows to graph/i)).toBeInTheDocument();
    expect(document.querySelector('svg')).not.toBeInTheDocument();
  });

  it('never surfaces GraphEdge.bytes as a byte count, even for a zero-weight edge', () => {
    // A Coarse-only flow has bytes: 0 (see GraphEdge.bytes doc in
    // lib/netflow.ts) — the edge must still render (thin), and the number
    // 0 must never appear as a rendered "0 B"-style claim anywhere.
    const coarseGraph: GraphView = {
      nodes: [
        { id: 'run-1', label: 'run-1', side: 'source' },
        { id: 'host:443', label: 'host', side: 'endpoint' },
      ],
      edges: [{ from: 'run-1', to: 'host:443', calls: 1, bytes: 0, errors: 0 }],
    };
    const { container } = render(<NetflowGraph graph={coarseGraph} weightBy="bytes" />);
    expect(container.querySelectorAll('line')).toHaveLength(1);
    expect(screen.queryByText(/0 B/)).not.toBeInTheDocument();
  });

  it('marks an errored edge distinctly from a clean one', () => {
    const errGraph: GraphView = {
      nodes: [
        { id: 'run-1', label: 'run-1', side: 'source' },
        { id: 'run-2', label: 'run-2', side: 'source' },
        { id: 'host:443', label: 'host', side: 'endpoint' },
      ],
      edges: [
        { from: 'run-1', to: 'host:443', calls: 1, bytes: 10, errors: 0 },
        { from: 'run-2', to: 'host:443', calls: 1, bytes: 10, errors: 1 },
      ],
    };
    const { container } = render(<NetflowGraph graph={errGraph} />);
    const lines = Array.from(container.querySelectorAll('line'));
    expect(lines).toHaveLength(2);
    const strokes = new Set(lines.map((l) => l.getAttribute('stroke')));
    expect(strokes.size).toBe(2);
  });
});
