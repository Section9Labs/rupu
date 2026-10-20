// @vitest-environment jsdom
// Plan 3b UI-3 dialogs: Validate all (Library → Agents), Install stock fleet
// (Library → Agents) and Initialize project (Projects).

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { api, ApiError, type ProjectRow } from '../../lib/api';
import { launchApi, type FleetListing } from '../../lib/api3b/launch';
import ValidateAllDrawer from './ValidateAllDrawer';
import FleetInstallDialog from './FleetInstallDialog';
import InitProjectDialog from './InitProjectDialog';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('ValidateAllDrawer', () => {
  it('lists every file, failing ones first, with the error', async () => {
    vi.spyOn(launchApi, 'validateAllAgents').mockResolvedValue({
      ok: false,
      rows: [
        { name: 'recon', layer: 'global', path: '/h/.rupu/agents/recon.md', ok: true },
        {
          name: 'scout',
          layer: 'project',
          scope_id: 'w1',
          path: '/p/.rupu/agents/scout.md',
          ok: false,
          error: 'tools: unknown tool `bsh`',
        },
      ],
    });
    render(<ValidateAllDrawer onClose={() => {}} />);
    const rows = await screen.findAllByRole('row');
    // header + 2
    expect(rows).toHaveLength(3);
    expect(within(rows[1]).getByText('scout')).toBeInTheDocument();
    expect(within(rows[1]).getByText('tools: unknown tool `bsh`')).toBeInTheDocument();
    expect(within(rows[2]).getByText('recon')).toBeInTheDocument();
    expect(screen.getByText('1 of 2 agent files fail')).toBeInTheDocument();
  });

  it('says all pass', async () => {
    vi.spyOn(launchApi, 'validateAllAgents').mockResolvedValue({
      ok: true,
      rows: [{ name: 'recon', layer: 'global', path: '/a.md', ok: true }],
    });
    render(<ValidateAllDrawer onClose={() => {}} />);
    expect(await screen.findByText('All 1 agent files pass')).toBeInTheDocument();
  });

  it('checks again', async () => {
    const v = vi.spyOn(launchApi, 'validateAllAgents').mockResolvedValue({ ok: true, rows: [] });
    render(<ValidateAllDrawer onClose={() => {}} />);
    await waitFor(() => expect(v).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
    await waitFor(() => expect(v).toHaveBeenCalledTimes(2));
  });

  it('shows a failed check inline', async () => {
    vi.spyOn(launchApi, 'validateAllAgents').mockRejectedValue(new ApiError(500, 'x', '{"error":"agents dir unreadable"}'));
    render(<ValidateAllDrawer onClose={() => {}} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('agents dir unreadable');
  });
});

const PROJECTS: ProjectRow[] = [{ ws_id: 'w1', name: 'harbor', path: '/src/harbor' } as ProjectRow];

function listing(over: Partial<FleetListing['entries'][number]>[]): FleetListing {
  const base = [
    { kind: 'agent' as const, name: 'recon', path: 'agents/recon.md', global: 'current' as const },
    { kind: 'agent' as const, name: 'service-analyst', path: 'agents/service-analyst.md', global: 'missing' as const },
    { kind: 'workflow' as const, name: 'network-assessment', path: 'workflows/network-assessment.yaml', global: 'modified' as const },
  ];
  return {
    entries: base.map((e, i) => ({ ...e, ...(over[i] ?? {}) })),
    global_root: '/h/.rupu',
    project_root: null,
  };
}

describe('FleetInstallDialog', () => {
  beforeEach(() => {
    vi.spyOn(api, 'getProjects').mockResolvedValue(PROJECTS);
  });

  it('lists each file with its status and offers to install the missing ones', async () => {
    vi.spyOn(launchApi, 'getFleet').mockResolvedValue(listing([]));
    render(<FleetInstallDialog onClose={() => {}} />);
    expect(await screen.findByText('agents/service-analyst.md')).toBeInTheDocument();
    expect(screen.getByText('installed')).toBeInTheDocument();
    expect(screen.getByText('missing')).toBeInTheDocument();
    expect(screen.getByText('edited')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Install 1 file' })).toBeEnabled();
  });

  it('installs, shows the pending state, then the refreshed statuses', async () => {
    const get = vi
      .spyOn(launchApi, 'getFleet')
      .mockResolvedValueOnce(listing([]))
      .mockResolvedValueOnce(listing([{}, { global: 'current' }]));
    let resolve: (v: Awaited<ReturnType<typeof launchApi.installFleet>>) => void = () => {};
    const install = vi.spyOn(launchApi, 'installFleet').mockImplementation(
      () => new Promise((r) => (resolve = r)),
    );
    const onInstalled = vi.fn();
    render(<FleetInstallDialog onClose={() => {}} onInstalled={onInstalled} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Install 1 file' }));
    expect(install).toHaveBeenCalledWith({ force: false });
    expect(screen.getByRole('button', { name: 'Installing…' })).toBeDisabled();
    resolve({ root: '/h/.rupu', created: 1, skipped: 2, overwrote: 0, files: [] });
    expect(
      await screen.findByText((_, el) => el?.tagName === 'P' && el.textContent === 'Created 1 · kept 2 · overwrote 0.'),
    ).toBeInTheDocument();
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
    expect(screen.queryByText('missing')).toBeNull();
    expect(onInstalled).toHaveBeenCalled();
  });

  it('force overwrites edited files after a confirm, styled as danger', async () => {
    vi.spyOn(launchApi, 'getFleet').mockResolvedValue(listing([]));
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const install = vi
      .spyOn(launchApi, 'installFleet')
      .mockResolvedValue({ root: '/h/.rupu', created: 1, skipped: 1, overwrote: 1, files: [] });
    render(<FleetInstallDialog onClose={() => {}} />);
    fireEvent.click(await screen.findByLabelText('Replace edited files'));
    fireEvent.click(screen.getByRole('button', { name: 'Install 2 files' }));
    expect(confirm).toHaveBeenCalled();
    await waitFor(() => expect(install).toHaveBeenCalledWith({ force: true }));
  });

  it('installs into a project’s .rupu/', async () => {
    const get = vi.spyOn(launchApi, 'getFleet').mockResolvedValue(listing([]));
    const install = vi
      .spyOn(launchApi, 'installFleet')
      .mockResolvedValue({ root: '/src/harbor/.rupu', created: 3, skipped: 0, overwrote: 0, files: [] });
    render(<FleetInstallDialog onClose={() => {}} />);
    const to = await screen.findByLabelText('Install to');
    await waitFor(() => expect(within(to).getByRole('option', { name: 'harbor (/src/harbor)' })).toBeInTheDocument());
    get.mockResolvedValue({
      ...listing([]),
      project_root: '/src/harbor/.rupu',
      entries: listing([]).entries.map((e) => ({ ...e, project: 'missing' as const })),
    });
    fireEvent.change(to, { target: { value: 'w1' } });
    await waitFor(() => expect(get).toHaveBeenLastCalledWith('w1'));
    fireEvent.click(await screen.findByRole('button', { name: 'Install 3 files' }));
    await waitFor(() => expect(install).toHaveBeenCalledWith({ force: false, project: 'w1' }));
  });

  it('shows a 409 inline', async () => {
    vi.spyOn(launchApi, 'getFleet').mockResolvedValue(listing([]));
    vi.spyOn(launchApi, 'installFleet').mockRejectedValue(
      new ApiError(409, 'x', '{"error":"project w1 has no .rupu/ directory; initialise it first"}'),
    );
    render(<FleetInstallDialog onClose={() => {}} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Install 1 file' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('initialise it first');
  });
});

describe('InitProjectDialog', () => {
  it('initialises a path with samples and lists what it wrote', async () => {
    const init = vi.spyOn(launchApi, 'initProject').mockResolvedValue({
      root: '/src/new',
      created: 2,
      skipped: 0,
      overwrote: 0,
      files: [
        { path: '.rupu/config.toml', action: 'created' },
        { path: '.gitignore', action: 'updated', detail: 'added .rupu/cache/' },
      ],
    });
    render(<InitProjectDialog onClose={() => {}} />);
    fireEvent.change(screen.getByLabelText('Project directory'), { target: { value: '/src/new' } });
    fireEvent.click(screen.getByLabelText('Add sample agents and workflows'));
    fireEvent.click(screen.getByRole('button', { name: 'Initialize' }));
    await waitFor(() => expect(init).toHaveBeenCalledWith({ path: '/src/new', with_samples: true }));
    expect(await screen.findByText('.rupu/config.toml')).toBeInTheDocument();
    expect(screen.getByText('added .rupu/cache/')).toBeInTheDocument();
  });

  it('refuses a relative path before asking', () => {
    const init = vi.spyOn(launchApi, 'initProject');
    render(<InitProjectDialog onClose={() => {}} />);
    fireEvent.change(screen.getByLabelText('Project directory'), { target: { value: 'src/new' } });
    expect(screen.getByRole('button', { name: 'Initialize' })).toBeDisabled();
    expect(screen.getByText('Enter an absolute path.')).toBeInTheDocument();
    expect(init).not.toHaveBeenCalled();
  });

  it('a 409 shows its message and offers the re-run', async () => {
    const init = vi
      .spyOn(launchApi, 'initProject')
      .mockRejectedValueOnce(
        new ApiError(409, 'x', '{"error":"/src/old is already a rupu project (.rupu/config.toml exists); pass force to re-run"}'),
      )
      .mockResolvedValueOnce({ root: '/src/old', created: 0, skipped: 1, overwrote: 3, files: [] });
    render(<InitProjectDialog onClose={() => {}} />);
    fireEvent.change(screen.getByLabelText('Project directory'), { target: { value: '/src/old' } });
    fireEvent.click(screen.getByRole('button', { name: 'Initialize' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('already a rupu project');
    fireEvent.click(screen.getByLabelText(/Re-run it, overwriting the samples/));
    fireEvent.click(screen.getByRole('button', { name: 'Re-run init' }));
    await waitFor(() => expect(init).toHaveBeenLastCalledWith({ path: '/src/old', with_samples: false, force: true }));
  });
});
