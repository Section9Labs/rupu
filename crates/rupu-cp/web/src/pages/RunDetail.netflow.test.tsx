// @vitest-environment jsdom
// RunDetail's Network tab — lazy-loads run-scoped netflow the same way the
// Findings tab lazy-loads findings (see RunDetail.tsx lines ~160 and
// ~313-334): a fetch keyed on (id, tab) with a ref guard for a single fetch
// per run id. This file only exercises that lazy-load/reset contract; the
// rendered content (table/summary/graph) is covered by the netflow
// component's own tests.
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
import type { NetflowResponse, GraphView } from '../lib/netflow';

// ---- Mock the netflow API client (Task 4) — the SUT under test here -------

const { fetchRunNetflow, fetchNetflowGraph } = vi.hoisted(() => ({
  fetchRunNetflow: vi.fn(),
  fetchNetflowGraph: vi.fn(),
}));
vi.mock('../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../lib/netflow')>('../lib/netflow');
  return { ...actual, fetchRunNetflow, fetchNetflowGraph };
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

beforeEach(() => {
  fetchRunNetflow.mockReset();
  fetchNetflowGraph.mockReset();
  fetchRunNetflow.mockResolvedValue({ flows: [], hosts: [], dropped_total: 0, asn_loaded: true } satisfies NetflowResponse);
  fetchNetflowGraph.mockResolvedValue({ nodes: [], edges: [] } satisfies GraphView);
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
// Router does not remount on a param change alone) — the ONLY thing that
// can reset the netflow state between runs is RunDetail's own [id] effect.
// Renders a Link so the test can navigate without unmounting.
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
    expect(fetchNetflowGraph).not.toHaveBeenCalled();
  });

  it('fetches once when the tab is opened, and not again on re-click', async () => {
    stubApi(GRAPH);
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: /network/i }));
    await waitFor(() => expect(fetchRunNetflow).toHaveBeenCalledWith('run-1'));
    expect(fetchNetflowGraph).toHaveBeenCalledWith('run:run-1');

    fireEvent.click(screen.getByRole('button', { name: /transcript/i }));
    fireEvent.click(screen.getByRole('button', { name: /network/i }));

    expect(fetchRunNetflow).toHaveBeenCalledTimes(1);
    expect(fetchNetflowGraph).toHaveBeenCalledTimes(1);
  });

  it('renders the fetched flows once loaded', async () => {
    fetchRunNetflow.mockResolvedValue({
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
      hosts: [],
      dropped_total: 0,
      asn_loaded: true,
    } satisfies NetflowResponse);
    stubApi(GRAPH);
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /network/i }));

    await screen.findByText('api.anthropic.com');
  });

  it('resets and re-fetches when navigating to a different run without unmounting', async () => {
    const getRunGraphSpy = vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});

    renderWithNav();
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: /network/i }));
    await waitFor(() => expect(fetchRunNetflow).toHaveBeenCalledWith('run-1'));
    expect(fetchRunNetflow).toHaveBeenCalledTimes(1);

    // Navigate to run-2 in the SAME mounted RunDetail instance (React Router
    // does not remount on a param-only change) — the [id] reset effect is
    // the only thing that can clear netflowRequestedRef here. The Network
    // tab stays selected (RunDetail never resets `tab` on an id change), so
    // the lazy-load effect should re-fire on its own once id flips.
    getRunGraphSpy.mockResolvedValue(GRAPH_2);
    fireEvent.click(screen.getByText('go-to-run-2'));

    await waitFor(() => expect(fetchRunNetflow).toHaveBeenCalledWith('run-2'));
    expect(fetchRunNetflow).toHaveBeenCalledTimes(2);
  });
});
