// @vitest-environment jsdom
// Agents list — scope column rendering.
//
// UsageBarChart → mocked (keeps recharts out of the test); useNavigate →
// mocked to assert navigation.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AgentSummary } from '../lib/api';
import { launchApi } from '../lib/api3b/launch';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

vi.mock('../components/charts/UsageBarChart', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-bar-chart" />,
}));

import Agents from './Agents';
import { commitQuery, firstCells } from '../components/query/queryTestUtils';
import { scopedEntry, withCustomerScope } from '../lib/customerScopeTestUtils';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

const USAGE = {
  input_tokens: 0,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: 0,
  cost_usd: null,
  priced: true,
  runs: 0,
};

const SCOPE_ROWS: AgentSummary[] = [
  { name: 'reviewer', scope: 'global', usage: USAGE, run_count: 2 },
  { name: 'my-project-fixer', scope: 'my-project', usage: USAGE, run_count: 0 },
];

describe('Agents scope column', () => {
  it('renders a scope chip per row, distinguishing global from project-scoped', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(SCOPE_ROWS);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        {withCustomerScope(<Agents />)}
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    expect(screen.getByText('global')).toBeInTheDocument();
    expect(screen.getByText('my-project')).toBeInTheDocument();
  });
});

describe('Agents cost column', () => {
  it('marks an agent whose spend was priced at the global rates', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue([
      { ...SCOPE_ROWS[0], usage: { ...USAGE, cost_usd: 1.2, pricing_error: 'acme layer broken' } },
      SCOPE_ROWS[1],
    ]);
    render(
      <MemoryRouter initialEntries={['/agents']}>
        {withCustomerScope(<Agents />)}
      </MemoryRouter>,
    );
    expect(await screen.findByRole('img', { name: 'Pricing unavailable: acme layer broken', hidden: true })).toBeInTheDocument();
    expect(screen.getAllByRole('img', { name: /^Pricing unavailable/, hidden: true })).toHaveLength(1);
  });
});

describe('Agents under a customer scope', () => {
  it('says the run counts and spend are not filtered by the customer scope', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(SCOPE_ROWS);

    render(
      <MemoryRouter initialEntries={[scopedEntry('acme', '/agents')]}>
        {withCustomerScope(<Agents />)}
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());
    expect(screen.getByRole('note')).toHaveTextContent('Run counts and spend here cover every customer.');
  });
});

describe('Agents query line', () => {
  const ROWS: AgentSummary[] = [
    { name: 'reviewer', scope: 'global', provider: 'anthropic', tools: ['bash'], usage: USAGE, run_count: 2 },
    { name: 'my-project-fixer', scope: 'my-project', provider: 'openai', tools: ['bash', 'grep'], usage: USAGE, run_count: 0 },
    { name: 'broken', scope: 'global', provider: 'anthropic', usage: USAGE, run_count: 0, load_error: 'unknown tool `bsh`' },
  ];

  it('narrows the table to the query in ?q=', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    render(
      <MemoryRouter initialEntries={['/agents?q=provider:anthropic -has:load_error']}>
        {withCustomerScope(<Agents />)}
      </MemoryRouter>,
    );
    await waitFor(() => expect(firstCells()).toEqual(['revieweranthropic']));
    expect(screen.getByTestId('query-count')).toHaveTextContent('1 of 3');
  });

  it('narrows the table as a query is typed and committed', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    render(
      <MemoryRouter initialEntries={['/agents']}>
        {withCustomerScope(<Agents />)}
      </MemoryRouter>,
    );
    await waitFor(() => expect(firstCells()).toHaveLength(3));
    commitQuery('Filter agents', 'tool:grep');
    await waitFor(() => expect(firstCells()).toEqual(['my-project-fixeropenai']));
  });

  it('says so when the query keeps nothing', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    render(
      <MemoryRouter initialEntries={['/agents?q=nothing-like-this']}>
        {withCustomerScope(<Agents />)}
      </MemoryRouter>,
    );
    expect(await screen.findByText('No agents match the filter.')).toBeInTheDocument();
  });
});

describe('Agents header: Validate all and Install stock fleet (Plan 3b)', () => {
  it('opens the validation drawer and the fleet dialog', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(SCOPE_ROWS);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(launchApi, 'validateAllAgents').mockResolvedValue({ ok: true, rows: [] });
    vi.spyOn(launchApi, 'getFleet').mockResolvedValue({ entries: [], global_root: '/h/.rupu', project_root: null });
    render(
      <MemoryRouter initialEntries={['/agents']}>
        {withCustomerScope(<Agents />)}
      </MemoryRouter>,
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Validate all' }));
    expect(await screen.findByRole('dialog', { name: 'Validate all agents' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    fireEvent.click(screen.getByRole('button', { name: 'Install stock fleet' }));
    expect(await screen.findByRole('dialog', { name: 'Install stock fleet' })).toBeInTheDocument();
  });
});
