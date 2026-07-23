// @vitest-environment jsdom
// Sessions — verifies the FilterBar slot order, that the Active/Archived
// FilterPills group and the host select drive the server request (not
// client-side filtering), that the agent subject cell truncates with a
// title tooltip, and that the kit's loading/empty/error states are in place.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../lib/api';
import type { SessionSummary, HostView } from '../lib/api';
import Sessions from './Sessions';

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

const REMOTE_SESSION: SessionSummary = {
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
  vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      <Sessions />
    </MemoryRouter>,
  );
}

describe('Sessions — FilterBar', () => {
  it('renders FilterBar slots in the fixed order: filters (Active/Archived pills), then scope (host select)', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();

    const pills = await screen.findByRole('button', { name: 'Active' });
    const hostSelect = screen.getByLabelText('Host filter');
    // The pills group precedes the host select in document order.
    expect(pills.compareDocumentPosition(hostSelect) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('renders the Active/Archived pills with Active active by default', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();

    await waitFor(() => expect(screen.getByRole('button', { name: 'Active' })).toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Archived' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Active' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'Archived' })).toHaveAttribute('aria-pressed', 'false');
  });
});

describe('Sessions — Active/Archived drives fetch', () => {
  it('defaults to scope "active"', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();

    await waitFor(() =>
      expect(sessionsSpy).toHaveBeenCalledWith(expect.objectContaining({ scope: 'active' })),
    );
  });

  it('clicking "Archived" re-fetches with scope: "archived"', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();
    await waitFor(() => screen.getByRole('button', { name: 'Archived' }));

    fireEvent.click(screen.getByRole('button', { name: 'Archived' }));

    await waitFor(() =>
      expect(sessionsSpy).toHaveBeenCalledWith(expect.objectContaining({ scope: 'archived' })),
    );
  });
});

describe('Sessions host filter — server-driven', () => {
  it('default fetch is called with host: "local" (fast path, not fan-out)', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();

    await waitFor(() =>
      expect(sessionsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'local' })),
    );
  });

  it('renders This host, registered (non-local) hosts, and All hosts — via the shared HostSelect', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());

    const options = screen.getAllByRole('option') as HTMLOptionElement[];
    expect(options.map((o) => o.textContent)).toEqual(['This host', 'All hosts', 'prod']);
  });

  it('"All hosts" option fetches without a host param (fan-out branch)', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();
    await waitFor(() => expect(screen.getByLabelText('Host filter')).toBeInTheDocument());

    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: '__all__' } });

    await waitFor(() => {
      const calls = sessionsSpy.mock.calls;
      const lastParams = calls[calls.length - 1]?.[0];
      expect(lastParams?.host).toBeUndefined();
    });
  });

  it('remote host option fetches with that host id', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();
    await waitFor(() =>
      expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'host_prod' } });

    await waitFor(() =>
      expect(sessionsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'host_prod' })),
    );
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
    vi.spyOn(api, 'getSessions').mockResolvedValue([localSession]);

    renderPage();

    await waitFor(() => expect(screen.getByText('local')).toBeInTheDocument());
  });
});

describe('Sessions — agent subject cell (table rules)', () => {
  it('is the one flexible/truncating subject column', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    const subjectCell = screen.getByText('fix-bug').closest('td');
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
        'Active sessions appear here once an agent conversation is started against this control plane.',
      ),
    ).toBeInTheDocument();
  });

  it('shows the kit EmptyState with the archived copy on the Archived tab', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Archived' }));

    await waitFor(() => expect(screen.getByText('No archived sessions')).toBeInTheDocument());
    expect(
      screen.getByText('Archived sessions appear here once an active conversation is closed.'),
    ).toBeInTheDocument();
  });

  it('shows the kit ErrorBanner when the fetch fails', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockRejectedValue(new Error('network down'));

    renderPage();

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('network down'));
  });
});

// ── Amendment #1 (2026-07-23 feedback round): Find on every table ──────────

describe('Sessions — Find', () => {
  const OTHER_SESSION: SessionSummary = {
    ...REMOTE_SESSION,
    session_id: 'sess-def456',
    agent_name: 'review-pr',
    host_id: 'local',
  };

  it('typing narrows rows by agent name, session id, or host id', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION, OTHER_SESSION]);

    renderPage();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find sessions…'), { target: { value: 'review' } });

    await waitFor(() => expect(screen.queryByText('fix-bug')).not.toBeInTheDocument());
    expect(screen.getByText('review-pr')).toBeInTheDocument();
  });

  it('footer shows "N matches of M loaded" while a query is active', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION, OTHER_SESSION]);

    renderPage();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find sessions…'), { target: { value: 'review' } });

    await waitFor(() => expect(screen.getByText('1 matches of 2 loaded')).toBeInTheDocument());
  });

  it('Esc clears the query', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION, OTHER_SESSION]);

    renderPage();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    const input = screen.getByPlaceholderText('Find sessions…') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'review' } });
    await waitFor(() => expect(screen.queryByText('fix-bug')).not.toBeInTheDocument());

    fireEvent.keyDown(input, { key: 'Escape' });

    await waitFor(() => expect(input.value).toBe(''));
    expect(screen.getByText('fix-bug')).toBeInTheDocument();
  });

  it('composes with the Active/Archived pill: query narrows within the active tab only', async () => {
    stubDeps();
    const activeSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);

    renderPage();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find sessions…'), { target: { value: 'zzz-no-match' } });

    await waitFor(() => expect(screen.getByText('No matches')).toBeInTheDocument());
    // The query does not itself trigger a new server fetch — it stays
    // client-side (Active/host params only).
    expect(activeSpy).toHaveBeenCalledWith(expect.objectContaining({ scope: 'active' }));
  });
});
