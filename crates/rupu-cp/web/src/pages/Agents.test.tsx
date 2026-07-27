// @vitest-environment jsdom
// Agents list — scope column rendering.
//
// UsageBarChart → mocked (keeps recharts out of the test); useNavigate →
// mocked to assert navigation.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AgentSummary } from '../lib/api';

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
        <Agents />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    expect(screen.getByText('global')).toBeInTheDocument();
    expect(screen.getByText('my-project')).toBeInTheDocument();
  });
});
