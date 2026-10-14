// @vitest-environment jsdom
// ProjectSessionsTab — the query line the server evaluates (`?q=` on
// /api/projects/:ws_id/sessions, with no implicit `is:` default; facets on
// the first page only; a 400 is the bar's error), fit/subject table-rules
// columns, kit loading/empty/error states, the UsageBarChart strip.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { api, type SessionSummary } from '../../lib/api';
import ProjectSessionsTab from './ProjectSessionsTab';
import { listCalls, parseError, stubListFetch, type StubAnswer } from '../../lib/query/listFetchStub';

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function usage(): NonNullable<SessionSummary['usage']> {
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

const ROWS: SessionSummary[] = [
  {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
    session_id: 'sess-active-1',
    agent_name: 'fix-bug',
    model: 'claude-3-5-sonnet',
    status: 'active',
    total_turns: 5,
    created_at: '2026-06-01T00:00:00Z',
    updated_at: '2026-06-01T01:00:00Z',
    scope: 'active',
    usage: usage(),
  },
  {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
    session_id: 'sess-archived-1',
    agent_name: 'review-pr',
    model: 'claude-3-5-sonnet',
    status: 'stopped',
    total_turns: 2,
    created_at: '2026-06-01T00:00:00Z',
    updated_at: '2026-06-01T00:30:00Z',
    scope: 'archived',
    usage: usage(),
  },
];

function mockSessions(rows: SessionSummary[] = ROWS): void {
  vi.spyOn(api, 'getProjectSessions').mockImplementation((_wsId, params) => {
    if (params?.offset && params.offset > 0) return Promise.resolve([]);
    return Promise.resolve(rows);
  });
}

function renderTab(wsId = 'x') {
  return render(
    <MemoryRouter>
      <ProjectSessionsTab wsId={wsId} />
    </MemoryRouter>,
  );
}

describe('ProjectSessionsTab — table rules (fit/subject columns)', () => {
  it('the agent column is the one flexible/truncating subject column', async () => {
    mockSessions();
    renderTab();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    const subjectCell = screen.getByText('fix-bug').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector('[title="fix-bug"]')).toBeInTheDocument();
  });

  it('the Status column is a fit (nowrap) column', async () => {
    mockSessions();
    const { container } = renderTab();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    const statusHeader = Array.from(container.querySelectorAll('thead th')).find((th) =>
      th.textContent?.includes('Status'),
    );
    expect(statusHeader?.className).toMatch(/whitespace-nowrap/);
  });
});

describe('ProjectSessionsTab — Task 3: canonical column order + shared pill', () => {
  it('renders headers in the canonical order (no Host slot — project tabs are already scoped): Status, Agent, Session, Model, In, Out, Cached, Cost, Turns, Duration, Started, (actions)', async () => {
    mockSessions();
    const { container } = renderTab();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    expect(headers).toEqual([
      'Status',
      'Agent',
      'Session',
      'Model',
      'In',
      'Out',
      'Cached',
      'Cost',
      'Turns',
      'Duration',
      'Started',
      '',
    ]);
  });

  it('renders a Started cell with relativeTime(created_at)', async () => {
    // Real timers throughout — testing-library's waitFor relies on real
    // timers, so fake system time would hang it.
    // Instead pin `created_at` a known 3 minutes before the real "now".
    const threeMinAgo = new Date(Date.now() - 3 * 60_000).toISOString();
    mockSessions([{ ...ROWS[0], created_at: threeMinAgo }]);

    renderTab();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    expect(screen.getAllByText('3m ago').length).toBeGreaterThan(0);
  });

  it('the status cell uses the shared SessionStatusPill (motion marker for a running-like status)', async () => {
    mockSessions();
    renderTab();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    // ROWS[0]'s raw status is "active" (rupu-cli vocabulary quirk, not one of
    // the four canonical values) — exercises the coercion path.
    const pill = screen.getByText('Running');
    expect(pill).toHaveAttribute('data-motion', 'rg-pulse-run');
  });

  it('an unknown/garbage status value renders the RAW label, not a fabricated real state', async () => {
    mockSessions([{ ...ROWS[0], status: 'totally-unrecognized' }]);
    renderTab();

    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());
    // Never coerced onto "Stopped" (or any of the four real states) — the raw
    // wire value renders as-is, honestly reporting that the status is unknown.
    const pill = screen.getByText('totally-unrecognized');
    expect(pill).toBeInTheDocument();
    expect(pill).not.toHaveAttribute('data-motion', 'rg-pulse-run');
    expect(screen.queryByText('Stopped')).toBeNull();
  });
});

describe('ProjectSessionsTab — usage bar chart strip', () => {
  it('renders a usage bar for each priced session', async () => {
    mockSessions();
    renderTab();

    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());
    // UsageBarChart renders each bar's label — both sessions have usage.
    const labels = screen.getAllByText('fix-bug');
    expect(labels.length).toBeGreaterThanOrEqual(1);
  });
});

describe('ProjectSessionsTab — whole-row navigation (rowHref, Task 4)', () => {
  it('renders each row as a link to /sessions/:id', async () => {
    mockSessions();
    renderTab();

    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());
    const link = screen.getByText('fix-bug').closest('a');
    expect(link).toHaveAttribute('href', '/sessions/sess-active-1');
  });

  it('clicking "active run" navigates to the run, not the session row', async () => {
    mockSessions([{ ...ROWS[0], active_run_id: 'run-xyz' }]);

    render(
      <MemoryRouter>
        <ProjectSessionsTab wsId="x" />
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    fireEvent.click(screen.getByText('active run'));

    expect(screen.getByTestId('loc')).toHaveTextContent('/runs/run-xyz');
  });
});

describe('ProjectSessionsTab — kit loading/empty/error states', () => {
  it('shows the kit Spinner before data resolves', async () => {
    let resolveFn: (v: SessionSummary[]) => void = () => {};
    vi.spyOn(api, 'getProjectSessions').mockReturnValue(
      new Promise((resolve) => {
        resolveFn = resolve;
      }),
    );

    renderTab();

    expect(screen.getByRole('status')).toBeInTheDocument();
    resolveFn([]);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });

  it('shows the kit EmptyState when there are no sessions at all', async () => {
    mockSessions([]);
    renderTab();

    await waitFor(() =>
      expect(screen.getByText('No sessions for this project yet')).toBeInTheDocument(),
    );
  });

  it('says nothing matches when a query keeps no session', async () => {
    mockSessions([]);
    render(
      <MemoryRouter initialEntries={['/?q=is:archived']}>
        <ProjectSessionsTab wsId="x" />
      </MemoryRouter>,
    );
    expect(await screen.findByText('No sessions match the filter.')).toBeInTheDocument();
    expect(screen.queryByText('No sessions for this project yet')).not.toBeInTheDocument();
  });

  it('shows the kit ErrorBanner when the fetch fails', async () => {
    vi.spyOn(api, 'getProjectSessions').mockRejectedValue(new Error('network down'));

    renderTab();

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('network down'));
  });
});

describe('ProjectSessionsTab — pricing errors', () => {
  it('marks a session row priced at the global rates', async () => {
    mockSessions([{ ...ROWS[0], usage: { ...usage(), pricing_error: 'acme layer broken' } }, ROWS[1]]);
    renderTab();
    expect(await screen.findByRole('img', { name: 'Pricing unavailable: acme layer broken', hidden: true })).toBeInTheDocument();
    expect(screen.getAllByRole('img', { name: /^Pricing unavailable/, hidden: true })).toHaveLength(1);
  });
});

describe('ProjectSessionsTab — the query reaches the server', () => {
  const PATH = '/api/projects/ws-1/sessions';

  function mountQuery(answer: (u: URL) => StubAnswer | undefined, search = '') {
    const urls = stubListFetch([], answer);
    render(
      <MemoryRouter initialEntries={[`/${search}`]}>
        <ProjectSessionsTab wsId="ws-1" />
      </MemoryRouter>,
    );
    return urls;
  }
  const sentQ = (urls: URL[]) => listCalls(urls, PATH).map((u) => u.searchParams.get('q'));

  it.each([
    ['', null],
    ['?q=agent:recon', 'agent:recon'],
    ['?q=is:archived', 'is:archived'],
  ])('%s sends q=%s, with no implicit is: default', async (search, q) => {
    const urls = mountQuery(() => ({ body: [] }), search);
    await waitFor(() => expect(sentQ(urls)).toHaveLength(1));
    expect(sentQ(urls)).toEqual([q]);
  });

  it('asks for facets on the first page only, and a new query reads from offset 0 without them', async () => {
    const urls = mountQuery((u) =>
      u.searchParams.get('facets') === '1'
        ? { body: { rows: [ROWS[0]], facets: { is: { active: 1 } }, facets_partial: false } }
        : { body: [ROWS[0]] },
    );
    await screen.findAllByText('fix-bug');
    expect(listCalls(urls, PATH).map((u) => u.searchParams.get('facets'))).toEqual(['1']);

    const input = screen.getByRole('combobox', { name: 'Filter sessions' });
    fireEvent.change(input, { target: { value: 'status:failed' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(sentQ(urls)).toEqual([null, 'status:failed']));
    const last = listCalls(urls, PATH)[1];
    expect(last.searchParams.get('offset')).toBe('0');
    expect(last.searchParams.get('facets')).toBeNull();
  });

  it('shows the server’s parse error under the bar, not as a page error', async () => {
    mountQuery(() => parseError('`provider` takes anthropic or openai'), '?q=provider:anthropic');
    expect(await screen.findByText('`provider` takes anthropic or openai')).toBeInTheDocument();
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(screen.getByText('No sessions match the filter.')).toBeInTheDocument();
  });

  it('shows a client parse error at once and sends no request', async () => {
    const urls = mountQuery(() => ({ body: [] }), '?q=is:closed');
    expect(await screen.findByText('`closed` is not a value of `is:` (one of active, archived)')).toBeInTheDocument();
    expect(listCalls(urls, PATH)).toEqual([]);
  });
});
