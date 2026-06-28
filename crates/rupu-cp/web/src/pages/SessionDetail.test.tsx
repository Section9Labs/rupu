// @vitest-environment jsdom
// SessionDetail composer — an operator types a message and sends it into a
// live session. The send is fire-and-forget (POST /api/sessions/:id/send) and
// the new turn surfaces via an immediate getSessionRuns() refetch.
//
// RunUsageTimeline (recharts) and TranscriptPanel (the conversation child) are
// both mocked so the test never pulls in their heavy deps.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeAll, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, type SessionSummary } from '../lib/api';

vi.mock('../components/charts/RunUsageTimeline', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-timeline-mock">chart</div>,
}));

vi.mock('../components/TranscriptPanel', () => ({
  __esModule: true,
  default: ({ path }: { path: string }) => <div data-testid="transcript-panel">transcript:{path}</div>,
}));

import SessionDetailPage from './SessionDetail';

// jsdom doesn't implement scrollIntoView (used by the conversation auto-scroll).
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const ACTIVE_SESSION: SessionSummary = {
  session_id: 'sess-1',
  agent_name: 'reviewer',
  model: 'opus',
  status: 'running',
  total_turns: 2,
  created_at: '2026-06-01T00:00:00Z',
  updated_at: '2026-06-01T00:05:00Z',
  scope: 'project',
};

const STOPPED_SESSION: SessionSummary = {
  ...ACTIVE_SESSION,
  status: 'stopped',
};

function stubApi(session: SessionSummary) {
  vi.spyOn(api, 'getSession').mockResolvedValue(session);
  vi.spyOn(api, 'getSessionUsageTimeline').mockResolvedValue([]);
  vi.spyOn(api, 'getSessionRuns').mockResolvedValue([]);
}

function renderPage(search = '') {
  return render(
    <MemoryRouter initialEntries={[`/sessions/sess-1${search}`]}>
      <Routes>
        <Route path="/sessions/:id" element={<SessionDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('SessionDetail composer', () => {
  it('sends a typed message into the session, clears the box, and refetches runs', async () => {
    stubApi(ACTIVE_SESSION);
    const sendSpy = vi.spyOn(api, 'sendSessionMessage').mockResolvedValue({ run_id: 'run-9' });
    const runsSpy = api.getSessionRuns as unknown as ReturnType<typeof vi.fn>;

    renderPage();

    const textarea = (await screen.findByLabelText('Message this session')) as HTMLTextAreaElement;
    // One initial conversation load on mount.
    await waitFor(() => expect(runsSpy).toHaveBeenCalled());
    const callsBeforeSend = runsSpy.mock.calls.length;

    fireEvent.change(textarea, { target: { value: '  hello there  ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Send' }));

    // Trimmed prompt is sent with the route's session id (no host → undefined).
    await waitFor(() => expect(sendSpy).toHaveBeenCalledWith('sess-1', 'hello there', undefined));
    // On success the textarea is cleared and a confirmation shows.
    await waitFor(() => expect(textarea.value).toBe(''));
    expect(screen.getByText(/Sent — turn queued/)).toBeInTheDocument();
    // The new turn is surfaced via an immediate getSessionRuns() refetch.
    await waitFor(() => expect(runsSpy.mock.calls.length).toBeGreaterThan(callsBeforeSend));
  });

  it('disables Send when the session is stopped', async () => {
    stubApi(STOPPED_SESSION);
    const sendSpy = vi.spyOn(api, 'sendSessionMessage').mockResolvedValue({ run_id: 'run-9' });

    renderPage();

    const sendBtn = await screen.findByRole('button', { name: 'Send' });
    expect(sendBtn).toBeDisabled();
    expect(screen.getByText(/Session is stopped/)).toBeInTheDocument();

    fireEvent.click(sendBtn);
    expect(sendSpy).not.toHaveBeenCalled();
  });

  it('renders a session error banner when the session has last_error', async () => {
    const failedSession: SessionSummary = {
      ...ACTIVE_SESSION,
      status: 'failed',
      last_error: 'provider: API error 401',
    };
    stubApi(failedSession);

    renderPage();

    // Wait for the session to load then check the banner appears.
    expect(await screen.findByRole('alert')).toBeInTheDocument();
    expect(screen.getByText('provider: API error 401')).toBeInTheDocument();
  });
});

describe('SessionDetail host-aware', () => {
  it('with ?host=h1 calls getSession with {host:"h1"}', async () => {
    const getSessionSpy = vi.spyOn(api, 'getSession').mockResolvedValue(ACTIVE_SESSION);
    vi.spyOn(api, 'getSessionUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getSessionRuns').mockResolvedValue([]);

    renderPage('?host=h1');

    await waitFor(() =>
      expect(getSessionSpy).toHaveBeenCalledWith('sess-1', { host: 'h1' }),
    );
  });

  it('with ?host=h1 calls getSessionRuns with {host:"h1"}', async () => {
    vi.spyOn(api, 'getSession').mockResolvedValue(ACTIVE_SESSION);
    vi.spyOn(api, 'getSessionUsageTimeline').mockResolvedValue([]);
    const getRunsSpy = vi.spyOn(api, 'getSessionRuns').mockResolvedValue([]);

    renderPage('?host=h1');

    await waitFor(() =>
      expect(getRunsSpy).toHaveBeenCalledWith('sess-1', { host: 'h1' }),
    );
  });

  it('with ?host=h1 calls getSessionUsageTimeline with {host:"h1"}', async () => {
    vi.spyOn(api, 'getSession').mockResolvedValue(ACTIVE_SESSION);
    const timelineSpy = vi.spyOn(api, 'getSessionUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getSessionRuns').mockResolvedValue([]);

    renderPage('?host=h1');

    await waitFor(() =>
      expect(timelineSpy).toHaveBeenCalledWith('sess-1', { host: 'h1' }),
    );
  });

  it('shows "on h1" chip in the header when ?host=h1', async () => {
    stubApi(ACTIVE_SESSION);

    renderPage('?host=h1');

    expect(await screen.findByText('on h1')).toBeInTheDocument();
  });

  it('no host chip when no ?host param', async () => {
    stubApi(ACTIVE_SESSION);

    renderPage();

    // Wait for the session to render then confirm no chip.
    await screen.findByText(ACTIVE_SESSION.session_id);
    expect(screen.queryByText(/^on /)).not.toBeInTheDocument();
  });
});
