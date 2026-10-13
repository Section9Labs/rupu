// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, ApiError, type ProjectRow, type SkillRow } from '../lib/api';
import Skills from './Skills';

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

const PROJECT = { ws_id: 'w1', name: 'acme', path: '/src/acme' } as ProjectRow;

function mount() {
  return render(
    <MemoryRouter initialEntries={['/library?tab=skills']}>
      <Skills />
    </MemoryRouter>,
  );
}

describe('Skills list', () => {
  it('renders a row per installed skill linking to its page', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getSkills').mockResolvedValue([
      row({}),
      row({ name: 'house-style', source: 'github:acme/skills', description: 'Write like us' }),
    ]);

    mount();

    await waitFor(() => expect(screen.getByText('house-style')).toBeInTheDocument());
    expect(screen.getByText('pdf').closest('a')).toHaveAttribute('href', '/skills/global/pdf');
    expect(screen.getByText('github:acme/skills')).toBeInTheDocument();
    expect(screen.getByText('Write like us')).toBeInTheDocument();
  });

  it('marks an invalid skill with a badge carrying its error', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getSkills').mockResolvedValue([
      row({ name: 'broken', status: 'invalid', description: null, error: 'name "brokn" does not match its directory' }),
    ]);

    mount();

    const badge = await screen.findByText('invalid');
    expect(badge).toHaveAttribute('title', 'name "brokn" does not match its directory');
  });

  it('shows how to install skills when none are installed', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getSkills').mockResolvedValue([]);

    mount();

    expect(await screen.findByText('rupu skill install <source>')).toBeInTheDocument();
  });

  it('the project selector lists that project’s layer and links project rows with ws_id', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([PROJECT]);
    const getSkills = vi.spyOn(api, 'getSkills').mockImplementation(async (wsId?: string) =>
      wsId === 'w1' ? [row({}), row({ name: 'notes', layer: 'project' })] : [row({})],
    );

    mount();

    await waitFor(() => expect(getSkills).toHaveBeenCalledWith(undefined));
    const select = await screen.findByLabelText('Project');
    await waitFor(() => expect(within(select).getByText('acme')).toBeInTheDocument());
    fireEvent.change(select, { target: { value: 'w1' } });

    await waitFor(() => expect(getSkills).toHaveBeenLastCalledWith('w1'));
    const notes = await screen.findByText('notes');
    expect(notes.closest('a')).toHaveAttribute('href', '/skills/project/notes?ws_id=w1');
  });

  it('shows a load failure in an error banner', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    const body = '{"error":"workspace w9 not found"}';
    vi.spyOn(api, 'getSkills').mockRejectedValue(new ApiError(404, body, body));

    mount();

    expect(await screen.findByRole('alert')).toHaveTextContent(/^workspace w9 not found$/);
  });
});
