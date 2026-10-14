// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, ApiError, type ProjectRow, type SkillRow } from '../lib/api';
import Skills from './Skills';
import { commitQuery, firstCells } from '../components/query/queryTestUtils';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function row(over: Partial<SkillRow>): SkillRow {
  return {
    name: 'pdf',
    layer: 'global',
    status: 'ok',
    description: 'Read and fill PDF forms',
    error: null,
    warnings: [],
    stamp: 'ab12cd34',
    source: null,
    both_layers: false,
    ...over,
  };
}

function listing(skills: SkillRow[], warnings: string[] = []) {
  return { skills, warnings };
}

const PROJECT = { ws_id: 'w1', name: 'acme', path: '/src/acme' } as ProjectRow;

function mount(entry = '/library?tab=skills') {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <Skills />
    </MemoryRouter>,
  );
}

describe('Skills list', () => {
  it('renders a row per installed skill linking to its page', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getSkillsListing').mockResolvedValue(listing([
      row({}),
      row({ name: 'house-style', source: 'github:acme/skills', description: 'Write like us' }),
    ]));

    mount();

    await waitFor(() => expect(screen.getByText('house-style')).toBeInTheDocument());
    expect(screen.getByText('pdf').closest('a')).toHaveAttribute('href', '/skills/global/pdf');
    expect(screen.getByText('github:acme/skills')).toBeInTheDocument();
    expect(screen.getByText('Write like us')).toBeInTheDocument();
  });

  it('marks an invalid skill with a badge carrying its error', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getSkillsListing').mockResolvedValue(listing([
      row({ name: 'broken', status: 'invalid', description: null, error: 'name "brokn" does not match its directory' }),
    ]));

    mount();

    const badge = await screen.findByText('invalid');
    expect(badge).toHaveAttribute('title', 'name "brokn" does not match its directory');
  });

  it('shows how to install skills when none are installed', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getSkillsListing').mockResolvedValue(listing([]));

    mount();

    expect(await screen.findByText('rupu skill install <source>')).toBeInTheDocument();
  });

  it('a project: term lists that project’s layer and links project rows with ws_id', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([PROJECT]);
    const getSkills = vi.spyOn(api, 'getSkillsListing').mockImplementation(async (wsId?: string) =>
      listing(wsId === 'w1' ? [row({}), row({ name: 'notes', layer: 'project' })] : [row({})]),
    );

    mount();

    await waitFor(() => expect(getSkills).toHaveBeenCalledWith(undefined));
    await screen.findByText('pdf');
    commitQuery('Filter skills', 'project:ACME');

    await waitFor(() => expect(getSkills).toHaveBeenLastCalledWith('w1'));
    const notes = await screen.findByText('notes');
    expect(notes.closest('a')).toHaveAttribute('href', '/skills/project/notes?ws_id=w1');
    expect(firstCells()).toEqual(['notes', 'pdf']);
  });

  it('a project: in the URL fetches only that project’s listing, then filters it', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([PROJECT]);
    const getSkills = vi.spyOn(api, 'getSkillsListing').mockImplementation(async (wsId?: string) =>
      listing(wsId === 'w1' ? [row({}), row({ name: 'notes', layer: 'project' }), row({ name: 'ocr', layer: 'project', status: 'invalid' })] : [row({})]),
    );

    mount('/library?tab=skills&q=project:acme layer:project -status:invalid');

    await waitFor(() => expect(firstCells()).toEqual(['notes']));
    expect(getSkills.mock.calls).toEqual([['w1']]);
    expect(screen.getByTestId('query-count')).toHaveTextContent('1 of 3');
  });

  it('a project: naming no project lists nothing and says so', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([PROJECT]);
    vi.spyOn(api, 'getSkillsListing').mockResolvedValue(listing([row({})]));

    mount('/library?tab=skills&q=project:nope');

    expect(await screen.findByText('no project named `nope`')).toBeInTheDocument();
    expect(screen.getByText('No skills match the filter.')).toBeInTheDocument();
  });

  it('shows a load failure in an error banner', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    const body = '{"error":"workspace w9 not found"}';
    vi.spyOn(api, 'getSkillsListing').mockRejectedValue(new ApiError(404, body, body));

    mount();

    expect(await screen.findByRole('alert')).toHaveTextContent(/^workspace w9 not found$/);
  });

  it('shows each layer the server could not read above the rows that did load', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getSkillsListing').mockResolvedValue(
      listing([row({})], ['project layer: /src/acme/.rupu/skills: permission denied']),
    );

    mount();

    const warning = await screen.findByText('project layer: /src/acme/.rupu/skills: permission denied');
    expect(warning.closest('[role="status"]')).toHaveTextContent(/^Some skills could not be listed/);
    expect(screen.getByText('pdf').closest('a')).toHaveAttribute('href', '/skills/global/pdf');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('shows no layer notice when the server reports no warnings', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getSkillsListing').mockResolvedValue(listing([row({})]));

    mount();

    await screen.findByText('pdf');
    expect(screen.queryByText('Some skills could not be listed')).not.toBeInTheDocument();
  });

  it('shows a row\'s install-record warning on its status badge', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getSkillsListing').mockResolvedValue(
      listing([row({ warnings: ['install record: expected `=` at line 1'] })]),
    );

    mount();

    const badge = await screen.findByText(/ok · 1 warning/);
    expect(badge).toHaveAttribute('title', 'install record: expected `=` at line 1');
  });
});
