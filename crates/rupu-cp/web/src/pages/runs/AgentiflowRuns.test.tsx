// @vitest-environment jsdom
// AgentiflowRuns — the Activity "agentiflows" list: one fetch, a sortable table,
// whole-row navigation to /agentiflows/:id, Refresh re-fetches.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AgentiflowRow } from '../../lib/api';
import { agentiflowsApi } from '../../lib/api3b/agentiflows';
import AgentiflowRuns from './AgentiflowRuns';
import { commitQuery } from '../../components/query/queryTestUtils';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const ROW: AgentiflowRow = {
  id: 'af_01ABC',
  name: 'rupu-self-review',
  codename: 'cobalt-harbor',
  codename_derived: true,
  status: 'completed',
  stop_reason: 'budget_exhausted:rounds',
  rounds: 6,
  goals_met: 1,
  goals_total: 3,
  spent_usd: 1.5,
  spent_tokens: 12_345,
  started_at: new Date(Date.now() - 3 * 60_000).toISOString(),
  ended_at: new Date().toISOString(),
  engagement_profiles: ['code'],
  runner_alive: null,
};

function renderList() {
  return render(
    <MemoryRouter>
      <AgentiflowRuns />
    </MemoryRouter>,
  );
}

describe('AgentiflowRuns', () => {
  it('renders a row per agentiflow and links it to its detail page', async () => {
    vi.spyOn(api, 'getAgentiflows').mockResolvedValue([ROW]);
    renderList();
    expect(await screen.findByText('rupu-self-review')).toBeInTheDocument();
    expect(screen.getByText('cobalt-harbor')).toBeInTheDocument();
    expect(screen.getByText('budget_exhausted:rounds')).toBeInTheDocument();
    expect(screen.getByText('1/3')).toBeInTheDocument();
    expect(screen.getByText('$1.50')).toBeInTheDocument();
    expect(screen.getByText('3m ago')).toBeInTheDocument();
    const link = screen.getByText('rupu-self-review').closest('a');
    expect(link).toHaveAttribute('href', '/agentiflows/af_01ABC');
  });

  it('flags a running row whose coordinator died', async () => {
    vi.spyOn(api, 'getAgentiflows').mockResolvedValue([
      { ...ROW, status: 'running', stop_reason: null, ended_at: null, runner_alive: false },
    ]);
    renderList();
    expect(await screen.findByText('coordinator gone')).toBeInTheDocument();
  });

  it('shows the empty state, which offers New agentiflow instead of a CLI hint', async () => {
    vi.spyOn(api, 'getAgentiflows').mockResolvedValue([]);
    renderList();
    expect(await screen.findByText('No agentiflow runs yet')).toBeInTheDocument();
    expect(screen.queryByText(/rupu agentiflow run/)).not.toBeInTheDocument();
    // The header's New agentiflow plus the empty state's.
    expect(screen.getAllByRole('button', { name: 'New agentiflow' })).toHaveLength(2);
  });

  it('New agentiflow opens the launcher sheet', async () => {
    vi.spyOn(api, 'getAgentiflows').mockResolvedValue([ROW]);
    vi.spyOn(agentiflowsApi, 'listDefs').mockResolvedValue([]);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    renderList();
    await screen.findByText('rupu-self-review');
    fireEvent.click(screen.getByRole('button', { name: 'New agentiflow' }));
    expect(await screen.findByRole('dialog', { name: 'New agentiflow run' })).toBeInTheDocument();
    expect(screen.queryByLabelText(/host/i)).not.toBeInTheDocument();
  });

  it('shows a fetch failure and re-fetches on Refresh', async () => {
    const spy = vi.spyOn(api, 'getAgentiflows').mockRejectedValueOnce(new Error('boom')).mockResolvedValue([ROW]);
    renderList();
    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
    fireEvent.click(screen.getByRole('button', { name: /refresh/i }));
    await waitFor(() => expect(screen.getByText('rupu-self-review')).toBeInTheDocument());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(spy).toHaveBeenCalledTimes(2);
  });
});

describe('AgentiflowRuns query line', () => {
  const ROWS: AgentiflowRow[] = [
    ROW,
    { ...ROW, id: 'af_2', name: 'net-sweep', codename: 'amber-quay', status: 'running', stop_reason: null, ended_at: null, runner_alive: false, engagement_profiles: ['network'] },
    { ...ROW, id: 'af_3', name: 'old-sweep', codename: 'slate-pier', stop_reason: 'goals_met', goals_met: 3, started_at: '2026-01-01T00:00:00Z' },
  ];
  const listed = () => ['rupu-self-review', 'net-sweep', 'old-sweep'].filter((n) => screen.queryByText(n) !== null);
  const mount = (entry: string) =>
    render(
      <MemoryRouter initialEntries={[entry]}>
        <AgentiflowRuns />
      </MemoryRouter>,
    );

  it('narrows the table to the query in ?q=', async () => {
    vi.spyOn(api, 'getAgentiflows').mockResolvedValue(ROWS);
    mount('/runs/agentiflows?q=status:completed since:7d');
    await waitFor(() => expect(listed()).toEqual(['rupu-self-review']));
    expect(screen.getByTestId('query-count')).toHaveTextContent('1 of 3');
  });

  it('narrows the table as a query is committed', async () => {
    vi.spyOn(api, 'getAgentiflows').mockResolvedValue(ROWS);
    mount('/runs/agentiflows');
    await waitFor(() => expect(listed()).toEqual(['rupu-self-review', 'net-sweep', 'old-sweep']));
    commitQuery('Filter agentiflows', 'stop:budget_exhausted,goals_met');
    await waitFor(() => expect(listed()).toEqual(['rupu-self-review', 'old-sweep']));
    commitQuery('Filter agentiflows', 'slate');
    await waitFor(() => expect(listed()).toEqual(['old-sweep']));
  });

  it('says so when the query keeps nothing', async () => {
    vi.spyOn(api, 'getAgentiflows').mockResolvedValue(ROWS);
    mount('/runs/agentiflows?q=has:coordinator_gone profile:code');
    expect(await screen.findByText('No agentiflows match the filter.')).toBeInTheDocument();
  });
});
