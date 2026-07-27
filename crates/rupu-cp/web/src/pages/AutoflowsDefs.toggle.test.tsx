// @vitest-environment jsdom
// AutoflowsDefs — Enabled state column + Enable/Disable toggle button.
// `scan_autoflow_defs` (rupu-cp) now lists disabled autoflow defs too (not
// just enabled ones), so a toggle is coherent both ways. The table uses
// whole-row navigation (rowHref → /workflows/:slug), so the toggle button's
// column must be `interactive: true` and the click must call BOTH
// preventDefault() and stopPropagation() — proven the same way
// pages/runs/WorkflowRuns.preventDefault.test.tsx does: `fireEvent.click`
// returns `false` iff `preventDefault()` fired somewhere in the dispatch path.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { api, type AutoflowDefRow } from '../lib/api';

import AutoflowsDefs from './AutoflowsDefs';

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

const ENABLED_ROW: AutoflowDefRow = {
  name: 'nightly-sweep',
  slug: 'nightly-sweep',
  trigger: 'cron',
  scope: 'global',
  enabled: true,
};

const DISABLED_ROW: AutoflowDefRow = {
  name: 'stale-cleanup',
  slug: 'stale-cleanup',
  trigger: 'cron',
  scope: 'global',
  enabled: false,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('AutoflowsDefs — Enabled column + toggle', () => {
  it('shows a Disable button for an enabled row and an Enable button for a disabled row', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([ENABLED_ROW, DISABLED_ROW]);

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    const disableBtn = screen.getByRole('button', { name: 'Disable nightly-sweep' });
    const enableBtn = screen.getByRole('button', { name: 'Enable stale-cleanup' });
    expect(disableBtn).toBeInTheDocument();
    expect(enableBtn).toBeInTheDocument();
    // interactive column → plain, unwrapped cell, never nested in the row's <a>.
    expect(disableBtn.closest('a')).toBeNull();
  });

  it('clicking Disable calls setAutoflowEnabled(slug, false) and refreshes, without navigating the row', async () => {
    const getDefsSpy = vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([ENABLED_ROW]);
    const toggleSpy = vi
      .spyOn(api, 'setAutoflowEnabled')
      .mockResolvedValue({ name: 'nightly-sweep', enabled: false });

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    const notCanceled = fireEvent.click(
      screen.getByRole('button', { name: 'Disable nightly-sweep' }),
    );
    expect(notCanceled).toBe(false);

    await waitFor(() => expect(toggleSpy).toHaveBeenCalledWith('nightly-sweep', false));
    await waitFor(() => expect(getDefsSpy).toHaveBeenCalledTimes(2));
    expect(screen.getByTestId('loc')).toHaveTextContent('/build/autoflows');
  });

  it('clicking Enable calls setAutoflowEnabled(slug, true)', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([DISABLED_ROW]);
    const toggleSpy = vi
      .spyOn(api, 'setAutoflowEnabled')
      .mockResolvedValue({ name: 'stale-cleanup', enabled: true });

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('stale-cleanup')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Enable stale-cleanup' }));

    await waitFor(() => expect(toggleSpy).toHaveBeenCalledWith('stale-cleanup', true));
  });

  it('surfaces a toggle failure via the page error mechanism', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([ENABLED_ROW]);
    vi.spyOn(api, 'setAutoflowEnabled').mockRejectedValue(new Error('boom'));

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Disable nightly-sweep' }));

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('boom'));
  });
});
