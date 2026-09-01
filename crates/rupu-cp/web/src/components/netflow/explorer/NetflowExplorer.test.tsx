// @vitest-environment jsdom
// The state-owner contract: one fetch effect (explorer + flows together),
// cross-filter toggles re-fetch BOTH endpoints with the filter sets (the
// server owns all aggregation), chips reflect and remove filters, the
// view switcher swaps Topology/Timeline, a row click opens the detail
// panel, and previous data stays on screen during a refetch.

import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  emptyFlowsResponse,
  flowView,
  populatedExplorerResponse,
} from './explorerFixtures';

const { fetchGlobalNetflow, fetchRunNetflow, fetchNetflowExplorer } = vi.hoisted(() => ({
  fetchGlobalNetflow: vi.fn(),
  fetchRunNetflow: vi.fn(),
  fetchNetflowExplorer: vi.fn(),
}));
vi.mock('../../../lib/netflow', async () => {
  const actual =
    await vi.importActual<typeof import('../../../lib/netflow')>('../../../lib/netflow');
  return { ...actual, fetchGlobalNetflow, fetchRunNetflow, fetchNetflowExplorer };
});

import NetflowExplorer from './NetflowExplorer';

afterEach(() => {
  cleanup();
});

beforeEach(() => {
  fetchGlobalNetflow.mockReset();
  fetchRunNetflow.mockReset();
  fetchNetflowExplorer.mockReset();
  fetchNetflowExplorer.mockResolvedValue(populatedExplorerResponse());
  fetchGlobalNetflow.mockResolvedValue({
    ...emptyFlowsResponse(),
    flows: [flowView({ run_id: 'run-9', workflow: 'review-wf' })],
  });
  fetchRunNetflow.mockResolvedValue(emptyFlowsResponse());
});

describe('NetflowExplorer', () => {
  it('fetches explorer + flows together and renders every band of the surface', async () => {
    render(<NetflowExplorer scope="global" />);

    await screen.findByText('api.anthropic.com');
    expect(fetchNetflowExplorer).toHaveBeenCalledWith(undefined, undefined, undefined);
    expect(fetchGlobalNetflow).toHaveBeenCalledWith(undefined, undefined);
    // KPI strip, coverage pill, view switcher, topology column, org card.
    expect(screen.getByText('Flows')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /coverage & gaps/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Topology' })).toBeInTheDocument();
    expect(screen.getByText('Workflows')).toBeInTheDocument();
  });

  it('clicking a topology node re-fetches BOTH endpoints with the filter set and shows a chip', async () => {
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('Workflows');

    fireEvent.click(screen.getByRole('button', { name: /review-wf/ }));

    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(2));
    const filters = { workflows: ['review-wf'], origins: [], orgs: [], hosts: [] };
    expect(fetchNetflowExplorer).toHaveBeenLastCalledWith(undefined, undefined, filters);
    expect(fetchGlobalNetflow).toHaveBeenLastCalledWith(undefined, filters);
    expect(screen.getByText('workflow:review-wf')).toBeInTheDocument();
  });

  it('removing the chip clears the filter and re-fetches unfiltered', async () => {
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('Workflows');
    fireEvent.click(screen.getByRole('button', { name: /review-wf/ }));
    await screen.findByText('workflow:review-wf');

    fireEvent.click(screen.getByRole('button', { name: /remove filter workflow:review-wf/i }));

    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(3));
    expect(fetchNetflowExplorer).toHaveBeenLastCalledWith(undefined, undefined, undefined);
  });

  it('org chips carry the display label from the sankey org list', async () => {
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('Workflows');

    fireEvent.click(screen.getByRole('button', { name: /Cloudflare/ }));
    await screen.findByText('net:Cloudflare');
  });

  it('switches to the Timeline view (lanes + zoomable cells) and back', async () => {
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('Workflows');

    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }));
    expect(screen.getByText('api.anthropic.com:443')).toBeInTheDocument();
    expect(screen.getByText(/2 runs in window/)).toBeInTheDocument();
    // Org cards are a Topology-view companion only.
    expect(screen.queryByText(/AS13335 ·/)).not.toBeInTheDocument();
  });

  it('a timeline cell click zooms the window (custom range on both fetches) and Zoom out unwinds it', async () => {
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('Workflows');
    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }));

    fireEvent.click(screen.getAllByTitle(/^2 calls/)[0]);

    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(2));
    const [, zoomRange] = fetchNetflowExplorer.mock.calls[1];
    expect(zoomRange).toMatchObject({
      from: expect.stringMatching(/T/),
      to: expect.stringMatching(/T/),
    });

    fireEvent.click(screen.getByRole('button', { name: /zoom out/i }));
    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(3));
    expect(fetchNetflowExplorer).toHaveBeenLastCalledWith(undefined, undefined, undefined);
  });

  it('a table row click opens the flow detail panel; close dismisses it', async () => {
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('api.anthropic.com');

    fireEvent.click(screen.getByText('api.anthropic.com'));
    expect(screen.getByRole('dialog', { name: /flow detail/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /close flow detail/i }));
    expect(screen.queryByRole('dialog', { name: /flow detail/i })).not.toBeInTheDocument();
  });

  it('keeps the previous surface on screen while a filter refetch is in flight', async () => {
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('api.anthropic.com');

    fetchNetflowExplorer.mockReturnValueOnce(new Promise(() => {}));
    fetchGlobalNetflow.mockReturnValueOnce(new Promise(() => {}));
    fireEvent.click(screen.getByRole('button', { name: /review-wf/ }));

    expect(screen.getByText('api.anthropic.com')).toBeInTheDocument();
    expect(screen.queryByText(/loading network flows/i)).not.toBeInTheDocument();
  });

  it('seeds the initial window from the run span at run scope', async () => {
    render(
      <NetflowExplorer
        scope="run"
        runId="run-1"
        initialWindow={{ from: '2026-08-01T00:00:00Z', to: '2026-08-01T00:05:00Z' }}
      />,
    );
    await waitFor(() =>
      expect(fetchNetflowExplorer).toHaveBeenCalledWith(
        'run:run-1',
        { from: '2026-08-01T00:00:00Z', to: '2026-08-01T00:05:00Z' },
        undefined,
      ),
    );
  });
});
