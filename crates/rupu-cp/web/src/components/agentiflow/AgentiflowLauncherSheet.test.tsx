// @vitest-environment jsdom
// AgentiflowLauncherSheet — New agentiflow: a definition picker, where the
// flow runs (the definition's own layer, a project, or a directory), Launch.
// No host picker: flows are local-only.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, ApiError, type ProjectRow } from '../../lib/api';
import { agentiflowsApi, type AgentiflowDefRow } from '../../lib/api3b/agentiflows';
import AgentiflowLauncherSheet from './AgentiflowLauncherSheet';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

const DEFS: AgentiflowDefRow[] = [
  {
    name: 'assess-lab',
    scope: 'lab-app',
    scope_kind: 'project',
    scope_id: 'ws_lab',
    description: null,
    lead: 'heron',
    engagement_profiles: ['web'],
    error: null,
  },
  {
    name: 'broken',
    scope: 'global',
    scope_kind: 'global',
    scope_id: null,
    description: null,
    lead: null,
    engagement_profiles: [],
    error: 'missing field `lead`',
  },
  {
    name: 'sweep',
    scope: 'global',
    scope_kind: 'global',
    scope_id: null,
    description: null,
    lead: 'kestrel',
    engagement_profiles: [],
    error: null,
  },
];

const PROJECTS = [{ ws_id: 'ws_lab', name: 'lab-app', path: '/srv/lab-app' }] as ProjectRow[];

beforeEach(() => {
  vi.spyOn(agentiflowsApi, 'listDefs').mockResolvedValue(DEFS);
  vi.spyOn(api, 'getProjects').mockResolvedValue(PROJECTS);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

function renderSheet(props: Partial<Parameters<typeof AgentiflowLauncherSheet>[0]> = {}) {
  const onClose = vi.fn();
  render(
    <MemoryRouter>
      <AgentiflowLauncherSheet onClose={onClose} {...props} />
    </MemoryRouter>,
  );
  return { onClose };
}

describe('AgentiflowLauncherSheet', () => {
  it('lists the definitions, a broken one disabled, and has no host picker', async () => {
    renderSheet();
    const picker = await screen.findByLabelText('Definition');
    await waitFor(() => expect(picker.querySelectorAll('option')).toHaveLength(3));
    const broken = screen.getByRole('option', { name: /broken/ });
    expect(broken).toBeDisabled();
    expect(screen.queryByLabelText(/host/i)).not.toBeInTheDocument();
  });

  it("launches in the definition's own layer by default and opens the new flow", async () => {
    const launch = vi.spyOn(agentiflowsApi, 'launch').mockResolvedValue({ id: 'af_9', status: 'started' });
    const { onClose } = renderSheet();
    await screen.findByRole('option', { name: /assess-lab/ });
    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));
    await waitFor(() =>
      expect(launch).toHaveBeenCalledWith('assess-lab', { scope_kind: 'project', scope_id: 'ws_lab' }),
    );
    expect(navigateMock).toHaveBeenCalledWith('/agentiflows/af_9');
    expect(onClose).toHaveBeenCalled();
  });

  it('starts preselected on the definition it was opened for', async () => {
    const launch = vi.spyOn(agentiflowsApi, 'launch').mockResolvedValue({ id: 'af_1', status: 'started' });
    renderSheet({ initial: { name: 'sweep', scope_kind: 'global', scope_id: null } });
    await waitFor(() => expect(screen.getByLabelText('Definition')).toHaveValue('global::sweep'));
    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));
    await waitFor(() => expect(launch).toHaveBeenCalledWith('sweep', { scope_kind: 'global', scope_id: null }));
  });

  it('runs in a registered project or a typed directory', async () => {
    const launch = vi.spyOn(agentiflowsApi, 'launch').mockResolvedValue({ id: 'af_2', status: 'started' });
    renderSheet({ initial: { name: 'sweep', scope_kind: 'global', scope_id: null } });
    const where = await screen.findByLabelText('Runs in');
    await screen.findByRole('option', { name: 'Project: lab-app' });

    fireEvent.change(where, { target: { value: 'project:ws_lab' } });
    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));
    await waitFor(() => expect(launch).toHaveBeenLastCalledWith('sweep', { working_dir: '/srv/lab-app' }));

    fireEvent.change(where, { target: { value: 'dir' } });
    const dir = screen.getByLabelText('Directory');
    expect(screen.getByRole('button', { name: 'Launch' })).toBeDisabled();
    fireEvent.change(dir, { target: { value: 'relative/path' } });
    expect(screen.getByRole('button', { name: 'Launch' })).toBeDisabled();
    fireEvent.change(dir, { target: { value: '/srv/other' } });
    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));
    await waitFor(() => expect(launch).toHaveBeenLastCalledWith('sweep', { working_dir: '/srv/other' }));
  });

  it("shows the server's message inline when the launch is refused", async () => {
    vi.spyOn(agentiflowsApi, 'launch').mockRejectedValue(
      new ApiError(400, 'bad', JSON.stringify({ error: 'lead agent `heron` did not load' })),
    );
    const { onClose } = renderSheet();
    await screen.findByRole('option', { name: /assess-lab/ });
    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('lead agent `heron` did not load');
    expect(onClose).not.toHaveBeenCalled();
    expect(navigateMock).not.toHaveBeenCalled();
  });

  it('a flow still starting after 30 s stays open with its log and a link', async () => {
    vi.spyOn(agentiflowsApi, 'launch').mockResolvedValue({ id: 'af_3', status: 'pending', log: 'resolving lead…' });
    renderSheet();
    await screen.findByRole('option', { name: /assess-lab/ });
    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));
    expect(await screen.findByText(/still starting/i)).toBeInTheDocument();
    expect(screen.getByText('resolving lead…')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /open af_3/i })).toHaveAttribute('href', '/agentiflows/af_3');
    expect(navigateMock).not.toHaveBeenCalled();
  });

  it('says so when there are no definitions', async () => {
    vi.spyOn(agentiflowsApi, 'listDefs').mockResolvedValue([]);
    renderSheet();
    expect(await screen.findByText(/no agentiflow definitions/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Launch' })).toBeDisabled();
  });
});
