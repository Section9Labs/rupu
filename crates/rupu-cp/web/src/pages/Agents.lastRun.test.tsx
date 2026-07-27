// @vitest-environment jsdom
// Agents list — Last run column (table-standardization Task 5). Mirrors
// Workflows' existing `last_run` rendering exactly: right-aligned, fit,
// `relativeTime(...)`, em-dash when null. Also asserts the definition-table
// canonical column order: Name, Scope, Description, Runs, Tokens, Cost,
// Last run (`docs/superpowers/plans/2026-07-24-rupu-cp-table-standardization.md`).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AgentSummary } from '../lib/api';

vi.mock('../components/charts/UsageBarChart', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-bar-chart" />,
}));

import Agents from './Agents';

const USAGE = {
  input_tokens: 0,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: 0,
  cost_usd: null,
  priced: true,
  runs: 0,
};

const ROWS: AgentSummary[] = [
  {
    name: 'reviewer',
    scope: 'global',
    usage: USAGE,
    run_count: 2,
    last_run: '2026-07-20T10:00:00Z',
  },
  {
    name: 'never-run-agent',
    scope: 'global',
    usage: USAGE,
    run_count: 0,
    last_run: null,
  },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Agents — Last run column', () => {
  it('renders a relativeTime cell for an agent with a last_run', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    // Header exists.
    expect(screen.getByText('Last run')).toBeInTheDocument();
    // reviewer's last_run is 2026-07-20, well in the past relative to "now" —
    // relativeTime never returns the em-dash for a valid non-null ISO string.
    const reviewerRow = screen.getByText('reviewer').closest('tr');
    expect(reviewerRow).not.toBeNull();
    expect(reviewerRow!.textContent).not.toMatch(/—\s*$/);
  });

  it('renders an em-dash in the Last run cell when last_run is null', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('never-run-agent')).toBeInTheDocument());

    const row = screen.getByText('never-run-agent').closest('tr');
    expect(row).not.toBeNull();
    const cells = Array.from(row!.querySelectorAll('td')).map((td) => td.textContent?.trim());
    // Runs cell is also '—' (run_count: 0), so assert the LAST cell (Last run,
    // there's no action column on Agents) is the em-dash.
    expect(cells[cells.length - 1]).toBe('—');
  });
});

describe('Agents — definition-table canonical column order', () => {
  it('orders columns Name, Scope, Description, Runs, Tokens, Cost, Last run', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);

    const { container } = render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    expect(headers).toEqual(['Name', 'Scope', 'Description', 'Runs', 'Tokens', 'Cost', 'Last run']);
  });
});
