// @vitest-environment jsdom
// Project Network tab — a thin mount of the shared <NetflowExplorer
// scope="project">. This file exercises the tab's own contract: the
// project scope param + project-id threading, the `key={wsId}` remount on
// a project switch, the popover-carried disclosure, and the server-echo
// empty-state branching. Explorer internals are covered by the explorer
// component tests.

import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NetflowResponse } from '../../lib/netflow';
import {
  emptyExplorerResponse,
  emptyFlowsResponse,
  flowView,
} from '../netflow/explorer/explorerFixtures';

const { fetchProjectNetflow, fetchNetflowExplorer } = vi.hoisted(() => ({
  fetchProjectNetflow: vi.fn(),
  fetchNetflowExplorer: vi.fn(),
}));
vi.mock('../../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../../lib/netflow')>('../../lib/netflow');
  return { ...actual, fetchProjectNetflow, fetchNetflowExplorer };
});

import ProjectNetworkTab from './ProjectNetworkTab';

afterEach(() => {
  cleanup();
});

beforeEach(() => {
  fetchProjectNetflow.mockReset();
  fetchNetflowExplorer.mockReset();
  fetchProjectNetflow.mockResolvedValue(emptyFlowsResponse());
  fetchNetflowExplorer.mockResolvedValue(emptyExplorerResponse());
});

describe('ProjectNetworkTab', () => {
  it('fetches project-scoped explorer + flows and renders the table once loaded', async () => {
    fetchProjectNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      flows: [flowView({ run_id: 'run-1', workflow: 'review-wf' })],
    } satisfies NetflowResponse);
    render(<ProjectNetworkTab wsId="ws-1" />);

    await waitFor(() =>
      expect(fetchNetflowExplorer).toHaveBeenCalledWith('project:ws-1', undefined, undefined),
    );
    expect(fetchProjectNetflow).toHaveBeenCalledWith('ws-1', undefined, undefined);
    await screen.findByText('api.anthropic.com');
    // Attribution columns show at project scope too.
    expect(screen.getByText('review-wf')).toBeInTheDocument();
  });

  it('surfaces the scope disclosure through the coverage popover', async () => {
    render(<ProjectNetworkTab wsId="ws-1" />);
    await screen.findByRole('button', { name: /coverage & gaps/i });

    fireEvent.click(screen.getByRole('button', { name: /coverage & gaps/i }));
    // (`git2 clones` is unique to the full disclosure; the table's
    // empty-state hint shares the scope-limit phrasing.)
    expect(screen.getByText(/git2 clones/i)).toBeInTheDocument();
  });

  it('keeps the loud in-place banner when every flow was dropped (empty table)', async () => {
    fetchNetflowExplorer.mockResolvedValue({ ...emptyExplorerResponse(), dropped_total: 4 });
    fetchProjectNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      dropped_total: 4,
    } satisfies NetflowResponse);
    render(<ProjectNetworkTab wsId="ws-1" />);

    await screen.findByText(/4 flows dropped/i);
  });

  it('re-fetches when wsId changes (key remount resets all view state)', async () => {
    const { rerender } = render(<ProjectNetworkTab wsId="ws-1" />);
    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledWith('ws-1', undefined, undefined));

    rerender(<ProjectNetworkTab wsId="ws-2" />);
    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledWith('ws-2', undefined, undefined));
  });

  it('threads the server-echoed window into the table, producing the range-aware empty state', async () => {
    fetchProjectNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      window: { from: '2026-08-17T14:00:00Z', to: null },
    } satisfies NetflowResponse);
    render(<ProjectNetworkTab wsId="ws-1" />);

    await screen.findByText(/no network flows in this range/i);
  });

  it('re-fetches with the new range when the time-range picker selection changes', async () => {
    render(<ProjectNetworkTab wsId="ws-1" />);
    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole('button', { name: /last hour/i }));

    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledTimes(2));
    const [, range] = fetchProjectNetflow.mock.calls[1];
    expect(range).toMatchObject({ from: expect.stringMatching(/T/) });
  });
});
