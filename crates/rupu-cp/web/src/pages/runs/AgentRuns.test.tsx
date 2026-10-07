// @vitest-environment jsdom
// AgentRuns — verifies the FilterBar slot order, that the lifecycle filter and
// host select drive the server request (not client-side filtering), that the
// agent subject cell renders the name + via/session sub-line, and that the
// kit's loading/empty states are in place.

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

describe('AgentRuns — FilterBar', () => {
  it('renders FilterBar slots in the fixed order: filters (lifecycle pills), then scope (host select)', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);

    renderPage();

    const pills = await screen.findByRole('button', { name: 'Running' });
    const hostSelect = screen.getByLabelText('Host filter');
    // The pills group precedes the host select in document order.
    expect(pills.compareDocumentPosition(hostSelect) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('renders the three lifecycle pills with Running active by default', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() => expect(screen.getByRole('button', { name: 'Running' })).toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Completed' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Failed / Rejected' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Running' })).toHaveAttribute('aria-pressed', 'true');
  });
});

describe('AgentRuns — lifecycle filter drives fetch params', () => {
  it('defaults to lifecycle "active" (Running)', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ lifecycle: 'active' })),
    );
  });

  it('clicking "Completed" re-fetches with lifecycle: "completed"', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);

    renderPage();
    await waitFor(() => screen.getByRole('button', { name: 'Completed' }));

    fireEvent.click(screen.getByRole('button', { name: 'Completed' }));

    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ lifecycle: 'completed' })),
    );
  });

  it('clicking "Failed / Rejected" re-fetches with lifecycle: "failed"', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);

    renderPage();
    await waitFor(() => screen.getByRole('button', { name: 'Failed / Rejected' }));

    fireEvent.click(screen.getByRole('button', { name: 'Failed / Rejected' }));

    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ lifecycle: 'failed' })),
    );
  });
});

describe('AgentRuns host filter — per-host loading (spec 2026-10-01)', () => {
  it('defaults to All hosts and fetches every registered host with its own host param', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(1);
    expect(screen.getByLabelText('Host filter')).toHaveValue('__all__');
  });

  it('renders This host, All hosts, then registered (non-local) hosts', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    const options = screen.getAllByRole('option') as HTMLOptionElement[];
    expect(options.map((o) => o.textContent)).toEqual(['This host', 'All hosts', 'prod']);
  });

  it('This host fetches only local', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    runsSpy.mockClear();
    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'local' } });
    await waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(0);
  });

  it('remote host option fetches with that host id', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'host_prod' } });
    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'host_prod' })),
    );
  });

  it('paints local rows while a remote host is still loading, naming it in the strip', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      p?.host === 'local' ? Promise.resolve([{ ...REMOTE_ROW, host_id: 'local' }]) : new Promise(() => {}),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    expect(screen.getByText('loading…')).toBeInTheDocument();
  });

  it('shows an offline remote honestly without hiding local rows', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([{ ...REMOTE_ROW, host_id: 'local' }])
        : Promise.reject(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}')),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText(/not included: prod \(offline\)/)).toBeInTheDocument());
    expect(screen.getByText(/fix-bug/)).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('names the hosts left out when every answering host has no agent runs', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([])
        : Promise.reject(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}')),
    );
    renderPage();
    await waitFor(() =>
      expect(screen.getByText('No agent runs on the hosts that answered')).toBeInTheDocument(),
    );
    expect(screen.getByText(/Not included: prod/)).toBeInTheDocument();
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
    // Source pill defaults to Standalone — switch to All to see the
    // session-sourced fixture row.
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));

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
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));

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
  it('renders a single-line status pill for a known status', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW]);

    renderPage();

    const status = await screen.findByText('Completed');
    expect(status.className).toMatch(/whitespace-nowrap/);
  });

  it('renders an em-dash when status is absent', async () => {
    stubDeps();
    const noStatusRow: AgentRunRow = { ...REMOTE_ROW, status: null };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([noStatusRow]);

    renderPage();

    await waitFor(() => expect(screen.getAllByText('—').length).toBeGreaterThan(0));
  });

  it('normalizes a session-branch "ok" wire status to the green Completed pill (no AlertCircle fallback)', async () => {
    stubDeps();
    const okRow: AgentRunRow = { ...SESSION_ROW, status: 'ok' };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([okRow]);

    renderPage();
    // Source pill defaults to Standalone — switch to All to see the
    // session-sourced fixture row.
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));

    // "Completed" also names the (unrelated) lifecycle filter pill button, so
    // disambiguate by tag: the status pill is a <span>, the filter is a
    // <button>.
    await waitFor(() => expect(screen.getAllByText('Completed').length).toBeGreaterThan(0));
    const status = screen.getAllByText('Completed').find((el) => el.tagName === 'SPAN');
    expect(status).toBeDefined();
    expect(status).toHaveClass('bg-status-done/10');
    // CheckCircle2 (the completed icon), never AlertCircle (the
    // unknown-status fallback) — proves the wire "ok" hit the real
    // descriptor, not StatusPill's fallback branch.
    expect(status?.querySelector('svg')).toHaveClass('lucide-circle-check');
  });

  it('normalizes a session-branch "error" wire status to the Failed pill', async () => {
    stubDeps();
    const errorRow: AgentRunRow = { ...SESSION_ROW, status: 'error' };
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([errorRow]);

    renderPage();
    // Source pill defaults to Standalone — switch to All to see the
    // session-sourced fixture row.
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));

    const status = await screen.findByText('Failed');
    expect(status.closest('span')).toHaveClass('bg-status-failed/10');
    expect(status.closest('span')?.querySelector('svg')).toHaveClass('lucide-circle-x');
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

  it('shows the kit EmptyState with the current copy when there are no rows', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() => expect(screen.getByText('No agent runs yet')).toBeInTheDocument());
    expect(
      screen.getByText('Standalone and session-bound agent invocations will appear here once they run.'),
    ).toBeInTheDocument();
  });

  it('shows the kit ErrorBanner when the fetch fails', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockRejectedValue(new Error('network down'));

    renderPage();

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('network down'));
  });
});

// ── Amendment #2 (2026-07-23 feedback round): Source FilterPills ───────────

describe('AgentRuns — Source filter', () => {
  it('defaults to Standalone: renders standalone rows, hides session rows', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW, SESSION_ROW]);

    renderPage();

    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    expect(screen.queryByText(/review-pr/)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Standalone' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('clicking "Session" shows only session-sourced rows', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW, SESSION_ROW]);

    renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Session' }));

    await waitFor(() => expect(screen.getByText(/review-pr/)).toBeInTheDocument());
    expect(screen.queryByText(/fix-bug/)).not.toBeInTheDocument();
  });

  it('clicking "All" shows both standalone and session rows', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW, SESSION_ROW]);

    renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'All' }));

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
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));

    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    // One header row + exactly one data row.
    expect(screen.getAllByRole('row')).toHaveLength(2);
  });
});

describe('AgentRuns — the Source filter while hosts are still loading or paging', () => {
  /** `n` session-turn rows, newest first, one a minute, starting `from` minutes before noon. */
  const sessionRows = (n: number, from = 0): AgentRunRow[] =>
    Array.from({ length: n }, (_, i) => ({
      ...SESSION_ROW,
      run_id: `run-sess-${from + i}`,
      started_at: new Date(Date.parse('2026-06-02T12:00:00Z') - (from + i) * 60_000).toISOString(),
    }));

  it('says it is still waiting, not "no match", when the default Standalone filter empties what loaded so far', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      p?.host === 'local' ? Promise.resolve(sessionRows(3)) : new Promise(() => {}),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('No matches yet · Waiting on prod…')).toBeInTheDocument());
    expect(screen.queryByText('No agent runs match this filter')).not.toBeInTheDocument();
    // Said once: the footer below does not repeat who it is waiting on.
    expect(screen.getAllByText(/waiting on prod/i)).toHaveLength(1);
  });

  it('keeps paging while the filter hides every loaded row, so a later page can match', async () => {
    stubDeps();
    // Page 0 is a full page of session turns; the first standalone run is further down.
    const local = [...sessionRows(22), { ...REMOTE_ROW, host_id: 'local', started_at: '2026-06-01T00:00:00Z' }];
    const spy = vi
      .spyOn(api, 'getAgentRuns')
      .mockImplementation((p) =>
        Promise.resolve(p?.host === 'local' ? local.slice(p.offset ?? 0, (p.offset ?? 0) + (p.limit ?? 20)) : []),
      );
    renderPage();
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());
    expect(callsFor(spy, 'local').some((c) => ((c[0] as { offset?: number }).offset ?? 0) > 0)).toBe(true);
  });
});

// ── Amendment #1 (2026-07-23 feedback round): Find on every table ──────────

describe('AgentRuns — Find', () => {
  it('typing narrows rows by agent name, run id, session id, or host id', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW, SESSION_ROW]);

    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));
    await waitFor(() => expect(screen.getByText(/review-pr/)).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find agents…'), { target: { value: 'review' } });

    await waitFor(() => expect(screen.queryByText(/fix-bug/)).not.toBeInTheDocument());
    expect(screen.getByText(/review-pr/)).toBeInTheDocument();
  });

  it('footer shows "N matches of M loaded" while a query is active', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW, SESSION_ROW]);

    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));
    await waitFor(() => expect(screen.getByText(/review-pr/)).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find agents…'), { target: { value: 'review' } });

    await waitFor(() => expect(screen.getByText('1 matches of 2 loaded')).toBeInTheDocument());
  });

  it('Esc clears the query', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW, SESSION_ROW]);

    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));
    await waitFor(() => expect(screen.getByText(/review-pr/)).toBeInTheDocument());

    const input = screen.getByPlaceholderText('Find agents…') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'review' } });
    await waitFor(() => expect(screen.queryByText(/fix-bug/)).not.toBeInTheDocument());

    fireEvent.keyDown(input, { key: 'Escape' });

    await waitFor(() => expect(input.value).toBe(''));
    expect(screen.getByText(/fix-bug/)).toBeInTheDocument();
  });

  it('composes with the Source pill: searching within Standalone narrows just that subset', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([REMOTE_ROW, SESSION_ROW]);

    renderPage();
    // Default Source is Standalone — fix-bug (standalone) is visible,
    // review-pr (session) is not, regardless of the query below.
    await waitFor(() => expect(screen.getByText(/fix-bug/)).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find agents…'), { target: { value: 'review' } });

    await waitFor(() => expect(screen.queryByText(/fix-bug/)).not.toBeInTheDocument());
    expect(screen.queryByText(/review-pr/)).not.toBeInTheDocument();
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
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));
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
  it('renders the crew chip and Find matches by codename', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([
      { ...REMOTE_ROW, codename: 'cobalt-harbor/heron#1' },
      { ...SESSION_ROW, codename: 'amber-fjord/kite#1' },
    ]);
    renderPage();
    await waitFor(() => expect(screen.getByText('cobalt-harbor')).toBeInTheDocument());
    fireEvent.change(screen.getByPlaceholderText('Find agents…'), { target: { value: 'cobalt' } });
    await waitFor(() => expect(screen.queryByText('amber-fjord')).not.toBeInTheDocument());
    expect(screen.getByText('cobalt-harbor')).toBeInTheDocument();
  });

  it('shows the run provider/model next to the agent name', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([
      { ...SESSION_ROW, provider: 'anthropic', model: 'claude-sonnet-4-6' },
    ]);
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));
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
    fireEvent.click(await screen.findByRole('button', { name: 'All' }));
    expect(await screen.findByText('review-pr · openai/gpt-5')).toBeInTheDocument();
  });
});

describe('AgentRuns — the global customer scope', () => {
  it('passes customer: "acme" on every per-host request and names hosts that can’t filter', async () => {
    stubDeps();
    const spy = vi.spyOn(api, 'getAgentRuns').mockImplementation((p) =>
      p?.host === 'host_prod'
        ? Promise.reject(new ApiError(501, "host host_prod can't report a customer for every run", ''))
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
        expect.objectContaining({ customer: 'acme', onHostsWithoutCustomer: expect.any(Function) }),
      );
    }
    expect(await screen.findByTestId('hosts-without-customer')).toHaveTextContent(/^prod runs an older rupu/);
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
