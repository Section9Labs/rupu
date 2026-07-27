// @vitest-environment jsdom
// Workflows list — row Delete action, added alongside the existing Run
// button. Same rowHref-safety contract as Workflows.rowHref.test.tsx: the
// action column is `interactive: true` and Delete must call BOTH
// preventDefault() and stopPropagation() so the row (link-wrapped via
// rowHref) doesn't navigate — proven the same way
// pages/runs/WorkflowRuns.preventDefault.test.tsx does: `fireEvent.click`
// returns `false` iff `preventDefault()` fired somewhere in the dispatch path.

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
  { name: 'nightly-sweep', scope: 'global', scope_kind: 'global', usage: USAGE, run_count: 3, last_run: null },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Workflows — row Delete action', () => {
  it('renders a Delete button per row in a plain (non-link-wrapped) action cell', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    const deleteBtn = screen.getByRole('button', { name: 'Delete nightly-sweep' });
    expect(deleteBtn).toBeInTheDocument();
    expect(deleteBtn.closest('a')).toBeNull();
  });

  it('does not call the API when the confirm dialog is dismissed', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    const deleteSpy = vi.spyOn(api, 'deleteWorkflow').mockResolvedValue();
    vi.spyOn(window, 'confirm').mockReturnValue(false);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete nightly-sweep' }));

    expect(window.confirm).toHaveBeenCalled();
    expect(deleteSpy).not.toHaveBeenCalled();
  });

  it('deletes the workflow definition and refreshes the list on confirm, without navigating the row', async () => {
    const getWorkflowsSpy = vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    const deleteSpy = vi.spyOn(api, 'deleteWorkflow').mockResolvedValue();
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    const notCanceled = fireEvent.click(screen.getByRole('button', { name: 'Delete nightly-sweep' }));
    expect(notCanceled).toBe(false);

    await waitFor(() => expect(deleteSpy).toHaveBeenCalledWith('nightly-sweep'));
    await waitFor(() => expect(getWorkflowsSpy).toHaveBeenCalledTimes(2));
    expect(screen.getByTestId('loc')).toHaveTextContent('/workflows');
  });

  it('surfaces a delete failure via the page error mechanism', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    vi.spyOn(api, 'deleteWorkflow').mockRejectedValue(new Error('boom'));
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete nightly-sweep' }));

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('boom'));
  });

  it('clicking Run still opens the launcher, unaffected by the new Delete button', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Run nightly-sweep' }));
    expect(screen.getByTestId('launcher-sheet')).toBeInTheDocument();
  });
});
