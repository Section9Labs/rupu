// @vitest-environment jsdom
// Build › Autoflows — the query line narrows the definition list.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AutoflowDefRow } from '../lib/api';
import { commitQuery, firstCells } from '../components/query/queryTestUtils';
import AutoflowsDefs from './AutoflowsDefs';
import { withCustomerScope } from '../lib/customerScopeTestUtils';

const ROWS: AutoflowDefRow[] = [
  { name: 'nightly-sweep', slug: 'nightly-sweep', trigger: 'cron', scope: 'global', enabled: true },
  { name: 'issue-triage', slug: 'triage', trigger: 'event', scope: 'shop-web', enabled: false },
  { name: 'weekly-report', slug: 'weekly-report', trigger: 'cron', scope: 'shop-web', enabled: false },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const mount = (entry: string) =>
  render(<MemoryRouter initialEntries={[entry]}>{withCustomerScope(<AutoflowsDefs />)}</MemoryRouter>);

describe('AutoflowsDefs query line', () => {
  it('narrows the table to the query in ?q=', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue(ROWS);
    mount('/build/autoflows?q=trigger:cron is:disabled');
    await waitFor(() => expect(firstCells()).toEqual(['weekly-report']));
    expect(screen.getByTestId('query-count')).toHaveTextContent('1 of 3');
  });

  it('narrows the table as a query is committed, bare words searching name and file', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue(ROWS);
    mount('/build/autoflows');
    await waitFor(() => expect(firstCells()).toHaveLength(3));
    commitQuery('Filter autoflows', 'triage');
    await waitFor(() => expect(firstCells()).toEqual(['issue-triage']));
    commitQuery('Filter autoflows', 'scope:shop-web');
    await waitFor(() => expect(firstCells()).toEqual(['issue-triage']));
  });

  it('says so when the query keeps nothing, and names a bad value', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue(ROWS);
    mount('/build/autoflows?q=is:enabled trigger:event');
    expect(await screen.findByText('No autoflows match the filter.')).toBeInTheDocument();
    cleanup();
    mount('/build/autoflows?q=is:paused');
    expect(await screen.findByRole('alert')).toHaveTextContent('`paused` is not a value of `is:` (one of enabled, disabled)');
  });
});
