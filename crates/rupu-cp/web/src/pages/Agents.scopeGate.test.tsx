// @vitest-environment jsdom
// Agents list — Delete must only be offered for GLOBAL rows, and must
// delete by `slug` (file stem), never `name` (frontmatter display name).
//
// Root cause this guards against: `DELETE /api/agents/:name` resolves ONLY
// against the global agents dir (`agents_dir` in rupu-cp/src/api/agents.rs),
// but the list merges project-local rows that SHADOW a same-named global
// row. Before this fix, a project-scoped row's Delete button silently
// removed the hidden GLOBAL file and returned `{deleted:true}` even though
// the row itself never disappears from the list — the wrong file destroyed,
// no error surfaced. Gating Delete to `scope === 'global'` closes that gap;
// deletion of project-scoped defs remains available on the detail page.
//
// Separately, `delete_agent` removes `<name>.md` by FILE STEM, but rows are
// keyed by frontmatter `name` — when a `.md` file's stem differs from its
// frontmatter `name` (hand- or CLI-authored files), passing `name` 404s or
// deletes an unrelated file. The row now carries `slug` (the file stem) and
// Delete must pass THAT.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AgentSummary } from '../lib/api';

vi.mock('../components/charts/UsageBarChart', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-bar-chart" />,
}));

vi.mock('../components/AgentLauncherSheet', () => ({
  __esModule: true,
  default: () => <div data-testid="agent-launcher-sheet" />,
}));

import Agents from './Agents';

const USAGE = {
  input_tokens: 0,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: 0,
  cost_usd: null,
  priced: true,
  runs: 0,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Agents — scope-gated Delete + slug-based delete', () => {
  it('renders no Delete button for a project-scoped row (Run/Session stay)', async () => {
    const ROWS: AgentSummary[] = [
      { name: 'reviewer', slug: 'reviewer', scope: 'my-project', usage: USAGE, run_count: 0 },
    ];
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    expect(screen.getByRole('button', { name: 'Run reviewer' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Start session with reviewer' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Delete reviewer' })).not.toBeInTheDocument();
  });

  it('renders a Delete button for a global row', async () => {
    const ROWS: AgentSummary[] = [
      { name: 'reviewer', slug: 'reviewer', scope: 'global', usage: USAGE, run_count: 0 },
    ];
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    expect(screen.getByRole('button', { name: 'Delete reviewer' })).toBeInTheDocument();
  });

  it('deletes by slug, not name, when the two differ', async () => {
    const ROWS: AgentSummary[] = [
      {
        name: 'code-reviewer',
        slug: 'my-file-stem',
        scope: 'global',
        usage: USAGE,
        run_count: 0,
      },
    ];
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    const deleteSpy = vi.spyOn(api, 'deleteAgent').mockResolvedValue();
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('code-reviewer')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete code-reviewer' }));

    await waitFor(() => expect(deleteSpy).toHaveBeenCalledWith('my-file-stem'));
    expect(deleteSpy).not.toHaveBeenCalledWith('code-reviewer');
  });
});
