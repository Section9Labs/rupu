// @vitest-environment jsdom
// Global Network page — a header over the shared <NetflowExplorer
// scope="global">. What this file exercises is the PAGE contract: global
// scope omits the scope param entirely, attribution columns show at this
// scope, the coverage popover carries the scope disclosure + dropped
// accounting (the v3 "honesty on demand" move), the all-dropped empty
// case still gets the loud banner, empty-state wording branches on the
// SERVER's window echo, it opens on the default window, and a since: term
// re-fetches with the new range. Explorer internals (filters, views, zoom)
// are covered by the explorer component tests.

import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NetflowResponse } from '../lib/netflow';
import {
  emptyExplorerResponse,
  emptyFlowsResponse,
  flowView,
} from '../components/netflow/explorer/explorerFixtures';

const { fetchGlobalNetflow, fetchNetflowExplorer, fetchNetflowLatest } = vi.hoisted(() => ({
  fetchGlobalNetflow: vi.fn(),
  fetchNetflowExplorer: vi.fn(),
  fetchNetflowLatest: vi.fn(),
}));
vi.mock('../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../lib/netflow')>('../lib/netflow');
  return { ...actual, fetchGlobalNetflow, fetchNetflowExplorer, fetchNetflowLatest };
});

import Netflow from './Netflow';
import { MemoryRouter } from 'react-router-dom';
import { scopedEntry, withCustomerScope } from '../lib/customerScopeTestUtils';
import { commitQuery } from '../components/query/queryTestUtils';

const NOW = new Date('2026-10-06T12:00:00Z');
const HOUR = { from: '2026-10-06T11:00:00.000Z' };
const DAY = { from: '2026-10-05T12:00:00.000Z' };

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/netflow']}>
      <Netflow />
    </MemoryRouter>,
  );
}

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(NOW);
  fetchGlobalNetflow.mockReset();
  fetchNetflowExplorer.mockReset();
  fetchNetflowLatest.mockReset();
  fetchNetflowLatest.mockResolvedValue({ latest: null });
  fetchGlobalNetflow.mockResolvedValue(emptyFlowsResponse());
  fetchNetflowExplorer.mockResolvedValue(emptyExplorerResponse());
});

describe('Netflow global page', () => {
  it('fetches global scope by omitting the scope param entirely', async () => {
    renderPage();
    await waitFor(() =>
      expect(fetchNetflowExplorer).toHaveBeenCalledWith(undefined, HOUR, undefined),
    );
    expect(fetchNetflowLatest).toHaveBeenCalledWith(undefined);
    expect(fetchGlobalNetflow).toHaveBeenCalledWith(HOUR, undefined);
  });

  it('renders the table once loaded, with Run/Workflow attribution columns at this scope', async () => {
    fetchGlobalNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      flows: [flowView({ run_id: 'run-9', workflow: 'review-wf' })],
    } satisfies NetflowResponse);
    renderPage();

    await screen.findByText('api.anthropic.com');
    expect(screen.getByText('run-9')).toBeInTheDocument();
    expect(screen.getByText('review-wf')).toBeInTheDocument();
  });

  it('surfaces the scope disclosure through the coverage popover', async () => {
    renderPage();
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
    renderPage();
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
    renderPage();

    await screen.findByText(/9 flows dropped/i);
    expect(screen.getByText(/No network flows recorded/i)).toBeInTheDocument();
  });

  it('threads the server-echoed window into the table, producing the range-aware empty state', async () => {
    fetchGlobalNetflow.mockResolvedValue({
      ...emptyFlowsResponse(),
      window: { from: '2026-08-17T14:00:00Z', to: null },
    } satisfies NetflowResponse);
    renderPage();

    await screen.findByText(/no network flows in this range/i);
  });

  it('re-fetches with the new range when the query names a window', async () => {
    renderPage();
    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(1));

    commitQuery('Filter network flows', 'since:24h');

    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(2));
    expect(fetchNetflowExplorer).toHaveBeenLastCalledWith(undefined, DAY, undefined);
    expect(fetchGlobalNetflow).toHaveBeenLastCalledWith(DAY, undefined);
  });
});

describe('Netflow under a customer scope', () => {
  it('says the flows cover every customer while scoped', async () => {
    render(
      <MemoryRouter initialEntries={[scopedEntry('acme', '/netflow')]}>{withCustomerScope(<Netflow />)}</MemoryRouter>,
    );
    expect(await screen.findByText('Flows here cover every customer.')).toBeInTheDocument();
  });

  it('shows no note unscoped', async () => {
    render(<MemoryRouter initialEntries={['/netflow']}>{withCustomerScope(<Netflow />)}</MemoryRouter>);
    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalled());
    expect(screen.queryByText('Flows here cover every customer.')).toBeNull();
  });
});
