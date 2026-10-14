// @vitest-environment jsdom
// Projects list — the query line narrows the table.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type ProjectRow } from '../lib/api';
import { commitQuery } from '../components/query/queryTestUtils';

vi.mock('../components/charts/UsageBarChart', () => ({ __esModule: true, default: () => <div /> }));

import Projects from './Projects';
import { withCustomerScope } from '../lib/customerScopeTestUtils';

const USAGE = { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: true, runs: 0 };
const TINT = { hue: 200 } as unknown as NonNullable<ProjectRow['customer']>['tint'];
const P = (over: Partial<ProjectRow> & Pick<ProjectRow, 'ws_id' | 'name'>): ProjectRow => ({
  path: `/code/${over.name}`,
  created_at: '2026-07-01T00:00:00Z',
  usage: USAGE,
  run_count: 0,
  ...over,
});

const ROWS: ProjectRow[] = [
  P({ ws_id: 'w1', name: 'shop-web', repo_remote: 'git@github.com:acme/shop-web.git', branch: 'main', run_count: 4, customer: { slug: 'acme', name: 'Acme', tint: TINT, archived: false } }),
  P({ ws_id: 'w2', name: 'infra', repo_remote: 'https://gitlab.com/acme/infra.git', branch: 'main', customer: null }),
  P({ ws_id: 'w3', name: 'scratch', branch: 'dev', run_count: 1, customer: null }),
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const mount = (entry: string) =>
  render(<MemoryRouter initialEntries={[entry]}>{withCustomerScope(<Projects />)}</MemoryRouter>);
const listed = () => ['shop-web', 'infra', 'scratch'].filter((n) => screen.queryByText(n) !== null);

describe('Projects query line', () => {
  it('narrows the table to the query in ?q=', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue(ROWS);
    mount('/projects?q=branch:main customer:none');
    await waitFor(() => expect(listed()).toEqual(['infra']));
    expect(screen.getByTestId('query-count')).toHaveTextContent('1 of 3');
  });

  it('narrows the table as a query is committed', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue(ROWS);
    mount('/projects');
    await waitFor(() => expect(listed()).toEqual(['shop-web', 'infra', 'scratch']));
    commitQuery('Filter projects', 'has:runs');
    await waitFor(() => expect(listed()).toEqual(['shop-web', 'scratch']));
    commitQuery('Filter projects', '-provider:github');
    await waitFor(() => expect(listed()).toEqual(['scratch']));
  });

  it('says so when the query keeps nothing', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue(ROWS);
    mount('/projects?q=customer:globex');
    expect(await screen.findByText('No projects match the filter.')).toBeInTheDocument();
  });
});
