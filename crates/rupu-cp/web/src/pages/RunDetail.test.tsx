// @vitest-environment jsdom
// RunDetail shell — the graph + "Token usage by turn" chart are PERSISTENT
// chrome (always rendered, regardless of the active tab). Below them a tab
// panel (Transcript · Events · Findings) FOLLOWS the step selected in the
// graph: selecting a for_each step shows the units file-browser; a normal step
// shows its transcript; the Events tab filters the feed to the selected step.
//
// Heavy children (RunGraph, TranscriptPanel, RunEventFeed, StepTranscriptBrowser,
// RunUsageTimeline) are mocked so the test drives selection through the graph's
// callback props without pulling in xyflow / recharts.
//
// REMOTE HOSTS: when ?host= is a non-local host id, getRunGraph and
// getRunUsageTimeline are called with the host parameter. All control calls
// include the host param.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import {
  api,
  ApiError,
  type RunGraphResponse,
  type FindingsResponse,
} from '../lib/api';
import type { NodeSelection } from '../components/RunGraph';
import type { SeqEvent } from '../components/RunEventFeed';

// ---- Mocks for heavy children -------------------------------------------

// RunGraph: expose buttons that fire the selection callbacks the page wires up.
vi.mock('../components/RunGraph', () => ({
  __esModule: true,
  default: (props: {
    onSelectNode?: (sel: NodeSelection) => void;
    onExpandFanout?: (stepId: string) => void;
    onOpenUnit?: (stepId: string, index: number) => void;
  }) => (
    <div data-testid="run-graph-mock">
      <button onClick={() => props.onSelectNode?.({ path: '/t/step-a.jsonl', live: false, label: 'step_a' })}>
        select-step-a
      </button>
      <button onClick={() => props.onExpandFanout?.('fan_out')}>select-fanout</button>
      {/* A real unit-square click fires onOpenUnit FIRST, then a spurious
          onSelectNode({ label: unit.key }) — replicate that exact order. */}
      <button
        onClick={() => {
          props.onOpenUnit?.('fan_out', 0);
          props.onSelectNode?.({ path: '/t/unit-0.jsonl', live: false, label: 'a.rs' });
        }}
      >
        open-unit
      </button>
    </div>
  ),
}));

vi.mock('../components/TranscriptPanel', () => ({
  __esModule: true,
  default: ({ path }: { path: string }) => <div data-testid="transcript-panel">transcript:{path}</div>,
}));

vi.mock('../components/run/StepTranscriptBrowser', () => ({
  __esModule: true,
  default: ({ stepId, initialUnitIndex }: { stepId: string; initialUnitIndex?: number }) => (
    <div data-testid="step-transcript-browser" data-initial-unit={String(initialUnitIndex ?? '')}>
      file-browser:{stepId}
    </div>
  ),
}));

// RunEventFeed: render one line per event so we can assert filtering.
vi.mock('../components/RunEventFeed', () => ({
  __esModule: true,
  default: ({ events }: { events: SeqEvent[] }) => (
    <div data-testid="event-feed">
      {events.map((e) => (
        <div key={e.seq}>evt:{String((e.event as { step_id?: string }).step_id ?? 'run')}</div>
      ))}
    </div>
  ),
}));

vi.mock('../components/charts/RunUsageTimeline', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-timeline-mock">chart</div>,
}));

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// ---- Fixtures ------------------------------------------------------------

const GRAPH: RunGraphResponse = {
  run: {
    id: 'run-1',
    workflow_name: 'nightly-scan',
    status: 'completed',
    started_at: '2026-06-01T00:00:00Z',
    finished_at: '2026-06-01T00:05:00Z',
  } as RunGraphResponse['run'],
  workflow: {
    steps: [
      { id: 'step_a', kind: 'step', agent: 'reviewer' },
      { id: 'fan_out', kind: 'for_each', for_each: 'files' },
    ],
  },
  step_results: [
    { step_id: 'step_a', success: true, transcript_path: '/t/step-a.jsonl' } as RunGraphResponse['step_results'][number],
  ],
  units: [
    {
      step_id: 'fan_out',
      index: 0,
      item: 'a.rs',
      run_id: 'run-1',
      transcript_path: '/t/unit-0.jsonl',
      output: '',
      success: true,
      finished_at: '2026-06-01T00:04:00Z',
    },
  ],
};

// An awaiting-approval run: gated on `step_a`, no approval recorded yet.
const AWAITING_GRAPH: RunGraphResponse = {
  run: {
    id: 'run-1',
    workflow_name: 'nightly-scan',
    status: 'awaiting_approval',
    started_at: '2026-06-01T00:00:00Z',
    awaiting_step_id: 'step_a',
    approval_prompt: 'Approve the plan before applying?',
  } as RunGraphResponse['run'],
  workflow: { steps: [{ id: 'step_a', kind: 'step', agent: 'reviewer' }] },
  step_results: [],
  units: [],
};

// A non-terminal (running) run — eligible for cancel from the header.
const RUNNING_GRAPH: RunGraphResponse = {
  run: {
    id: 'run-1',
    workflow_name: 'nightly-scan',
    status: 'running',
    started_at: '2026-06-01T00:00:00Z',
  } as RunGraphResponse['run'],
  workflow: { steps: [{ id: 'step_a', kind: 'step', agent: 'reviewer' }] },
  step_results: [],
  units: [],
};

// A paused run — eligible for Resume from the paused banner.
const PAUSED_GRAPH: RunGraphResponse = {
  run: {
    id: 'run-1',
    workflow_name: 'nightly-scan',
    status: 'paused',
    started_at: '2026-06-01T00:00:00Z',
  } as RunGraphResponse['run'],
  workflow: { steps: [{ id: 'step_a', kind: 'step', agent: 'reviewer' }] },
  step_results: [],
  units: [],
};

const FINDINGS: FindingsResponse = {
  findings: [],
  summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 },
};

const EMPTY_USAGE = {
  input_tokens: 0,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: 0,
  cost_usd: null,
  priced: false,
  runs: 0,
};

// ---- Render helpers -------------------------------------------------------

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/runs/run-1']}>
      <Routes>
        <Route path="/runs/:id" element={<RunDetailLoaded />} />
      </Routes>
    </MemoryRouter>,
  );
}

function renderRemotePage(hostId = 'h-abc') {
  return render(
    <MemoryRouter initialEntries={[`/runs/run-1?host=${hostId}`]}>
      <Routes>
        <Route path="/runs/:id" element={<RunDetailLoaded />} />
      </Routes>
    </MemoryRouter>,
  );
}

// Imported here so the vi.mock factories above are hoisted before the module
// graph resolves RunDetail's child imports.
import RunDetailLoaded from './RunDetail';

// ---- Local run tests -----------------------------------------------------

describe('RunDetail shell', () => {
  function stubApi() {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    // Stub the SSE subscription: emit one step-scoped + one run-level event,
    // then return a no-op unsubscribe.
    vi.spyOn(api, 'subscribeRunLog').mockImplementation((_id, onEvent) => {
      onEvent({ type: 'step_completed', run_id: 'run-1', step_id: 'step_a', success: true, duration_ms: 1 });
      onEvent({ type: 'run_started', run_id: 'run-1', event_version: 1, workflow_path: 'wf.yaml', started_at: 'x' });
      return () => {};
    });
  }

  it('renders the graph AND the usage chart as persistent chrome on every tab', async () => {
    stubApi();
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    // Both present on the default (Transcript) tab.
    expect(screen.getByTestId('usage-timeline-mock')).toBeInTheDocument();

    // Switch to Events — chrome stays mounted.
    fireEvent.click(screen.getByText('Events'));
    expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument();
    expect(screen.getByTestId('usage-timeline-mock')).toBeInTheDocument();

    // Switch to Findings — chrome still present.
    fireEvent.click(screen.getByText(/Findings/));
    expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument();
    expect(screen.getByTestId('usage-timeline-mock')).toBeInTheDocument();
  });

  it('shows the file-browser in Transcript when a for_each step is selected', async () => {
    stubApi();
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());

    fireEvent.click(screen.getByText('select-fanout'));

    expect(screen.getByTestId('step-transcript-browser')).toHaveTextContent('file-browser:fan_out');
    expect(screen.queryByTestId('transcript-panel')).not.toBeInTheDocument();
  });

  it('opens the file-browser (not the empty state) when a unit square is clicked', async () => {
    stubApi();
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());

    // Drive the REAL unit-square path: onOpenUnit(stepId, index) FIRST, then
    // the spurious onSelectNode({ label: unit.key }) — same order RunGraph fires.
    fireEvent.click(screen.getByText('open-unit'));

    // The for_each file-browser renders for the step (NOT the unit key), and we
    // never fall through to the "No transcript" empty-state.
    const browser = screen.getByTestId('step-transcript-browser');
    expect(browser).toHaveTextContent('file-browser:fan_out');
    expect(browser).toHaveAttribute('data-initial-unit', '0');
    expect(screen.queryByText(/No transcript yet/)).not.toBeInTheDocument();
  });

  it('shows the transcript panel when a normal step is selected', async () => {
    stubApi();
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());

    fireEvent.click(screen.getByText('select-step-a'));

    expect(screen.getByTestId('transcript-panel')).toHaveTextContent('transcript:/t/step-a.jsonl');
    expect(screen.queryByTestId('step-transcript-browser')).not.toBeInTheDocument();
  });

  it('filters the Events feed to the selected step', async () => {
    stubApi();
    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());

    // Select the normal step, then open the Events tab.
    fireEvent.click(screen.getByText('select-step-a'));
    fireEvent.click(screen.getByText('Events'));

    const feed = screen.getByTestId('event-feed');
    // The step-scoped event survives; the run-level event is filtered out.
    expect(feed).toHaveTextContent('evt:step_a');
    expect(feed).not.toHaveTextContent('evt:run');
  });

  it('approves and rejects an awaiting run via the approval-gate controls', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(AWAITING_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    const approveSpy = vi.spyOn(api, 'approveRun').mockResolvedValue(undefined);
    const rejectSpy = vi.spyOn(api, 'rejectRun').mockResolvedValue(undefined);

    renderPage();

    // Wait for the awaiting banner's Approve button to render.
    const approveBtn = await screen.findByRole('button', { name: 'Approve run' });

    // Approving (default mode = Ask) records the decision → "Approved — resuming…".
    fireEvent.click(approveBtn);
    await waitFor(() => expect(approveSpy).toHaveBeenCalledWith('run-1', 'ask'));
    await screen.findByText(/Approved — resuming/);

    // Re-render fresh to drive the reject path independently.
    cleanup();
    renderPage();
    const rejectBtn = await screen.findByRole('button', { name: 'Reject run' });
    fireEvent.click(rejectBtn);

    const reasonInput = await screen.findByLabelText('Rejection reason');
    fireEvent.change(reasonInput, { target: { value: 'not safe' } });
    fireEvent.click(screen.getByRole('button', { name: 'Confirm rejection' }));

    await waitFor(() => expect(rejectSpy).toHaveBeenCalledWith('run-1', 'not safe'));
  });

  it('cancels a running run via the header Cancel button (after confirm)', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(RUNNING_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    const cancelSpy = vi.spyOn(api, 'cancelRun').mockResolvedValue(undefined);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();

    const cancelBtn = await screen.findByRole('button', { name: 'Cancel run' });
    fireEvent.click(cancelBtn);

    expect(confirmSpy).toHaveBeenCalled();
    await waitFor(() => expect(cancelSpy).toHaveBeenCalledWith('run-1'));
  });

  it('shows Pause on a Running run and calls POST /api/runs/:id/pause when clicked', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(RUNNING_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    const pauseSpy = vi.spyOn(api, 'pauseRun').mockResolvedValue(undefined);

    renderPage();

    const pauseBtn = await screen.findByRole('button', { name: 'Pause run' });
    // No Resume control before the run is paused.
    expect(screen.queryByRole('button', { name: 'Resume run' })).not.toBeInTheDocument();

    fireEvent.click(pauseBtn);

    await waitFor(() => expect(pauseSpy).toHaveBeenCalledWith('run-1'));
    // Pausing succeeded (optimistic update) — the run now shows Resume instead.
    await screen.findByRole('button', { name: 'Resume run' });
  });

  it('surfaces a failed pause via the server error message (no silent no-op)', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(RUNNING_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'pauseRun').mockRejectedValue(new ApiError(409, 'run run-1 is not running'));

    renderPage();

    const pauseBtn = await screen.findByRole('button', { name: 'Pause run' });
    fireEvent.click(pauseBtn);

    await screen.findByText('run run-1 is not running');
  });

  it('shows Resume on a Paused run and calls POST /api/runs/:id/resume when clicked', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(PAUSED_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    const resumeSpy = vi.spyOn(api, 'resumeRun').mockResolvedValue(undefined);

    renderPage();

    // No Pause control on a paused (non-running) run.
    expect(screen.queryByRole('button', { name: 'Pause run' })).not.toBeInTheDocument();

    const resumeBtn = await screen.findByRole('button', { name: 'Resume run' });
    fireEvent.click(resumeBtn);

    await waitFor(() => expect(resumeSpy).toHaveBeenCalledWith('run-1'));
  });

  it('renders a read-only-deploy message when resume returns 501', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(PAUSED_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'resumeRun').mockRejectedValue(
      new ApiError(501, 'resuming a paused run requires `rupu cp serve`'),
    );

    renderPage();

    const resumeBtn = await screen.findByRole('button', { name: 'Resume run' });
    fireEvent.click(resumeBtn);

    await screen.findByText(/requires/i);
    expect(screen.getByText(/rupu cp serve/)).toBeInTheDocument();
  });

  it('renders the server message when resume is rejected with a 4xx (non-501)', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(PAUSED_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'resumeRun').mockRejectedValue(
      new ApiError(409, 'run run-1 is `running`, not `paused`'),
    );

    renderPage();

    const resumeBtn = await screen.findByRole('button', { name: 'Resume run' });
    fireEvent.click(resumeBtn);

    await screen.findByText('run run-1 is `running`, not `paused`');
  });

  it('approves an awaiting run in the selected (Bypass) mode', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(AWAITING_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    const approveSpy = vi.spyOn(api, 'approveRun').mockResolvedValue(undefined);

    renderPage();

    const approveBtn = await screen.findByRole('button', { name: 'Approve run' });
    // Pick Bypass in the mode picker, then approve.
    fireEvent.change(screen.getByLabelText('Resume mode'), { target: { value: 'bypass' } });
    fireEvent.click(approveBtn);

    await waitFor(() => expect(approveSpy).toHaveBeenCalledWith('run-1', 'bypass'));
  });
});

// ---- Remote host gating tests --------------------------------------------

describe('RunDetail — remote host (?host=)', () => {
  it('calls getRunGraph and getRunUsageTimeline with host, renders graph and usage', async () => {
    const REMOTE_GRAPH: RunGraphResponse = {
      run: {
        id: 'run-1',
        workflow_name: 'remote-scan',
        status: 'completed',
        started_at: '2026-06-01T00:00:00Z',
        finished_at: '2026-06-01T00:05:00Z',
      } as RunGraphResponse['run'],
      workflow: {
        steps: [
          { id: 'step_a', kind: 'step', agent: 'reviewer' },
        ],
      },
      step_results: [
        { step_id: 'step_a', success: true, transcript_path: '/t/step-a.jsonl' } as RunGraphResponse['step_results'][number],
      ],
      units: [],
      usage: EMPTY_USAGE,
    };
    const getRunGraphSpy = vi.spyOn(api, 'getRunGraph').mockResolvedValue(REMOTE_GRAPH);
    const getTimelineSpy = vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);

    renderRemotePage();

    // Header should render the workflow name from getRunGraph.
    await waitFor(() => expect(screen.getByText('remote-scan')).toBeInTheDocument());

    // getRunGraph was called with the host option.
    expect(getRunGraphSpy).toHaveBeenCalledWith('run-1', { host: 'h-abc' });

    // getRunUsageTimeline was called with the host option.
    expect(getTimelineSpy).toHaveBeenCalledWith('run-1', { host: 'h-abc' });

    // The graph and usage-timeline mocks are in the DOM.
    expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument();
    expect(screen.getByTestId('usage-timeline-mock')).toBeInTheDocument();

    // The gating note is NOT visible.
    expect(screen.queryByTestId('remote-graph-note')).not.toBeInTheDocument();
  });

  it('passes host to subscribeRunLog for remote runs', async () => {
    const REMOTE_GRAPH: RunGraphResponse = {
      run: {
        id: 'run-1',
        workflow_name: 'remote-scan',
        status: 'completed',
        started_at: '2026-06-01T00:00:00Z',
        finished_at: '2026-06-01T00:05:00Z',
      } as RunGraphResponse['run'],
      workflow: {
        steps: [
          { id: 'step_a', kind: 'step', agent: 'reviewer' },
        ],
      },
      step_results: [
        { step_id: 'step_a', success: true, transcript_path: '/t/step-a.jsonl' } as RunGraphResponse['step_results'][number],
      ],
      units: [],
      usage: EMPTY_USAGE,
    };
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(REMOTE_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    const subscribeSpy = vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);

    renderRemotePage();

    await waitFor(() => expect(screen.getByText('remote-scan')).toBeInTheDocument());

    expect(subscribeSpy).toHaveBeenCalledWith(
      'run-1',
      expect.any(Function),
      expect.any(Function),
      { host: 'h-abc' },
    );
  });

  it('passes host to approveRun when approving a remote awaiting run', async () => {
    const remoteAwaiting: RunGraphResponse = {
      run: {
        id: 'run-1',
        workflow_name: 'remote-scan',
        status: 'awaiting_approval',
        started_at: '2026-06-01T00:00:00Z',
        awaiting_step_id: 'step_a',
        approval_prompt: 'Remote approval needed?',
      } as RunGraphResponse['run'],
      workflow: { steps: [{ id: 'step_a', kind: 'step', agent: 'reviewer' }] },
      step_results: [],
      units: [],
      usage: EMPTY_USAGE,
    };
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(remoteAwaiting);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    const approveSpy = vi.spyOn(api, 'approveRun').mockResolvedValue(undefined);

    renderRemotePage();

    const approveBtn = await screen.findByRole('button', { name: 'Approve run' });
    fireEvent.click(approveBtn);

    await waitFor(() =>
      expect(approveSpy).toHaveBeenCalledWith('run-1', 'ask', 'h-abc'),
    );
  });

  it('shows the host badge in the header for remote runs', async () => {
    const REMOTE_GRAPH: RunGraphResponse = {
      run: {
        id: 'run-1',
        workflow_name: 'remote-scan',
        status: 'completed',
        started_at: '2026-06-01T00:00:00Z',
        finished_at: '2026-06-01T00:05:00Z',
      } as RunGraphResponse['run'],
      workflow: {
        steps: [
          { id: 'step_a', kind: 'step', agent: 'reviewer' },
        ],
      },
      step_results: [
        { step_id: 'step_a', success: true, transcript_path: '/t/step-a.jsonl' } as RunGraphResponse['step_results'][number],
      ],
      units: [],
      usage: EMPTY_USAGE,
    };
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(REMOTE_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);

    renderRemotePage('my-staging-host');

    await waitFor(() => expect(screen.getByText('remote-scan')).toBeInTheDocument());
    expect(screen.getByText('my-staging-host')).toBeInTheDocument();
  });
});
