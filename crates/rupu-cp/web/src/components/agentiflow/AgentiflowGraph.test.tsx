// @vitest-environment jsdom
// AgentiflowGraph recursion — a dispatched `workflow` unit is not a leaf: it
// carries an expand chevron, starts collapsed, and on expand lazy-loads its own
// run graph (`GET /api/runs/:id/graph`) and unfolds its steps as a nested
// sub-flow. Agent units stay leaves (transcript link, no chevron).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AgentiflowDetail as Detail, type RunGraphResponse } from '../../lib/api';
import AgentiflowGraph from './AgentiflowGraph';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const DETAIL: Detail = {
  record: {
    id: 'af_X',
    name: 'web-assess',
    codename: 'cobalt-harbor',
    codename_derived: false,
    status: 'running',
    stop_reason: null,
    rounds: 1,
    goals_met: 0,
    goals_total: 1,
    spent_usd: 0.1,
    spent_tokens: 100,
    started_at: '2026-10-07T15:00:00Z',
    ended_at: null,
    engagement_profiles: ['network'],
    runner_alive: true,
    trigger: 'agentiflow',
    runner_pid: 1,
    goals: [{ id: 'g', met: false, current: 0, target: 1 }],
  },
  def: null,
  budget_state: 'ok',
  events: [{ ts: '2026-10-07T15:00:00Z', kind: 'run_started', goals: 1 }],
  units: [
    {
      unit_id: 'unit_agent',
      codename: 'cobalt-harbor/heron#1',
      codename_derived: false,
      agent: 'recon',
      participant: 'recon#1',
      kind: 'agent',
      status: { state: 'done', success: true, output: 'ok' },
      pgid: null,
      started_at: '2026-10-07T15:00:01Z',
      transcript_path: '/tmp/a.jsonl',
    },
    {
      unit_id: 'run_wf1',
      codename: 'cobalt-harbor/siskin#1',
      codename_derived: false,
      agent: 'network-assessment',
      participant: 'wf#1',
      kind: 'workflow',
      status: { state: 'running' },
      pgid: null,
      started_at: '2026-10-07T15:00:02Z',
      transcript_path: null,
    },
  ],
  lead_transcripts: [],
};

const GRAPH = {
  run: {} as unknown,
  workflow: {
    steps: [
      { id: 'discover-hosts', kind: 'step', agent: 'recon' },
      { id: 'enumerate-services', kind: 'for_each', agent: 'service-analyst', for_each: '{{ steps.discover-hosts.hosts }}' },
    ],
    edges: [],
  },
  step_results: [{ run_id: 'run_wf1', step_id: 'discover-hosts', success: true, codename: 'cobalt-harbor/siskin.a#1' }],
  units: [],
} as unknown as RunGraphResponse;

function renderGraph() {
  return render(
    <MemoryRouter>
      <AgentiflowGraph detail={DETAIL} />
    </MemoryRouter>,
  );
}

describe('AgentiflowGraph recursion', () => {
  it('expands a workflow unit into its nested steps, lazy-loading the run graph', async () => {
    const spy = vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    renderGraph();

    // Collapsed by default: no step rows, and we haven't fetched anything.
    expect(screen.queryByText('discover-hosts')).not.toBeInTheDocument();
    expect(spy).not.toHaveBeenCalled();

    // The agent unit is a leaf (a transcript link, no expand control); only the
    // workflow unit carries an expand chevron.
    expect(screen.queryByRole('button', { name: /Expand cobalt-harbor\/heron#1 flow/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Expand cobalt-harbor\/siskin#1 flow/ }));

    // The sub-flow fetches the unit's own run graph and unfolds its steps.
    expect(spy).toHaveBeenCalledWith('run_wf1');
    expect(await screen.findByText('discover-hosts')).toBeInTheDocument();
    expect(screen.getByText('enumerate-services')).toBeInTheDocument();
    // The for_each step carries its kind badge; the plain step does not.
    expect(screen.getByText('for_each')).toBeInTheDocument();
    // Step status folds from the sub-run's step_results.
    expect(screen.getByText('done')).toBeInTheDocument();

    // Collapsing hides the steps again (and does not refetch).
    fireEvent.click(screen.getByRole('button', { name: /Collapse cobalt-harbor\/siskin#1 flow/ }));
    expect(screen.queryByText('discover-hosts')).not.toBeInTheDocument();
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('surfaces a failed sub-graph load instead of an empty flow', async () => {
    vi.spyOn(api, 'getRunGraph').mockRejectedValue(new Error('boom'));
    renderGraph();
    fireEvent.click(screen.getByRole('button', { name: /Expand cobalt-harbor\/siskin#1 flow/ }));
    expect(await screen.findByText("couldn't load this flow")).toBeInTheDocument();
  });

  it('keeps every round open by default — collapse only applies below level 2', () => {
    const twoRounds: Detail = {
      ...DETAIL,
      record: { ...DETAIL.record, status: 'completed', stop_reason: 'goals_met', ended_at: '2026-10-07T15:03:00Z', rounds: 2 },
      events: [
        { ts: '2026-10-07T15:00:00Z', kind: 'run_started', goals: 1 },
        { ts: '2026-10-07T15:01:00Z', kind: 'round', round: 0, budget: 'ok', goals_met: 0, goals_total: 1 },
        { ts: '2026-10-07T15:02:00Z', kind: 'round', round: 1, budget: 'ok', goals_met: 1, goals_total: 1 },
        { ts: '2026-10-07T15:03:00Z', kind: 'run_stopped', stop_reason: 'goals_met' },
      ],
      units: [
        { ...DETAIL.units[0], unit_id: 'u_r0', kind: 'agent', status: { state: 'done', success: true }, started_at: '2026-10-07T15:00:30Z' },
        { ...DETAIL.units[0], unit_id: 'u_r1', codename: 'cobalt-harbor/lynx#1', agent: 'triage', kind: 'agent', status: { state: 'done', success: true }, started_at: '2026-10-07T15:01:30Z' },
      ],
    };
    render(
      <MemoryRouter>
        <AgentiflowGraph detail={twoRounds} />
      </MemoryRouter>,
    );
    // Both rounds render, and NEITHER is collapsed (no "(hidden)" affordance) —
    // the old default only opened the last round.
    expect(screen.getByText('round 0')).toBeInTheDocument();
    expect(screen.getByText('round 1')).toBeInTheDocument();
    expect(screen.queryByText(/hidden/)).not.toBeInTheDocument();
    // One unit per round is mounted (the kind badge appears once per unit).
    expect(screen.getAllByText('agent')).toHaveLength(2);
  });

  it('keeps an agent unit a leaf with a transcript link', () => {
    vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
    renderGraph();
    const links = screen.getAllByRole('link');
    expect(links.some((l) => l.getAttribute('href') === `/transcript?path=${encodeURIComponent('/tmp/a.jsonl')}&live=0`)).toBe(true);
    // The workflow unit still offers the full-page DAG link.
    expect(within(document.body).getByRole('link', { name: 'open flow →' })).toHaveAttribute('href', '/runs/run_wf1');
  });
});
