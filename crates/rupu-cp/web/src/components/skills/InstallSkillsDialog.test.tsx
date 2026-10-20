// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { InstallSkillsDialog } from './InstallSkillsDialog';
import type { ProjectRow } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const jsonResponse = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

const PROJECTS = [{ ws_id: 'ws_acme', name: 'acme-web', path: '/src/acme-web' } as ProjectRow];

const FOUND = [
  { name: 'nmap', rel: 'nmap', description: 'Port and service discovery', status: 'new' },
  { name: 'ffuf', rel: 'ffuf', description: 'Web content discovery', status: 'up_to_date' },
  { name: 'nuclei', rel: 'nuclei', description: 'Template-based scanning', status: 'locally_modified', reason: 'edited since it was installed' },
  { name: 'Bad Name', rel: 'bad', description: null, status: 'invalid', reason: '"Bad Name": not a valid name' },
];

type Handler = (url: string, body: Record<string, unknown>) => Response;

function mockServer(handler: Handler) {
  return vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
    const body = init?.body ? (JSON.parse(init.body as string) as Record<string, unknown>) : {};
    return handler(String(input), body);
  });
}

function bodies(fetchMock: ReturnType<typeof vi.spyOn>) {
  return fetchMock.mock.calls.map(([, init]: unknown[]) => JSON.parse((init as RequestInit).body as string) as Record<string, unknown>);
}

function renderDialog(onInstalled = vi.fn()) {
  render(<InstallSkillsDialog projects={PROJECTS} onClose={() => {}} onInstalled={onInstalled} />);
  return onInstalled;
}

async function find(source = 'example/skills') {
  fireEvent.change(screen.getByLabelText('Source'), { target: { value: source } });
  fireEvent.click(screen.getByRole('button', { name: 'Find skills' }));
  return screen.findByRole('list', { name: 'Found in source' });
}

describe('InstallSkillsDialog', () => {
  it('previews the source with a dry run and ticks the new skills', async () => {
    const fetchMock = mockServer(() => jsonResponse({ dry_run: true, layer: 'global', found: FOUND }));
    renderDialog();

    const list = await find();

    expect(bodies(fetchMock)[0]).toEqual({ source: 'example/skills', dry_run: true });
    const nmap = within(list).getByRole('checkbox', { name: 'nmap' });
    expect(nmap).toBeChecked();
    expect(within(list).getByRole('checkbox', { name: 'ffuf' })).toBeDisabled();
    expect(within(list).getByRole('checkbox', { name: 'nuclei' })).toBeDisabled();
    expect(within(list).getByRole('checkbox', { name: 'Bad Name' })).toBeDisabled();
    expect(list).toHaveTextContent('up to date');
    expect(list).toHaveTextContent('edited locally');
    expect(list).toHaveTextContent('not a valid name');
    expect(screen.getByRole('button', { name: 'Install 1 skill' })).toBeEnabled();
  });

  it('replaces an edited copy only with the force check, and sends force', async () => {
    const fetchMock = mockServer((_url, body) =>
      body.dry_run ? jsonResponse({ dry_run: true, layer: 'global', found: FOUND }) : jsonResponse({ installed: [], skipped: [] }),
    );
    renderDialog();
    const list = await find();

    fireEvent.click(screen.getByRole('checkbox', { name: 'Replace locally edited copies' }));
    const nuclei = within(list).getByRole('checkbox', { name: 'nuclei' });
    expect(nuclei).toBeEnabled();
    fireEvent.click(nuclei);
    const install = screen.getByRole('button', { name: 'Install 2 skills' });
    // Replacing a copy is destructive: styled danger, and asked first.
    expect(install.className).toContain('bg-err');
    fireEvent.click(install);

    expect(screen.getByText(/Replace the installed copy of nuclei\? Its local edits are lost\./)).toBeInTheDocument();
    expect(bodies(fetchMock)).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'Replace and install' }));

    await waitFor(() => expect(bodies(fetchMock)).toHaveLength(3));
    expect(bodies(fetchMock)[1]).toEqual({ source: 'example/skills', skills: ['nmap', 'nuclei'], force: true });
  });

  it('backs out of a replace without installing anything', async () => {
    const fetchMock = mockServer(() => jsonResponse({ dry_run: true, layer: 'global', found: FOUND }));
    renderDialog();
    const list = await find();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Replace locally edited copies' }));
    fireEvent.click(within(list).getByRole('checkbox', { name: 'nuclei' }));
    fireEvent.click(screen.getByRole('button', { name: 'Install 2 skills' }));

    fireEvent.click(screen.getByRole('button', { name: 'Back' }));

    expect(screen.queryByText(/Its local edits are lost/)).not.toBeInTheDocument();
    expect(bodies(fetchMock)).toHaveLength(1);
  });

  it('installs new skills with one click when nothing would be replaced, even with force on', async () => {
    const fetchMock = mockServer((_url, body) =>
      body.dry_run ? jsonResponse({ dry_run: true, layer: 'global', found: FOUND }) : jsonResponse({ installed: [], skipped: [] }),
    );
    renderDialog();
    await find();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Replace locally edited copies' }));

    const install = screen.getByRole('button', { name: 'Install 1 skill' });
    expect(install.className).not.toContain('bg-err');
    fireEvent.click(install);

    await waitFor(() => expect(bodies(fetchMock)).toHaveLength(3));
  });

  it('unticks the edited copies when the force check is cleared', async () => {
    mockServer(() => jsonResponse({ dry_run: true, layer: 'global', found: FOUND }));
    renderDialog();
    const list = await find();
    const force = screen.getByRole('checkbox', { name: 'Replace locally edited copies' });
    fireEvent.click(force);
    fireEvent.click(within(list).getByRole('checkbox', { name: 'nuclei' }));

    fireEvent.click(force);

    expect(within(list).getByRole('checkbox', { name: 'nuclei' })).not.toBeChecked();
    expect(screen.getByRole('button', { name: 'Install 1 skill' })).toBeInTheDocument();
  });

  it('installs into a project, then re-reads the source to show what changed', async () => {
    let installed = false;
    const fetchMock = mockServer((_url, body) => {
      if (!body.dry_run) {
        installed = true;
        return jsonResponse({
          installed: [{ name: 'nmap', layer: 'project', path: '/p', outcome: 'created', content_hash: 'ab', tracked: true }],
          skipped: [],
        });
      }
      const nmap = { ...FOUND[0], status: installed ? 'up_to_date' : 'new' };
      return jsonResponse({ dry_run: true, layer: 'project', found: [nmap] });
    });
    const onInstalled = renderDialog();
    fireEvent.change(screen.getByLabelText('Install to'), { target: { value: 'ws_acme' } });
    fireEvent.change(screen.getByLabelText('Ref'), { target: { value: 'v2' } });
    await find();

    fireEvent.click(screen.getByRole('button', { name: 'Install 1 skill' }));

    expect(await screen.findByText('Installed nmap.')).toBeInTheDocument();
    expect(bodies(fetchMock)[1]).toEqual({ source: 'example/skills', ref: 'v2', project: 'ws_acme', skills: ['nmap'], force: false });
    // Confirmed by the data: the next dry run says it is installed.
    const list = await screen.findByRole('list', { name: 'Found in source' });
    await waitFor(() => expect(list).toHaveTextContent('up to date'));
    expect(onInstalled).toHaveBeenCalled();
  });

  it('shows a refusal under the action, and forgets the preview when the source changes', async () => {
    mockServer((_url, body) =>
      body.dry_run
        ? jsonResponse({ dry_run: true, layer: 'global', found: FOUND })
        : jsonResponse({ error: '/skills/nmap already exists; pass --force to replace it' }, 409),
    );
    renderDialog();
    await find();

    fireEvent.click(screen.getByRole('button', { name: 'Install 1 skill' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('already exists');
    fireEvent.change(screen.getByLabelText('Source'), { target: { value: 'example/other' } });
    expect(screen.queryByRole('list', { name: 'Found in source' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Find skills' })).toBeInTheDocument();
  });

  it('shows a failed dry run inline', async () => {
    mockServer(() => jsonResponse({ error: 'no SKILL.md found in /tmp/x' }, 400));
    renderDialog();
    fireEvent.change(screen.getByLabelText('Source'), { target: { value: '/tmp/x' } });
    fireEvent.click(screen.getByRole('button', { name: 'Find skills' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('no SKILL.md found');
  });
});
