// @vitest-environment jsdom
// RunDetail's Network tab — mounts the shared <NetflowExplorer> lazily
// (nothing fetches until the tab is first opened), keeps it alive but
// hidden across tab re-clicks (no refetch), keys it on the run id (a run
// switch remounts and refetches), and seeds the run's OWN span as the
// initial window. The explorer's internal behavior (filters, views,
// popover) is covered by the explorer component tests; this file only
// exercises RunDetail's mounting/keying/window-seeding contract.
//
// Heavy children (RunGraph, TranscriptPanel, RunEventFeed,
// StepTranscriptBrowser, RunUsageTimeline) are mocked the same way
// RunDetail.test.tsx mocks them, so this file doesn't pull in xyflow /
// recharts.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { Link, MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, type RunGraphResponse, type FindingsResponse } from '../lib/api';
import type { ExplorerResponse, NetflowResponse } from '../lib/netflow';

// ---- Mock the netflow API client — the SUT under test here ----------------

const { fetchRunNetflow, fetchNetflowExplorer } = vi.hoisted(() => ({
  fetchRunNetflow: vi.fn(),
  fetchNetflowExplorer: vi.fn(),
}));
vi.mock('../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../lib/netflow')>('../lib/netflow');
  return { ...actual, fetchRunNetflow, fetchNetflowExplorer };
});

// ---- Mocks for heavy children (mirrors RunDetail.test.tsx) -----------------

vi.mock('../components/RunGraph', () => ({
  __esModule: true,
  default: () => <div data-testid="run-graph-mock" />,
}));

vi.mock('../components/TranscriptPanel', () => ({
  __esModule: true,
  default: () => <div data-testid="transcript-panel" />,
}));

vi.mock('../components/run/StepTranscriptBrowser', () => ({
  __esModule: true,
  default: () => <div data-testid="step-transcript-browser" />,
}));

vi.mock('../components/RunEventFeed', () => ({
  __esModule: true,
  default: () => <div data-testid="event-feed" />,
}));

vi.mock('../components/charts/RunUsageTimeline', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-timeline-mock" />,
}));

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function emptyExplorer(): ExplorerResponse {
  return {
    sankey: { workflows: [], origins: [], orgs: [], wf_origin: [], origin_org: [] },
    timeline: {
      bucket_ms: 0,
      from: '2026-06-01T00:00:00Z',
      to: '2026-06-01T00:05:00Z',
      lanes: [],
      runs: [],
      runs_in_window: 0,
    },
    histogram: { from: null, to: null, bucket_ms: 0, buckets: [] },
    kpis: {
      flows: 0,
      endpoints: 0,
      orgs: 0,
      errors: 0,
      bytes_in: null,
      bytes_out: null,
      bytes_partial: false,
      p95_ms: undefined,
    },
    dropped_total: 0,
    asn_loaded: true,
    window: { from: '2026-06-01T00:00:00Z', to: '2026-06-01T00:05:00Z' },
  };
}

function emptyFlows(): NetflowResponse {
  return {
    flows: [],
    hosts: [],
    dropped_total: 0,
    asn_loaded: true,
    window: { from: '2026-06-01T00:00:00Z', to: '2026-06-01T00:05:00Z' },
  };
}

beforeEach(() => {
  fetchRunNetflow.mockReset();
  fetchNetflowExplorer.mockReset();
  fetchRunNetflow.mockResolvedValue(emptyFlows());
  fetchNetflowExplorer.mockResolvedValue(emptyExplorer());
  vi.spyOn(api, 'getRunAutoflow').mockResolvedValue(null);
});

// ---- Fixtures ---------------------------------------------------------------

const GRAPH: RunGraphResponse = {
  run: {
    id: 'run-1',
    workflow_name: 'nightly-scan',
    status: 'completed',
    started_at: '2026-06-01T00:00:00Z',
    finished_at: '2026-06-01T00:05:00Z',
  } as RunGraphResponse['run'],
  workflow: { steps: [{ id: 'step_a', kind: 'step', agent: 'reviewer' }] },
  step_results: [],
  units: [],
};

const GRAPH_2: RunGraphResponse = {
  ...GRAPH,
  run: { ...(GRAPH.run as RunGraphResponse['run']), id: 'run-2' } as RunGraphResponse['run'],
};

const FINDINGS: FindingsResponse = {
  findings: [],
  summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 },
};

/** The window the explorer must seed at run scope: the run's OWN span,
 *  padded ±5 minutes (mockup parity — teardown flows stamped just past
 *  finished_at must not silently fall outside the default window). */
const RUN_SPAN = { from: '2026-05-31T23:55:00.000Z', to: '2026-06-01T00:10:00.000Z' };

function stubApi(graph: RunGraphResponse) {
  vi.spyOn(api, 'getRunGraph').mockResolvedValue(graph);
  vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
  vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
  vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
}

function renderPage(runId = 'run-1') {
  return render(
    <MemoryRouter initialEntries={[`/runs/${runId}`]}>
      <Routes>
        <Route path="/runs/:id" element={<RunDetailLoaded />} />
      </Routes>
    </MemoryRouter>,
  );
}

// Same route element instance across a run-1 -> run-2 navigation (React
// Router does not remount on a param change alone) — only the explorer's
// `key={run.id}` can reset netflow state between runs. Renders a Link so
// the test can navigate without unmounting.
function renderWithNav() {
  return render(
    <MemoryRouter initialEntries={['/runs/run-1']}>
      <Routes>
        <Route
          path="/runs/:id"
          element={
            <>
              <Link to="/runs/run-2">go-to-run-2</Link>
              <RunDetailLoaded />
            </>
          }
        />
      </Routes>
    </MemoryRouter>,
  );
}

// Imported here so the vi.mock factories above are hoisted before the module
// graph resolves RunDetail's child imports.
import RunDetailLoaded from './RunDetail';

describe('RunDetail netflow tab', () => {
  it('does not fetch netflow until the tab is opened', async () => {
    stubApi(GRAPH);
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    expect(fetchRunNetflow).not.toHaveBeenCalled();
    expect(fetchNetflowExplorer).not.toHaveBeenCalled();
  });

  it("fetches once on open — seeded with the run's own span — and not again on re-click", async () => {
    stubApi(GRAPH);
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: /network/i }));
    // Run scope: the initial window IS the run's span (the server echo
    // still decides all wording downstream), and both fetches carry it.
    await waitFor(() =>
      expect(fetchRunNetflow).toHaveBeenCalledWith('run-1', RUN_SPAN, undefined),
    );
    expect(fetchNetflowExplorer).toHaveBeenCalledWith('run:run-1', RUN_SPAN, undefined);

    fireEvent.click(screen.getByRole('button', { name: /transcript/i }));
    fireEvent.click(screen.getByRole('button', { name: /network/i }));

    // Kept mounted-but-hidden across the tab round-trip: no refetch.
    expect(fetchRunNetflow).toHaveBeenCalledTimes(1);
    expect(fetchNetflowExplorer).toHaveBeenCalledTimes(1);
  });

  it('renders the fetched flows once loaded', async () => {
    fetchRunNetflow.mockResolvedValue({
      ...emptyFlows(),
      flows: [
        {
          id: 'f1',
          ts: '2026-08-03T00:00:00Z',
          ctx: { origin: { kind: 'provider', name: 'anthropic' } },
          fidelity: 'http',
          method: 'POST',
          scheme: 'https',
          host: 'api.anthropic.com',
          port: 443,
          path: '/v1/messages',
          outcome: 'ok',
          body_complete: true,
        },
      ],
    } satisfies NetflowResponse);
    stubApi(GRAPH);
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /network/i }));

    await screen.findByText('api.anthropic.com');
  });

  it('remounts (and re-fetches) when navigating to a different run without unmounting RunDetail', async () => {
    const getRunGraphSpy = vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});

    renderWithNav();
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: /network/i }));
    await waitFor(() =>
      expect(fetchRunNetflow).toHaveBeenCalledWith('run-1', RUN_SPAN, undefined),
    );
    expect(fetchRunNetflow).toHaveBeenCalledTimes(1);

    // Navigate to run-2 in the SAME mounted RunDetail instance — the
    // explorer's `key={run.id}` is what remounts it with run-2's scope.
    getRunGraphSpy.mockResolvedValue(GRAPH_2);
    fireEvent.click(screen.getByText('go-to-run-2'));

    await waitFor(() =>
      expect(fetchRunNetflow).toHaveBeenCalledWith('run-2', RUN_SPAN, undefined),
    );
    expect(fetchRunNetflow).toHaveBeenCalledTimes(2);
  });

  it('keeps the previous data on screen while a range change is in flight (no loading flash)', async () => {
    fetchRunNetflow.mockResolvedValueOnce({
      ...emptyFlows(),
      flows: [
        {
          id: 'f1',
          ts: '2026-08-03T00:00:00Z',
          ctx: { origin: { kind: 'provider', name: 'anthropic' } },
          fidelity: 'http',
          method: 'POST',
          scheme: 'https',
          host: 'first-window-host.example.com',
          port: 443,
          path: '/v1/messages',
          outcome: 'ok',
          body_complete: true,
        },
      ],
    } satisfies NetflowResponse);
    stubApi(GRAPH);
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /network/i }));
    await screen.findByText('first-window-host.example.com');

    // The second fetch (triggered by the range change below) never
    // resolves during this test's assertions, so the in-flight state is
    // observable.
    let resolveSecond!: (v: NetflowResponse) => void;
    fetchRunNetflow.mockReturnValueOnce(
      new Promise<NetflowResponse>((resolve) => {
        resolveSecond = resolve;
      }),
    );
    fetchNetflowExplorer.mockResolvedValueOnce(emptyExplorer());

    fireEvent.click(screen.getByRole('button', { name: /last hour/i }));

    // Deliberate v3 behavior change from the pre-explorer tab: a filter/
    // range change must NOT collapse the whole surface to a loading line —
    // the previous result stays visible until the new one lands.
    expect(screen.getByText('first-window-host.example.com')).toBeInTheDocument();
    expect(screen.queryByText(/loading network flows/i)).not.toBeInTheDocument();

    resolveSecond(emptyFlows());
    await waitFor(() =>
      expect(screen.queryByText('first-window-host.example.com')).not.toBeInTheDocument(),
    );
  });

  it('clears a stale netflow error once a subsequent range change succeeds', async () => {
    fetchRunNetflow.mockRejectedValueOnce(new Error('boom'));
    stubApi(GRAPH);
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /network/i }));
    await screen.findByText(/boom/i);

    fetchRunNetflow.mockResolvedValueOnce(emptyFlows());
    fetchNetflowExplorer.mockResolvedValueOnce(emptyExplorer());

    fireEvent.click(screen.getByRole('button', { name: /last hour/i }));

    await waitFor(() => expect(screen.queryByText(/boom/i)).not.toBeInTheDocument());
    // Window WAS applied (the fixtures echo a bounded window) — the
    // range-aware empty state, not the unbounded one, is the honest read.
    await waitFor(() =>
      expect(screen.getByText(/no network flows in this range/i)).toBeInTheDocument(),
    );
  });
});
