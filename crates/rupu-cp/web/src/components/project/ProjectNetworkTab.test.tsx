// @vitest-environment jsdom
// ProjectNetworkTab — project-scoped netflow aggregate. Self-fetches on the
// `wsId` prop the way ProjectCoverageTab does (this tab body only mounts
// when the "Network" tab is active, so mount-time fetch already IS the
// lazy-load — no ref-guard needed the way RunDetail's longer-lived tab body
// needs one). Exercises the fetch wiring (project-scoped calls, re-fetch on
// wsId change) and the dropped-flows surfacing; the netflow components'
// own tests cover their internals.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';

const { fetchProjectNetflow, fetchNetflowGraph } = vi.hoisted(() => ({
  fetchProjectNetflow: vi.fn(),
  fetchNetflowGraph: vi.fn(),
}));
vi.mock('../../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../../lib/netflow')>('../../lib/netflow');
  return { ...actual, fetchProjectNetflow, fetchNetflowGraph };
});

import ProjectNetworkTab from './ProjectNetworkTab';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// `restoreAllMocks` (above) doesn't clear call history for a plain
// `vi.fn()` created via `vi.hoisted` the way it does for a `vi.spyOn`
// spy — without this, exact-call-count assertions (added below, Important
// 5) see every prior test's calls accumulated onto the same mock.
beforeEach(() => {
  fetchProjectNetflow.mockReset();
  fetchNetflowGraph.mockReset();
});

describe('ProjectNetworkTab', () => {
  it('fetches project-scoped netflow and graph, and renders the table once loaded', async () => {
    fetchProjectNetflow.mockResolvedValue({
      flows: [
        {
          id: '1', ts: '2026-08-03T00:00:00Z',
          ctx: { origin: { kind: 'provider', name: 'anthropic' } },
          fidelity: 'http', method: 'POST', scheme: 'https',
          host: 'api.anthropic.com', port: 443, path: '/v1/messages',
          outcome: 'ok', body_complete: true,
          bytes_in: 200, bytes_out: 50, duration_ms: 340,
        },
      ],
      hosts: [{ host: 'api.anthropic.com', port: 443, calls: 1, bytes_in: 200, bytes_out: 50, errors: 0 }],
      dropped_total: 0,
      asn_loaded: true,
      window: { from: null, to: null },
    });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<ProjectNetworkTab wsId="ws-1" />);

    expect(fetchProjectNetflow).toHaveBeenCalledWith('ws-1');
    expect(fetchNetflowGraph).toHaveBeenCalledWith('project:ws-1');
    await waitFor(() => expect(screen.getAllByText('api.anthropic.com').length).toBeGreaterThan(0));
  });

  it('surfaces a non-zero dropped count alongside surviving flows', async () => {
    fetchProjectNetflow.mockResolvedValue({
      flows: [
        {
          id: '1', ts: '2026-08-03T00:00:00Z',
          ctx: { origin: { kind: 'provider', name: 'anthropic' } },
          fidelity: 'http', method: 'POST', scheme: 'https',
          host: 'api.anthropic.com', port: 443, path: '/v1/messages',
          outcome: 'ok', body_complete: true,
          bytes_in: 200, bytes_out: 50, duration_ms: 340,
        },
      ],
      hosts: [],
      dropped_total: 4,
      asn_loaded: true,
      window: { from: null, to: null },
    });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<ProjectNetworkTab wsId="ws-1" />);

    await waitFor(() => expect(screen.getByText(/4 flows dropped/i)).toBeInTheDocument());
  });

  it('surfaces a non-zero dropped count even when every flow was dropped', async () => {
    // Fix round 1: NetflowTable's dropped banner now renders alongside its
    // empty state rather than being unreachable behind it.
    fetchProjectNetflow.mockResolvedValue({
      flows: [],
      hosts: [],
      dropped_total: 6,
      asn_loaded: true,
      window: { from: null, to: null },
    });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<ProjectNetworkTab wsId="ws-1" />);

    await waitFor(() => expect(screen.getByText(/6 flows dropped/i)).toBeInTheDocument());
    expect(screen.getByText(/No network flows recorded/i)).toBeInTheDocument();
  });

  it('states the scope limit and the non-HTTP blind spot even when flows exist', async () => {
    // Fix round 1: this disclosure used to be missing from the project
    // panel entirely — NetflowTable's own hint only fires on its empty
    // branch, so a project with real traffic (the common case) never
    // showed it. Asserted here unconditionally, i.e. with non-empty flows.
    fetchProjectNetflow.mockResolvedValue({
      flows: [
        {
          id: '1', ts: '2026-08-03T00:00:00Z',
          ctx: { origin: { kind: 'provider', name: 'anthropic' } },
          fidelity: 'http', method: 'POST', scheme: 'https',
          host: 'api.anthropic.com', port: 443, path: '/v1/messages',
          outcome: 'ok', body_complete: true,
          bytes_in: 200, bytes_out: 50, duration_ms: 340,
        },
      ],
      hosts: [{ host: 'api.anthropic.com', port: 443, calls: 1, bytes_in: 200, bytes_out: 50, errors: 0 }],
      dropped_total: 0,
      asn_loaded: true,
      window: { from: null, to: null },
    });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<ProjectNetworkTab wsId="ws-1" />);

    expect(screen.getByText(/bash subprocess/i)).toBeInTheDocument();
    expect(screen.getByText(/git2|object.store|WebSocket/i)).toBeInTheDocument();
    await waitFor(() => expect(screen.getAllByText('api.anthropic.com').length).toBeGreaterThan(0));
  });

  it('re-fetches when wsId changes', async () => {
    fetchProjectNetflow.mockResolvedValue({
      flows: [],
      hosts: [],
      dropped_total: 0,
      asn_loaded: true,
      window: { from: null, to: null },
    });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    const { rerender } = render(<ProjectNetworkTab wsId="ws-1" />);
    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledWith('ws-1'));

    rerender(<ProjectNetworkTab wsId="ws-2" />);
    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledWith('ws-2'));
  });

  // Important 5 (whole-branch review round 1): nothing previously asserted
  // `data.window` actually reaches `NetflowTable`'s `appliedWindow` prop,
  // or that changing the picker's range re-fetches at all.

  it('threads the server-echoed window into the table, producing the range-aware empty state', async () => {
    fetchProjectNetflow.mockResolvedValue({
      flows: [],
      hosts: [],
      dropped_total: 0,
      asn_loaded: true,
      window: { from: '2026-08-17T14:00:00Z', to: null },
    });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<ProjectNetworkTab wsId="ws-1" />);

    await waitFor(() =>
      expect(screen.getByText(/No network flows in this range/i)).toBeInTheDocument(),
    );
  });

  it('re-fetches with the new range when the time-range picker selection changes', async () => {
    fetchProjectNetflow.mockResolvedValue({
      flows: [],
      hosts: [],
      dropped_total: 0,
      asn_loaded: true,
      window: { from: null, to: null },
    });
    fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] });

    render(<ProjectNetworkTab wsId="ws-1" />);
    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledTimes(1));
    expect(fetchProjectNetflow).toHaveBeenCalledWith('ws-1');

    fireEvent.click(screen.getByRole('button', { name: /last hour/i }));

    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledTimes(2));
    const secondCallArgs = fetchProjectNetflow.mock.calls[1];
    expect(secondCallArgs[0]).toBe('ws-1');
    expect(secondCallArgs[1]).toHaveProperty('from');
    expect(secondCallArgs[1].to).toBeUndefined();
  });
});
