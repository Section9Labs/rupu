// @vitest-environment jsdom
// TranscriptBrowser — the agentiflow Transcript tab as a two-pane browser:
// every transcript grouped by round on the left (lead + units, a workflow
// unit's step transcripts nested + lazy-loaded), the selected one in the viewer
// on the right (default: the last lead round). TranscriptPanel is mocked (heavy;
// its own tests) so we can assert which path the viewer is showing.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AgentiflowDetail as Detail, type RunGraphResponse } from '../../lib/api';

vi.mock('../TranscriptPanel', () => ({
  __esModule: true,
  default: ({ path, live }: { path: string; live: boolean }) => (
    <div data-testid="viewer">
      {path}|{String(live)}
    </div>
  ),
}));

import TranscriptBrowser from './TranscriptBrowser';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const DETAIL: Detail = {
  record: {
    id: 'af_T',
    name: 't',
    codename: 'cobalt-harbor',
    codename_derived: false,
    status: 'completed',
    stop_reason: 'goals_met',
    rounds: 2,
    goals_met: 1,
    goals_total: 1,
    spent_usd: 1,
    spent_tokens: 100,
    started_at: '2026-10-07T15:00:00Z',
    ended_at: '2026-10-07T15:03:00Z',
    engagement_profiles: ['network'],
    runner_alive: null,
    trigger: 'agentiflow',
    runner_pid: null,
    goals: [],
  },
  def: null,
  budget_state: null,
  events: [
    { ts: '2026-10-07T15:00:00Z', kind: 'run_started' },
    { ts: '2026-10-07T15:01:00Z', kind: 'round', round: 0 },
    { ts: '2026-10-07T15:02:00Z', kind: 'round', round: 1 },
    { ts: '2026-10-07T15:03:00Z', kind: 'run_stopped' },
  ],
  units: [
    { unit_id: 'u_recon', codename: 'cobalt-harbor/elk#1', codename_derived: false, agent: 'recon', participant: 'recon#1', kind: 'agent', status: { state: 'done', success: true }, pgid: null, started_at: '2026-10-07T15:00:30Z', transcript_path: '/u/recon.jsonl' },
    { unit_id: 'run_wf', codename: 'cobalt-harbor/siskin#1', codename_derived: false, agent: 'net-assess', participant: 'wf#1', kind: 'workflow', status: { state: 'done', success: true }, pgid: null, started_at: '2026-10-07T15:00:40Z', transcript_path: null },
  ],
  lead_transcripts: [
    { round: 0, path: '/lead/r0.jsonl' },
    { round: 1, path: '/lead/r1.jsonl' },
  ],
};

const WF_GRAPH = {
  run: {} as unknown,
  workflow: { steps: [{ id: 'discover', kind: 'step', agent: 'crawl' }], edges: [] },
  // No codename on the step → the row renders the plain `<id> · <agent>` label.
  step_results: [{ run_id: 'run_wf', step_id: 'discover', success: true, transcript_path: '/wf/discover.jsonl' }],
  units: [],
} as unknown as RunGraphResponse;

/** The list row (a button) whose text contains every needle. */
function rowWith(...needles: string[]): HTMLElement {
  const btn = screen.getAllByRole('button').find((b) => needles.every((n) => (b.textContent ?? '').includes(n)));
  if (!btn) throw new Error(`no row matching ${needles.join(' + ')}`);
  return btn;
}

function renderBrowser() {
  return render(
    <MemoryRouter>
      <TranscriptBrowser detail={DETAIL} running={false} />
    </MemoryRouter>,
  );
}

describe('TranscriptBrowser', () => {
  it('groups by round and defaults the viewer to the last lead round', () => {
    renderBrowser();
    // Both rounds listed; the last round is open by default.
    expect(screen.getByRole('button', { name: /Round 0/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Round 1/ })).toBeInTheDocument();
    // Viewer shows the last lead round, not live (the run is over).
    expect(screen.getByTestId('viewer')).toHaveTextContent('/lead/r1.jsonl|false');
    expect(screen.getByText('Lead · round 1')).toBeInTheDocument();
  });

  it('selects a unit transcript on click', () => {
    renderBrowser();
    // Round 0 is collapsed by default — open it, then pick the recon unit.
    fireEvent.click(screen.getByRole('button', { name: /Round 0/ }));
    fireEvent.click(rowWith('elk', 'recon'));
    expect(screen.getByTestId('viewer')).toHaveTextContent('/u/recon.jsonl|false');
  });

  it('lazy-loads a workflow unit into nested step transcripts', async () => {
    const spy = vi.spyOn(api, 'getRunGraph').mockResolvedValue(WF_GRAPH);
    renderBrowser();
    fireEvent.click(screen.getByRole('button', { name: /Round 0/ }));
    // No sub-graph fetched until the workflow unit is expanded.
    expect(spy).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Expand workflow' }));
    expect(spy).toHaveBeenCalledWith('run_wf');
    // The nested step transcript appears and loads into the viewer on click.
    const step = await screen.findByText('discover · crawl');
    fireEvent.click(step);
    expect(screen.getByTestId('viewer')).toHaveTextContent('/wf/discover.jsonl|false');
  });

  it('filters the list by query', () => {
    renderBrowser();
    fireEvent.change(screen.getByPlaceholderText(/Filter by codename/), { target: { value: 'siskin' } });
    // Only the round holding the siskin workflow unit survives.
    expect(screen.getByRole('button', { name: /Round 0/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Round 1/ })).not.toBeInTheDocument();
    // Its workflow row is shown (filtering force-opens the round).
    const list = screen.getByRole('button', { name: 'Expand workflow' }).closest('div');
    expect(within(list!).getByText('flow')).toBeInTheDocument();
  });
});
