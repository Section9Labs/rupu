// @vitest-environment jsdom
// Workflows list — definition-table canonical column order
// (table-standardization Task 5): Name, Scope, Runs, Tokens, Cost, Last run,
// then the row action. Workflows has no Trigger/Description column.
//
// CodeEditor / LauncherSheet / UsageBarChart are mocked (mirrors
// Workflows.scope.test.tsx / Workflows.rowHref.test.tsx).

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
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Workflows — definition-table canonical column order', () => {
  it('orders columns Name, Scope, Runs, Tokens, Cost, Last run, (action)', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    const { container } = render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    expect(headers).toEqual(['Name', 'Scope', 'Runs', 'Tokens', 'Cost', 'Last run', '']);
  });
});
