// @vitest-environment jsdom
// Global Netflow page — the union scope: every flow across every run plus
// `system`-origin egress that carries no run_id (in practice, only the
// rare passive update-notice check — CP's own fleet traffic and its
// ASN-table refresh are both wired to `NullSink` and recorded nowhere;
// see `ScopeDisclosure.tsx`'s header comment). This file only exercises
// the load/render contract (data arrives → summary + table render;
// dropped flows surface even with zero rows); the netflow components' own
// tests cover their internals.

import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';

const { fetchGlobalNetflow, fetchNetflowGraph } = vi.hoisted(() => ({
  fetchGlobalNetflow: vi.fn(),
  fetchNetflowGraph: vi.fn(),
}));
vi.mock('../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../lib/netflow')>('../lib/netflow');
  return { ...actual, fetchGlobalNetflow, fetchNetflowGraph };
});

import Netflow from './Netflow';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Netflow global page', () => {
  it('renders the summary and table once loaded', async () => {
    fetchGlobalNetflow.mockResolvedValue({
      flows: [
        {
          id: '1', ts: '2026-08-03T00:00:00Z',
          ctx: { origin: { kind: 'update' } },
          fidelity: 'http', method: 'GET', scheme: 'https',
          host: 'api.github.com', port: 443, path: '/releases',
          outcome: 'ok', body_complete: true,
          bytes_in: 10, bytes_out: 5, duration_ms: 12,
        },
      ],
      hosts: [{ host: 'api.github.com', port: 443, calls: 1, bytes_in: 10, bytes_out: 5, errors: 0, p50_ms: 12, p95_ms: 12 }],
      dropped_total: 0,
      asn_loaded: true,
    });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<MemoryRouter><Netflow /></MemoryRouter>);

    // Renders in both NetflowSummary (host rollup) and NetflowTable (per-flow
    // row) — both are mounted, hence >=1 rather than a single unique match.
    await waitFor(() => expect(screen.getAllByText('api.github.com').length).toBeGreaterThan(0));
  });

  it('surfaces the dropped count at global scope, even when every flow was dropped', async () => {
    // Fix round 1: this test used to assert only the empty-state text,
    // which passed even when the dropped count silently vanished — the
    // name overclaimed what it verified. NetflowTable now renders the
    // dropped banner alongside the empty state rather than instead of it.
    fetchGlobalNetflow.mockResolvedValue({ flows: [], hosts: [], dropped_total: 9, asn_loaded: true });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<MemoryRouter><Netflow /></MemoryRouter>);
    await waitFor(() =>
      expect(screen.getByText(/No network flows recorded/i)).toBeInTheDocument(),
    );
    expect(screen.getByText(/9 flows dropped/i)).toBeInTheDocument();
  });

  it('fetches global scope by omitting the scope param entirely', async () => {
    fetchGlobalNetflow.mockResolvedValue({ flows: [], hosts: [], dropped_total: 0, asn_loaded: true });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<MemoryRouter><Netflow /></MemoryRouter>);

    await waitFor(() => expect(fetchNetflowGraph).toHaveBeenCalled());
    expect(fetchNetflowGraph).toHaveBeenCalledWith();
  });

  it('states the scope limit and the non-HTTP blind spot up front', async () => {
    fetchGlobalNetflow.mockResolvedValue({ flows: [], hosts: [], dropped_total: 0, asn_loaded: true });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<MemoryRouter><Netflow /></MemoryRouter>);

    expect(screen.getByText(/bash subprocess/i)).toBeInTheDocument();
    expect(screen.getByText(/git2|object.store|WebSocket/i)).toBeInTheDocument();
  });
});
