// @vitest-environment jsdom
// Library → Agentiflows: the definitions (name, layer, lead · profiles) with a
// row Launch, and New agentiflow — a YAML editor the server validates as you
// type.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../lib/api';
import { agentiflowsApi, type AgentiflowDefRow } from '../lib/api3b/agentiflows';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});
// CodeMirror stays out of the test: the editor is a plain textarea.
vi.mock('../components/CodeEditor', () => ({
  __esModule: true,
  default: ({ value, onChange, ariaLabel }: { value: string; onChange: (v: string) => void; ariaLabel?: string }) => (
    <textarea aria-label={ariaLabel} value={value} onChange={(e) => onChange(e.target.value)} />
  ),
}));

import AgentiflowDefs from './AgentiflowDefs';

const ROWS: AgentiflowDefRow[] = [
  {
    name: 'assess-lab',
    scope: 'global',
    scope_kind: 'global',
    scope_id: null,
    description: 'Assess the lab app',
    lead: 'heron',
    engagement_profiles: ['network', 'web'],
    error: null,
  },
  {
    name: 'self-review',
    scope: 'demo-repo',
    scope_kind: 'project',
    scope_id: 'ws_demo',
    description: null,
    lead: 'reviewer-lead',
    engagement_profiles: ['code'],
    error: null,
  },
  {
    name: 'half-written',
    scope: 'global',
    scope_kind: 'global',
    scope_id: null,
    description: null,
    lead: null,
    engagement_profiles: [],
    error: 'missing field `lead` at line 1',
  },
];

beforeEach(() => {
  vi.spyOn(agentiflowsApi, 'listDefs').mockResolvedValue(ROWS);
  vi.spyOn(api, 'getProjects').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

function renderTab() {
  return render(
    <MemoryRouter>
      <AgentiflowDefs />
    </MemoryRouter>,
  );
}

describe('AgentiflowDefs', () => {
  it('lists each definition with its layer, lead and profiles', async () => {
    renderTab();
    const row = (await screen.findByText('assess-lab')).closest('tr') as HTMLElement;
    expect(within(row).getByText('global')).toBeInTheDocument();
    expect(within(row).getByText('heron · network, web')).toBeInTheDocument();
    const project = screen.getByText('self-review').closest('tr') as HTMLElement;
    expect(within(project).getByText('project')).toBeInTheDocument();
    expect(within(project).getByText('demo-repo')).toBeInTheDocument();
    // A file that does not parse is still listed, with why.
    expect(screen.getByText('missing field `lead` at line 1')).toBeInTheDocument();
  });

  it('rows link to the definition page, pinned to their layer', async () => {
    renderTab();
    const link = (await screen.findByText('self-review')).closest('a');
    expect(link).toHaveAttribute('href', '/agentiflows/defs/self-review?scope_kind=project&scope_id=ws_demo');
  });

  it('row Launch opens the launcher on that definition', async () => {
    renderTab();
    fireEvent.click(await screen.findByRole('button', { name: 'Launch self-review' }));
    const dialog = await screen.findByRole('dialog', { name: /new agentiflow run/i });
    await waitFor(() =>
      expect(within(dialog).getByLabelText('Definition')).toHaveValue('project:ws_demo:self-review'),
    );
  });

  it('a broken definition has no Launch', async () => {
    renderTab();
    await screen.findByText('half-written');
    expect(screen.queryByRole('button', { name: 'Launch half-written' })).not.toBeInTheDocument();
  });

  it('New agentiflow validates the draft on the server as you type, then creates it', async () => {
    const validate = vi
      .spyOn(agentiflowsApi, 'validateDef')
      .mockResolvedValueOnce({ ok: false, error: 'unknown engagement profile `wbe`' })
      .mockResolvedValue({ ok: true, name: 'probe', lead: 'heron', engagement_profiles: ['web'] });
    const create = vi.spyOn(agentiflowsApi, 'createDef').mockResolvedValue({
      name: 'probe',
      def: {},
      yaml: '',
      error: null,
      scope: 'global',
      scope_kind: 'global',
      scope_id: null,
    });
    renderTab();
    fireEvent.click(await screen.findByRole('button', { name: 'New agentiflow' }));
    const dialog = await screen.findByRole('dialog', { name: 'New agentiflow' });
    const editor = within(dialog).getByLabelText('New agentiflow definition');

    fireEvent.change(editor, { target: { value: 'name: probe\nlead: heron\nengagement_profiles: [wbe]\n' } });
    expect(await within(dialog).findByText("unknown engagement profile `wbe`")).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Create' })).toBeDisabled();

    fireEvent.change(editor, { target: { value: 'name: probe\nlead: heron\nengagement_profiles: [web]\n' } });
    expect(await within(dialog).findByText('✓ valid')).toBeInTheDocument();
    expect(validate).toHaveBeenLastCalledWith('name: probe\nlead: heron\nengagement_profiles: [web]\n', undefined, expect.anything());

    fireEvent.click(within(dialog).getByRole('button', { name: 'Create' }));
    await waitFor(() => expect(create).toHaveBeenCalledWith('name: probe\nlead: heron\nengagement_profiles: [web]\n'));
    expect(navigateMock).toHaveBeenCalledWith('/agentiflows/defs/probe?scope_kind=global');
  });

  it("New agentiflow shows a 409 from Create inline", async () => {
    vi.spyOn(agentiflowsApi, 'validateDef').mockResolvedValue({ ok: true, name: 'assess-lab', lead: 'heron', engagement_profiles: [] });
    const { ApiError } = await import('../lib/api');
    vi.spyOn(agentiflowsApi, 'createDef').mockRejectedValue(
      new ApiError(409, 'x', JSON.stringify({ error: 'agentiflow definition assess-lab already exists' })),
    );
    renderTab();
    fireEvent.click(await screen.findByRole('button', { name: 'New agentiflow' }));
    const dialog = await screen.findByRole('dialog', { name: 'New agentiflow' });
    await within(dialog).findByText('✓ valid');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Create' }));
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('already exists');
  });

  it('an empty library offers New agentiflow', async () => {
    vi.spyOn(agentiflowsApi, 'listDefs').mockResolvedValue([]);
    renderTab();
    expect(await screen.findByText(/no agentiflow definitions/i)).toBeInTheDocument();
  });
});
