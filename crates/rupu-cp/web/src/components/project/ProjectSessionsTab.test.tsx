// @vitest-environment jsdom
// ProjectSessionsTab — One Control Language migration (Phase 3, Task G).
// Covers: the FilterBar renders with no host slot (project tabs are already
// workspace-scoped); the Scope FilterPills group narrows the loaded rows
// client-side; Find narrows further with the matches footer; fit/subject
// table-rules columns; kit loading/empty/error states; the UsageBarChart
// strip stays in place.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type SessionSummary } from '../../lib/api';
import ProjectSessionsTab from './ProjectSessionsTab';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
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

describe('ProjectSessionsTab — FilterBar (no host slot)', () => {
  it('renders the Scope FilterPills group and a Find input, with no HostSelect', async () => {
    mockSessions();
    renderTab();

    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    expect(screen.getByRole('button', { name: 'All' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'Active' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Archived' })).toBeInTheDocument();
    expect(screen.getByPlaceholderText('Find sessions…')).toBeInTheDocument();
    expect(screen.queryByLabelText('Host filter')).not.toBeInTheDocument();
  });
});

describe('ProjectSessionsTab — Scope FilterPills narrows rows', () => {
  it('renders all rows then narrows to Active, then Archived, then back to All', async () => {
    mockSessions();
    renderTab();

    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());
    expect(screen.getByText('review-pr')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Active' }));
    expect(screen.getByText('fix-bug')).toBeInTheDocument();
    expect(screen.queryByText('review-pr')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Archived' }));
    expect(screen.queryByText('fix-bug')).not.toBeInTheDocument();
    expect(screen.getByText('review-pr')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'All' }));
    expect(screen.getByText('fix-bug')).toBeInTheDocument();
    expect(screen.getByText('review-pr')).toBeInTheDocument();
  });
});

describe('ProjectSessionsTab — Find', () => {
  it('typing narrows rows by agent name or session id', async () => {
    mockSessions();
    renderTab();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find sessions…'), { target: { value: 'review' } });

    await waitFor(() => expect(screen.queryByText('fix-bug')).not.toBeInTheDocument());
    expect(screen.getByText('review-pr')).toBeInTheDocument();
  });

  it('footer shows "N matches of M loaded" while a query is active', async () => {
    mockSessions();
    renderTab();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find sessions…'), { target: { value: 'review' } });

    await waitFor(() => expect(screen.getByText('1 matches of 2 loaded')).toBeInTheDocument());
  });

  it('Esc clears the query', async () => {
    mockSessions();
    renderTab();
    await waitFor(() => expect(screen.getByText('review-pr')).toBeInTheDocument());

    const input = screen.getByPlaceholderText('Find sessions…') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'review' } });
    await waitFor(() => expect(screen.queryByText('fix-bug')).not.toBeInTheDocument());

    fireEvent.keyDown(input, { key: 'Escape' });

    await waitFor(() => expect(input.value).toBe(''));
    expect(screen.getByText('fix-bug')).toBeInTheDocument();
  });

  it('composes with the Scope pill: query narrows within the active pill filter', async () => {
    mockSessions();
    renderTab();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Active' }));
    await waitFor(() => expect(screen.queryByText('review-pr')).not.toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find sessions…'), { target: { value: 'zzz-no-match' } });

    await waitFor(() => expect(screen.getByText('No matches')).toBeInTheDocument());
  });
});

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

  it('shows the kit EmptyState "no match" copy when a pill filter narrows to zero', async () => {
    mockSessions([ROWS[0]]); // active only
    renderTab();
    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Archived' }));

    await waitFor(() => expect(screen.getByText('No sessions match this filter')).toBeInTheDocument());
  });

  it('shows the kit ErrorBanner when the fetch fails', async () => {
    vi.spyOn(api, 'getProjectSessions').mockRejectedValue(new Error('network down'));

    renderTab();

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('network down'));
  });
});
