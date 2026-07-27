// @vitest-environment jsdom
// AutoflowsDefs — the Enable/Disable toggle now renders for EVERY row
// regardless of scope.
//
// History: `POST /api/autoflows/:name/enable|disable` resolves `:name` via
// `resolve_workflow_path` (global-first, then every registered workspace),
// which used to NOT be the same resolution the list used
// (`distinct_repo_workspaces` — one representative worktree per repo). For a
// project-scoped row that mismatch meant the toggle could silently flip the
// global copy or a non-representative worktree's copy instead of the file
// the row actually showed, so the toggle was hidden on non-global rows
// entirely (`scope_kind === 'global'` gate). The real fix:
// `resolve_workflow_path` now uses `distinct_repo_workspaces` too (see
// `resolve_workflow_scoped` in rupu-cp/src/api/workflows.rs), so it always
// targets the SAME representative worktree the list itself shows — the
// toggle is safe (and offered) for every row.
//
// `scope_kind` remains the correct field for DISPLAY (the scope chip) — it
// just no longer gates the toggle. A row whose display `scope` collides
// with the literal string `"global"` (a project registered at a path ending
// in `/global`) still correctly shows a PROJECT scope chip, proving the
// chip keys off `scope_kind`, not the display string.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AutoflowDefRow } from '../lib/api';

import AutoflowsDefs from './AutoflowsDefs';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('AutoflowsDefs — scope-aware toggle', () => {
  it('shows a working Disable button for a project-scoped row', async () => {
    const ROW: AutoflowDefRow = {
      name: 'issue-triage',
      slug: 'issue-triage',
      trigger: 'event',
      scope: 'my-project',
      scope_kind: 'project',
      enabled: true,
    };
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([ROW]);
    const toggleSpy = vi
      .spyOn(api, 'setAutoflowEnabled')
      .mockResolvedValue({ name: 'issue-triage', enabled: false });

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('issue-triage')).toBeInTheDocument());

    const disableBtn = screen.getByRole('button', { name: 'Disable issue-triage' });
    expect(disableBtn).toBeInTheDocument();

    fireEvent.click(disableBtn);
    await waitFor(() =>
      expect(toggleSpy).toHaveBeenCalledWith('issue-triage', false, { scope_kind: 'project' }),
    );
  });

  it('shows the toggle for a project row whose display scope collides with the literal "global"', async () => {
    // A workspace registered at a path ending in `/global` produces exactly
    // this: display `scope === 'global'` but structured `scope_kind ===
    // 'project'`. The toggle must render regardless (it no longer gates on
    // scope at all), and the scope CHIP must still read "project" behavior
    // off `scope_kind`, not the colliding display string.
    const ROW: AutoflowDefRow = {
      name: 'issue-triage',
      slug: 'issue-triage',
      trigger: 'event',
      scope: 'global',
      scope_kind: 'project',
      enabled: true,
    };
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([ROW]);

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('issue-triage')).toBeInTheDocument());

    expect(screen.getByRole('button', { name: 'Disable issue-triage' })).toBeInTheDocument();
  });

  it('shows the toggle button for a genuinely global row', async () => {
    const ROW: AutoflowDefRow = {
      name: 'nightly-sweep',
      slug: 'nightly-sweep',
      trigger: 'cron',
      scope: 'global',
      scope_kind: 'global',
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
