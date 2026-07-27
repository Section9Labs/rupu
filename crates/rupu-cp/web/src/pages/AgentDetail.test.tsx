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

// jsdom's localStorage is unreliable under this Node version — install a
// simple in-memory implementation we fully control (mirrors
// NewAgentModal.test.tsx's `installLocalStorage`).
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

const RAW = `---\nname: reviewer\ndescription: Reviews code.\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\n\nYou review code.\n`;

const AGENT: AgentDetail = {
  name: 'reviewer',
  description: 'Reviews code.',
  provider: 'anthropic',
  model: 'claude-sonnet-4-6',
  scope: 'global',
  scope_kind: 'global',
  usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: true, runs: 0 },
  run_count: 0,
  system_prompt: 'You review code.',
  raw: RAW,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
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

    // `scopeSelectorFor(AGENT)` — AGENT is `scope_kind: 'global'` — is
    // threaded through so Save targets the exact resolved file, the same
    // way Delete does (see the scope-threading test below for the
    // project-scoped case).
    await waitFor(() =>
      expect(saveSpy).toHaveBeenCalledWith('reviewer', next, { scope_kind: 'global' }),
    );
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
    const delSpy = vi.spyOn(api, 'deleteAgent').mockResolvedValue({ deleted: true, scope: 'global', scope_kind: 'global' });
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Delete reviewer' }));

    await waitFor(() =>
      expect(delSpy).toHaveBeenCalledWith('reviewer', { scope_kind: 'global' }),
    );
    expect(navigateMock).toHaveBeenCalledWith('/agents');
  });

  it('Delete (cancelled at confirm) does nothing', async () => {
    vi.spyOn(api, 'getAgent').mockResolvedValue(AGENT);
    const delSpy = vi.spyOn(api, 'deleteAgent').mockResolvedValue({ deleted: true, scope: 'global', scope_kind: 'global' });
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Delete reviewer' }));
    expect(delSpy).not.toHaveBeenCalled();
    expect(navigateMock).not.toHaveBeenCalled();
  });

  // ── Scope: scope-aware Delete + visible scope indicator + named confirm ──
  // `DELETE /api/agents/:name` resolves project-aware (global-then-
  // registered-projects, by file stem — `resolve_agent_scoped` in
  // `rupu-cp/src/api/agents.rs`), the SAME layer walk this page's
  // `getAgent` load uses — so Delete is offered (and safe) for every scope,
  // and the confirm dialog names the resolved layer before the operator
  // confirms.

  it('a project-scoped agent shows a scope chip and a Delete button whose confirm names the project scope', async () => {
    vi.spyOn(api, 'getAgent').mockResolvedValue({
      ...AGENT,
      scope: 'my-project',
      scope_kind: 'project',
    });
    const delSpy = vi
      .spyOn(api, 'deleteAgent')
      .mockResolvedValue({ deleted: true, scope: 'my-project', scope_kind: 'project' });
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage();

    expect(await screen.findByText('my-project')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Delete reviewer' }));

    expect(confirmSpy).toHaveBeenCalledWith(
      expect.stringContaining('project: my-project'),
    );
    await waitFor(() =>
      expect(delSpy).toHaveBeenCalledWith('reviewer', { scope_kind: 'project' }),
    );
  });

  it('a global agent shows a scope chip and a Delete button whose confirm names "global"', async () => {
    vi.spyOn(api, 'getAgent').mockResolvedValue(AGENT);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);
    renderPage();

    expect(await screen.findByText('global')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Delete reviewer' }));

    expect(confirmSpy).toHaveBeenCalledWith(expect.stringContaining('global'));
  });

  // ── Scope: Save threads the same selector Delete does ────────────────────
  // `PUT /api/agents/:name` used to write unconditionally to the global
  // layer; for a project-only agent this silently created a hidden global
  // shadow while the visible project file stayed unchanged (reading as
  // "Save did nothing"). The fix threads `scopeSelectorFor(agent)` through
  // `saveAgent` — the SAME shape `deleteAgent` already gets — so Save
  // always targets the exact file this page resolved and is showing.

  it('a project-scoped agent threads scope_kind/scope_id on Save', async () => {
    vi.spyOn(api, 'getAgent').mockResolvedValue({
      ...AGENT,
      scope: 'my-project',
      scope_kind: 'project',
      scope_id: 'ws_a',
    });
    const next = `${RAW}\nMore guidance.\n`;
    const saveSpy = vi.spyOn(api, 'saveAgent').mockResolvedValue({
      ...AGENT,
      raw: next,
      scope: 'my-project',
      scope_kind: 'project',
      scope_id: 'ws_a',
    });
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Edit definition' }));
    const editor = (await screen.findByTestId('code-editor')) as HTMLTextAreaElement;
    fireEvent.change(editor, { target: { value: next } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() =>
      expect(saveSpy).toHaveBeenCalledWith('reviewer', next, {
        scope_kind: 'project',
        scope_id: 'ws_a',
      }),
    );
  });

  it('flag unset (default): Edit still shows the classic code editor, not the Agent Builder', async () => {
    vi.spyOn(api, 'getAgent').mockResolvedValue(AGENT);
    vi.spyOn(api, 'getConfig').mockResolvedValue({ cp: {} } as never);
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Edit definition' }));

    expect(await screen.findByTestId('code-editor')).toBeInTheDocument();
    expect(screen.queryByLabelText(/agent name/i)).not.toBeInTheDocument();
  });
});

describe('AgentDetail — Agent Builder edit (next flag)', () => {
  it('renders the Agent Builder seeded from agent.raw and saves via saveFrom', async () => {
    installLocalStorage();
    window.localStorage.setItem('rupu.cp.agentUi', 'next');
    vi.spyOn(api, 'getAgent').mockResolvedValue(AGENT);
    vi.spyOn(api, 'getConfig').mockResolvedValue({ cp: { agent_authoring_ui: 'next' } } as never);
    const next = `${RAW}\nMore guidance.\n`;
    const saveSpy = vi.spyOn(api, 'saveAgent').mockResolvedValue({ ...AGENT, raw: next });

    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Edit definition' }));

    // Agent Builder's name input is present, seeded from agent.raw; classic
    // CodeEditor is not mounted.
    const nameInput = (await screen.findByLabelText(/agent name/i)) as HTMLInputElement;
    expect(nameInput.value).toBe('reviewer');
    expect(screen.queryByTestId('code-editor')).not.toBeInTheDocument();

    fireEvent.change(nameInput, { target: { value: 'reviewer-2' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(saveSpy).toHaveBeenCalled());
    const [savedName, savedRaw] = saveSpy.mock.calls[0] as [string, string];
    expect(savedName).toBe('reviewer');
    expect(typeof savedRaw).toBe('string');
    expect(savedRaw).toContain('name: reviewer-2');
  });
});
