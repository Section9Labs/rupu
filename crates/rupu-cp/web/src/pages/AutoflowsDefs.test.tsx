// @vitest-environment jsdom
// AutoflowsDefs — kit adoption (table-standardization Task 5). This page
// never adopted the shared Spinner/ErrorBanner/EmptyState kit: bare
// "Loading…" text, an inline error <div>, and a hand-rolled empty state.
// Also covers the definition-table canonical column order applied to the
// fields AutoflowDefRow actually has (name, trigger, scope — no
// runs/tokens/cost/last_run on this row type, so those columns are not
// fabricated).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AutoflowDefRow } from '../lib/api';

import AutoflowsDefs from './AutoflowsDefs';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const ROWS: AutoflowDefRow[] = [
  { name: 'nightly-sweep', slug: 'nightly-sweep', trigger: 'cron', scope: 'global', enabled: true },
];

describe('AutoflowsDefs — kit adoption', () => {
  it('renders the shared Spinner (role=status) while loading, not bare "Loading…" text', async () => {
    let resolveFn: (v: AutoflowDefRow[]) => void = () => {};
    vi.spyOn(api, 'getAutoflowDefs').mockReturnValue(
      new Promise((r) => {
        resolveFn = r;
      }),
    );

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );

    expect(screen.getByRole('status')).toBeInTheDocument();

    resolveFn([]);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });

  it('renders the shared ErrorBanner (role=alert) on a load failure', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockRejectedValue(new Error('boom'));

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByRole('alert')).toBeInTheDocument());
    expect(screen.getByRole('alert')).toHaveTextContent('boom');
  });

  it('renders the shared EmptyState when there are no autoflow definitions', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([]);

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );

    await waitFor(() =>
      expect(screen.getByText('No autoflow workflows')).toBeInTheDocument(),
    );
    // The kit EmptyState wraps its title/hint in a dashed-border rounded box —
    // distinguishing it from the old hand-rolled AutoflowsEmpty markup would
    // require snapshotting exact classes, so instead assert the copy the kit
    // component actually renders (hint text) is present alongside the title.
    expect(
      screen.getByText(
        'Workflows with autoflow triggers configured (enabled or disabled) will appear here.',
      ),
    ).toBeInTheDocument();
  });

  it('orders columns Name, Scope, Trigger, Enabled — the canonical fields AutoflowDefRow actually has, plus the trailing action column', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue(ROWS);

    const { container } = render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    // The action column's header is '' (it carries the Enable/Disable button,
    // not a header label) — same convention as Workflows'/Agents' trailing
    // action column.
    expect(headers).toEqual(['Name', 'Scope', 'Trigger', 'Enabled', '']);
  });

  it('the Name column is the flexible/truncating subject column (title carries the untruncated value)', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    const subjectCell = screen.getByText('nightly-sweep').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector('[title="nightly-sweep"]')).toBeInTheDocument();
  });

  it('the Scope and Trigger columns are fit (nowrap) columns', async () => {
    vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue(ROWS);

    const { container } = render(
      <MemoryRouter initialEntries={['/build/autoflows']}>
        <AutoflowsDefs />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('nightly-sweep')).toBeInTheDocument());

    const headerCells = Array.from(container.querySelectorAll('thead th'));
    const scopeHeader = headerCells.find((th) => th.textContent?.trim() === 'Scope');
    const triggerHeader = headerCells.find((th) => th.textContent?.trim() === 'Trigger');
    expect(scopeHeader?.className).toMatch(/whitespace-nowrap/);
    expect(triggerHeader?.className).toMatch(/whitespace-nowrap/);
  });
});
