// @vitest-environment jsdom
// Workflows list — autoflows are just workflows with an `autoflow:` block,
// so the same list now surfaces which rows are autoflows (an "Autoflow"
// badge + Enabled/Disabled state) and lets the operator flip them right
// there (reuses `api.setAutoflowEnabled`, the same call `AutoflowsDefs.tsx`
// uses) — the operator complaint this addresses: "I should be able to
// delete workflows that are autoflows in the workflow list... we should
// just reference which are workflows and which autoflows... I do not see
// why we should not allow them to be enable/disable/delete."
//
// `autoflow_enabled` is `null`/absent for a plain workflow (no `autoflow:`
// block) and `true`/`false` for one that has the block — the badge/toggle
// render only in the latter case.

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

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Workflows — autoflow badge + toggle', () => {
  it('a plain workflow (autoflow_enabled: null) shows no Autoflow badge and no Enable/Disable toggle', async () => {
    const ROWS: WorkflowSummary[] = [
      {
        name: 'plain-review',
        scope: 'global',
        scope_kind: 'global',
        usage: USAGE,
        run_count: 0,
        last_run: null,
        autoflow_enabled: null,
      },
    ];
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('plain-review')).toBeInTheDocument());

    expect(screen.queryByText('Autoflow')).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Disable plain-review' }),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Enable plain-review' })).not.toBeInTheDocument();
    // Delete/Run stay available regardless.
    expect(screen.getByRole('button', { name: 'Run plain-review' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Delete plain-review' })).toBeInTheDocument();
  });

  it('an enabled autoflow row shows the Autoflow badge and a Disable toggle', async () => {
    const ROWS: WorkflowSummary[] = [
      {
        name: 'nightly-sweep',
        scope: 'global',
        scope_kind: 'global',
        usage: USAGE,
        run_count: 0,
        last_run: null,
        autoflow_enabled: true,
      },
    ];
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    expect(screen.getByText('Autoflow')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Disable nightly-sweep' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Enable nightly-sweep' })).not.toBeInTheDocument();
  });

  it('a disabled autoflow row shows the Autoflow badge and an Enable toggle', async () => {
    const ROWS: WorkflowSummary[] = [
      {
        name: 'stale-cleanup',
        scope: 'global',
        scope_kind: 'global',
        usage: USAGE,
        run_count: 0,
        last_run: null,
        autoflow_enabled: false,
      },
    ];
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('stale-cleanup')).toBeInTheDocument());

    expect(screen.getByText('Autoflow')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Enable stale-cleanup' })).toBeInTheDocument();
  });

  it('clicking Disable calls setAutoflowEnabled(name, false), refreshes, and does not navigate the row', async () => {
    const ROWS: WorkflowSummary[] = [
      {
        name: 'nightly-sweep',
        scope: 'global',
        scope_kind: 'global',
        usage: USAGE,
        run_count: 0,
        last_run: null,
        autoflow_enabled: true,
      },
    ];
    const getWorkflowsSpy = vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    const toggleSpy = vi
      .spyOn(api, 'setAutoflowEnabled')
      .mockResolvedValue({ name: 'nightly-sweep', enabled: false });

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    const notCanceled = fireEvent.click(
      screen.getByRole('button', { name: 'Disable nightly-sweep' }),
    );
    expect(notCanceled).toBe(false);

    await waitFor(() =>
      expect(toggleSpy).toHaveBeenCalledWith('nightly-sweep', false, { scope_kind: 'global' }),
    );
    await waitFor(() => expect(getWorkflowsSpy).toHaveBeenCalledTimes(2));
    expect(screen.getByTestId('loc')).toHaveTextContent('/workflows');
  });

  it('a toggle failure surfaces via the page error mechanism', async () => {
    const ROWS: WorkflowSummary[] = [
      {
        name: 'nightly-sweep',
        scope: 'global',
        scope_kind: 'global',
        usage: USAGE,
        run_count: 0,
        last_run: null,
        autoflow_enabled: true,
      },
    ];
    vi.spyOn(api, 'getWorkflows').mockResolvedValue(ROWS);
    vi.spyOn(api, 'setAutoflowEnabled').mockRejectedValue(new Error('boom'));

    render(
      <MemoryRouter initialEntries={['/workflows']}>
        <Workflows />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Disable nightly-sweep' }));

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('boom'));
  });
});
