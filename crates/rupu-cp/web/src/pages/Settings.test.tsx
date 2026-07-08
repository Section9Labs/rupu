// @vitest-environment jsdom
// Settings page — General tab renders resolved values + provenance badges +
// lock toggles from a mocked getConfig(); token status is masked (never a
// value); Save submits a patch to putGlobalConfig; API errors (400 validation,
// 501 read-only) surface inline.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, ApiError, type ConfigView } from '../lib/api';

import Settings from './Settings';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const MOCK_CONFIG: ConfigView = {
  effective: {
    default_provider: null,
    default_model: 'claude-sonnet-4-6',
    permission_mode: 'ask',
    log_level: null,
    bash: { timeout_secs: null, env_allowlist: null },
    retry: { max_attempts: null, initial_delay_ms: null },
    providers: {},
    scm: { default: null },
    issues: { default: null },
    ui: {
      color: null,
      theme: null,
      syntax: { theme: null },
      palette: { theme: null },
      live_view: null,
      pager: null,
      editor: null,
    },
    triggers: {},
    autoflow: {
      enabled: null,
      repo: null,
      checkout: null,
      worktree_root: null,
      permission_mode: null,
      strict_templates: null,
      max_active: null,
      cleanup_after: null,
    },
    pricing: { agents: {} },
    storage: {},
    policy: { lock: ['permission_mode'] },
    cp: { max_workspace_bytes: null },
  },
  provenance: {
    default_model: { source: 'project', locked: false },
    permission_mode: { source: 'global', locked: true },
  },
  raw_global: 'permission_mode = "ask"\n[policy]\nlock = ["permission_mode"]\n',
  raw_project: null,
  cp: { max_workspace_bytes: null },
  status: { bind: '127.0.0.1:7878', token_set: false, restart_required_keys: ['bind', 'token'] },
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('Settings page', () => {
  it('renders resolved values from getConfig on the General tab', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    const modelInput = (await screen.findByLabelText('Default model')) as HTMLInputElement;
    expect(modelInput.value).toBe('claude-sonnet-4-6');

    const modeSelect = screen.getByLabelText('Permission mode') as HTMLSelectElement;
    expect(modeSelect.value).toBe('ask');
  });

  it('shows a provenance badge for a field resolved from the project layer', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    expect(screen.getByText('project')).toBeInTheDocument();
  });

  it('shows a lock glyph on the key enforced by global policy', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Permission mode');
    // permission_mode is in policy.lock -> locked glyph + accessible "Unlock" label.
    expect(screen.getByRole('button', { name: /unlock permission_mode/i })).toBeInTheDocument();
    // default_model is NOT locked -> accessible "Lock" label (unlocked glyph).
    expect(screen.getByRole('button', { name: /^lock default_model$/i })).toBeInTheDocument();
  });

  it('renders the CP bearer token as masked, never as a value', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: 'CP-Runtime' }));

    expect(await screen.findByText('not configured')).toBeInTheDocument();
    expect(screen.queryByText('•••')).not.toBeInTheDocument();
  });

  it('renders "set" when token_set is true, still without any value', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue({
      ...MOCK_CONFIG,
      status: { ...MOCK_CONFIG.status, token_set: true },
    });

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: 'CP-Runtime' }));

    expect(await screen.findByText('••• set')).toBeInTheDocument();
  });

  it('clicking Save calls putGlobalConfig with a patch of the changed field', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);
    const putSpy = vi.spyOn(api, 'putGlobalConfig').mockResolvedValue({ ok: true, restart_required: [] });

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    const modelInput = await screen.findByLabelText('Default model');
    fireEvent.change(modelInput, { target: { value: 'claude-opus-4-6' } });

    fireEvent.click(screen.getByRole('button', { name: /save changes/i }));

    await waitFor(() => expect(putSpy).toHaveBeenCalledTimes(1));
    expect(putSpy.mock.calls[0][0]).toEqual({
      patch: { default_model: 'claude-opus-4-6' },
    });
  });

  it('clearing a field never produces a silent no-op save', async () => {
    // Regression test for the "silent no-op" bug: clearing a text/select
    // field called onChange(key, undefined); pendingPatch stored the
    // `undefined`, Save looked "dirty", and `JSON.stringify` silently
    // dropped the undefined-valued key — so the PUT sent an empty (or
    // partial) patch, the backend no-op'd it, and the UI reported success
    // with nothing actually changed. A cleared field must never register as
    // an unsaved change, and Save must never call putGlobalConfig with an
    // empty effective patch.
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);
    const putSpy = vi.spyOn(api, 'putGlobalConfig').mockResolvedValue({ ok: true, restart_required: [] });

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    const modelInput = (await screen.findByLabelText('Default model')) as HTMLInputElement;
    expect(modelInput.value).toBe('claude-sonnet-4-6');

    // Clear the field — the change handler receives `undefined`.
    fireEvent.change(modelInput, { target: { value: '' } });

    // Must NOT register as a pending edit.
    expect(screen.queryByText(/unsaved change/i)).not.toBeInTheDocument();
    // The displayed value reverts to the still-resolved value, not blank.
    expect(modelInput.value).toBe('claude-sonnet-4-6');

    const saveButton = screen.getByRole('button', { name: /save changes/i }) as HTMLButtonElement;
    expect(saveButton.disabled).toBe(true);

    fireEvent.click(saveButton);
    await Promise.resolve();

    // No silent success: the backend patch endpoint is never invoked with
    // an empty/no-op patch.
    expect(putSpy).not.toHaveBeenCalled();
  });

  it('surfaces a 400 validation error inline', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);
    vi.spyOn(api, 'putGlobalConfig').mockRejectedValue(
      new ApiError(400, 'unknown key `bogus_key`', '{"error":"unknown key `bogus_key`"}'),
    );

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    const modelInput = await screen.findByLabelText('Default model');
    fireEvent.change(modelInput, { target: { value: 'claude-opus-4-6' } });
    fireEvent.click(screen.getByRole('button', { name: /save changes/i }));

    expect(await screen.findByText(/unknown key `bogus_key`/i)).toBeInTheDocument();
  });

  it('shows a read-only message on a 501 from putGlobalConfig', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);
    vi.spyOn(api, 'putGlobalConfig').mockRejectedValue(
      new ApiError(501, 'editing config requires `rupu cp serve`'),
    );

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    const modelInput = await screen.findByLabelText('Default model');
    fireEvent.change(modelInput, { target: { value: 'claude-opus-4-6' } });
    fireEvent.click(screen.getByRole('button', { name: /save changes/i }));

    expect(await screen.findByText(/read-only deploy/i)).toBeInTheDocument();
    expect(screen.getByText(/rupu cp serve/)).toBeInTheDocument();
  });

  it('clicking the lock toggle calls putPolicy with the updated lock list', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);
    const policySpy = vi.spyOn(api, 'putPolicy').mockResolvedValue({ ok: true });

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: /^lock default_model$/i }));

    await waitFor(() => expect(policySpy).toHaveBeenCalledTimes(1));
    expect(policySpy.mock.calls[0][0]).toEqual(
      expect.arrayContaining(['permission_mode', 'default_model']),
    );
  });

  // ── Raw TOML tab (T5) ────────────────────────────────────────────────────

  it('Raw tab shows raw_global highlighted and in an editable textarea', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: 'Raw' }));

    // Highlighted, read-only preview of the current file.
    const highlighted = document.querySelector('code.hljs');
    expect(highlighted).not.toBeNull();
    expect(highlighted!.textContent).toContain(MOCK_CONFIG.raw_global);

    // Separate editable textarea, seeded with the same text.
    const editor = screen.getByLabelText(/edit raw toml/i) as HTMLTextAreaElement;
    expect(editor.value).toBe(MOCK_CONFIG.raw_global);
  });

  it('editing the Raw tab and clicking Save posts { raw } to putGlobalConfig', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);
    const putSpy = vi.spyOn(api, 'putGlobalConfig').mockResolvedValue({ ok: true, restart_required: [] });

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: 'Raw' }));

    const editor = screen.getByLabelText(/edit raw toml/i) as HTMLTextAreaElement;
    const nextRaw = 'default_model = "claude-opus-4-6"\n';
    fireEvent.change(editor, { target: { value: nextRaw } });

    fireEvent.click(screen.getByRole('button', { name: /^save$/i }));

    await waitFor(() => expect(putSpy).toHaveBeenCalledTimes(1));
    expect(putSpy.mock.calls[0][0]).toEqual({ raw: nextRaw });
  });

  it('surfaces a 400 validation error inline in the raw editor', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);
    vi.spyOn(api, 'putGlobalConfig').mockRejectedValue(
      new ApiError(400, 'invalid TOML: expected an equals, found a newline at line 1 column 5'),
    );

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: 'Raw' }));

    const editor = screen.getByLabelText(/edit raw toml/i) as HTMLTextAreaElement;
    fireEvent.change(editor, { target: { value: 'not valid toml =' } });
    fireEvent.click(screen.getByRole('button', { name: /^save$/i }));

    expect(await screen.findByText(/invalid toml/i)).toBeInTheDocument();
  });

  // ── Policy tab (T5) ──────────────────────────────────────────────────────

  it('Policy tab reflects provenance[key].locked and Save calls putPolicy with the updated list', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);
    const policySpy = vi.spyOn(api, 'putPolicy').mockResolvedValue({ ok: true });

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: 'Policy' }));

    const permCheckbox = screen.getByRole('checkbox', { name: /^permission_mode$/i }) as HTMLInputElement;
    const modelCheckbox = screen.getByRole('checkbox', { name: /^default_model$/i }) as HTMLInputElement;
    expect(permCheckbox.checked).toBe(true);
    expect(modelCheckbox.checked).toBe(false);

    fireEvent.click(modelCheckbox);
    fireEvent.click(screen.getByRole('button', { name: /save policy/i }));

    await waitFor(() => expect(policySpy).toHaveBeenCalledTimes(1));
    expect(policySpy.mock.calls[0][0]).toEqual(
      expect.arrayContaining(['permission_mode', 'default_model']),
    );
    expect((policySpy.mock.calls[0][0] as string[]).length).toBe(2);
  });

  // ── Runtime status tab (T5) ──────────────────────────────────────────────

  it('Runtime status tab shows bind, a masked token, and the restart-required note', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: 'Runtime status' }));

    expect(await screen.findByText('127.0.0.1:7878')).toBeInTheDocument();
    expect(screen.getByText('not set')).toBeInTheDocument();
    expect(screen.queryByText('•••')).not.toBeInTheDocument();
    expect(screen.getByText(/requires restarting/i)).toBeInTheDocument();
    expect(screen.getByText('bind, token')).toBeInTheDocument();
  });

  it('Runtime status tab shows "••• set" (never a value) when token_set is true', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue({
      ...MOCK_CONFIG,
      status: { ...MOCK_CONFIG.status, token_set: true },
    });

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: 'Runtime status' }));

    expect(await screen.findByText('••• set')).toBeInTheDocument();
  });

  it('shows loading state before data arrives', () => {
    vi.spyOn(api, 'getConfig').mockImplementation(() => new Promise(() => {}));

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    expect(screen.getByText(/loading settings/i)).toBeInTheDocument();
  });

  it('shows error state when getConfig fails', async () => {
    vi.spyOn(api, 'getConfig').mockRejectedValue(new Error('network failure'));

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    expect(await screen.findByText(/network failure/i)).toBeInTheDocument();
  });

  // ── Redesign (T4): field grouping + namespaced Policy sections ──────────

  it('General tab groups fields under section headings instead of one flat list', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    expect(screen.getByText('Defaults')).toBeInTheDocument();
    expect(screen.getByText('Runtime behavior')).toBeInTheDocument();
  });

  it('Policy tab groups keys under a namespace section heading', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );

    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: 'Policy' }));

    // permission_mode and default_model are both root-level keys -> "general".
    expect(screen.getByText('general')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: /^permission_mode$/i })).toBeInTheDocument();
  });
});
