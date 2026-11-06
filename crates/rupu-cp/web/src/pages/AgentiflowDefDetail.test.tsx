// @vitest-environment jsdom
// /agentiflows/defs/:name — the workflow editor's layout for one agentiflow
// definition: a YAML pane with the server's validity badge, Save, Revert,
// Delete (confirmed) and Launch.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, ApiError } from '../lib/api';
import { agentiflowsApi, type AgentiflowDefDetail } from '../lib/api3b/agentiflows';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});
vi.mock('../components/CodeEditor', () => ({
  __esModule: true,
  default: ({ value, onChange, ariaLabel }: { value: string; onChange: (v: string) => void; ariaLabel?: string }) => (
    <textarea aria-label={ariaLabel} value={value} onChange={(e) => onChange(e.target.value)} />
  ),
}));

import AgentiflowDefDetailPage from './AgentiflowDefDetail';

const YAML = 'name: self-review\ndescription: Review the demo repo.\nlead: reviewer-lead\nengagement_profiles: [code]\n';
const DETAIL: AgentiflowDefDetail = {
  name: 'self-review',
  def: { name: 'self-review', description: 'Review the demo repo.', lead: 'reviewer-lead', engagement_profiles: ['code'] },
  yaml: YAML,
  error: null,
  scope: 'demo-repo',
  scope_kind: 'project',
  scope_id: 'ws_demo',
};

let getDef: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  getDef = vi.spyOn(agentiflowsApi, 'getDef').mockResolvedValue(DETAIL);
  vi.spyOn(agentiflowsApi, 'validateDef').mockResolvedValue({
    ok: true,
    name: 'self-review',
    lead: 'reviewer-lead',
    engagement_profiles: ['code'],
  });
  vi.spyOn(agentiflowsApi, 'listDefs').mockResolvedValue([]);
  vi.spyOn(api, 'getProjects').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

function renderPage(url = '/agentiflows/defs/self-review?scope_kind=project&scope_id=ws_demo') {
  return render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route path="/agentiflows/defs/:name" element={<AgentiflowDefDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('AgentiflowDefDetail', () => {
  it('loads the file the URL pins, with its layer, and a server validity badge', async () => {
    renderPage();
    expect(await screen.findByRole('heading', { name: 'self-review' })).toBeInTheDocument();
    expect(getDef).toHaveBeenCalledWith('self-review', { scope_kind: 'project', scope_id: 'ws_demo' });
    expect(screen.getByText('demo-repo')).toBeInTheDocument();
    expect(screen.getByLabelText('Agentiflow definition')).toHaveValue(YAML);
    expect(await screen.findByText('✓ valid')).toBeInTheDocument();
    // Validated against the project's own profiles.
    expect(agentiflowsApi.validateDef).toHaveBeenCalledWith(YAML, 'ws_demo', expect.anything());
  });

  it('without a selector, reads the project-first match', async () => {
    renderPage('/agentiflows/defs/self-review');
    await screen.findByRole('heading', { name: 'self-review' });
    expect(getDef).toHaveBeenCalledWith('self-review', undefined);
  });

  it('Save and Revert are off until the draft changes; an invalid draft cannot be saved', async () => {
    const validate = vi.spyOn(agentiflowsApi, 'validateDef');
    renderPage();
    const editor = await screen.findByLabelText('Agentiflow definition');
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Revert' })).toBeDisabled();

    validate.mockResolvedValue({ ok: false, error: 'pool agent `ghost` not found' });
    fireEvent.change(editor, { target: { value: YAML + 'pool: { agents: [ghost] }\n' } });
    expect(await screen.findByText("pool agent `ghost` not found")).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Revert' })).toBeEnabled();

    fireEvent.click(screen.getByRole('button', { name: 'Revert' }));
    expect(editor).toHaveValue(YAML);
  });

  it('Save PUTs the draft to the same layer', async () => {
    const edited = YAML.replace('Review the demo repo.', 'Review it again.');
    const save = vi.spyOn(agentiflowsApi, 'saveDef').mockResolvedValue({ ...DETAIL, yaml: edited });
    renderPage();
    const editor = await screen.findByLabelText('Agentiflow definition');
    fireEvent.change(editor, { target: { value: edited } });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled());
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() =>
      expect(save).toHaveBeenCalledWith('self-review', edited, { scope_kind: 'project', scope_id: 'ws_demo' }),
    );
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled());
  });

  it("Save's error shows inline", async () => {
    vi.spyOn(agentiflowsApi, 'saveDef').mockRejectedValue(
      new ApiError(400, 'x', JSON.stringify({ error: 'name `other` must equal `self-review`' })),
    );
    renderPage();
    const editor = await screen.findByLabelText('Agentiflow definition');
    fireEvent.change(editor, { target: { value: YAML + '# edit\n' } });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled());
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('name `other` must equal `self-review`');
  });

  it('Delete confirms first, then removes this layer and goes back to the list', async () => {
    const del = vi
      .spyOn(agentiflowsApi, 'deleteDef')
      .mockResolvedValue({ deleted: true, scope: 'demo-repo', scope_kind: 'project' });
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Delete' }));
    const card = await screen.findByRole('alertdialog', { name: /delete self-review\?/i });
    expect(card).toHaveTextContent('demo-repo');
    expect(del).not.toHaveBeenCalled();
    fireEvent.click(within(card).getByRole('button', { name: 'Delete' }));
    await waitFor(() =>
      expect(del).toHaveBeenCalledWith('self-review', { scope_kind: 'project', scope_id: 'ws_demo' }),
    );
    expect(navigateMock).toHaveBeenCalledWith('/agentiflows/defs');
  });

  it('Launch opens the launcher on this definition', async () => {
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Launch' }));
    expect(await screen.findByRole('dialog', { name: /new agentiflow run/i })).toBeInTheDocument();
  });

  it('a file that does not parse still opens, with why', async () => {
    getDef.mockResolvedValue({ ...DETAIL, def: null, error: 'missing field `lead`' });
    renderPage();
    expect(await screen.findByText('missing field `lead`')).toBeInTheDocument();
    expect(screen.getByLabelText('Agentiflow definition')).toHaveValue(YAML);
  });

  it('a missing definition says so', async () => {
    getDef.mockRejectedValue(new ApiError(404, 'x', JSON.stringify({ error: 'agentiflow definition nope not found' })));
    renderPage('/agentiflows/defs/nope');
    expect(await screen.findByText('agentiflow definition nope not found')).toBeInTheDocument();
  });
});
