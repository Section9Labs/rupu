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
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
import {
  api,
  ApiError,
  type AutoflowRunContext,
  type RunGraphResponse,
  type FindingsResponse,
} from '../lib/api';
import type { NodeSelection } from '../components/RunGraph';
import { customerRow, withCustomerScope } from '../lib/customerScopeTestUtils';
import type { SeqEvent } from '../components/RunEventFeed';

// ---- Mocks for heavy children -------------------------------------------

// RunGraph: expose buttons that fire the selection callbacks the page wires up.
vi.mock('../components/RunGraph', () => ({
  __esModule: true,
  default: (props: {
    model?: { nodes: { id: string; warnings?: unknown[] }[] };
    onSelectNode?: (sel: NodeSelection) => void;
    onExpandFanout?: (stepId: string) => void;
    onOpenUnit?: (stepId: string, index: number) => void;
  }) => (
    <div data-testid="run-graph-mock">
      {/* the steps the model marks with a warning (what each node's ⚠ reads) */}
      <span data-testid="graph-warned">
        {(props.model?.nodes ?? []).filter((n) => n.warnings?.length).map((n) => n.id).join(',')}
      </span>
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

vi.mock('../components/TranscriptPanel', async () => {
  const { useContext } = await import('react');
  const { SubrunIdentityContext } = await import('../components/transcript/subrunIdentity');
  function MockTranscriptPanel({ path }: { path: string }) {
    // Surface the sub-run identities RunDetail provides around the transcript.
    const ids = useContext(SubrunIdentityContext);
    const summary = [...ids.entries()].map(([k, v]) => `${k}=${v.provider ?? ''}/${v.model ?? ''}`).join(',');
    return (
      <div data-testid="transcript-panel" data-subruns={summary}>
        transcript:{path}
      </div>
    );
  }
  return { __esModule: true, default: MockTranscriptPanel };
});

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
  default: ({ series }: { series: unknown[] }) => (
    <div data-testid="usage-timeline-mock" data-points={series.length}>
      chart
    </div>
  ),
}));

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// Every test in this file exercises RunDetail, which now always fetches
// GET /api/runs/:id/autoflow alongside the graph. Default it to "not an
// autoflow run" (null → no panel) so the many pre-existing tests below don't
// need to know about it; the dedicated Autoflow-panel tests re-stub it.
beforeEach(() => {
  vi.spyOn(api, 'getRunAutoflow').mockResolvedValue(null);
  // GET /api/runs/:id/usage is polled by every run page; default it to "older
  // remote CP" (404 → fall back to the graph's one-shot usage) so the tests
  // that predate the live endpoint keep asserting the graph numbers. The live
  // tests below re-stub it.
  vi.spyOn(api, 'getRunUsage').mockRejectedValue(new ApiError(404, 'not found'));
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

// A multi-gate awaiting-approval run (Task 5b-2b, spec §7): two concurrent
// DAG paths each parked their own gate in the same batch-park wave.
const MULTI_GATE_GRAPH: RunGraphResponse = {
  run: {
    id: 'run-1',
    workflow_name: 'nightly-scan',
    status: 'awaiting_approval',
    started_at: '2026-06-01T00:00:00Z',
    // Derived-compat fields mirror the FIRST gate only — the multi-gate UI
    // must read `awaiting` (the full set), not these.
    awaiting_step_id: 'gate_a',
    approval_prompt: 'Approve path A?',
    awaiting: [
      { step_id: 'gate_a', prompt: 'Approve path A?', since: '2026-06-01T00:00:00Z' },
      { step_id: 'gate_b', prompt: 'Approve path B?', since: '2026-06-01T00:00:00Z' },
    ],
  } as RunGraphResponse['run'],
  workflow: {
    steps: [
      { id: 'gate_a', kind: 'gate' },
      { id: 'gate_b', kind: 'gate' },
    ],
  },
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
import { formatTokens } from '../lib/usage';

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

  it('says a run has warnings in its own banner, without changing any step or the run status', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation((_id, onEvent) => {
      onEvent({
        type: 'step_warning', run_id: 'run-1', step_id: 'fan_out', index: 0,
        message: 'host gpu-9 did not stream coverage',
      });
      return () => {};
    });
    renderPage();

    const banner = await screen.findByTestId('run-warnings');
    expect(banner).toHaveTextContent('1 warning');
    expect(banner).toHaveTextContent('fan_out · unit 0');
    expect(banner).toHaveTextContent('host gpu-9 did not stream coverage');
    // a completed run stays completed — the warning is a notice, not a status
    expect(screen.getByText('Completed')).toBeInTheDocument();
  });

  it('keeps a warning that has scrolled out of the 2000-event live window', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation((_id, onEvent) => {
      // The log replays from byte 0: the warning is the FIRST event, then far
      // more than RunDetail's 2000-event window of ordinary ones push it out.
      onEvent({
        type: 'step_warning', run_id: 'run-1', step_id: 'step_a',
        message: 'host gpu-9 did not stream coverage',
      });
      for (let i = 0; i < 2100; i++) {
        onEvent({ type: 'step_working', run_id: 'run-1', step_id: 'step_a', note: `tick ${i}` });
      }
      return () => {};
    });
    renderPage();

    const banner = await screen.findByTestId('run-warnings');
    expect(banner).toHaveTextContent('1 warning');
    expect(banner).toHaveTextContent('host gpu-9 did not stream coverage');
    // the graph node still carries its marker
    expect(screen.getByTestId('graph-warned')).toHaveTextContent('step_a');
  });

  it('a log replay after a reconnect does not list the same warning twice', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation((_id, onEvent) => {
      const w = { type: 'step_warning', run_id: 'run-1', step_id: 'step_a', message: 'dup me' } as const;
      onEvent(w);
      onEvent({ ...w });
      return () => {};
    });
    renderPage();
    expect(await screen.findByTestId('run-warnings')).toHaveTextContent('1 warning');
  });

  it('shows no warnings banner on a clean run', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    renderPage();
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    expect(screen.queryByTestId('run-warnings')).toBeNull();
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

  it('renders one approve/reject control PER gate on a multi-gate run, each targeting its own gate id', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(MULTI_GATE_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    const approveSpy = vi.spyOn(api, 'approveRun').mockResolvedValue(undefined);
    const rejectSpy = vi.spyOn(api, 'rejectRun').mockResolvedValue(undefined);

    renderPage();

    // Two distinct Approve controls, one per gate — NOT the single-gate
    // "Approve run" control.
    await screen.findByRole('button', { name: 'Approve gate_a' });
    screen.getByRole('button', { name: 'Approve gate_b' });
    expect(screen.queryByRole('button', { name: 'Approve run' })).not.toBeInTheDocument();

    // Both prompts render.
    expect(screen.getByText('Approve path A?')).toBeInTheDocument();
    expect(screen.getByText('Approve path B?')).toBeInTheDocument();

    // Approving gate_b calls the API with gate_b's id — gate_a is untouched.
    fireEvent.click(screen.getByRole('button', { name: 'Approve gate_b' }));
    await waitFor(() =>
      expect(approveSpy).toHaveBeenCalledWith('run-1', 'ask', undefined, 'gate_b'),
    );
    expect(approveSpy).not.toHaveBeenCalledWith('run-1', 'ask', undefined, 'gate_a');
    // gate_b's own row now shows the resuming state; gate_a's is untouched.
    await screen.findByText(/Approved — resuming/);
    screen.getByRole('button', { name: 'Approve gate_a' });

    // Rejecting gate_a calls the API with gate_a's id.
    fireEvent.click(screen.getByRole('button', { name: 'Reject gate_a' }));
    const reasonInput = await screen.findByLabelText('Rejection reason for gate_a');
    fireEvent.change(reasonInput, { target: { value: 'gate_a looks bad' } });
    fireEvent.click(screen.getByRole('button', { name: 'Confirm rejection for gate_a' }));

    await waitFor(() =>
      expect(rejectSpy).toHaveBeenCalledWith('run-1', 'gate_a looks bad', undefined, 'gate_a'),
    );
  });

  it('keeps the single-gate control unchanged when a run has exactly one parked gate', async () => {
    // Primary safety invariant: a 1-gate run must render the SAME single
    // "Approve run"/"Reject run" control this task started with, not a
    // 1-row rendering of the multi-gate list.
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(AWAITING_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    const approveSpy = vi.spyOn(api, 'approveRun').mockResolvedValue(undefined);

    renderPage();

    await screen.findByRole('button', { name: 'Approve run' });
    expect(screen.queryByRole('button', { name: /^Approve gate_/ })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Approve run' }));
    // Byte-for-byte the pre-existing call shape: no trailing host/gate args.
    await waitFor(() => expect(approveSpy).toHaveBeenCalledWith('run-1', 'ask'));
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

  it('cancels a paused run via the Cancel button in the Paused banner (after confirm)', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(PAUSED_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    const cancelSpy = vi.spyOn(api, 'cancelRun').mockResolvedValue(undefined);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();

    // Resume and Cancel are both reachable from the Paused banner.
    await screen.findByRole('button', { name: 'Resume run' });
    const cancelBtn = await screen.findByRole('button', { name: 'Cancel run' });
    fireEvent.click(cancelBtn);

    expect(confirmSpy).toHaveBeenCalled();
    await waitFor(() => expect(cancelSpy).toHaveBeenCalledWith('run-1'));
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

  it('passes host to pauseRun when pausing a remote running run', async () => {
    const remoteRunning: RunGraphResponse = {
      run: {
        id: 'run-1',
        workflow_name: 'remote-scan',
        status: 'running',
        started_at: '2026-06-01T00:00:00Z',
      } as RunGraphResponse['run'],
      workflow: { steps: [{ id: 'step_a', kind: 'step', agent: 'reviewer' }] },
      step_results: [],
      units: [],
      usage: EMPTY_USAGE,
    };
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(remoteRunning);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    const pauseSpy = vi.spyOn(api, 'pauseRun').mockResolvedValue(undefined);

    renderRemotePage();

    const pauseBtn = await screen.findByRole('button', { name: 'Pause run' });
    fireEvent.click(pauseBtn);

    await waitFor(() => expect(pauseSpy).toHaveBeenCalledWith('run-1', 'h-abc'));
  });

  it('passes host to resumeRun when resuming a remote paused run', async () => {
    const remotePaused: RunGraphResponse = {
      run: {
        id: 'run-1',
        workflow_name: 'remote-scan',
        status: 'paused',
        started_at: '2026-06-01T00:00:00Z',
      } as RunGraphResponse['run'],
      workflow: { steps: [{ id: 'step_a', kind: 'step', agent: 'reviewer' }] },
      step_results: [],
      units: [],
      usage: EMPTY_USAGE,
    };
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(remotePaused);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    const resumeSpy = vi.spyOn(api, 'resumeRun').mockResolvedValue(undefined);

    renderRemotePage();

    const resumeBtn = await screen.findByRole('button', { name: 'Resume run' });
    fireEvent.click(resumeBtn);

    await waitFor(() => expect(resumeSpy).toHaveBeenCalledWith('run-1', 'h-abc'));
  });

  // Operator-reported gap this task fixes: RunDetail's Archive/Delete omitted
  // `host`, hitting the local run store even when viewing a remote-host run —
  // and for a CP-launched SSH run, the local store holds a MIRROR sharing the
  // same run id, so the local delete silently "succeeds" while destroying the
  // CP's only local copy and leaving the actual remote run untouched.
  it('passes host to archiveRun when archiving a remote terminal run', async () => {
    const remoteCompleted: RunGraphResponse = {
      run: {
        id: 'run-1',
        workflow_name: 'remote-scan',
        status: 'completed',
        started_at: '2026-06-01T00:00:00Z',
        finished_at: '2026-06-01T00:05:00Z',
      } as RunGraphResponse['run'],
      workflow: { steps: [{ id: 'step_a', kind: 'step', agent: 'reviewer' }] },
      step_results: [],
      units: [],
      usage: EMPTY_USAGE,
    };
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(remoteCompleted);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    const archiveSpy = vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);

    renderRemotePage();

    const archiveBtn = await screen.findByRole('button', { name: /Archive/ });
    fireEvent.click(archiveBtn);

    await waitFor(() => expect(archiveSpy).toHaveBeenCalledWith('run-1', 'h-abc'));
  });

  it('passes host to deleteRun when deleting a remote terminal run', async () => {
    const remoteCompleted: RunGraphResponse = {
      run: {
        id: 'run-1',
        workflow_name: 'remote-scan',
        status: 'completed',
        started_at: '2026-06-01T00:00:00Z',
        finished_at: '2026-06-01T00:05:00Z',
      } as RunGraphResponse['run'],
      workflow: { steps: [{ id: 'step_a', kind: 'step', agent: 'reviewer' }] },
      step_results: [],
      units: [],
      usage: EMPTY_USAGE,
    };
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(remoteCompleted);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const deleteSpy = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);

    renderRemotePage();

    const deleteBtn = await screen.findByRole('button', { name: /Delete/ });
    fireEvent.click(deleteBtn);

    await waitFor(() => expect(deleteSpy).toHaveBeenCalledWith('run-1', 'h-abc'));
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

// ---- Autoflow panel --------------------------------------------------------

const AUTOFLOW_CONTEXT: AutoflowRunContext = {
  repo_ref: 'github:Section9Labs/rupu',
  issue_ref: 'github:Section9Labs/rupu/issues/42',
  entity: '42',
  workflow_name: 'issue-supervisor-dispatch',
  status: 'completed',
  failure: null,
  cycle_id: 'afc_ctx_new',
  workspace_path: '/home/matt/.rupu/autoflows/worktrees/rupu/42',
  host_id: null,
  claim: {
    issue_ref: 'github:Section9Labs/rupu/issues/42',
    issue_display_ref: 'acme/rupu#42',
    repo_ref: 'github:Section9Labs/rupu',
    issue_title: 'Flaky retry path',
    issue_url: 'https://example.test/issues/42',
    workflow: 'issue-supervisor-dispatch',
    status: 'running',
    last_run_id: 'run-1',
    last_error: null,
    last_summary: null,
    pr_url: null,
    claim_owner: 'worker-1',
    lease_expires_at: '2026-07-08T00:00:00Z',
    updated_at: '2026-07-07T00:00:00Z',
  },
  prior_cycles: [
    {
      cycle_id: 'afc_ctx_old',
      mode: 'tick',
      started_at: '2026-07-01T10:00:00Z',
      finished_at: '2026-07-01T10:00:05Z',
      ran_cycles: 1,
      skipped_cycles: 0,
      failed_cycles: 0,
      worker_name: 'local',
      events: [
        {
          kind: 'run_launched',
          issue_ref: 'github:Section9Labs/rupu/issues/42',
          issue_display_ref: 'acme/rupu#42',
          workflow: 'issue-supervisor-dispatch',
          run_id: 'run-old-1',
          status: 'completed',
        },
      ],
    },
  ],
};

describe('RunDetail — Autoflow panel', () => {
  function stubApi() {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
  }

  it('renders the Autoflow panel (entity link, claim status, cycle, project/host) for an autoflow run', async () => {
    stubApi();
    vi.spyOn(api, 'getRunAutoflow').mockResolvedValue(AUTOFLOW_CONTEXT);

    renderPage();

    const panel = await screen.findByTestId('autoflow-panel');
    // Entity: linked issue ref + title from the claim.
    const entityLink = screen.getByRole('link', { name: 'acme/rupu#42' });
    expect(entityLink).toHaveAttribute('href', 'https://example.test/issues/42');
    expect(panel).toHaveTextContent('Flaky retry path');
    // Claim status.
    expect(panel).toHaveTextContent('Running');
    // Cycle id.
    expect(panel).toHaveTextContent('afc_ctx_new');
    // Project/host chips — host_id is null (local) and the project basename
    // is derived from workspace_path.
    expect(panel).toHaveTextContent('local');
    expect(panel).toHaveTextContent('42');
    // The full cycle history moved to the Cycles TAB — the panel no longer
    // duplicates the prior-cycle list (see the "Cycles tab" describe block).
    expect(panel).not.toHaveTextContent('afc_ctx_old');
  });

  it('renders NO Autoflow panel for a plain (non-autoflow) run', async () => {
    stubApi();
    vi.spyOn(api, 'getRunAutoflow').mockResolvedValue(null);

    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    expect(screen.queryByTestId('autoflow-panel')).not.toBeInTheDocument();
  });
});

// ---- Cycles tab -------------------------------------------------------------

describe('RunDetail — Cycles tab', () => {
  function stubApi() {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
  }

  it('shows a Cycles tab for an autoflow run; clicking it renders the linked cycle table', async () => {
    stubApi();
    vi.spyOn(api, 'getRunAutoflow').mockResolvedValue(AUTOFLOW_CONTEXT);

    renderPage();

    const cyclesTabBtn = await screen.findByRole('button', { name: 'Cycles' });
    fireEvent.click(cyclesTabBtn);

    const table = await screen.findByTestId('cycles-tab');
    // Current cycle row (this run) — cycle id + "current" badge.
    expect(table).toHaveTextContent('afc_ctx_new');
    expect(table).toHaveTextContent('current');
    // Prior cycle row — cycle id, its resolved workflow/run.
    expect(table).toHaveTextContent('afc_ctx_old');
    expect(table).toHaveTextContent('issue-supervisor-dispatch');

    // The prior cycle's row (which carries a run_id via its event log) links
    // to that run. CyclesTab declares no `subject` column, so SortableTable's
    // one-accessible-link-per-row rule (I7) falls back to the first column
    // (Cycle) as the row's sole focusable/announced link; the Run cell's own
    // link is present for mouse clicks only (aria-hidden + tabIndex=-1).
    const priorRunLink = screen.getByRole('link', { name: /afc_ctx_old/ });
    expect(priorRunLink).toHaveAttribute('href', '/runs/run-old-1');
  });

  it('shows NO Cycles tab for a plain (non-autoflow) run', async () => {
    stubApi();
    vi.spyOn(api, 'getRunAutoflow').mockResolvedValue(null);

    renderPage();

    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    expect(screen.queryByRole('button', { name: 'Cycles' })).not.toBeInTheDocument();
  });
});

// ---- Failed / unpersisted run ---------------------------------------------

// Mirrors the shape `synthesize_unpersisted_run` + `unpersisted_run_dag`
// produce server-side for an autoflow run that failed before ever writing
// `run.json` (crates/rupu-cp/src/api/runs.rs / graph.rs): a single "run" step,
// `status: "failed"`, and `error_message` carrying the failure detail.
const FAILED_UNPERSISTED_GRAPH: RunGraphResponse = {
  run: {
    id: 'run_01KWYZ2QY4XYZ',
    workflow_name: 'issue-supervisor-dispatch',
    status: 'failed',
    started_at: '2026-07-01T10:00:00Z',
    finished_at: '2026-07-01T10:00:00Z',
    error_message: '401 invalid x-api-key',
  } as RunGraphResponse['run'],
  workflow: { steps: [{ id: 'run', kind: 'step', agent: 'issue-supervisor-dispatch' }] },
  step_results: [],
  units: [],
  usage: EMPTY_USAGE,
};

describe('RunDetail — failed / unpersisted run', () => {
  it('shows the failure reason instead of crashing or spinning forever', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(FAILED_UNPERSISTED_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    // Also an autoflow-history hit (this IS the autoflow run that failed to
    // dispatch) — but the panel isn't the point of this test, so keep it null
    // to isolate the assertion to the failed-run chrome.
    vi.spyOn(api, 'getRunAutoflow').mockResolvedValue(null);

    render(
      <MemoryRouter initialEntries={['/runs/run_01KWYZ2QY4XYZ']}>
        <Routes>
          <Route path="/runs/:id" element={<RunDetailLoaded />} />
        </Routes>
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('issue-supervisor-dispatch')).toBeInTheDocument());
    expect(screen.getByText('401 invalid x-api-key')).toBeInTheDocument();
    // The graph still renders (single synthesized "run" node) rather than
    // getting stuck on "Loading run…".
    expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument();
    expect(screen.queryByText('Loading run…')).not.toBeInTheDocument();
  });
});

describe('RunDetail — codename', () => {
  function stubCodenameApi(graph: RunGraphResponse) {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(graph);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
  }

  it('renders the crew chip in the header', async () => {
    stubCodenameApi({
      ...GRAPH,
      run: { ...GRAPH.run, codename: 'cobalt-harbor', codename_derived: false } as RunGraphResponse['run'],
    });
    renderPage();
    await waitFor(() => expect(screen.getByText('cobalt-harbor')).toBeInTheDocument());
  });

  it('names the crew in the gate banner', async () => {
    stubCodenameApi({
      ...GRAPH,
      run: {
        ...GRAPH.run,
        status: 'awaiting_approval',
        awaiting_step_id: 'step_a',
        codename: 'cobalt-harbor',
        codename_derived: false,
      } as RunGraphResponse['run'],
    });
    renderPage();
    await waitFor(() =>
      expect(screen.getByText(/cobalt-harbor is waiting for approval/)).toBeInTheDocument(),
    );
  });
});

describe('RunDetail selection label codenames', () => {
  const CODENAME_GRAPH: RunGraphResponse = {
    ...GRAPH,
    step_results: [
      { ...GRAPH.step_results[0], codename: 'jade-reef/heron' },
    ],
    units: [{ ...GRAPH.units[0], codename: 'jade-reef/lynx#0' }],
  };

  function stub() {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(CODENAME_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
  }

  it('shows the selected step codename next to its agent', async () => {
    stub();
    renderPage();
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    fireEvent.click(screen.getByText('select-step-a'));
    const sel = screen.getByTestId('run-selected-label');
    expect(sel).toHaveTextContent('step_a');
    expect(sel).toHaveTextContent('heron · reviewer');
  });

  it('shows the selected unit codename leaf when a unit is opened', async () => {
    stub();
    renderPage();
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    fireEvent.click(screen.getByText('open-unit'));
    expect(screen.getByTestId('run-selected-label')).toHaveTextContent('lynx#0');
  });

  it('provides dispatch_started sub-run identities to the transcript area', async () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(CODENAME_GRAPH);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation((_id, onEvent) => {
      onEvent({
        type: 'dispatch_started', run_id: 'run-1', sub_run_id: 'sub_9', agent: 'scanner',
        transcript_path: '/t/sub.jsonl', codename: 'jade-reef/heron>lynx#1', provider: 'anthropic', model: 'claude-opus-5-5',
      });
      return () => {};
    });
    renderPage();
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    fireEvent.click(screen.getByText('select-step-a'));
    expect(screen.getByTestId('transcript-panel')).toHaveAttribute('data-subruns', 'sub_9=anthropic/claude-opus-5-5');
  });
});

// ---- Live usage (GET /api/runs/:id/usage) ---------------------------------

describe('RunDetail — live usage', () => {
  const LIVE_SUMMARY = {
    input_tokens: 1_000_000,
    output_tokens: 200_000,
    cached_tokens: 34_567,
    total_tokens: 1_234_567,
    cost_usd: 1.5,
    priced: true,
    runs: 1,
  };
  const liveResp = (over: Record<string, unknown> = {}) => ({
    summary: LIVE_SUMMARY,
    steps: {},
    turns: 2,
    partial: false,
    epoch: '17000000000000000001',
    points_from: 0,
    points: [
      { turn: 1, label: 'step_a', tokens_in: 10, tokens_out: 1, tokens_cached: 0 },
      { turn: 2, label: 'step_a', tokens_in: 20, tokens_out: 2, tokens_cached: 0 },
    ],
    ...over,
  });

  function stubBase(graph: RunGraphResponse) {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(graph);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
  }

  it('renders the live summary in the header and the live points in the chart', async () => {
    stubBase({ ...RUNNING_GRAPH, usage: EMPTY_USAGE });
    const usageSpy = vi.spyOn(api, 'getRunUsage').mockResolvedValue(liveResp());

    renderPage();

    await waitFor(() => expect(screen.getByText(formatTokens(1_234_567))).toBeInTheDocument());
    expect(screen.getByText(formatTokens(1_000_000))).toBeInTheDocument();
    expect(screen.getByText('$1.50')).toBeInTheDocument();
    expect(screen.getByTestId('usage-timeline-mock')).toHaveAttribute('data-points', '2');
    expect(usageSpy).toHaveBeenCalledWith('run-1', { host: undefined });
    // No partial marker on a complete summary.
    expect(screen.queryByText(/≥/)).not.toBeInTheDocument();
  });

  it('shows cache writes next to cached only when there are any', async () => {
    stubBase({ ...RUNNING_GRAPH, usage: EMPTY_USAGE });
    vi.spyOn(api, 'getRunUsage').mockResolvedValue(
      liveResp({ summary: { ...LIVE_SUMMARY, cache_write_tokens: 45_678 } }),
    );

    renderPage();

    const label = await screen.findByText('cache write');
    expect(label.parentElement).toHaveTextContent(`cache write ${formatTokens(45_678)}`);
    expect(screen.getByText('cached')).toBeInTheDocument();
  });

  it('hides cache writes when zero or absent', async () => {
    stubBase({ ...RUNNING_GRAPH, usage: EMPTY_USAGE });
    vi.spyOn(api, 'getRunUsage').mockResolvedValue(liveResp());

    renderPage();

    await waitFor(() => expect(screen.getByText(formatTokens(1_234_567))).toBeInTheDocument());
    expect(screen.queryByText('cache write')).not.toBeInTheDocument();
  });

  it('marks a partial total with a lower-bound sign and an explanatory title', async () => {
    stubBase({ ...GRAPH, usage: EMPTY_USAGE });
    vi.spyOn(api, 'getRunUsage').mockResolvedValue(
      liveResp({ summary: { ...LIVE_SUMMARY, partial: true }, partial: true }),
    );

    renderPage();

    const total = await screen.findByText(`≥${formatTokens(1_234_567)}`);
    expect(total).toHaveAttribute(
      'title',
      'Some transcripts were not readable on this CP (remote host not yet mirrored)',
    );
  });

  it('goes live after a new run\'s transient 404 (run.json not landed when the page mounted)', async () => {
    stubBase({ ...RUNNING_GRAPH, usage: EMPTY_USAGE });
    // The mount-time request races the launch: 404, then the run exists.
    const usageSpy = vi
      .spyOn(api, 'getRunUsage')
      .mockRejectedValueOnce(new ApiError(404, 'not found'))
      .mockResolvedValue(liveResp());

    renderPage();

    await waitFor(() => expect(screen.getByText(formatTokens(1_234_567))).toBeInTheDocument());
    expect(screen.getByTestId('usage-timeline-mock')).toHaveAttribute('data-points', '2');
    expect(usageSpy.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it('keys liveness to the loaded run: navigating in-app to a just-launched run still goes live', async () => {
    // Launching from a run page navigates /runs/run-a → /runs/run-b in place.
    // For one render `graph` still holds run-a's (running) run; run-b's first
    // usage request must not count as "run known", or its launch-race 404
    // would read as an older CP and stop live usage for good.
    const graphA = { ...RUNNING_GRAPH, run: { ...RUNNING_GRAPH.run, id: 'run-a' }, usage: EMPTY_USAGE };
    const graphB = { ...RUNNING_GRAPH, run: { ...RUNNING_GRAPH.run, id: 'run-b' }, usage: EMPTY_USAGE };
    let resolveB: (g: RunGraphResponse) => void = () => {};
    vi.spyOn(api, 'getRunGraph').mockImplementation((id: string) =>
      id === 'run-a'
        ? Promise.resolve(graphA)
        : new Promise<RunGraphResponse>((resolve) => {
            resolveB = resolve;
          }),
    );
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    let bCalls = 0;
    vi.spyOn(api, 'getRunUsage').mockImplementation(async (id: string) => {
      if (id === 'run-a') {
        return liveResp({ summary: { ...LIVE_SUMMARY, total_tokens: 111_111 } }) as never;
      }
      bCalls += 1;
      // run-b's run.json has not landed yet when its page first renders.
      if (bCalls === 1) throw new ApiError(404, 'not found');
      return liveResp({ summary: { ...LIVE_SUMMARY, total_tokens: 7_654_321 } }) as never;
    });

    function GoToB() {
      const navigate = useNavigate();
      return <button onClick={() => navigate('/runs/run-b')}>navigate-to-run-b</button>;
    }
    render(
      <MemoryRouter initialEntries={['/runs/run-a']}>
        <GoToB />
        <Routes>
          <Route path="/runs/:id" element={<RunDetailLoaded />} />
        </Routes>
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText(formatTokens(111_111))).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'navigate-to-run-b' }));
    // run-b's first request goes out while its graph is still loading → 404.
    await waitFor(() => expect(bCalls).toBe(1));
    resolveB(graphB);

    await waitFor(() => expect(screen.getByText(formatTokens(7_654_321))).toBeInTheDocument());
    // …and it keeps polling (the 2 s interval), rather than being written off.
    await waitFor(() => expect(bCalls).toBeGreaterThanOrEqual(3), { timeout: 3500 });
  });

  it('falls back to the graph usage + one-shot timeline when the endpoint 404s', async () => {
    stubBase({
      ...GRAPH,
      usage: { ...EMPTY_USAGE, input_tokens: 42, total_tokens: 4242, priced: true },
    });
    const timelineSpy = vi
      .spyOn(api, 'getRunUsageTimeline')
      .mockResolvedValue([{ turn: 1, label: 'step_a', tokens_in: 1, tokens_out: 1, tokens_cached: 0 }]);
    // beforeEach already rejects getRunUsage with a 404.

    renderPage();

    await waitFor(() => expect(screen.getByText(formatTokens(4242))).toBeInTheDocument());
    await waitFor(() =>
      expect(screen.getByTestId('usage-timeline-mock')).toHaveAttribute('data-points', '1'),
    );
    expect(timelineSpy).toHaveBeenCalledWith('run-1', undefined);
  });
});

// A manual pause records the paused step in `awaiting_step_id` with no
// prompt. That is not a gate: neither the paused run nor the run resumed
// from it may show an approval banner.
describe('RunDetail: pause leftovers are not approval gates', () => {
  function stubWith(status: 'running' | 'paused') {
    const graph: RunGraphResponse = {
      run: {
        id: 'run-1',
        workflow_name: 'review',
        status,
        started_at: '2026-06-01T00:00:00Z',
        awaiting_step_id: 'assess',
      } as RunGraphResponse['run'],
      workflow: { steps: [{ id: 'assess', kind: 'for_each', agent: 'assessor' }] },
      step_results: [],
      units: [],
      usage: EMPTY_USAGE,
    };
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(graph);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
  }

  it.each(['running', 'paused'] as const)('a %s run shows no approval banner', async (status) => {
    stubWith(status);
    renderPage();
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    expect(screen.queryByRole('button', { name: 'Approve run' })).not.toBeInTheDocument();
    expect(screen.queryByText(/Awaiting approval/)).not.toBeInTheDocument();
  });
});

// The run's customer chip (customers Plan 2B): a recorded slug resolves to the
// active customer; `null` is "No customer"; an ABSENT key is the muted
// "Unknown customer" (never "No customer"); a derived attribution is muted
// with a tooltip. With no customers defined, none/unknown show nothing.
describe('RunDetail: customer chip', () => {
  function stubCustomerRun(fields: { customer?: string | null; customer_derived?: boolean }) {
    const graph: RunGraphResponse = {
      ...GRAPH,
      run: { ...GRAPH.run, ...fields } as RunGraphResponse['run'],
    };
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(graph);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
  }
  function renderWithCustomers(customers = [customerRow('acme', { name: 'Acme Corp' })]) {
    return render(
      <MemoryRouter initialEntries={['/runs/run-1']}>
        {withCustomerScope(
          <Routes>
            <Route path="/runs/:id" element={<RunDetailLoaded />} />
          </Routes>,
          { customers },
        )}
      </MemoryRouter>,
    );
  }

  it('shows the recorded customer resolved from the active list', async () => {
    stubCustomerRun({ customer: 'acme', customer_derived: false });
    renderWithCustomers();
    expect(await screen.findByText('Acme Corp')).toBeInTheDocument();
    expect(screen.queryByTitle(/project's current customer/i)).toBeNull();
  });

  it('mutes a derived attribution and explains it in a tooltip', async () => {
    stubCustomerRun({ customer: 'acme', customer_derived: true });
    renderWithCustomers();
    const chip = (await screen.findByText('Acme Corp')).closest('span[title]');
    expect(chip).toHaveAttribute('title', expect.stringMatching(/current customer/i));
    expect(chip?.className).toMatch(/text-ink-dim/);
  });

  it('shows "No customer" for a recorded null', async () => {
    stubCustomerRun({ customer: null });
    renderWithCustomers();
    expect(await screen.findByText('No customer')).toBeInTheDocument();
  });

  it('shows "Unknown customer" — never "No customer" — when the key is absent', async () => {
    stubCustomerRun({});
    renderWithCustomers();
    expect(await screen.findByText('Unknown customer')).toBeInTheDocument();
    expect(screen.queryByText('No customer')).toBeNull();
  });

  it('shows a slug the active list does not hold under its slug', async () => {
    stubCustomerRun({ customer: 'gone' });
    renderWithCustomers();
    expect(await screen.findByText('gone')).toBeInTheDocument();
  });

  it('shows nothing for none / unknown when no customers are defined', async () => {
    stubCustomerRun({ customer: null });
    renderWithCustomers([]);
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    expect(screen.queryByText('No customer')).toBeNull();
  });

  it('resolves an archived customer (loaded once, lazily) and shows its archived state', async () => {
    stubCustomerRun({ customer: 'oldco' });
    const spy = vi
      .spyOn(api, 'getCustomers')
      .mockImplementation(async (o) => (o?.archived ? [customerRow('oldco', { name: 'Old Co', archived: true })] : []));
    renderWithCustomers([]);
    expect(await screen.findByText('Old Co')).toBeInTheDocument();
    expect(screen.getByText('(archived)')).toBeInTheDocument();
    expect(spy.mock.calls.filter(([o]) => o?.archived).length).toBe(1);
  });

  it('marks the header cost when the run was priced at the global rates', async () => {
    const graph: RunGraphResponse = {
      ...GRAPH,
      usage: { ...EMPTY_USAGE, cost_usd: 1.2, priced: true, pricing_error: 'customer acme: config layer does not parse' },
    };
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(graph);
    vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
    vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
    renderWithCustomers();
    expect(
      await screen.findByRole('img', { name: 'Pricing unavailable: customer acme: config layer does not parse' }),
    ).toBeInTheDocument();
  });

  it('does not load the archived list when the slug is an active customer', async () => {
    stubCustomerRun({ customer: 'acme' });
    const spy = vi.spyOn(api, 'getCustomers').mockResolvedValue([customerRow('acme', { name: 'Acme Corp' })]);
    renderWithCustomers();
    expect(await screen.findByText('Acme Corp')).toBeInTheDocument();
    expect(spy.mock.calls.filter(([o]) => o?.archived).length).toBe(0);
  });
});
