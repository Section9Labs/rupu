// @vitest-environment jsdom
// AgentiflowDetail — header + goals + budget (always shown) plus the tabbed
// Flow (graph + fleet) / Findings / Assets / Messages / Transcript / Events
// views over one GET /api/agentiflows/:id. TranscriptPanel is mocked (heavy;
// its own tests); the Findings/Assets/Messages tabs fetch on open and aren't
// exercised here.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, within, fireEvent } from '@testing-library/react';
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

import AgentiflowDetail from './AgentiflowDetail';

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

    // Events tab, newest first.
    fireEvent.click(screen.getByRole('button', { name: 'Events' }));
    const events = screen.getByRole('heading', { name: 'Events' }).closest('section')!;
    const items = within(events).getAllByRole('listitem');
    expect(items).toHaveLength(3);
    expect(items[0]).toHaveTextContent('budget_exhausted:rounds');
    expect(items[1]).toHaveTextContent('provider: API error 401');
    expect(items[2]).toHaveTextContent('1 goal');
  });

  it('falls back to the record alone without a definition snapshot', async () => {
    vi.spyOn(api, 'getAgentiflow').mockResolvedValue({ ...DETAIL, def: null, lead_transcripts: [] });
    renderDetail();
    // The goal id stands in for the objective; no caps to compare against.
    expect(await screen.findByText('real-issues')).toBeInTheDocument();
    expect(screen.getByText(/Definition snapshot unavailable/)).toBeInTheDocument();
    // The transcript tab shows the empty state when there is no lead transcript.
    fireEvent.click(screen.getByRole('button', { name: 'Transcript' }));
    expect(screen.getByText('No lead transcript yet.')).toBeInTheDocument();
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
