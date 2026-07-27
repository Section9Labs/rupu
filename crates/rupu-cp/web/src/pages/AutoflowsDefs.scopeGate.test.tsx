// @vitest-environment jsdom
// AutoflowsDefs — the Enable/Disable toggle must only be offered for GLOBAL
// rows; a project-scoped row shows the (read-only) Enabled state chip with
// no toggle at all.
//
// Root cause this guards against: `POST /api/autoflows/:name/enable|disable`
// resolves `:name` via `resolve_workflow_path` (global-first, then EVERY
// registered workspace), which is NOT the same resolution the list uses
// (`distinct_repo_workspaces` — one REPRESENTATIVE worktree per repo). For a
// project-scoped row, the toggle could therefore flip the global copy or a
// non-representative worktree's copy instead of the file the row actually
// shows — `load()` then re-renders unchanged state, reading as "the toggle
// did nothing," and a second click compounds the wrong-file write. Gating
// the toggle to `scope === 'global'` closes that gap.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AutoflowDefRow } from '../lib/api';

import AutoflowsDefs from './AutoflowsDefs';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('AutoflowsDefs — scope-gated toggle', () => {
  it('shows the Enabled state chip but no toggle button for a project-scoped row', async () => {
    const ROW: AutoflowDefRow = {
      name: 'issue-triage',
      slug: 'issue-triage',
      trigger: 'event',
      scope: 'my-project',
      enabled: true,
    };
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([ROW]);

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('issue-triage')).toBeInTheDocument());

    // The Enabled column's chip is a plain-text badge ("Enabled" — matching
    // both the column header cell and the chip itself, so assert there are
    // at least two matches rather than a single unambiguous one).
    expect(screen.getAllByText('Enabled').length).toBeGreaterThanOrEqual(2);
    expect(screen.queryByRole('button', { name: 'Disable issue-triage' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Enable issue-triage' })).not.toBeInTheDocument();
  });

  it('shows the toggle button for a global row', async () => {
    const ROW: AutoflowDefRow = {
      name: 'nightly-sweep',
      slug: 'nightly-sweep',
      trigger: 'cron',
      scope: 'global',
      enabled: true,
    };
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([ROW]);

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    expect(screen.getByRole('button', { name: 'Disable nightly-sweep' })).toBeInTheDocument();
  });
});
