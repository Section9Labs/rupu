// @vitest-environment jsdom
// Agents list — Delete now renders for EVERY row regardless of scope, and
// must delete by `slug` (file stem), never `name` (frontmatter display
// name).
//
// History: `DELETE /api/agents/:name` used to resolve ONLY against the
// global agents dir (`agents_dir` in rupu-cp/src/api/agents.rs), while the
// list merges project-local rows that can shadow a same-named global row —
// clicking Delete on a project-scoped row therefore silently removed the
// hidden GLOBAL file. The fix was to hide Delete on non-global rows
// (`scope_kind === 'global'` gate) rather than fix the endpoint. The
// operator complaint this PR addresses: that hid Delete for every
// project-scoped agent, which is most of them. The real fix is
// `resolve_agent_scoped` (rupu-cp/src/api/agents.rs) — the endpoint now
// resolves the SAME global-then-registered-projects way the detail GET
// does, keyed by file stem — so Delete is safe (and offered) for every row.
// The confirm dialog names the resolved scope so the operator still knows
// which layer's file is about to be removed.
//
// Separately, `delete_agent` removes `<name>.md` by FILE STEM, but rows are
// keyed by frontmatter `name` — when a `.md` file's stem differs from its
// frontmatter `name` (hand- or CLI-authored files), passing `name` 404s or
// deletes an unrelated file. The row carries `slug` (the file stem) and
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

describe('Agents — scope-aware Delete + slug-based delete', () => {
  it('renders a Delete button for a project-scoped row (alongside Run/Session)', async () => {
    const ROWS: AgentSummary[] = [
      {
        name: 'reviewer',
        slug: 'reviewer',
        scope: 'my-project',
        scope_kind: 'project',
        usage: USAGE,
        run_count: 0,
      },
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
    expect(screen.getByRole('button', { name: 'Delete reviewer' })).toBeInTheDocument();
  });

  it("the confirm dialog names the project scope for a project-scoped row's Delete", async () => {
    const ROWS: AgentSummary[] = [
      {
        name: 'reviewer',
        slug: 'reviewer',
        scope: 'my-project',
        scope_kind: 'project',
        usage: USAGE,
        run_count: 0,
      },
    ];
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    const deleteSpy = vi.spyOn(api, 'deleteAgent').mockResolvedValue();
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete reviewer' }));

    expect(confirmSpy).toHaveBeenCalledWith(expect.stringContaining('project: my-project'));
    await waitFor(() => expect(deleteSpy).toHaveBeenCalledWith('reviewer'));
  });

  it('the confirm dialog names "global" for a global row\'s Delete', async () => {
    const ROWS: AgentSummary[] = [
      {
        name: 'reviewer',
        slug: 'reviewer',
        scope: 'global',
        scope_kind: 'global',
        usage: USAGE,
        run_count: 0,
      },
    ];
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete reviewer' }));

    expect(confirmSpy).toHaveBeenCalledWith(expect.stringContaining('global'));
  });

  it('renders Delete for a project row whose workspace basename is literally "global" (display `scope` never gates anything)', async () => {
    const ROWS: AgentSummary[] = [
      {
        name: 'reviewer',
        slug: 'reviewer',
        // Display `scope` collides with the literal global sentinel, but
        // `scope_kind` correctly says this is a PROJECT row — nothing here
        // should ever gate on the display string.
        scope: 'global',
        scope_kind: 'project',
        usage: USAGE,
        run_count: 0,
      },
    ];
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete reviewer' }));
    // Confirm names the PROJECT layer despite the colliding display string.
    expect(confirmSpy).toHaveBeenCalledWith(expect.stringContaining('project: global'));
  });

  it('deletes by slug, not name, when the two differ', async () => {
    const ROWS: AgentSummary[] = [
      {
        name: 'code-reviewer',
        slug: 'my-file-stem',
        scope: 'global',
        scope_kind: 'global',
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
