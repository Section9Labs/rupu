// @vitest-environment jsdom
// AgentRuns — every registered host with one request each and honest
// per-host loading/offline state; the agent subject cell (name + via/session
// sub-line); status normalization; kit loading/empty states; whole-row
// navigation; the customer scope. The query line itself is
// AgentRuns.query.test.tsx; row actions are AgentRuns.actions.test.tsx.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { api, ApiError } from '../../lib/api';
import type { AgentRunRow } from '../../lib/api';
import { REG_LOCAL, REG_PROD, callsFor, onlyHost } from '../../lib/perHost/testUtils';
import AgentRuns from './AgentRuns';
import { ACME, scopedEntry, withCustomerScope } from '../../lib/customerScopeTestUtils';

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const REMOTE_ROW: AgentRunRow = {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
  run_id: 'run-abc123',
  source: 'standalone',
  agent: 'fix-bug',
  status: 'completed',
  started_at: '2026-06-01T00:00:00Z',
  turns: 3,
  usage: { input_tokens: 100, output_tokens: 50, cached_tokens: 0, total_tokens: 150, cost_usd: null, priced: false, runs: 1 },
  host_id: 'host_prod',
};

const SESSION_ROW: AgentRunRow = {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
  run_id: 'run-def456',
  source: 'session',
  agent: 'review-pr',
  session_id: 'sess-01HXYZ0123456789ABCDEF',
  trigger_source: 'session_turn',
  status: 'completed',
  started_at: '2026-06-02T00:00:00Z',
  turns: 5,
  usage: { input_tokens: 10, output_tokens: 5, cached_tokens: 0, total_tokens: 15, cost_usd: null, priced: false, runs: 1 },
  host_id: 'local',
};

function stubDeps() {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      {withCustomerScope(<AgentRuns />)}
    </MemoryRouter>,
  );
}

describe('AgentRuns — per-host loading (spec 2026-10-01)', () => {
  it('fetches every registered host with its own host param', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(1);
  });

  it('paints local rows while a remote host is still loading', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      p?.host === 'local' ? Promise.resolve([{ ...REMOTE_ROW, host_id: 'local' }]) : new Promise(() => {}),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    expect(screen.queryByTestId('host-alert')).not.toBeInTheDocument();
  });

  it('shows an offline remote honestly without hiding local rows', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([{ ...REMOTE_ROW, host_id: 'local' }])
        : Promise.reject(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}')),
    );
    renderPage();
    expect(await screen.findByTestId('host-alert')).toHaveTextContent('prod offline: its runs are missing');
    expect(screen.getByText(/fix-bug/)).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('says which host is missing when the hosts that answered have no agent runs', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([])
        : Promise.reject(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}')),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('No agent runs')).toBeInTheDocument());
    expect(screen.getByTestId('host-alert')).toHaveTextContent('prod offline: its runs are missing');
  });

  it('names the host that is still loading while nothing has painted yet', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      p?.host === 'local' ? Promise.resolve([]) : new Promise(() => {}),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('Waiting on prod…')).toBeInTheDocument());
    expect(screen.queryByText('No agent runs')).not.toBeInTheDocument();
  });

  it('says no host answered when every host failed', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockRejectedValue(new ApiError(502, 'x', '{"error":"down"}'));
    renderPage();
    await waitFor(() => expect(screen.getByText('No hosts answered')).toBeInTheDocument());
    expect(screen.getByTestId('host-alert')).toHaveTextContent(
      'Local offline: its runs are missing · prod offline: its runs are missing',
    );
  });

  it('Host column renders host_id from the row', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW]);

    renderPage();

    await waitFor(() => expect(screen.getByText('host_prod')).toBeInTheDocument());
  });

  it('Host column falls back to "local" when host_id is absent', async () => {
    stubDeps();
    const localRow: AgentRunRow = { ...REMOTE_ROW, host_id: undefined };
    // Only local answers with the row — under All hosts a mock that answered
    // every host would legitimately yield one row per host.
    vi.spyOn(api, 'getAgentRuns').mockImplementation(onlyHost('local', [localRow]));

    renderPage();

    await waitFor(() => expect(screen.getByText('local')).toBeInTheDocument());
  });
});

describe('AgentRuns — agent subject cell', () => {
  it('renders the agent name and the via/session sub-line for a session-bound row', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW]);

    renderPage();

    await waitFor(() => expect(screen.getByText(/review-pr/)).toBeInTheDocument());
    expect(screen.getByText('session_turn')).toBeInTheDocument();
    // Operator decision (table-standardization Task 4): the row itself
    // navigates to the transcript/workflow view, so this competing
    // destination is an explicit inner control (a button that navigates
    // programmatically), never a nested <a> — see the "row navigation"
    // describe block below for the navigation assertion.
    // Exact match on the truncated shortId — a plain /sess-01h/i regex would
    // now also match the row-actions buttons' aria-labels (Task 3: "Archive
    // session sess-01H…", "Restore session sess-01H…", "Delete session
    // sess-01H…"), which all contain that same substring.
    const sessionControl = screen.getByRole('button', { name: 'sess-01H…' });
    expect(sessionControl).toBeInTheDocument();
  });

  it('truncates the agent name cell with a title tooltip', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW]);

    renderPage();

    const name = await screen.findByText(/review-pr/);
    // AgentName's inner span carries the full identity tooltip; the
    // subject wrapper still truncates.
    expect(name.parentElement).toHaveAttribute('title', 'cobalt-harbor/heron#1 · review-pr');
    expect(name.closest('[class*="truncate"]')).not.toBeNull();
  });

  it('falls back to an em-dash when the row has no agent name', async () => {
    stubDeps();
    const noAgentRow: AgentRunRow = { ...REMOTE_ROW, agent: null };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([noAgentRow]);

    renderPage();

    await waitFor(() => expect(screen.getAllByText('—').length).toBeGreaterThan(0));
  });
});

describe('AgentRuns — status column', () => {
  it('renders the status as the shared pill-shaped chip for a known status', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW]);

    renderPage();

    const status = await screen.findByText('Completed');
    // The one status chip of the visual language: a fully-rounded pill.
    expect(status.className).toMatch(/rounded-full/);
  });

  it('renders an em-dash when status is absent', async () => {
    stubDeps();
    const noStatusRow: AgentRunRow = { ...REMOTE_ROW, status: null };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([noStatusRow]);

    renderPage();

    await waitFor(() => expect(screen.getAllByText('—').length).toBeGreaterThan(0));
  });

  it('normalizes a session-branch "ok" wire status to the green Completed chip (no unknown-status fallback)', async () => {
    stubDeps();
    const okRow: AgentRunRow = { ...SESSION_ROW, status: 'ok' };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([okRow]);

    renderPage();

    const status = await screen.findByText('Completed');
    // The green completed tone — proves the wire "ok" hit the real descriptor,
    // not an unknown-status neutral fallback.
    expect(status).toHaveClass('bg-status-done/10');
  });

  it('normalizes a session-branch "error" wire status to the Failed chip', async () => {
    stubDeps();
    const errorRow: AgentRunRow = { ...SESSION_ROW, status: 'error' };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([errorRow]);

    renderPage();

    const status = await screen.findByText('Failed');
    expect(status).toHaveClass('bg-status-failed/10');
  });
});

describe('AgentRuns — kit loading/empty states', () => {
  it('shows the kit Spinner with the current loading copy before data resolves', async () => {
    stubDeps();
    let resolveFn: (v: AgentRunRow[]) => void = () => {};
    vi.spyOn(api, 'getAgentRuns').mockReturnValue(
      new Promise((resolve) => { resolveFn = resolve; }),
    );

    renderPage();

    expect(screen.getByRole('status')).toHaveAttribute('aria-label', 'Loading agent runs…');
    resolveFn([]);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });

  it('says no agent run is running when there are no rows and no query', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() => expect(screen.getByText('No agent runs')).toBeInTheDocument());
    expect(
      screen.getByText(
        'Standalone runs appear here once an agent runs. source:session lists session turns.',
      ),
    ).toBeInTheDocument();
  });

  it('says nothing matches when a query keeps no run', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
    render(
      <MemoryRouter initialEntries={['/?q=status:error']}>
        {withCustomerScope(<AgentRuns />)}
      </MemoryRouter>,
    );
    expect(await screen.findByText('No agent runs match the filter.')).toBeInTheDocument();
    expect(screen.queryByText('No agent runs')).not.toBeInTheDocument();
  });

  it('shows the kit ErrorBanner when the fetch fails', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockRejectedValue(new Error('network down'));

    renderPage();

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('network down'));
  });
});

describe('AgentRuns — source', () => {
  it('renders the session rows the server returns (the server filters by source, not the page)', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation(onlyHost('local', [{ ...REMOTE_ROW, host_id: 'local' }, SESSION_ROW]));

    renderPage();

    await waitFor(() => expect(screen.getByText(/review-pr/)).toBeInTheDocument());
    expect(screen.getByText(/fix-bug/)).toBeInTheDocument();
  });

  it('regression: no run_id appears twice in the rendered table (the dedupe payload shape)', async () => {
    stubDeps();
    // The backend now dedupes a session-turn run's standalone-meta.json row
    // and its session.json row into ONE merged row (see
    // `dedupe_agent_runs_by_run_id` in run_streams.rs) before the wire ever
    // carries it — this fixture is that already-merged shape (a single row
    // sharing REMOTE_ROW's run_id), guarding against the frontend ever
    // rendering it twice.
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW]);

    renderPage();

    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    // One header row + exactly one data row.
    expect(screen.getAllByRole('row')).toHaveLength(2);
  });
});

// ── Task 4 (table-standardization plan) — operator decision: row click goes
// to the transcript/workflow view; the session link is an explicit inner
// control that must not trigger row navigation ─────────────────────────────

describe('AgentRuns — whole-row navigation (rowHref) goes to the transcript view', () => {
  const ROW_WITH_TRANSCRIPT: AgentRunRow = {
    ...REMOTE_ROW,
    run_id: 'run-transcript-1',
    transcript_path: '/rupu/agents/fix-bug/transcripts/run-transcript-1.jsonl',
    status: 'completed',
  };

  it('renders the row as a link to the transcript view when transcript_path is present', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([ROW_WITH_TRANSCRIPT]);

    render(
      <MemoryRouter>
        {withCustomerScope(<AgentRuns />)}
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    const link = screen.getByText(/fix-bug/).closest('a');
    // REMOTE_ROW's host_id ('host_prod') isn't local, so &host= is appended
    // (parity with the pre-existing usage-chart `to` computation).
    expect(link).toHaveAttribute(
      'href',
      `/transcript?path=${encodeURIComponent(ROW_WITH_TRANSCRIPT.transcript_path!)}&live=0&host=host_prod`,
    );
  });

  it('a row WITH transcript_path keeps the row-hover affordance', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([ROW_WITH_TRANSCRIPT]);

    renderPage();

    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    const tr = screen.getByText(/fix-bug/).closest('tr')!;
    expect(tr.className).toMatch(/hover:bg-bg/);
  });

  it('the Run id cell no longer carries its own duplicate link to the same destination', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([ROW_WITH_TRANSCRIPT]);

    renderPage();

    const idCell = await screen.findByText(/run-tran/);
    expect(idCell.tagName).not.toBe('A');
  });

  it('a row with no transcript_path renders no row link at all', async () => {
    stubDeps();
    const noTranscript: AgentRunRow = { ...REMOTE_ROW, transcript_path: null };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([noTranscript]);

    renderPage();

    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    expect(screen.getByText(/fix-bug/).closest('a')).toBeNull();
  });

  // M5 (whole-branch-review): a dead (unlinked) row must not still show the
  // hover highlight that implies it's clickable — see SortableTable.tsx's
  // `isDeadLinkRow`.
  it("a row with no transcript_path does not carry the row-hover affordance", async () => {
    stubDeps();
    const noTranscript: AgentRunRow = { ...REMOTE_ROW, transcript_path: null };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([noTranscript]);

    renderPage();

    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    const tr = screen.getByText(/fix-bug/).closest('tr')!;
    expect(tr.className).not.toMatch(/hover:bg-bg/);
  });

  it('clicking the inner session control navigates to the session, not the row transcript destination', async () => {
    stubDeps();
    const sessionRowWithTranscript: AgentRunRow = {
      ...SESSION_ROW,
      transcript_path: '/rupu/agents/review-pr/transcripts/run-def456.jsonl',
    };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([sessionRowWithTranscript]);

    render(
      <MemoryRouter>
        {withCustomerScope(<AgentRuns />)}
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText(/review-pr/)).toBeInTheDocument());

    // Exact match — see the sibling "agent subject cell" test's comment for
    // why a loose /sess-01h/i regex is now ambiguous (Task 3 row actions).
    fireEvent.click(screen.getByRole('button', { name: 'sess-01H…' }));

    expect(screen.getByTestId('loc')).toHaveTextContent(
      `/sessions/${encodeURIComponent(sessionRowWithTranscript.session_id!)}`,
    );
  });
});

// ── Table standardization Task 3: Turns before Duration, matching Sessions ──

describe('AgentRuns — Turns/Duration column order (table-standardization Task 3)', () => {
  it('Turns precedes Duration in the header row (matches the canonical Sessions order)', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW]);

    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    const turnsIdx = headers.indexOf('Turns');
    const durationIdx = headers.indexOf('Duration');
    expect(turnsIdx).toBeGreaterThanOrEqual(0);
    expect(durationIdx).toBeGreaterThan(turnsIdx);
  });
});

describe('AgentRuns — codenames', () => {
  it('renders the crew chip', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation(
      onlyHost('local', [
        { ...REMOTE_ROW, host_id: 'local', codename: 'cobalt-harbor/heron#1' },
        { ...SESSION_ROW, codename: 'amber-fjord/kite#1' },
      ]),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('cobalt-harbor')).toBeInTheDocument());
    expect(screen.getByText('amber-fjord')).toBeInTheDocument();
  });

  it('shows the run provider/model next to the agent name', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([
      { ...SESSION_ROW, provider: 'anthropic', model: 'claude-sonnet-4-6' },
    ]);
    renderPage();
    expect(
      await screen.findByText('heron#1 · review-pr · anthropic/claude-sonnet-4-6'),
    ).toBeInTheDocument();
  });

  it('a row with no codename still names its agent + provider/model', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([
      { ...SESSION_ROW, codename: '', provider: 'openai', model: 'gpt-5' },
    ]);
    renderPage();
    expect(await screen.findByText('review-pr · openai/gpt-5')).toBeInTheDocument();
  });
});

describe('AgentRuns — the global customer scope', () => {
  it('passes customer: "acme" beside the query on every per-host request and names hosts that can’t filter', async () => {
    stubDeps();
    const spy = vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      p?.host === 'host_prod'
        ? Promise.reject(
            new ApiError(501, 'x', JSON.stringify({ error: "host host_prod can't report a customer for every run" })),
          )
        : Promise.resolve([]),
    );
    render(
      <MemoryRouter initialEntries={[scopedEntry('acme')]}>
        {withCustomerScope(<AgentRuns />)}
      </MemoryRouter>,
    );
    for (const host of ['local', 'host_prod']) {
      await waitFor(() => expect(callsFor(spy, host).length).toBeGreaterThan(0));
      expect(callsFor(spy, host)[0][0]).toEqual(
        expect.objectContaining({
          customer: 'acme',
          q: 'source:standalone',
          onHeaders: expect.any(Function),
        }),
      );
    }
    expect(await screen.findByTestId('host-alert')).toHaveTextContent("prod can't filter by customer: its runs are left out");
  });

  it('names the hosts the X-Rupu-Hosts-Without-Customer header lists, by name', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation((p) => {
      if (p?.host === 'local') p.onHeaders?.({ withoutCustomer: ['host_prod'], unfilterable: [] });
      return Promise.resolve([]);
    });
    render(
      <MemoryRouter initialEntries={[scopedEntry('acme')]}>
        {withCustomerScope(<AgentRuns />)}
      </MemoryRouter>,
    );
    expect(await screen.findByTestId('host-alert')).toHaveTextContent("prod can't filter by customer: its runs are left out");
  });

  it('an unscoped page sends no customer', async () => {
    stubDeps();
    const spy = vi.spyOn(api, 'getAgentRuns').mockImplementation(onlyHost('local', []));
    renderPage();
    await waitFor(() => expect(callsFor(spy, 'local').length).toBeGreaterThan(0));
    expect(callsFor(spy, 'local')[0][0]).not.toHaveProperty('customer');
    expect(screen.queryByTestId('host-alert')).not.toBeInTheDocument();
  });
});

describe('AgentRuns — the Customer column', () => {
  const headers = (c: HTMLElement) =>
    Array.from(c.querySelectorAll('thead th')).map((th) => th.textContent?.trim() ?? '');

  it('shows a Customer column after Host when unscoped, and none while scoped', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      Promise.resolve(p?.host === 'local' || !p?.host ? [{ ...REMOTE_ROW, host_id: 'local', customer: 'acme' }] : []),
    );
    const { container, unmount } = render(
      <MemoryRouter>{withCustomerScope(<AgentRuns />, { customers: [ACME] })}</MemoryRouter>,
    );
    await waitFor(() => expect(headers(container)).toContain('Customer'));
    const h = headers(container);
    expect(h.indexOf('Customer')).toBe(h.indexOf('Host') + 1);
    expect(screen.getByText('Acme')).toBeInTheDocument();
    unmount();

    const scoped = render(
      <MemoryRouter initialEntries={[scopedEntry('acme')]}>
        {withCustomerScope(<AgentRuns />, { customers: [ACME] })}
      </MemoryRouter>,
    );
    await waitFor(() => expect(scoped.container.querySelector('thead')).not.toBeNull());
    expect(headers(scoped.container)).not.toContain('Customer');
  });
});
