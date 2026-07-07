// @vitest-environment jsdom
// Agents list — New-agent flow. Clicking "New agent" opens a modal with a
// CodeEditor seeded from the template; Create posts the raw definition and
// navigates to the new agent's detail page.
//
// CodeEditor → mocked <textarea>; UsageBarChart → mocked (keeps recharts out of
// the test); useNavigate → mocked to assert navigation.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AgentDetail, type AgentSummary } from '../lib/api';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

vi.mock('../components/charts/UsageBarChart', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-bar-chart" />,
}));

vi.mock('../components/CodeEditor', () => ({
  __esModule: true,
  default: ({ value, onChange, ariaLabel }: { value: string; onChange: (v: string) => void; ariaLabel?: string }) => (
    <textarea
      data-testid="code-editor"
      aria-label={ariaLabel}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  ),
}));

import Agents from './Agents';

const CREATED: AgentDetail = {
  name: 'my-agent',
  description: 'A short description.',
  provider: 'anthropic',
  model: 'claude-sonnet-4-6',
  scope: 'global',
  usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: true, runs: 0 },
  run_count: 0,
  system_prompt: 'You are a helpful agent. ...',
  raw: 'raw',
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

describe('Agents new-agent flow', () => {
  it('opens the modal, seeds the template, creates, and navigates', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue([]);
    const createSpy = vi.spyOn(api, 'createAgent').mockResolvedValue(CREATED);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'New agent' }));

    const editor = (await screen.findByTestId('code-editor')) as HTMLTextAreaElement;
    expect(editor.value).toContain('name: my-agent');

    fireEvent.click(screen.getByRole('button', { name: 'Create' }));

    await waitFor(() => expect(createSpy).toHaveBeenCalledTimes(1));
    expect(createSpy.mock.calls[0][0]).toContain('name: my-agent');
    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/agents/my-agent'));
  });
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
