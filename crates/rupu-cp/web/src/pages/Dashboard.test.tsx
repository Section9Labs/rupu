// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type DashboardResponse, type UsageOverview, type UsageTimelineBucket, type FindingsResponse, type RunListRow } from '../lib/api';
import type { UsageSummary } from '../lib/usage';
import Dashboard from './Dashboard';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const USAGE_SUMMARY: UsageSummary = {
  input_tokens: 3000, output_tokens: 1000, cached_tokens: 0, total_tokens: 4000,
  cost_usd: 12.5, priced: true, runs: 4,
};

const DASHBOARD: DashboardResponse = {
  runs: {
    total: 9,
    by_status: {
      running: 1, awaiting_approval: 2, paused: 0, pending: 0,
      completed: 4, failed: 1, rejected: 1, cancelled: 0,
    },
  },
  recent_runs: [
    {
      id: 'run-abc12345', workflow_name: 'nightly-scan', status: 'completed',
      started_at: '2026-06-20T00:00:00Z', finished_at: '2026-06-20T00:05:00Z',
      usage: USAGE_SUMMARY,
    },
  ],
  sessions: { total: 3, active: 1, archived: 2 },
  workers: { total: 2 },
  coverage: { targets: 5, assertions: 40 },
};

const OVERVIEW: UsageOverview = {
  summary: USAGE_SUMMARY,
  breakdown: [
    {
      provider: 'anthropic', model: 'claude-opus', agent: '',
      input_tokens: 3000, output_tokens: 1000, cached_tokens: 0, total_tokens: 4000,
      cost_usd: 12.5, priced: true, runs: 4,
    },
  ],
};

const TIMELINE: UsageTimelineBucket[] = [
  { bucket: '2026-06-19', rows: OVERVIEW.breakdown },
  { bucket: '2026-06-20', rows: OVERVIEW.breakdown },
];

const FINDINGS: FindingsResponse = {
  findings: [],
  summary: { total: 7, critical: 1, high: 2, medium: 2, low: 1, info: 1 },
};

const RUNS: RunListRow[] = [
  { id: 'r1', workflow_name: 'w', status: 'failed', started_at: new Date().toISOString(), trigger: 'manual', turns: 1, usage: USAGE_SUMMARY },
];

function mockApi() {
  vi.spyOn(api, 'getDashboard').mockResolvedValue(DASHBOARD);
  vi.spyOn(api, 'getUsage').mockResolvedValue(OVERVIEW);
  vi.spyOn(api, 'getUsageTimeline').mockResolvedValue(TIMELINE);
  vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
  vi.spyOn(api, 'getRuns').mockResolvedValue(RUNS);
}

describe('Dashboard', () => {
  it('renders the triage ribbon, spend headline, breakdown, and recent runs', async () => {
    mockApi();
    render(<MemoryRouter><Dashboard /></MemoryRouter>);

    // Spend ($12.50) shows in the headline, the breakdown cost cell, and the footer.
    await waitFor(() => expect(screen.getAllByText('$12.50').length).toBeGreaterThanOrEqual(1));

    // Triage ribbon chips ("Running" / "Awaiting approval" also appear in the
    // donut legend, so assert at least one match; "Open findings" is ribbon-only).
    expect(screen.getAllByText('Running').length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText('Awaiting approval').length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('Open findings')).toBeInTheDocument();

    // Breakdown model (also in the timeline legend) + recent run.
    expect(screen.getAllByText('claude-opus').length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('nightly-scan')).toBeInTheDocument();
  });

  it('passes a timeline fetch through the api client', async () => {
    mockApi();
    render(<MemoryRouter><Dashboard /></MemoryRouter>);
    await waitFor(() => expect(api.getUsageTimeline).toHaveBeenCalled());
  });
});
