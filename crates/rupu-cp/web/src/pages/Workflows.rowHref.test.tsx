// @vitest-environment jsdom
// Workflows list — Task 4 (table-standardization plan): whole-row navigation
// (rowHref) to /workflows/:name, replacing the subject-cell's own <Link>.
// The Run action button must not trigger row navigation.
//
// CodeEditor / LauncherSheet / UsageBarChart are mocked to keep CodeMirror,
// the launcher sheet, and recharts out of this test (mirrors
// Workflows.scope.test.tsx).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
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

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

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

describe('Workflows — whole-row navigation (rowHref)', () => {
  it('renders each row as a link to /workflows/:name', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());
    const link = screen.getByText('nightly-sweep').closest('a');
    expect(link).toHaveAttribute('href', '/workflows/nightly-sweep');

    // The subject cell already carried its own inline <Link> before this
    // task — prove the WHOLE row is now link-wrapped (not just that one
    // cell) by checking a plain, never-linked cell (the scope chip).
    const scopeLink = screen.getByText('global').closest('a');
    expect(scopeLink).toHaveAttribute('href', '/workflows/nightly-sweep');
  });

  it('clicking Run does not navigate the row (opens the launcher sheet instead)', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Run nightly-sweep' }));

    expect(screen.getByTestId('loc')).toHaveTextContent('/workflows');
    expect(screen.getByTestId('launcher-sheet')).toBeInTheDocument();
  });
});
