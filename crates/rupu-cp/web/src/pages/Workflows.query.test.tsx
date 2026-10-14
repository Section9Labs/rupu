// @vitest-environment jsdom
// Workflows list — the query line narrows the table.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type WorkflowSummary } from '../lib/api';
import { commitQuery, firstCells } from '../components/query/queryTestUtils';

vi.mock('../components/charts/UsageBarChart', () => ({ __esModule: true, default: () => <div /> }));
vi.mock('../components/LauncherSheet', () => ({ __esModule: true, default: () => <div /> }));
vi.mock('../components/CodeEditor', () => ({ __esModule: true, default: () => <textarea /> }));

import Workflows from './Workflows';
import { withCustomerScope } from '../lib/customerScopeTestUtils';

const USAGE = { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: true, runs: 0 };
const RECENT = new Date(Date.now() - 3_600_000).toISOString();

const ROWS: WorkflowSummary[] = [
  { name: 'nightly-sweep', scope: 'global', usage: USAGE, run_count: 3, last_run: RECENT, autoflow_enabled: true },
  { name: 'my-project-flow', scope: 'my-project', usage: USAGE, run_count: 1, last_run: '2026-01-01T00:00:00Z' },
  { name: 'triage', scope: 'global', usage: USAGE, run_count: 0, last_run: null, autoflow_enabled: false },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const mount = (entry: string) =>
  render(<MemoryRouter initialEntries={[entry]}>{withCustomerScope(<Workflows />)}</MemoryRouter>);

describe('Workflows query line', () => {
  it('narrows the table to the query in ?q=', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    mount('/workflows?q=scope:global -autoflow:enabled');
    await waitFor(() => expect(firstCells()).toEqual(['triageAutoflowDisabled']));
    expect(screen.getByTestId('query-count')).toHaveTextContent('1 of 3');
  });

  it('narrows the table as a query is committed', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    mount('/workflows');
    await waitFor(() => expect(firstCells()).toHaveLength(3));
    commitQuery('Filter workflows', 'since:1d');
    await waitFor(() => expect(firstCells()).toEqual(['nightly-sweepAutoflowEnabled']));
  });

  it('says so when the query keeps nothing', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    mount('/workflows?q=autoflow:disabled has:runs');
    expect(await screen.findByText('No workflows match the filter.')).toBeInTheDocument();
  });
});
