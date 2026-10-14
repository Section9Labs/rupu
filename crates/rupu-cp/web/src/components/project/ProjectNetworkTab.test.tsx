// @vitest-environment jsdom
// Project Network tab — a thin mount of the shared <NetflowExplorer
// scope="project">. This file exercises the tab's own contract: the
// project scope param + project-id threading, the `key={wsId}` remount on
// a project switch, the project's latest run behind its default window,
// the popover-carried disclosure, and the server-echo empty-state
// branching. Explorer internals are covered by the explorer
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

const { fetchProjectNetflow, fetchNetflowExplorer, fetchNetflowLatest } = vi.hoisted(() => ({
  fetchProjectNetflow: vi.fn(),
  fetchNetflowExplorer: vi.fn(),
  fetchNetflowLatest: vi.fn(),
}));
vi.mock('../../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../../lib/netflow')>('../../lib/netflow');
  return { ...actual, fetchProjectNetflow, fetchNetflowExplorer, fetchNetflowLatest };
});

import { MemoryRouter } from 'react-router-dom';
import ProjectNetworkTab from './ProjectNetworkTab';
import { commitQuery } from '../query/queryTestUtils';

const NOW = new Date('2026-10-06T12:00:00Z');
const HOUR = { from: '2026-10-06T11:00:00.000Z' };
const DAY = { from: '2026-10-05T12:00:00.000Z' };

const wrapper = ({ children }: { children: React.ReactNode }) => (
  <MemoryRouter initialEntries={['/projects/ws-1/network']}>{children}</MemoryRouter>
);

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(NOW);
  fetchProjectNetflow.mockReset();
  fetchNetflowExplorer.mockReset();
  fetchNetflowLatest.mockReset();
  fetchNetflowLatest.mockResolvedValue({ latest: null });
  fetchProjectNetflow.mockResolvedValue(emptyFlowsResponse());
  fetchNetflowExplorer.mockResolvedValue(emptyExplorerResponse());
});

describe('ProjectNetworkTab', () => {
  it('fetches project-scoped explorer + flows and renders the table once loaded', async () => {
    fetchProjectNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      flows: [flowView({ run_id: 'run-1', workflow: 'review-wf' })],
    } satisfies NetflowResponse);
    render(<ProjectNetworkTab wsId="ws-1" />, { wrapper });

    await waitFor(() =>
      expect(fetchNetflowExplorer).toHaveBeenCalledWith('project:ws-1', HOUR, undefined),
    );
    expect(fetchNetflowLatest).toHaveBeenCalledWith('project:ws-1');
    expect(fetchProjectNetflow).toHaveBeenCalledWith('ws-1', HOUR, undefined);
    await screen.findByText('api.anthropic.com');
    // Attribution columns show at project scope too.
    expect(screen.getByText('review-wf')).toBeInTheDocument();
  });

  it('surfaces the scope disclosure through the coverage popover', async () => {
    render(<ProjectNetworkTab wsId="ws-1" />, { wrapper });
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
    render(<ProjectNetworkTab wsId="ws-1" />, { wrapper });

    await screen.findByText(/4 flows dropped/i);
  });

  it('re-fetches when wsId changes (key remount resets all view state)', async () => {
    const { rerender } = render(<ProjectNetworkTab wsId="ws-1" />, { wrapper });
    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledWith('ws-1', HOUR, undefined));

    rerender(<ProjectNetworkTab wsId="ws-2" />);
    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledWith('ws-2', HOUR, undefined));
    expect(fetchNetflowLatest).toHaveBeenCalledWith('project:ws-2');
  });

  it('threads the server-echoed window into the table, producing the range-aware empty state', async () => {
    fetchProjectNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      window: { from: '2026-08-17T14:00:00Z', to: null },
    } satisfies NetflowResponse);
    render(<ProjectNetworkTab wsId="ws-1" />, { wrapper });

    await screen.findByText(/no network flows in this range/i);
  });

  it('re-fetches with the new range when the query names a window', async () => {
    render(<ProjectNetworkTab wsId="ws-1" />, { wrapper });
    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledTimes(1));

    commitQuery('Filter network flows', 'since:24h');

    await waitFor(() => expect(fetchProjectNetflow).toHaveBeenCalledTimes(2));
    expect(fetchProjectNetflow).toHaveBeenLastCalledWith('ws-1', DAY, undefined);
  });
});
