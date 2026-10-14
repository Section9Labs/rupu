// @vitest-environment jsdom
// ProjectRunsTab — the query line the server evaluates (`?q=` on
// /api/projects/:ws_id/runs, with no implicit default; facets on the first
// page only; a 400 is the bar's error), fit/subject table-rules columns, kit
// loading/empty/error states.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type RunListRow } from '../../lib/api';
import ProjectRunsTab from './ProjectRunsTab';
import { listCalls, parseError, stubListFetch, type StubAnswer } from '../../lib/query/listFetchStub';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function usage(): RunListRow['usage'] {
  return {
    input_tokens: 100,
    output_tokens: 20,
    cached_tokens: 0,
    total_tokens: 120,
    cost_usd: 0.01,
    priced: true,
    runs: 1,
  };
}

const ROWS: RunListRow[] = [
  {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
    id: 'r-run-manual',
    workflow_name: 'wf-running-manual',
    status: 'running',
    started_at: '2026-06-01T00:00:00Z',
    trigger: 'manual',
    turns: 1,
    usage: usage(),
  },
  {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
    id: 'r-done-cron',
    workflow_name: 'wf-completed-cron',
    status: 'completed',
    started_at: '2026-06-01T00:00:00Z',
    trigger: 'cron',
    turns: 2,
    usage: usage(),
  },
  {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
    id: 'r-fail-event',
    workflow_name: 'wf-failed-event',
    status: 'failed',
    started_at: '2026-06-01T00:00:00Z',
    trigger: 'event',
    turns: 3,
    usage: usage(),
  },
  {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
    id: 'r-await-manual',
    workflow_name: 'wf-awaiting-manual',
    status: 'awaiting_approval',
    started_at: '2026-06-01T00:00:00Z',
    trigger: 'manual',
    turns: 1,
    usage: usage(),
  },
];

function mockRuns(rows: RunListRow[] = ROWS): void {
  vi.spyOn(api, 'getProjectRuns').mockImplementation((_wsId, params) => {
    // Page 0 returns all rows; any subsequent offset returns empty.
    if (params?.offset && params.offset > 0) return Promise.resolve([]);
    return Promise.resolve(rows);
  });
  // ProjectUsageTimeline (Task U4) fetches independently of the run list
  // above — mock it too so this test isn't tripped up by an unmocked call.
  vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);
}

function renderTab(wsId = 'x') {
  return render(
    <MemoryRouter>
      <ProjectRunsTab wsId={wsId} />
    </MemoryRouter>,
  );
}

describe('ProjectRunsTab — table rules (fit/subject columns)', () => {
  it('the workflow column is the one flexible/truncating subject column', async () => {
    mockRuns();
    renderTab();
    await waitFor(() => expect(screen.getByText('wf-running-manual')).toBeInTheDocument());

    const subjectCell = screen.getByText('wf-running-manual').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector('[title="wf-running-manual"]')).toBeInTheDocument();
  });

  it('the Status column is a fit (nowrap) column', async () => {
    mockRuns();
    const { container } = renderTab();
    await waitFor(() => expect(screen.getByText('wf-running-manual')).toBeInTheDocument());

    const statusHeader = Array.from(container.querySelectorAll('thead th')).find((th) =>
      th.textContent?.includes('Status'),
    );
    expect(statusHeader?.className).toMatch(/whitespace-nowrap/);
  });
});

describe('ProjectRunsTab — kit loading/empty/error states', () => {
  it('shows the kit Spinner before data resolves', async () => {
    let resolveFn: (v: RunListRow[]) => void = () => {};
    vi.spyOn(api, 'getProjectRuns').mockReturnValue(
      new Promise((resolve) => {
        resolveFn = resolve;
      }),
    );
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);

    renderTab();

    expect(screen.getByRole('status')).toBeInTheDocument();
    resolveFn([]);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });

  it('shows the kit EmptyState when there are no runs at all', async () => {
    mockRuns([]);
    renderTab();

    await waitFor(() => expect(screen.getByText('No runs for this project yet')).toBeInTheDocument());
  });

  it('says nothing matches when a query keeps no run', async () => {
    mockRuns([]);
    render(
      <MemoryRouter initialEntries={['/?q=status:failed']}>
        <ProjectRunsTab wsId="x" />
      </MemoryRouter>,
    );
    expect(await screen.findByText('No runs match the filter.')).toBeInTheDocument();
    expect(screen.queryByText('No runs for this project yet')).not.toBeInTheDocument();
  });

  it('shows the kit ErrorBanner when the fetch fails', async () => {
    vi.spyOn(api, 'getProjectRuns').mockRejectedValue(new Error('network down'));
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);

    renderTab();

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('network down'));
  });
});

describe('ProjectRunsTab — codenames', () => {
  it('renders each run’s crew name', async () => {
    mockRuns([
      { ...ROWS[0], id: 'r-a', workflow_name: 'wf-a', codename: 'cobalt-harbor/heron#1' },
      { ...ROWS[1], id: 'r-b', workflow_name: 'wf-b', codename: 'amber-fjord/kite#1' },
    ]);
    renderTab();
    await waitFor(() => expect(screen.getByText('cobalt-harbor')).toBeInTheDocument());
    expect(screen.getByText('amber-fjord')).toBeInTheDocument();
  });
});

describe('ProjectRunsTab — pricing errors', () => {
  it('marks a run row priced at the global rates', async () => {
    mockRuns([{ ...ROWS[0], usage: { ...usage(), pricing_error: 'acme layer broken' } }, ROWS[1]]);
    renderTab();
    expect(await screen.findByRole('img', { name: 'Pricing unavailable: acme layer broken', hidden: true })).toBeInTheDocument();
    expect(screen.getAllByRole('img', { name: /^Pricing unavailable/, hidden: true })).toHaveLength(1);
  });
});

describe('ProjectRunsTab — the query reaches the server', () => {
  const PATH = '/api/projects/ws-1/runs';

  function mountQuery(answer: (u: URL) => StubAnswer | undefined, search = '') {
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);
    const urls = stubListFetch([], answer);
    render(
      <MemoryRouter initialEntries={[`/${search}`]}>
        <ProjectRunsTab wsId="ws-1" />
      </MemoryRouter>,
    );
    return urls;
  }
  const sentQ = (urls: URL[]) => listCalls(urls, PATH).map((u) => u.searchParams.get('q'));

  it.each([
    ['', null],
    ['?q=status:failed', 'status:failed'],
    ['?q=trigger:cron since:7d', 'trigger:cron since:7d'],
  ])('%s sends q=%s, with no implicit default', async (search, q) => {
    const urls = mountQuery(() => ({ body: [] }), search);
    await waitFor(() => expect(sentQ(urls)).toHaveLength(1));
    expect(sentQ(urls)).toEqual([q]);
  });

  it('asks for facets on the first page only, and a new query reads from offset 0 without them', async () => {
    const urls = mountQuery((u) =>
      u.searchParams.get('facets') === '1'
        ? { body: { rows: [ROWS[0]], facets: { status: { running: 1 } }, facets_partial: false } }
        : { body: [ROWS[0]] },
    );
    await screen.findByText('wf-running-manual');
    expect(listCalls(urls, PATH).map((u) => u.searchParams.get('facets'))).toEqual(['1']);

    const input = screen.getByRole('combobox', { name: 'Filter runs' });
    fireEvent.change(input, { target: { value: 'status:failed' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(sentQ(urls)).toEqual([null, 'status:failed']));
    const last = listCalls(urls, PATH)[1];
    expect(last.searchParams.get('offset')).toBe('0');
    expect(last.searchParams.get('facets')).toBeNull();
  });

  it('shows the server’s parse error under the bar, not as a page error', async () => {
    mountQuery(() => parseError('`trigger` takes manual, cron or event'), '?q=trigger:cron');
    expect(await screen.findByText('`trigger` takes manual, cron or event')).toBeInTheDocument();
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(screen.getByText('No runs match the filter.')).toBeInTheDocument();
  });

  it('shows a client parse error at once and sends no request', async () => {
    const urls = mountQuery(() => ({ body: [] }), '?q=status:done');
    expect(
      await screen.findByText('`done` is not a value of `status:` (one of pending, running, awaiting_approval, paused, completed, failed, rejected, cancelled)'),
    ).toBeInTheDocument();
    expect(listCalls(urls, PATH)).toEqual([]);
  });
});
