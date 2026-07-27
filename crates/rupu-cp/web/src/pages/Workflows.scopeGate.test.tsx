// @vitest-environment jsdom
// Workflows list — Delete must only be offered for GLOBAL rows.
//
// Root cause this guards against: `DELETE /api/workflows/:name` resolves
// ONLY against the global workflows dir (`workflows_dir` in
// rupu-cp/src/api/workflows.rs), but the list merges project-local rows
// that SHADOW a same-named global row. Before this fix, a project-scoped
// row's Delete button silently removed the hidden GLOBAL file and returned
// `{deleted:true}` even though the row itself never disappears from the
// list — the wrong file destroyed, no error surfaced. Gating Delete to
// `scope === 'global'` closes that gap; deletion of project-scoped defs
// remains available on the detail page.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
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

describe('Workflows — scope-gated Delete', () => {
  it('renders no Delete button for a project-scoped row (Run stays)', async () => {
    const ROWS: WorkflowSummary[] = [
      { name: 'nightly-sweep', scope: 'my-project', usage: USAGE, run_count: 0, last_run: null },
    ];
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    expect(screen.getByRole('button', { name: 'Run nightly-sweep' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Delete nightly-sweep' })).not.toBeInTheDocument();
  });

  it('renders a Delete button for a global row', async () => {
    const ROWS: WorkflowSummary[] = [
      { name: 'nightly-sweep', scope: 'global', usage: USAGE, run_count: 0, last_run: null },
    ];
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    expect(screen.getByRole('button', { name: 'Delete nightly-sweep' })).toBeInTheDocument();
  });
});
