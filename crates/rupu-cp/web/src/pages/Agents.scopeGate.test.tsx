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
// no error surfaced. Gating Delete to `scope_kind === 'global'` closes that
// gap. Deleting a project-scoped definition is NOT currently supported
// anywhere in the CP (the detail page's Delete is gated the same way) — the
// filesystem or `rupu` CLI is the current workaround.
//
// The gate keys off the STRUCTURED `scope_kind` discriminator, never the
// display `scope` string — `scope` is a project's path basename and can
// legally equal the literal string `"global"` for a project registered at a
// path whose last segment is named `global`, which would defeat a
// `scope === 'global'` gate.
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
    expect(screen.queryByRole('button', { name: 'Delete reviewer' })).not.toBeInTheDocument();
  });

  it('renders a Delete button for a global row', async () => {
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

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    expect(screen.getByRole('button', { name: 'Delete reviewer' })).toBeInTheDocument();
  });

  it('renders no Delete for a project row whose workspace basename is literally "global" (proves the scope_kind gate, not the display string)', async () => {
    const ROWS: AgentSummary[] = [
      {
        name: 'reviewer',
        slug: 'reviewer',
        // Display `scope` collides with the literal global sentinel, but
        // `scope_kind` correctly says this is a PROJECT row — the gate must
        // key off `scope_kind`, or this row would wrongly offer Delete and
        // destroy the real global `reviewer.md` instead.
        scope: 'global',
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

    expect(screen.queryByRole('button', { name: 'Delete reviewer' })).not.toBeInTheDocument();
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
