// @vitest-environment jsdom
// AgentRuns — verifies the FilterBar slot order, that the lifecycle filter and
// host select drive the server request (not client-side filtering), that the
// agent subject cell renders the name + via/session sub-line, and that the
// kit's loading/empty states are in place.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import type { AgentRunRow, HostView } from '../../lib/api';
import AgentRuns from './AgentRuns';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const LOCAL_HOST: HostView = {
  id: 'local',
  name: 'Local',
  transport_kind: 'local',
  status: 'online',
  active_run_count: 0,
};
const REMOTE_HOST: HostView = {
  id: 'host_prod',
  name: 'prod',
  transport_kind: 'http_cp',
  status: 'online',
  active_run_count: 1,
};

const REMOTE_ROW: AgentRunRow = {
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
  vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      <AgentRuns />
    </MemoryRouter>,
  );
}

describe('AgentRuns — FilterBar', () => {
  it('renders FilterBar slots in the fixed order: filters (lifecycle pills), then scope (host select)', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);

    renderPage();

    const pills = await screen.findByRole('button', { name: 'Running' });
    const hostSelect = screen.getByLabelText('Host');
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

describe('AgentRuns host filter — server-driven', () => {
  it('default fetch is called with host: "local" (fast path, not fan-out)', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'local' })),
    );
  });

  it('remote host option fetches with that host id', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);

    renderPage();
    await waitFor(() =>
      expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByLabelText('Host'), { target: { value: 'host_prod' } });

    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'host_prod' })),
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
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([localRow]);

    renderPage();

    await waitFor(() => expect(screen.getByText('local')).toBeInTheDocument());
  });
});

describe('AgentRuns — agent subject cell', () => {
  it('renders the agent name and the via/session sub-line for a session-bound row', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW]);

    renderPage();

    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());
    expect(screen.getByText('session_turn')).toBeInTheDocument();
    const sessionLink = screen.getByRole('link', { name: /sess-01h/i });
    expect(sessionLink).toHaveAttribute('href', `/sessions/${encodeURIComponent(SESSION_ROW.session_id!)}`);
  });

  it('truncates the agent name cell with a title tooltip', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([SESSION_ROW]);

    renderPage();

    const name = await screen.findByText('review-pr');
    expect(name).toHaveAttribute('title', 'review-pr');
    expect(name.className).toMatch(/truncate/);
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
