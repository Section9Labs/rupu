// @vitest-environment jsdom
// The state-owner contract: the query line is the window and the
// cross-filters; it opens on the default window; graph clicks and zooms
// write terms into it; the reads carry what it asks for (the server owns all
// aggregation); a row click opens the detail panel, and previous data stays
// on screen during a refetch.

import '@testing-library/jest-dom/vitest';
import { act, render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { EMPTY_NETFLOW_FILTERS } from '../../../lib/netflow';
import { commitQuery } from '../../query/queryTestUtils';
import {
  emptyFlowsResponse,
  flowView,
  populatedExplorerResponse,
} from './explorerFixtures';

const { fetchGlobalNetflow, fetchRunNetflow, fetchNetflowExplorer, fetchNetflowLatest } = vi.hoisted(() => ({
  fetchGlobalNetflow: vi.fn(),
  fetchRunNetflow: vi.fn(),
  fetchNetflowExplorer: vi.fn(),
  fetchNetflowLatest: vi.fn(),
}));
vi.mock('../../../lib/netflow', async () => {
  const actual =
    await vi.importActual<typeof import('../../../lib/netflow')>('../../../lib/netflow');
  return { ...actual, fetchGlobalNetflow, fetchRunNetflow, fetchNetflowExplorer, fetchNetflowLatest };
});

import NetflowExplorer, { type NetflowExplorerProps } from './NetflowExplorer';

const NOW = new Date('2026-10-06T12:00:00Z');
const HOUR = { from: '2026-10-06T11:00:00.000Z' };
const LABEL = 'Filter network flows';

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(NOW);
  fetchGlobalNetflow.mockReset();
  fetchRunNetflow.mockReset();
  fetchNetflowExplorer.mockReset();
  fetchNetflowLatest.mockReset();
  fetchNetflowLatest.mockResolvedValue({ latest: null });
  fetchNetflowExplorer.mockResolvedValue(populatedExplorerResponse());
  fetchGlobalNetflow.mockResolvedValue({
    ...emptyFlowsResponse(),
    flows: [flowView({ run_id: 'run-9', workflow: 'review-wf' })],
  });
  fetchRunNetflow.mockResolvedValue(emptyFlowsResponse());
});

function renderExplorer(props: NetflowExplorerProps = { scope: 'global' }, url = '/network') {
  return render(
    <MemoryRouter initialEntries={[url]}>
      <NetflowExplorer {...props} />
    </MemoryRouter>,
  );
}

const defaultLine = () => screen.queryByTestId('netflow-default-window')?.textContent ?? null;

describe('NetflowExplorer: the default window', () => {
  it('with no run recorded, opens on the last hour and says so', async () => {
    renderExplorer();
    await screen.findByText('api.anthropic.com');
    expect(fetchNetflowLatest).toHaveBeenCalledWith(undefined);
    expect(fetchNetflowExplorer).toHaveBeenCalledWith(undefined, HOUR, undefined);
    expect(fetchGlobalNetflow).toHaveBeenCalledWith(HOUR, undefined);
    expect(defaultLine()).toBe(
      'Showing the last hour (the default). since: or until: picks another window; window:all shows every flow.',
    );
  });

  it("opens on the latest run's span when it is shorter than an hour", async () => {
    fetchNetflowLatest.mockResolvedValue({
      latest: { run_id: 'run_7', workflow: 'nightly', started_at: '2026-10-05T08:00:00Z', finished_at: '2026-10-05T08:20:00Z' },
    });
    renderExplorer();
    await screen.findByText('api.anthropic.com');
    const span = { from: '2026-10-05T07:55:00.000Z', to: '2026-10-05T08:25:00.000Z' };
    expect(fetchNetflowExplorer).toHaveBeenCalledWith(undefined, span, undefined);
    expect(fetchGlobalNetflow).toHaveBeenCalledWith(span, undefined);
    expect(defaultLine()).toBe(
      "Showing the latest run's span (nightly, run_7: 20 min), the default: it is shorter than the last hour. since: or until: picks another window; window:all shows every flow.",
    );
  });

  it('asks the project for its latest run at project scope', async () => {
    renderExplorer({ scope: 'project', projectId: 'ws_1' });
    await waitFor(() => expect(fetchNetflowLatest).toHaveBeenCalledWith('project:ws_1'));
  });

  it('opens on the last hour when the latest run could not be read, and says why', async () => {
    fetchNetflowLatest.mockRejectedValue(new Error('index down'));
    renderExplorer();
    await screen.findByText('api.anthropic.com');
    expect(fetchNetflowExplorer).toHaveBeenCalledWith(undefined, HOUR, undefined);
    expect(defaultLine()).toMatch(/The latest run couldn't be read: index down\.\)$/);
  });

  it("a run's tab opens on the run's span without asking for the latest run", async () => {
    renderExplorer({ scope: 'run', runId: 'run-1', initialWindow: { from: '2026-08-01T00:00:00Z', to: '2026-08-01T00:05:00Z' } });
    await waitFor(() =>
      expect(fetchNetflowExplorer).toHaveBeenCalledWith('run:run-1', { from: '2026-08-01T00:00:00Z', to: '2026-08-01T00:05:00Z' }, undefined),
    );
    expect(fetchNetflowLatest).not.toHaveBeenCalled();
    expect(defaultLine()).toMatch(/^Showing this run's span \(the default\)/);
  });

  it('run scope without a span opens on every flow', async () => {
    renderExplorer({ scope: 'run', runId: 'run-1' });
    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledWith('run:run-1', {}, undefined));
    expect(fetchRunNetflow).toHaveBeenCalledWith('run-1', {}, undefined);
  });
});

describe('NetflowExplorer: the query', () => {
  it('a window and keyed terms from the URL reach both reads', async () => {
    renderExplorer({ scope: 'global' }, '/network?nq=since:6h%20origin:provider:anthropic%20-host:api.github.com:443');
    await screen.findByText('api.anthropic.com');
    const filters = { ...EMPTY_NETFLOW_FILTERS, origins: ['provider:anthropic'], not_hosts: ['api.github.com:443'] };
    expect(fetchNetflowExplorer).toHaveBeenCalledWith(undefined, { from: '2026-10-06T06:00:00.000Z' }, filters);
    expect(fetchGlobalNetflow).toHaveBeenCalledWith({ from: '2026-10-06T06:00:00.000Z' }, filters);
    expect(defaultLine()).toBeNull();
  });

  it('window:all reads every flow', async () => {
    renderExplorer({ scope: 'global' }, '/network?nq=window:all');
    await screen.findByText('api.anthropic.com');
    expect(fetchNetflowExplorer).toHaveBeenCalledWith(undefined, {}, undefined);
  });

  it('a query the reads cannot take shows why and keeps what was loaded', async () => {
    renderExplorer();
    await screen.findByText('api.anthropic.com');
    commitQuery(LABEL, 'anthropic');
    expect(screen.getByRole('alert')).toHaveTextContent('the network view takes keyed terms only');
    expect(fetchNetflowExplorer).toHaveBeenCalledTimes(1);
    expect(screen.getByText('api.anthropic.com')).toBeInTheDocument();
  });

  it('clicking a topology node writes its term and refetches; clicking it again drops it', async () => {
    renderExplorer();
    await screen.findByText('Workflows');

    fireEvent.click(screen.getByRole('button', { name: /^review-wf/ }));
    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(2));
    const filters = { ...EMPTY_NETFLOW_FILTERS, workflows: ['review-wf'] };
    expect(fetchNetflowExplorer).toHaveBeenLastCalledWith(undefined, HOUR, filters);
    expect(fetchGlobalNetflow).toHaveBeenLastCalledWith(HOUR, filters);
    expect(screen.getByRole('button', { name: 'Remove workflow: review-wf' })).toBeInTheDocument();

    expect(screen.getByRole('button', { name: /^review-wf/ })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(screen.getByRole('button', { name: /^review-wf/ }));
    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(3));
    expect(fetchNetflowExplorer).toHaveBeenLastCalledWith(undefined, HOUR, undefined);
    expect(screen.queryByRole('button', { name: 'Remove workflow: review-wf' })).not.toBeInTheDocument();
  });

  it('an org card click writes the endpoint as a host: term', async () => {
    renderExplorer();
    await screen.findByText('Workflows');
    fireEvent.click(screen.getAllByRole('button', { name: /cdn\.anthropic\.com/ })[0]);
    await waitFor(() =>
      expect(fetchNetflowExplorer).toHaveBeenLastCalledWith(undefined, HOUR, { ...EMPTY_NETFLOW_FILTERS, hosts: ['cdn.anthropic.com:443'] }),
    );
  });

  it('switches to the Timeline view (lanes + zoomable cells) and back', async () => {
    renderExplorer();
    await screen.findByText('Workflows');

    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }));
    expect(screen.getByText('api.anthropic.com:443')).toBeInTheDocument();
    expect(screen.getByText(/2 runs in window/)).toBeInTheDocument();
    // Org cards are a Topology-view companion only.
    expect(screen.queryByText(/AS13335 ·/)).not.toBeInTheDocument();
  });

  it('a timeline cell click writes the zoomed window into the query, and Zoom out unwinds it', async () => {
    renderExplorer();
    await screen.findByText('Workflows');
    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }));

    fireEvent.click(screen.getAllByTitle(/^2 calls/)[0]);

    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(2));
    const [, zoomRange] = fetchNetflowExplorer.mock.calls[1];
    expect(zoomRange).toEqual({ from: expect.stringMatching(/T/), to: expect.stringMatching(/T/) });
    expect(screen.getByRole('button', { name: `Remove since: ${zoomRange.from}` })).toBeInTheDocument();
    expect(defaultLine()).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /zoom out/i }));
    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalledTimes(3));
    expect(fetchNetflowExplorer).toHaveBeenLastCalledWith(undefined, HOUR, undefined);
  });
});

describe('NetflowExplorer: loading and errors', () => {
  it('fetches explorer + flows together and renders every band of the surface', async () => {
    renderExplorer();
    await screen.findByText('api.anthropic.com');
    expect(screen.getByText('Flows')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /coverage & gaps/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Topology' })).toBeInTheDocument();
    expect(screen.getByText('Workflows')).toBeInTheDocument();
  });

  it('a table row click opens the flow detail panel; close dismisses it', async () => {
    renderExplorer();
    await screen.findByText('api.anthropic.com');

    fireEvent.click(screen.getByText('api.anthropic.com'));
    expect(screen.getByRole('dialog', { name: /flow detail/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /close flow detail/i }));
    expect(screen.queryByRole('dialog', { name: /flow detail/i })).not.toBeInTheDocument();
  });

  it('keeps the previous surface on screen while a refetch is in flight', async () => {
    renderExplorer();
    await screen.findByText('api.anthropic.com');

    fetchNetflowExplorer.mockReturnValueOnce(new Promise(() => {}));
    fetchGlobalNetflow.mockReturnValueOnce(new Promise(() => {}));
    fireEvent.click(screen.getByRole('button', { name: /^review-wf/ }));

    expect(screen.getByText('api.anthropic.com')).toBeInTheDocument();
    expect(screen.queryByText(/loading network flows/i)).not.toBeInTheDocument();
  });

  describe('"Updating…"', () => {
    const updating = () => screen.queryByText('Updating…');

    it('is not shown on the initial load', () => {
      fetchNetflowExplorer.mockReturnValue(new Promise(() => {}));
      fetchGlobalNetflow.mockReturnValue(new Promise(() => {}));
      renderExplorer();
      expect(updating()).toBeNull();
    });

    it('is not shown while one section is still on its first load', async () => {
      fetchGlobalNetflow.mockReturnValue(new Promise(() => {}));
      renderExplorer();
      await screen.findByText('Workflows');
      expect(screen.getByText(/loading network flows/i)).toBeInTheDocument();
      expect(updating()).toBeNull();
    });

    it('is shown while a refetch runs over existing data, and cleared when it lands', async () => {
      renderExplorer();
      await screen.findByText('api.anthropic.com');
      expect(updating()).toBeNull();

      let resolveExplorer!: (v: unknown) => void;
      let resolveFlows!: (v: unknown) => void;
      fetchNetflowExplorer.mockReturnValueOnce(new Promise((r) => (resolveExplorer = r)));
      fetchGlobalNetflow.mockReturnValueOnce(new Promise((r) => (resolveFlows = r)));
      fireEvent.click(screen.getByRole('button', { name: /^review-wf/ }));

      expect(screen.getByText('Updating…')).toHaveAttribute('role', 'status');
      expect(screen.getByText('api.anthropic.com')).toBeInTheDocument();

      await act(async () => {
        resolveExplorer(populatedExplorerResponse());
      });
      expect(screen.getByText('Updating…')).toBeInTheDocument();

      await act(async () => {
        resolveFlows({
          ...emptyFlowsResponse(),
          flows: [flowView({ run_id: 'run-9', workflow: 'review-wf' })],
        });
      });
      expect(updating()).toBeNull();
    });

    it('is cleared when the refetch fails', async () => {
      renderExplorer();
      await screen.findByText('api.anthropic.com');
      fetchNetflowExplorer.mockRejectedValueOnce(new Error('explorer down'));
      fetchGlobalNetflow.mockRejectedValueOnce(new Error('flows down'));
      fireEvent.click(screen.getByRole('button', { name: /^review-wf/ }));
      await screen.findByText('flows down');
      expect(updating()).toBeNull();
    });
  });

  it('renders the aggregate sections while the flows request is still pending', async () => {
    fetchGlobalNetflow.mockReturnValue(new Promise(() => {}));
    renderExplorer();
    await screen.findByText('Workflows');
    expect(screen.getByText('Flows')).toBeInTheDocument();
    expect(screen.getByText(/loading network flows/i)).toBeInTheDocument();
  });

  it('renders the flows table while the explorer request is still pending', async () => {
    fetchNetflowExplorer.mockReturnValue(new Promise(() => {}));
    renderExplorer();
    await screen.findByText('api.anthropic.com');
    expect(screen.queryByText('Workflows')).not.toBeInTheDocument();
    expect(screen.getByText(/loading network flows/i)).toBeInTheDocument();
  });

  it('before either response arrives, renders the query line and one loading line', async () => {
    fetchNetflowExplorer.mockReturnValue(new Promise(() => {}));
    fetchGlobalNetflow.mockReturnValue(new Promise(() => {}));
    renderExplorer();
    await waitFor(() => expect(fetchNetflowExplorer).toHaveBeenCalled());
    expect(screen.getByRole('combobox', { name: LABEL })).toBeInTheDocument();
    expect(screen.getAllByText(/loading network flows/i)).toHaveLength(1);
  });

  it('an explorer error leaves the query line and the table usable', async () => {
    fetchNetflowExplorer.mockRejectedValue(new Error('explorer down'));
    renderExplorer();
    await screen.findByText('explorer down');
    await screen.findByText('api.anthropic.com');
    expect(screen.getByRole('combobox', { name: LABEL })).toBeInTheDocument();
  });

  it('a flows error leaves the aggregates on screen', async () => {
    fetchGlobalNetflow.mockRejectedValue(new Error('flows down'));
    renderExplorer();
    await screen.findByText('flows down');
    expect(screen.getByText('Workflows')).toBeInTheDocument();
  });
});
