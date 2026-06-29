// @vitest-environment jsdom
// NewAgentModal — Describe/Edit toggle + AI generate.
// Opens the modal via "New agent", switches to Describe mode, types a prompt,
// clicks Generate, and asserts the generated raw definition loads into the editor.

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

vi.mock('../components/charts/UsageBarChart', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-bar-chart" />,
}));

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

import Agents from './Agents';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

describe('NewAgentModal describe mode', () => {
  it('generates a draft into the editor', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue([]);
    vi.spyOn(api, 'generateModels').mockResolvedValue([
      { provider: 'anthropic', models: ['claude-sonnet-4-6'], is_default: true },
    ]);
    const gen = vi.spyOn(api, 'generateAgent').mockResolvedValue({
      raw: 'name: drafted',
      provider: 'anthropic',
      model: 'claude-sonnet-4-6',
      attempts: 1,
    });

    render(
      <MemoryRouter>
        <Agents />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'New agent' }));
    fireEvent.click(await screen.findByRole('button', { name: /describe/i }));
    fireEvent.change(screen.getByLabelText(/describe the agent/i), {
      target: { value: 'a code reviewer' },
    });
    fireEvent.click(screen.getByRole('button', { name: /generate/i }));

    await waitFor(() => expect(gen).toHaveBeenCalled());
    expect(await screen.findByDisplayValue(/name: drafted/)).toBeInTheDocument();
  });
});
