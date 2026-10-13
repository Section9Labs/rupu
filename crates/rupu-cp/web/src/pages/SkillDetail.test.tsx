// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, ApiError, type SkillDetail } from '../lib/api';
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
    files: ['scripts/fill.py', 'scripts/README.md'],
    content_hash: '9f8e7d6c',
    body: '# Filling PDFs\n\nRun `scripts/fill.py`.',
    source_record: null,
    source_error: null,
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

  it('lists the skill’s files as the server reports them', async () => {
    vi.spyOn(api, 'getSkill').mockResolvedValue(detail({}));

    mount('/skills/global/pdf');

    const files = await screen.findByRole('list', { name: 'Files' });
    expect(within(files).getAllByRole('listitem').map((li) => li.textContent)).toEqual([
      'scripts/fill.py',
      'scripts/README.md',
    ]);
  });

  it('shows an unreadable install record next to the source', async () => {
    vi.spyOn(api, 'getSkill').mockResolvedValue(
      detail({ source: null, source_error: 'source.json: expected value at line 1 column 1' }),
    );

    mount('/skills/global/pdf');

    expect(
      await screen.findByText('unreadable (source.json: expected value at line 1 column 1)'),
    ).toBeInTheDocument();
  });

  it('renders every warning, repeated ones included, without a key collision', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.spyOn(api, 'getSkill').mockResolvedValue(detail({ warnings: ['unknown key "x"', 'unknown key "x"'] }));

    mount('/skills/global/pdf');

    await screen.findByRole('heading', { level: 1, name: 'pdf' });
    expect(screen.getAllByText('unknown key "x"')).toHaveLength(2);
    expect(consoleError.mock.calls.map((c) => String(c[0]))).not.toContainEqual(
      expect.stringMatching(/same key/),
    );
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
    const body = '{"error":"no installed skill \\"nope\\""}';
    vi.spyOn(api, 'getSkill').mockRejectedValue(new ApiError(404, body, body));

    mount('/skills/global/nope');

    expect(await screen.findByRole('alert')).toHaveTextContent(/^no installed skill "nope"$/);
  });
});
