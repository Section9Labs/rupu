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
  vi.unstubAllGlobals();
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

  it('classic raw/describe UI renders when the flag is unset (default)', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue([]);
    vi.spyOn(api, 'generateModels').mockResolvedValue([
      { provider: 'anthropic', models: ['claude-sonnet-4-6'], is_default: true },
    ]);
    vi.spyOn(api, 'getConfig').mockResolvedValue({ cp: {} } as never);

    render(
      <MemoryRouter>
        <Agents />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'New agent' }));
    fireEvent.click(await screen.findByRole('button', { name: /edit raw/i }));
    expect(await screen.findByTestId('code-editor')).toBeInTheDocument();
    expect(screen.queryByLabelText(/agent name/i)).not.toBeInTheDocument();
  });
});

// jsdom's localStorage is unreliable under this Node version — install a
// simple in-memory implementation we fully control (mirrors
// ThemeProvider.test.tsx's `installLocalStorage`).
function installLocalStorage() {
  const store = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => store.set(k, String(v)),
    removeItem: (k: string) => store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => Array.from(store.keys())[i] ?? null,
    get length() {
      return store.size;
    },
  });
}

describe('NewAgentModal — Agent Builder (next flag)', () => {
  it('renders the Agent Builder instead of the classic UI and creates via createFrom', async () => {
    installLocalStorage();
    window.localStorage.setItem('rupu.cp.agentUi', 'next');
    vi.spyOn(api, 'getAgents').mockResolvedValue([]);
    vi.spyOn(api, 'generateModels').mockResolvedValue([
      { provider: 'anthropic', models: ['claude-sonnet-4-6'], is_default: true },
    ]);
    vi.spyOn(api, 'getConfig').mockResolvedValue({ cp: { agent_authoring_ui: 'next' } } as never);
    const created = vi.spyOn(api, 'createAgent').mockResolvedValue({
      name: 'my-cool-agent',
    } as never);

    render(
      <MemoryRouter>
        <Agents />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'New agent' }));

    // Agent Builder's name input is present; the classic raw editor is not.
    const nameInput = await screen.findByLabelText(/agent name/i);
    expect(nameInput).toBeInTheDocument();
    expect(screen.queryByTestId('code-editor')).not.toBeInTheDocument();

    fireEvent.change(nameInput, { target: { value: 'my-cool-agent' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create agent' }));

    await waitFor(() => expect(created).toHaveBeenCalled());
    const raw = created.mock.calls[0][0] as string;
    expect(raw).toContain('name: my-cool-agent');
  });
});
