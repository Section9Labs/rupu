// @vitest-environment jsdom
// Workflows list — Delete now renders for EVERY row regardless of scope, and
// the confirm dialog names the resolved scope.
//
// History: `DELETE /api/workflows/:name` used to resolve ONLY against the
// global workflows dir (`workflows_dir` in rupu-cp/src/api/workflows.rs),
// while the list merges project-local rows that can shadow a same-named
// global row — clicking Delete on a project-scoped row therefore silently
// removed the hidden GLOBAL file. The fix was to hide Delete on non-global
// rows (`scope_kind === 'global'` gate) rather than fix the endpoint —
// exactly the operator complaint this PR addresses ("no delete of the
// agent/workflow"), since most real workflows are project-scoped. The real
// fix is `resolve_workflow_scoped` (rupu-cp/src/api/workflows.rs) — the
// endpoint now resolves the SAME global-then-registered-projects way the
// detail GET does — so Delete is safe (and offered) for every row.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type WorkflowSummary } from '../lib/api';

vi.mock('../components/charts/UsageBarChart', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-bar-chart" />,
}));

vi.mock('../components/LauncherSheet', () => ({
  __esModule: true,
  default: () => <div data-testid="launcher-sheet" />,
}));

vi.mock('../components/CodeEditor', () => ({
  __esModule: true,
  default: () => <textarea data-testid="code-editor" />,
}));

import Workflows from './Workflows';

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

describe('Workflows — scope-aware Delete', () => {
  it('renders a Delete button for a project-scoped row (Run stays too)', async () => {
    const ROWS: WorkflowSummary[] = [
      {
        name: 'nightly-sweep',
        scope: 'my-project',
        scope_kind: 'project',
        usage: USAGE,
        run_count: 0,
        last_run: null,
      },
    ];
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    expect(screen.getByRole('button', { name: 'Run nightly-sweep' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Delete nightly-sweep' })).toBeInTheDocument();
  });

  it("the confirm dialog names the project scope for a project-scoped row's Delete", async () => {
    const ROWS: WorkflowSummary[] = [
      {
        name: 'nightly-sweep',
        scope: 'my-project',
        scope_kind: 'project',
        usage: USAGE,
        run_count: 0,
        last_run: null,
      },
    ];
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    const deleteSpy = vi
      .spyOn(api, 'deleteWorkflow')
      .mockResolvedValue({ deleted: true, scope: 'my-project', scope_kind: 'project' });
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete nightly-sweep' }));

    expect(confirmSpy).toHaveBeenCalledWith(expect.stringContaining('project: my-project'));
    await waitFor(() =>
      expect(deleteSpy).toHaveBeenCalledWith('nightly-sweep', { scope_kind: 'project' }),
    );
  });

  it('the confirm dialog names "global" for a global row\'s Delete', async () => {
    const ROWS: WorkflowSummary[] = [
      {
        name: 'nightly-sweep',
        scope: 'global',
        scope_kind: 'global',
        usage: USAGE,
        run_count: 0,
        last_run: null,
      },
    ];
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete nightly-sweep' }));

    expect(confirmSpy).toHaveBeenCalledWith(expect.stringContaining('global'));
  });

  it('renders Delete for a project row whose workspace basename is literally "global" (display `scope` never gates anything)', async () => {
    const ROWS: WorkflowSummary[] = [
      {
        name: 'nightly-sweep',
        // Display `scope` collides with the literal global sentinel, but
        // `scope_kind` correctly says this is a PROJECT row.
        scope: 'global',
        scope_kind: 'project',
        usage: USAGE,
        run_count: 0,
        last_run: null,
      },
    ];
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete nightly-sweep' }));
    expect(confirmSpy).toHaveBeenCalledWith(expect.stringContaining('project: global'));
  });
});
