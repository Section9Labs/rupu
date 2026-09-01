// @vitest-environment jsdom
// Global Network page — a header over the shared <NetflowExplorer
// scope="global">. What this file exercises is the PAGE contract: global
// scope omits the scope param entirely, attribution columns show at this
// scope, the coverage popover carries the scope disclosure + dropped
// accounting (the v3 "honesty on demand" move), the all-dropped empty
// case still gets the loud banner, empty-state wording branches on the
// SERVER's window echo, and a preset change re-fetches with the new
// range. Explorer internals (filters, views, zoom) are covered by the
// explorer component tests.

import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NetflowResponse } from '../lib/netflow';
import {
  emptyExplorerResponse,
  emptyFlowsResponse,
  flowView,
} from '../components/netflow/explorer/explorerFixtures';

const { fetchGlobalNetflow, fetchNetflowExplorer } = vi.hoisted(() => ({
  fetchGlobalNetflow: vi.fn(),
  fetchNetflowExplorer: vi.fn(),
}));
vi.mock('../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../lib/netflow')>('../lib/netflow');
  return { ...actual, fetchGlobalNetflow, fetchNetflowExplorer };
});

import Netflow from './Netflow';

afterEach(() => {
  cleanup();
});

beforeEach(() => {
  fetchGlobalNetflow.mockReset();
  fetchNetflowExplorer.mockReset();
  fetchGlobalNetflow.mockResolvedValue(emptyFlowsResponse());
  fetchNetflowExplorer.mockResolvedValue(emptyExplorerResponse());
});

describe('Netflow global page', () => {
  it('fetches global scope by omitting the scope param entirely', async () => {
    render(<Netflow />);
    await waitFor(() =>
      expect(fetchNetflowExplorer).toHaveBeenCalledWith(undefined, undefined, undefined),
    );
    expect(fetchGlobalNetflow).toHaveBeenCalledWith(undefined, undefined);
  });

  it('renders the table once loaded, with Run/Workflow attribution columns at this scope', async () => {
    fetchGlobalNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      flows: [flowView({ run_id: 'run-9', workflow: 'review-wf' })],
    } satisfies NetflowResponse);
    render(<Netflow />);

    await screen.findByText('api.anthropic.com');
    expect(screen.getByText('run-9')).toBeInTheDocument();
    expect(screen.getByText('review-wf')).toBeInTheDocument();
  });

  it('surfaces the scope disclosure through the coverage popover', async () => {
    render(<Netflow />);
    await screen.findByRole('button', { name: /coverage & gaps/i });

    fireEvent.click(screen.getByRole('button', { name: /coverage & gaps/i }));
    // The single-sourced disclosure (ScopeDisclosure.tsx) — scope limit +
    // the non-HTTP blind spot — must be reachable here at every scope.
    // (`git2 clones` is unique to the full disclosure text; the shorter
    // empty-state hint below the table shares the scope-limit phrasing.)
    expect(screen.getByText(/git2 clones/i)).toBeInTheDocument();
    expect(screen.getAllByText(/rupu's own egress/i).length).toBeGreaterThan(0);
  });

  it('states the dropped accounting in the popover when loss occurred', async () => {
    fetchNetflowExplorer.mockResolvedValue({ ...emptyExplorerResponse(), dropped_total: 12 });
    fetchGlobalNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      flows: [flowView()],
      dropped_total: 12,
    } satisfies NetflowResponse);
    render(<Netflow />);
    await screen.findByText('api.anthropic.com');

    fireEvent.click(screen.getByRole('button', { name: /coverage & gaps/i }));
    expect(screen.getByText(/12 flows dropped/i)).toBeInTheDocument();
  });

  it('keeps the loud in-place banner when every flow was dropped (empty table)', async () => {
    fetchNetflowExplorer.mockResolvedValue({ ...emptyExplorerResponse(), dropped_total: 9 });
    fetchGlobalNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      dropped_total: 9,
    } satisfies NetflowResponse);
    render(<Netflow />);

    await screen.findByText(/9 flows dropped/i);
    expect(screen.getByText(/No network flows recorded/i)).toBeInTheDocument();
  });

  it('threads the server-echoed window into the table, producing the range-aware empty state', async () => {
    fetchGlobalNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      window: { from: '2026-08-17T14:00:00Z', to: null },
    } satisfies NetflowResponse);
    render(<Netflow />);

    await screen.findByText(/no network flows in this range/i);
  });

  it('re-fetches with the new range when a preset is selected', async () => {
    render(<Netflow />);
    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole('button', { name: /last hour/i }));

    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(2));
    const [, range] = fetchNetflowExplorer.mock.calls[1];
    expect(range).toMatchObject({ from: expect.stringMatching(/T/) });
    const [flowsRange] = fetchGlobalNetflow.mock.calls[1];
    expect(flowsRange).toMatchObject({ from: expect.stringMatching(/T/) });
  });
});
