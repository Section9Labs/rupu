// @vitest-environment jsdom
// Workers list — One Control Language migration (Phase 3, Task H). Covers
// the kit loading/empty states and the table-rules subject/fit columns (no
// filters exist on this page — the win here is chrome, not FilterBar).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type WorkerView } from '../lib/api';

import Workers from './Workers';

const WORKER: WorkerView = {
  version: 1,
  worker_id: 'wkr_01JZTESTWORKER001',
  kind: 'cli',
  name: 'matt-mbp',
  host: 'mbp.local',
  capabilities: {},
  registered_at: '2026-07-01T00:00:00Z',
  last_seen_at: new Date().toISOString(),
  active_run_count: 0,
  total_run_count: 3,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/workers']}>
      <Workers />
    </MemoryRouter>,
  );
}

describe('Workers — kit loading/empty states', () => {
  it('shows the kit Spinner while the initial fetch is in flight', () => {
    vi.spyOn(api, 'getWorkers').mockImplementation(() => new Promise(() => {}));
    renderPage();

    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByText('Loading workers…')).toBeInTheDocument();
  });

  it('renders the kit EmptyState with the existing copy when there are no workers', async () => {
    vi.spyOn(api, 'getWorkers').mockResolvedValue([]);
    renderPage();

    await waitFor(() => expect(screen.getByText('No workers registered')).toBeInTheDocument());
    expect(screen.getByText(/workers appear here once you run a workflow/i)).toBeInTheDocument();
  });

  it('renders the kit ErrorBanner on fetch failure', async () => {
    vi.spyOn(api, 'getWorkers').mockRejectedValue(new Error('boom'));
    renderPage();

    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
  });
});

describe('Workers — table rules', () => {
  it('the name column is the one flexible/truncating subject column', async () => {
    vi.spyOn(api, 'getWorkers').mockResolvedValue([WORKER]);
    renderPage();

    await waitFor(() => expect(screen.getByText('matt-mbp')).toBeInTheDocument());

    const subjectCell = screen.getByText('matt-mbp').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector('[title="matt-mbp"]')).toBeInTheDocument();
  });

  it('the Total runs column is a fit (nowrap) column', async () => {
    vi.spyOn(api, 'getWorkers').mockResolvedValue([WORKER]);
    const { container } = renderPage();

    await waitFor(() => expect(screen.getByText('matt-mbp')).toBeInTheDocument());

    const header = Array.from(container.querySelectorAll('thead th')).find((th) =>
      th.textContent?.includes('Total runs'),
    );
    expect(header?.className).toMatch(/whitespace-nowrap/);
  });
});
