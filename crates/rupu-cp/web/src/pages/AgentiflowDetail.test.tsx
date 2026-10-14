// @vitest-environment jsdom
// AgentiflowDetail — header + goals + budget (always shown) plus the tabbed
// Flow (graph + fleet) / Assets / Findings / Network / Messages / Transcript / Events
// views over one GET /api/agentiflows/:id. TranscriptPanel is mocked (heavy;
// its own tests); the Findings/Assets/Messages tabs fetch on open and aren't
// exercised here.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, within, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, ApiError, type AgentiflowDetail as Detail } from '../lib/api';

vi.mock('../components/TranscriptPanel', () => ({
  __esModule: true,
  default: ({ path, live }: { path: string; live: boolean }) => (
    <div data-testid="transcript-panel">
      transcript:{path}:{String(live)}
    </div>
  ),
}));

const { fetchAgentiflowNetflow, fetchNetflowExplorer } = vi.hoisted(() => ({
  fetchAgentiflowNetflow: vi.fn(),
  fetchNetflowExplorer: vi.fn(),
}));
vi.mock('../lib/netflow', async () => {
  const actual = await vi.importActual<typeof import('../lib/netflow')>('../lib/netflow');
  return { ...actual, fetchAgentiflowNetflow, fetchNetflowExplorer };
});

import AgentiflowDetail from './AgentiflowDetail';
import {
  emptyFlowsResponse,
  populatedExplorerResponse,
} from '../components/netflow/explorer/explorerFixtures';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const DETAIL: Detail = {
  record: {
    id: 'af_01ABC',
    name: 'rupu-self-review',
    codename: 'cobalt-harbor',
    codename_derived: true,
    status: 'completed',
    stop_reason: 'budget_exhausted:rounds',
    rounds: 6,
    goals_met: 0,
    goals_total: 1,
    spent_usd: 0.25,
    spent_tokens: 5000,
    started_at: '2026-10-07T15:07:08Z',
    ended_at: '2026-10-07T15:07:09Z',
    engagement_profiles: ['code'],
    runner_alive: null,
    trigger: 'agentiflow',
    runner_pid: null,
    goals: [{ id: 'real-issues', met: false, current: 1, target: 3 }],
  },
  def: {
    name: 'rupu-self-review',
    description: 'Review rupu with a fleet.',
    lead: 'rupu-review-lead',
    engagement_profiles: ['code'],
    trigger: null,
    goals: [
      {
        id: 'real-issues',
        objective: 'Find real issues',
        required: true,
        verify_with: null,
        predicate: 'findings, count >= 3',
        target: {},
      },
    ],
    coverage: null,
    budget: { usd: 1, rounds: 6, tokens: 100_000 },
    scope: {},
    pool: { agents: ['recon'], workflows: 'all' },
    round: null,
  },
  budget_state: 'hard:rounds',
  events: [
    { ts: '2026-10-07T15:07:08Z', kind: 'run_started', goals: 1, engagement_profiles: ['code'] },
    {
      ts: '2026-10-07T15:07:09Z',
      kind: 'round',
      round: 0,
      outcome: 'error',
      error: 'provider: API error 401',
      budget: 'ok',
      goals_met: 0,
      goals_total: 1,
      spent_usd: 0,
      spent_tokens: 0,
    },
    { ts: '2026-10-07T15:07:10Z', kind: 'run_stopped', stop_reason: 'budget_exhausted:rounds', detail: 'rounds' },
  ],
  units: [
    {
      unit_id: 'unit_1',
      codename: 'cobalt-harbor/heron#1',
      codename_derived: false,
      agent: 'recon',
      participant: 'recon#1',
      kind: 'agent',
      status: { state: 'done', success: true, output: 'found three things' },
      pgid: null,
      started_at: '2026-10-07T15:07:08Z',
      transcript_path: '/tmp/u1.jsonl',
    },
    {
      unit_id: 'unit_2',
      codename: 'cobalt-harbor/lynx#1',
      codename_derived: false,
      agent: 'triage',
      participant: null,
      kind: 'agent',
      status: { state: 'failed', error: 'spawn failed' },
      pgid: null,
      started_at: null,
      transcript_path: null,
    },
  ],
  lead_transcripts: [
    { round: 0, path: '/tmp/lead/transcript.r0.jsonl' },
    { round: 1, path: '/tmp/lead/transcript.r1.jsonl' },
  ],
};

function renderDetail() {
  return render(
    <MemoryRouter initialEntries={['/agentiflows/af_01ABC']}>
      <Routes>
        <Route path="/agentiflows/:id" element={<AgentiflowDetail />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('AgentiflowDetail', () => {
  it('renders the header, goals, budget, fleet, lead transcript and events', async () => {
    vi.spyOn(api, 'getAgentiflow').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'getAgentiflowEvents').mockResolvedValue({ events: DETAIL.events });
    renderDetail();

    expect(await screen.findByRole('heading', { name: 'rupu-self-review' })).toBeInTheDocument();
    expect(screen.getByText('af_01ABC')).toBeInTheDocument();
    expect(screen.getByText('Review rupu with a fleet.')).toBeInTheDocument();

    // Goals: the objective comes from the definition, the counters from the record.
    expect(screen.getByText('Find real issues')).toBeInTheDocument();
    expect(screen.getByText('findings, count >= 3')).toBeInTheDocument();
    expect(screen.getByText('1/3')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: 'real-issues progress' })).toHaveAttribute('aria-valuenow', '33');

    // Budget: rounds 6/6 is at its cap; usd 0.25/1 is not.
    expect(screen.getByText('hard:rounds')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: 'rounds used' })).toHaveAttribute('aria-valuenow', '100');
    expect(screen.getByRole('progressbar', { name: 'spend used' })).toHaveAttribute('aria-valuenow', '25');

    // Fleet (the default Flow tab). Unit identities also render in the graph
    // above, so scope these assertions to the Fleet panel.
    const fleet = screen.getByRole('heading', { name: 'Fleet' }).closest('section')!;
    expect(within(fleet).getByText('heron#1 · recon')).toBeInTheDocument();
    expect(within(fleet).getByText('lynx#1 · triage')).toBeInTheDocument();
    expect(within(fleet).getByText('spawn failed')).toBeInTheDocument();
    expect(within(fleet).getByText('found three things')).toBeInTheDocument();
    expect(within(fleet).getByRole('link', { name: 'transcript' })).toHaveAttribute(
      'href',
      `/transcript?path=${encodeURIComponent('/tmp/u1.jsonl')}&live=0`,
    );

    // Transcript tab: the LAST round's, not live (the run is over).
    fireEvent.click(screen.getByRole('button', { name: 'Transcript' }));
    expect(screen.getByTestId('transcript-panel')).toHaveTextContent('/tmp/lead/transcript.r1.jsonl:false');

    // Events tab: the shared live-events feed (RunEventFeed / EventCard), fed by
    // GET /api/agentiflows/:id/events (fetched async, hence findByText).
    fireEvent.click(screen.getByRole('button', { name: 'Events' }));
    expect(await screen.findByText('3 events')).toBeInTheDocument();
    expect(screen.getByText('Engagement started')).toBeInTheDocument();
    expect(screen.getByText('Engagement stopped')).toBeInTheDocument();
    expect(screen.getByText('provider: API error 401')).toBeInTheDocument();
  });

  it('shows the Children tab — the deep launch tree under the flow', async () => {
    vi.spyOn(api, 'getAgentiflow').mockResolvedValue(DETAIL);
    const getChildren = vi.spyOn(api, 'getAgentiflowChildren').mockResolvedValue([
      {
        handle: 'u_recon',
        kind: 'agent',
        name: 'cellgw-recon',
        run_id: 'run_recon',
        parent_run_id: 'af_01ABC',
        depth: 1,
        codename: 'cobalt-harbor/heron>pipit#1',
        address: 'pipit#1',
        status: 'done',
        success: true,
        error: null,
        tokens: { input: 10, output: 20 },
        duration_ms: 1000,
        started_at: '2026-10-07T15:07:08Z',
        finished_at: '2026-10-07T15:07:09Z',
        transcript_path: '/tmp/run_recon.jsonl',
        legacy: false,
      },
      {
        handle: 's_1',
        kind: 'subagent',
        name: 'port-scanner',
        run_id: 'run_sub',
        parent_run_id: 'run_recon',
        depth: 2,
        codename: 'cobalt-harbor/heron>pipit#1>otter#1',
        address: null,
        status: 'running',
        success: null,
        error: null,
        tokens: null,
        duration_ms: null,
        started_at: '2026-10-07T15:07:08Z',
        finished_at: null,
        transcript_path: '/tmp/run_sub.jsonl',
        legacy: false,
      },
    ]);
    renderDetail();
    await screen.findByRole('heading', { name: 'rupu-self-review' });

    fireEvent.click(screen.getByRole('button', { name: 'Children' }));
    const panel = await screen.findByTestId('children-panel');
    expect(getChildren).toHaveBeenCalledWith('af_01ABC', expect.anything());
    // The unit and its own sub-agent both appear (own-segment codenames).
    expect(within(panel).getByText('pipit#1')).toBeInTheDocument();
    expect(within(panel).getByText('otter#1')).toBeInTheDocument();
    // The count rides on the tab label once loaded.
    expect(await screen.findByRole('button', { name: 'Children (2)' })).toBeInTheDocument();
  });

  it('loads the engagement span of the whole flow on the Network tab', async () => {
    vi.spyOn(api, 'getAgentiflow').mockResolvedValue(DETAIL);
    fetchNetflowExplorer.mockResolvedValue(populatedExplorerResponse());
    fetchAgentiflowNetflow.mockResolvedValue(emptyFlowsResponse());
    renderDetail();
    expect(await screen.findByRole('heading', { name: 'rupu-self-review' })).toBeInTheDocument();
    expect(fetchNetflowExplorer).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Network' }));
    const window = { from: '2026-10-07T15:02:08.000Z', to: '2026-10-07T15:12:09.000Z' };
    await waitFor(() =>
      expect(fetchNetflowExplorer).toHaveBeenCalledWith('agentiflow:af_01ABC', window, undefined),
    );
    expect(fetchAgentiflowNetflow).toHaveBeenCalledWith('af_01ABC', window, undefined);
  });

  it('falls back to the record alone without a definition snapshot', async () => {
    vi.spyOn(api, 'getAgentiflow').mockResolvedValue({ ...DETAIL, def: null, lead_transcripts: [] });
    renderDetail();
    // The goal id stands in for the objective; no caps to compare against.
    expect(await screen.findByText('real-issues')).toBeInTheDocument();
    expect(screen.getByText(/Definition snapshot unavailable/)).toBeInTheDocument();
    // The transcript browser has no lead transcript to default to, so the
    // viewer prompts for a selection (the unit list is still there to click).
    fireEvent.click(screen.getByRole('button', { name: 'Transcript' }));
    expect(screen.getByText('Select a transcript to view it.')).toBeInTheDocument();
  });

  it('tails the lead live while the run is running, and flags a dead coordinator', async () => {
    vi.spyOn(api, 'getAgentiflow').mockResolvedValue({
      ...DETAIL,
      record: { ...DETAIL.record, status: 'running', stop_reason: null, ended_at: null, runner_alive: false, runner_pid: 4242 },
    });
    renderDetail();
    expect(await screen.findByText(/no longer running/)).toHaveTextContent('pid 4242');
    // The lead transcript tails live while the run is running.
    fireEvent.click(screen.getByRole('button', { name: 'Transcript' }));
    expect(screen.getByTestId('transcript-panel')).toHaveTextContent('transcript.r1.jsonl:true');
  });

  it('says so when the agentiflow does not exist', async () => {
    vi.spyOn(api, 'getAgentiflow').mockRejectedValue(new ApiError(404, 'not found', '{"error":"not found"}'));
    renderDetail();
    expect(await screen.findByText('Agentiflow not found')).toBeInTheDocument();
  });
});
