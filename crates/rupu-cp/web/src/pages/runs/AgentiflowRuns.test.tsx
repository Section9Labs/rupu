// @vitest-environment jsdom
// AgentiflowRuns — the Activity "agentiflows" list: one fetch, a sortable table,
// whole-row navigation to /agentiflows/:id, Refresh re-fetches.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AgentiflowRow } from '../../lib/api';
import AgentiflowRuns from './AgentiflowRuns';

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

  it('shows the empty state', async () => {
    vi.spyOn(api, 'getAgentiflows').mockResolvedValue([]);
    renderList();
    expect(await screen.findByText('No agentiflow runs yet')).toBeInTheDocument();
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
