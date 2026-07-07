// @vitest-environment jsdom
// Workflows list — Scope column. Global and project-scoped workflow
// definitions must render distinguishable scope chips, mirroring the
// AutoflowsDefs Build page (see ScopeChip in components/ScopeChip.tsx).
//
// CodeEditor / LauncherSheet / UsageBarChart are mocked to keep CodeMirror,
// the launcher sheet, and recharts out of this test.

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

const ROWS: WorkflowSummary[] = [
  { name: 'nightly-sweep', scope: 'global', usage: USAGE, run_count: 3, last_run: null },
  { name: 'my-project-flow', scope: 'my-project', usage: USAGE, run_count: 1, last_run: null },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Workflows scope column', () => {
  it('renders a scope chip per row, distinguishing global from project-scoped', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    expect(screen.getByText('global')).toBeInTheDocument();
    expect(screen.getByText('my-project')).toBeInTheDocument();
  });
});
