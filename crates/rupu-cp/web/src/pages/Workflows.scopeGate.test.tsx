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
// `scope_kind === 'global'` closes that gap. Deleting a project-scoped
// definition is NOT currently supported anywhere in the CP (the detail
// page's Delete is gated the same way) — the filesystem or `rupu` CLI is
// the current workaround.
//
// The gate keys off the STRUCTURED `scope_kind` discriminator, never the
// display `scope` string — `scope` is a project's path basename and can
// legally equal the literal string `"global"` for a project registered at a
// path whose last segment is named `global`, which would defeat a
// `scope === 'global'` gate.

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
    expect(screen.queryByRole('button', { name: 'Delete nightly-sweep' })).not.toBeInTheDocument();
  });

  it('renders a Delete button for a global row', async () => {
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

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    expect(screen.getByRole('button', { name: 'Delete nightly-sweep' })).toBeInTheDocument();
  });

  it('renders no Delete for a project row whose workspace basename is literally "global" (proves the scope_kind gate, not the display string)', async () => {
    const ROWS: WorkflowSummary[] = [
      {
        name: 'nightly-sweep',
        // Display `scope` collides with the literal global sentinel, but
        // `scope_kind` correctly says this is a PROJECT row — the gate must
        // key off `scope_kind`, or this row would wrongly offer Delete and
        // destroy the real global `nightly-sweep.yaml` instead.
        scope: 'global',
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

    expect(screen.queryByRole('button', { name: 'Delete nightly-sweep' })).not.toBeInTheDocument();
  });
});
