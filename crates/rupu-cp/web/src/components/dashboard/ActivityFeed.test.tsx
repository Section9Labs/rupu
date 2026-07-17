// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
// fireEvent, not user-event: @testing-library/user-event is NOT a dependency
// and this plan adds none.
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { ActivityFeed } from './ActivityFeed';
import type { CycleRollup, DashboardRecentRun } from '../../lib/api';

afterEach(() => {
  cleanup();
});

const wrap = (ui: React.ReactNode) => render(<MemoryRouter>{ui}</MemoryRouter>);

const cycle: CycleRollup = {
  cycle_id: 'cyc_1',
  worker_name: 'nightly-review',
  started_at: '2026-07-16T03:00:00Z',
  finished_at: '2026-07-16T03:12:00Z',
  ran: 12,
  skipped: 0,
  failed: 2,
  runs: [
    { run_id: 'r_ok_1', status: 'completed' },
    { run_id: 'r_ok_2', status: 'completed' },
    { run_id: 'r_bad', status: 'failed' },
  ],
};

const manualRun: DashboardRecentRun = {
  id: 'run_m1',
  workflow_name: 'adhoc',
  status: 'completed',
  started_at: '2026-07-16T09:00:00Z',
  finished_at: '2026-07-16T09:01:00Z',
  trigger: 'manual',
};

describe('ActivityFeed', () => {
  it('collapses a 12-run cycle into ONE row', () => {
    wrap(<ActivityFeed cycles={[cycle]} recentManual={[]} />);
    expect(screen.getByText(/nightly-review/)).toBeInTheDocument();
    // The whole point: 12 runs, one row.
    expect(screen.getAllByRole('listitem')).toHaveLength(1);
  });

  it('shows the cycle outcome tally', () => {
    wrap(<ActivityFeed cycles={[cycle]} recentManual={[]} />);
    expect(screen.getByText(/2 failed/)).toBeInTheDocument();
  });

  it('renders manual runs individually, never grouped', () => {
    wrap(<ActivityFeed cycles={[]} recentManual={[manualRun, { ...manualRun, id: 'run_m2' }]} />);
    expect(screen.getAllByRole('listitem')).toHaveLength(2);
  });

  it('folds clean runs behind a pill and shows the failure', () => {
    // The cycle has failures, so it auto-expands. Its two clean runs must fold
    // away; the failed one must stay visible.
    wrap(<ActivityFeed cycles={[cycle]} recentManual={[]} />);
    expect(screen.getByText('r_bad')).toBeInTheDocument();
    expect(screen.queryByText('r_ok_1')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /\+2 clean/ })).toBeInTheDocument();
  });

  it('restores the clean runs when the pill is clicked — hidden, never lost', () => {
    wrap(<ActivityFeed cycles={[cycle]} recentManual={[]} />);
    fireEvent.click(screen.getByRole('button', { name: /\+2 clean/ }));
    expect(screen.getByText('r_ok_1')).toBeInTheDocument();
  });
});
