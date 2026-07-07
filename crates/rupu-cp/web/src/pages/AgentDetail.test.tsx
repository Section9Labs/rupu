// @vitest-environment jsdom
// AgentDetail edit/delete — the operator edits an agent's `.md` definition in
// the browser, saves it (validated server-side), or deletes the agent.
//
// `CodeEditor` is mocked to a plain <textarea> so the test never pulls in the
// real (lazy) CodeMirror chunk; `useNavigate` is mocked to assert navigation.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, ApiError, type AgentDetail } from '../lib/api';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

// Mock CodeEditor to a controlled <textarea> — keeps CodeMirror out of the test.
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

import AgentDetailPage from './AgentDetail';

const RAW = `---\nname: reviewer\ndescription: Reviews code.\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\n\nYou review code.\n`;

const AGENT: AgentDetail = {
  name: 'reviewer',
  description: 'Reviews code.',
  provider: 'anthropic',
  model: 'claude-sonnet-4-6',
  scope: 'global',
  usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: true, runs: 0 },
  run_count: 0,
  system_prompt: 'You review code.',
  raw: RAW,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/agents/reviewer']}>
      <Routes>
        <Route path="/agents/:name" element={<AgentDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('AgentDetail edit/delete', () => {
  it('Edit reveals the editor seeded with the raw definition', async () => {
    vi.spyOn(api, 'getAgent').mockResolvedValue(AGENT);
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Edit definition' }));

    const editor = (await screen.findByTestId('code-editor')) as HTMLTextAreaElement;
    expect(editor.value).toBe(RAW);
  });

  it('editing + Save calls saveAgent(name, draft) and exits edit mode', async () => {
    vi.spyOn(api, 'getAgent').mockResolvedValue(AGENT);
    const next = `${RAW}\nMore guidance.\n`;
    const saveSpy = vi.spyOn(api, 'saveAgent').mockResolvedValue({ ...AGENT, raw: next });
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Edit definition' }));
    const editor = (await screen.findByTestId('code-editor')) as HTMLTextAreaElement;

    // Save disabled until the draft diverges from the saved raw.
    const saveBtn = screen.getByRole('button', { name: 'Save' });
    expect(saveBtn).toBeDisabled();

    fireEvent.change(editor, { target: { value: next } });
    expect(saveBtn).not.toBeDisabled();
    fireEvent.click(saveBtn);

    await waitFor(() => expect(saveSpy).toHaveBeenCalledWith('reviewer', next));
    // On success the editor closes (read-only highlight returns).
    await waitFor(() => expect(screen.queryByTestId('code-editor')).not.toBeInTheDocument());
  });

  it('surfaces a rejected Save as an inline alert', async () => {
    vi.spyOn(api, 'getAgent').mockResolvedValue(AGENT);
    vi.spyOn(api, 'saveAgent').mockRejectedValue(
      new ApiError(400, 'invalid frontmatter: missing model'),
    );
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Edit definition' }));
    const editor = (await screen.findByTestId('code-editor')) as HTMLTextAreaElement;
    fireEvent.change(editor, { target: { value: `${RAW}x` } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('invalid frontmatter: missing model');
    // Stays in edit mode so the operator can fix it.
    expect(screen.getByTestId('code-editor')).toBeInTheDocument();
  });

  it('Delete (confirmed) calls deleteAgent and navigates to /agents', async () => {
    vi.spyOn(api, 'getAgent').mockResolvedValue(AGENT);
    const delSpy = vi.spyOn(api, 'deleteAgent').mockResolvedValue();
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Delete reviewer' }));

    await waitFor(() => expect(delSpy).toHaveBeenCalledWith('reviewer'));
    expect(navigateMock).toHaveBeenCalledWith('/agents');
  });

  it('Delete (cancelled at confirm) does nothing', async () => {
    vi.spyOn(api, 'getAgent').mockResolvedValue(AGENT);
    const delSpy = vi.spyOn(api, 'deleteAgent').mockResolvedValue();
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Delete reviewer' }));
    expect(delSpy).not.toHaveBeenCalled();
    expect(navigateMock).not.toHaveBeenCalled();
  });
});
