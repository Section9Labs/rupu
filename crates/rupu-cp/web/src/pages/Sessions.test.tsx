// @vitest-environment jsdom
// Sessions — every registered host with one request each and honest
// per-host loading/offline state; the agent subject cell truncating with a
// title tooltip; the kit's loading/empty/error states; row navigation; the
// customer scope. The query line itself is Sessions.query.test.tsx.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { api, ApiError } from '../lib/api';
import type { SessionSummary } from '../lib/api';
import { REG_LOCAL, REG_PROD, callsFor, onlyHost } from '../lib/perHost/testUtils';
import Sessions from './Sessions';
import { scopedEntry, withCustomerScope } from '../lib/customerScopeTestUtils';

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const REMOTE_SESSION: SessionSummary = {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
  session_id: 'sess-abc123',
  agent_name: 'fix-bug',
  model: 'claude-3-5-sonnet',
  status: 'active',
  total_turns: 5,
  created_at: '2026-06-01T00:00:00Z',
  updated_at: '2026-06-01T01:00:00Z',
  scope: 'active',
  host_id: 'host_prod',
};

function stubDeps() {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      {withCustomerScope(<Sessions />)}
    </MemoryRouter>,
  );
}

describe('Sessions — per-host loading', () => {
  it('fetches every registered host with its own host param', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(callsFor(sessionsSpy, 'local')).toHaveLength(1));
    expect(callsFor(sessionsSpy, 'host_prod')).toHaveLength(1);
  });

  it('shows an offline remote in the line under the bar without hiding local rows', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([{ ...REMOTE_SESSION, host_id: 'local' }])
        : Promise.reject(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}')),
    );
    renderPage();
    expect(await screen.findByTestId('host-alert')).toHaveTextContent('prod offline: its sessions are missing');
    expect(screen.getByText(/fix-bug/)).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('says it is waiting on a host, not that there are no sessions, while that host loads', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockImplementation((p) =>
      p?.host === 'local' ? Promise.resolve([]) : new Promise(() => {}),
    );
    renderPage();
    expect(await screen.findByText('Waiting on prod…')).toBeInTheDocument();
    expect(screen.queryByText('No active sessions')).not.toBeInTheDocument();
  });

  it('says no host answered when every host failed', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockRejectedValue(new ApiError(502, 'x', '{"error":"down"}'));
    renderPage();
    expect(await screen.findByText('No hosts answered')).toBeInTheDocument();
    expect(screen.getByTestId('host-alert')).toHaveTextContent(
      'Local offline: its sessions are missing · prod offline: its sessions are missing',
    );
    expect(screen.queryByText('No active sessions')).not.toBeInTheDocument();
  });

  it('Host column renders host_id from the row', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);

    renderPage();

    await waitFor(() => expect(screen.getByText('host_prod')).toBeInTheDocument());
  });

  it('Host column falls back to "local" when host_id is absent', async () => {
    stubDeps();
    const localSession: SessionSummary = { ...REMOTE_SESSION, host_id: undefined };
    // Only the local host answers with it: the loader tags a row that lacks
    // host_id with the slice it came from, so answering every host would
    // render one copy per host.
    vi.spyOn(api, 'getSessions').mockImplementation(onlyHost('local', [localSession]));

    renderPage();

    await waitFor(() => expect(screen.getByText('local')).toBeInTheDocument());
  });
});

describe('Sessions — agent subject cell (table rules)', () => {
  it('is the one flexible/truncating subject column', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);

    renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    const subjectCell = screen.getByText(/fix-bug/).closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    // Title tooltip carries the untruncated value.
    expect(subjectCell?.querySelector('[title="fix-bug"]')).toBeInTheDocument();
  });

  it('the Host column cell is a fit column (nowrap, shrink-to-content)', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);

    renderPage();

    const cell = (await screen.findByText('host_prod')).closest('td');
    expect(cell?.className).toMatch(/whitespace-nowrap/);
  });
});

describe('Sessions — kit loading/empty/error states', () => {
  it('shows the kit Spinner with the current loading copy before data resolves', async () => {
    stubDeps();
    let resolveFn: (v: SessionSummary[]) => void = () => {};
    vi.spyOn(api, 'getSessions').mockReturnValue(
      new Promise((resolve) => { resolveFn = resolve; }),
    );

    renderPage();

    expect(screen.getByRole('status')).toHaveAttribute('aria-label', 'Loading sessions…');
    resolveFn([]);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });

  it('shows the kit EmptyState with the current copy when there are no active sessions', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();

    await waitFor(() => expect(screen.getByText('No active sessions')).toBeInTheDocument());
    expect(
      screen.getByText(
        'Active sessions appear here once an agent conversation is started against this control plane; is:archived lists closed ones.',
      ),
    ).toBeInTheDocument();
  });

  it('says nothing matches when a query keeps no session', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([]);
    render(
      <MemoryRouter initialEntries={['/sessions?q=is:archived']}>
        {withCustomerScope(<Sessions />)}
      </MemoryRouter>,
    );
    expect(await screen.findByText('No sessions match the filter.')).toBeInTheDocument();
    expect(screen.queryByText('No active sessions')).not.toBeInTheDocument();
  });

  it('shows the kit ErrorBanner when the fetch fails', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockRejectedValue(new Error('network down'));

    renderPage();

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('network down'));
  });
});

// ── Task 3 (2026-07-24 table-standardization plan): canonical column order,
// the shared SessionStatusPill, and the new Started column ────────────────

describe('Sessions — canonical run-table column order', () => {
  it('renders headers in the canonical order: Status, Agent, Session, Host, Model, In, Out, Cached, Cost, Turns, Duration, Started, (actions)', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);

    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    expect(headers).toEqual([
      'Status',
      'Agent',
      'Session',
      'Host',
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
    // Real timers throughout — the per-host list's poll timers + testing-library's
    // waitFor both rely on real timers, so fake system time would hang them.
    // Instead pin `created_at` a known 3 minutes before the real "now".
    const threeMinAgo = new Date(Date.now() - 3 * 60_000).toISOString();
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([
      { ...REMOTE_SESSION, created_at: threeMinAgo },
    ]);

    renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    expect(screen.getByText('3m ago')).toBeInTheDocument();
  });
});

describe('Sessions — status cell uses the shared status chip', () => {
  it('a "running"-like status renders the running chip with a live dot', async () => {
    stubDeps();
    // rupu-cli's raw session status ("active") is not one of the four
    // canonical values (idle|running|failed|stopped) — this also exercises
    // the coercion path (sessionStatusTone maps "active" -> "running").
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);

    renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    const pill = screen.getByText('Running');
    // The one status chip: a leading dot marks the live (running) state.
    expect(pill.querySelector('[aria-hidden="true"]')).not.toBeNull();
  });

  it('an unknown/garbage status value renders the RAW label, not a fabricated real state', async () => {
    stubDeps();
    const garbageSession: SessionSummary = { ...REMOTE_SESSION, status: 'zzz-not-a-real-status' };
    vi.spyOn(api, 'getSessions').mockResolvedValue([garbageSession]);

    renderPage();

    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    // Never coerced onto "Stopped" (or any of the four real states) — the raw
    // wire value renders as-is, honestly reporting that the status is unknown.
    const pill = screen.getByText('zzz-not-a-real-status');
    expect(pill).toBeInTheDocument();
    expect(pill).not.toHaveAttribute('data-motion', 'rg-pulse-run');
    expect(screen.queryByText('Stopped')).toBeNull();
  });
});

// ── Task 4 (table-standardization plan): whole-row navigation ──────────────

describe('Sessions — whole-row navigation (rowHref)', () => {
  it('renders each row as a link to /sessions/:id (with ?host= for a remote session)', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);

    renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    const link = screen.getByText(/fix-bug/).closest('a');
    expect(link).toHaveAttribute('href', `/sessions/${REMOTE_SESSION.session_id}?host=host_prod`);
  });

  it('clicking Archive does not navigate the row', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);
    vi.spyOn(api, 'archiveSession').mockResolvedValue(undefined);

    render(
      <MemoryRouter>
        {withCustomerScope(<Sessions />)}
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText(`Archive session ${REMOTE_SESSION.session_id}`));

    expect(vi.mocked(api.archiveSession)).toHaveBeenCalledWith(
      REMOTE_SESSION.session_id,
      REMOTE_SESSION.host_id,
    );
    expect(screen.getByTestId('loc')).toHaveTextContent('/');
  });

  it('clicking "active run" navigates to the run, not the session row', async () => {
    stubDeps();
    const withActiveRun: SessionSummary = { ...REMOTE_SESSION, active_run_id: 'run-xyz' };
    vi.spyOn(api, 'getSessions').mockResolvedValue([withActiveRun]);

    render(
      <MemoryRouter>
        {withCustomerScope(<Sessions />)}
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    fireEvent.click(screen.getByText('active run'));

    expect(screen.getByTestId('loc')).toHaveTextContent('/runs/run-xyz');
  });
});

describe('Sessions — codenames', () => {
  it('renders each session’s crew chip', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockImplementation(
      onlyHost('host_prod', [
        { ...REMOTE_SESSION, codename: 'cobalt-harbor/heron#1' },
        { ...REMOTE_SESSION, session_id: 'sess-zzz', agent_name: 'other', codename: 'amber-fjord/kite#1' },
      ]),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('cobalt-harbor')).toBeInTheDocument());
    expect(screen.getByText('amber-fjord')).toBeInTheDocument();
  });
});

describe('Sessions — the global customer scope', () => {
  it('passes customer: "acme" on every per-host request, beside the query', async () => {
    stubDeps();
    const spy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);
    render(
      <MemoryRouter initialEntries={[scopedEntry('acme')]}>
        {withCustomerScope(<Sessions />)}
      </MemoryRouter>,
    );
    for (const host of ['local', 'host_prod']) {
      await waitFor(() => expect(callsFor(spy, host).length).toBeGreaterThan(0));
      expect(callsFor(spy, host)[0][0]).toEqual(
        expect.objectContaining({ customer: 'acme', q: 'is:active', onHeaders: expect.any(Function) }),
      );
    }
  });

  it('an unscoped page sends no customer', async () => {
    stubDeps();
    const spy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(callsFor(spy, 'local').length).toBeGreaterThan(0));
    expect(callsFor(spy, 'local')[0][0]).not.toHaveProperty('customer');
  });

  it('names the hosts the X-Rupu-Hosts-Without-Customer header lists, by name', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockImplementation((p) => {
      if (p?.host === 'local') p.onHeaders?.({ withoutCustomer: ['host_prod'], unfilterable: [] });
      return Promise.resolve([]);
    });
    render(
      <MemoryRouter initialEntries={[scopedEntry('acme')]}>
        {withCustomerScope(<Sessions />)}
      </MemoryRouter>,
    );
    expect(await screen.findByTestId('host-alert')).toHaveTextContent(
      "prod can't filter by customer: its sessions are left out",
    );
  });

  it('marks a session cost priced at the wrong rates', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockImplementation(
      onlyHost('host_prod', [
        {
          ...REMOTE_SESSION,
          usage: { input_tokens: 1, output_tokens: 1, cached_tokens: 0, total_tokens: 2, cost_usd: 1, priced: true, runs: 1, pricing_error: 'globex layer broken' },
        },
      ]),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('host_prod')).toBeInTheDocument());
    // The row-link cells are aria-hidden (mouse-only), so find the mark by its tooltip.
    expect(screen.getAllByTitle('globex layer broken').length).toBeGreaterThan(0);
  });
});
