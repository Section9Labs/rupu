// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, type SkillDetail } from '../lib/api';
import SkillDetailPage from './SkillDetail';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function detail(over: Partial<SkillDetail>): SkillDetail {
  return {
    name: 'pdf',
    layer: 'global',
    status: 'ok',
    description: 'Read and fill PDF forms',
    error: null,
    warnings: [],
    stamp: 'ab12cd34',
    source: 'github:acme/skills',
    both_layers: false,
    path: '/home/u/.rupu/skills/pdf',
    license: 'MIT',
    compatibility: 'needs python3',
    allowed_tools: ['Bash', 'Read'],
    mapped_tools: ['bash', 'read_file'],
    disable_model_invocation: false,
    metadata: { owner: 'docs-team' },
    files: ['SKILL.md', 'scripts/fill.py'],
    content_hash: '9f8e7d6c',
    body: '# Filling PDFs\n\nRun `scripts/fill.py`.',
    source_record: null,
    ...over,
  };
}

function mount(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/skills/:layer/:name" element={<SkillDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('Skill detail', () => {
  it('renders the body as markdown and the skill’s facts', async () => {
    const getSkill = vi.spyOn(api, 'getSkill').mockResolvedValue(detail({}));

    mount('/skills/global/pdf');

    expect(await screen.findByRole('heading', { name: 'Filling PDFs' })).toBeInTheDocument();
    expect(getSkill).toHaveBeenCalledWith('global', 'pdf', undefined);
    expect(screen.getByRole('heading', { level: 1, name: 'pdf' })).toBeInTheDocument();
    expect(screen.getByText('github:acme/skills')).toBeInTheDocument();
    expect(screen.getByText('needs python3')).toBeInTheDocument();
    expect(screen.getByText('MIT')).toBeInTheDocument();
    expect(screen.getByText('Bash')).toBeInTheDocument();
    expect(screen.getByText('read_file')).toBeInTheDocument();
    expect(screen.getByText(/informational, never a grant/i)).toBeInTheDocument();
    expect(screen.getByText('docs-team')).toBeInTheDocument();
    expect(screen.getByText('9f8e7d6c')).toBeInTheDocument();
  });

  it('lists the skill’s files without SKILL.md', async () => {
    vi.spyOn(api, 'getSkill').mockResolvedValue(detail({}));

    mount('/skills/global/pdf');

    const files = await screen.findByRole('list', { name: 'Files' });
    expect(within(files).getAllByRole('listitem').map((li) => li.textContent)).toEqual(['scripts/fill.py']);
  });

  it('passes ws_id for a project skill', async () => {
    const getSkill = vi.spyOn(api, 'getSkill').mockResolvedValue(detail({ name: 'notes', layer: 'project' }));

    mount('/skills/project/notes?ws_id=w1');

    await screen.findByRole('heading', { level: 1, name: 'notes' });
    expect(getSkill).toHaveBeenCalledWith('project', 'notes', 'w1');
  });

  it('shows an invalid skill’s error in a banner', async () => {
    vi.spyOn(api, 'getSkill').mockResolvedValue(
      detail({ status: 'invalid', error: 'name "brokn" does not match its directory', body: null, description: null }),
    );

    mount('/skills/global/pdf');

    expect(await screen.findByRole('alert')).toHaveTextContent('name "brokn" does not match its directory');
  });

  it('shows a 404 as an error', async () => {
    vi.spyOn(api, 'getSkill').mockRejectedValue(new Error('no installed skill "nope"'));

    mount('/skills/global/nope');

    expect(await screen.findByRole('alert')).toHaveTextContent('no installed skill "nope"');
  });
});
