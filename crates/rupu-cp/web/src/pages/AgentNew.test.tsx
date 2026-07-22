// @vitest-environment jsdom
// AgentNew — the full-page Agent Builder route (/agents/new). Renders the
// card composer with the whole content area, fetches generator models and
// agent names on mount, and navigates to the created agent's detail page.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../lib/api';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

vi.mock('../components/CodeEditor', () => ({
  __esModule: true,
  default: ({
    value,
    onChange,
    ariaLabel,
  }: {
    value: string;
    onChange: (v: string) => void;
    ariaLabel?: string;
  }) => (
    <textarea
      data-testid="code-editor"
      aria-label={ariaLabel}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  ),
}));

import AgentNew from './AgentNew';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

describe('AgentNew (full-page Agent Builder)', () => {
  it('renders the builder, fetches agent names for the Dispatch card, and creates + navigates', async () => {
    const getAgents = vi.spyOn(api, 'getAgents').mockResolvedValue([
      { name: 'code-reviewer' } as never,
    ]);
    vi.spyOn(api, 'generateModels').mockResolvedValue([
      { provider: 'anthropic', models: ['claude-sonnet-4-6'], is_default: true },
    ]);
    const created = vi.spyOn(api, 'createAgent').mockResolvedValue({
      name: 'my-cool-agent',
    } as never);

    render(
      <MemoryRouter>
        <AgentNew />
      </MemoryRouter>,
    );

    const nameInput = await screen.findByLabelText(/agent name/i);
    await waitFor(() => expect(getAgents).toHaveBeenCalled());

    fireEvent.change(nameInput, { target: { value: 'my-cool-agent' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create agent' }));

    await waitFor(() => expect(created).toHaveBeenCalled());
    expect(created.mock.calls[0][0] as string).toContain('name: my-cool-agent');
    await waitFor(() =>
      expect(navigateMock).toHaveBeenCalledWith('/agents/my-cool-agent'),
    );
  });

  it('Cancel returns to the agents list', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue([]);
    vi.spyOn(api, 'generateModels').mockResolvedValue([]);

    render(
      <MemoryRouter>
        <AgentNew />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }));
    expect(navigateMock).toHaveBeenCalledWith('/agents');
  });

  it('surfaces a create failure inline without navigating', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue([]);
    vi.spyOn(api, 'generateModels').mockResolvedValue([]);
    vi.spyOn(api, 'createAgent').mockRejectedValue(new Error('duplicate agent name'));

    render(
      <MemoryRouter>
        <AgentNew />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'Create agent' }));
    expect(await screen.findByText('duplicate agent name')).toBeInTheDocument();
    expect(navigateMock).not.toHaveBeenCalled();
  });
});
