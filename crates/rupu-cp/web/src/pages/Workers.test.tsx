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
import { commitQuery } from '../components/query/queryTestUtils';

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

describe('Workers query line', () => {
  const OLD = new Date(Date.now() - 3_600_000).toISOString();
  const ROWS: WorkerView[] = [
    WORKER,
    { ...WORKER, worker_id: 'wkr_2', name: 'autoflow-daemon', kind: 'autoflow_serve', host: 'sutajio', last_seen_at: OLD, active_run_count: 1 },
    { ...WORKER, worker_id: 'wkr_3', name: 'kali-b', host: 'kali-b.local', capabilities: { backends: ['anthropic'] }, total_run_count: 0 },
  ];
  const listed = () => ['matt-mbp', 'autoflow-daemon', 'kali-b'].filter((n) => screen.queryByText(n) !== null);
  const mount = (entry: string) =>
    render(
      <MemoryRouter initialEntries={[entry]}>
        <Workers />
      </MemoryRouter>,
    );

  it('narrows the table to the query in ?q=', async () => {
    vi.spyOn(api, 'getWorkers').mockResolvedValue(ROWS);
    mount('/workers?q=kind:cli -has:runs');
    await waitFor(() => expect(listed()).toEqual(['kali-b']));
    expect(screen.getByTestId('query-count')).toHaveTextContent('1 of 3');
  });

  it('narrows the table as a query is committed', async () => {
    vi.spyOn(api, 'getWorkers').mockResolvedValue(ROWS);
    mount('/workers');
    await waitFor(() => expect(listed()).toEqual(['matt-mbp', 'autoflow-daemon', 'kali-b']));
    commitQuery('Filter workers', 'status:stale');
    await waitFor(() => expect(listed()).toEqual(['autoflow-daemon']));
  });

  it('says so when the query keeps nothing', async () => {
    vi.spyOn(api, 'getWorkers').mockResolvedValue(ROWS);
    mount('/workers?q=backend:openai');
    expect(await screen.findByText('No workers match the filter.')).toBeInTheDocument();
  });
});
