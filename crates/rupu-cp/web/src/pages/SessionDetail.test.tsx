// @vitest-environment jsdom
// SessionDetail composer — an operator types a message and sends it into a
// live session. The send is fire-and-forget (POST /api/sessions/:id/send) and
// the new turn run surfaces via an immediate getAgentRuns() refetch.
//
// RunUsageTimeline is mocked so the test never pulls in recharts.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, type SessionSummary } from '../lib/api';

vi.mock('../components/charts/RunUsageTimeline', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-timeline-mock">chart</div>,
}));

import SessionDetailPage from './SessionDetail';

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
  vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/sessions/sess-1']}>
      <Routes>
        <Route path="/sessions/:id" element={<SessionDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('SessionDetail composer', () => {
  it('sends a typed message into the session and clears the box', async () => {
    stubApi(ACTIVE_SESSION);
    const sendSpy = vi.spyOn(api, 'sendSessionMessage').mockResolvedValue({ run_id: 'run-9' });

    renderPage();

    const textarea = (await screen.findByLabelText('Message this session')) as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: '  hello there  ' } });

    fireEvent.click(screen.getByRole('button', { name: 'Send' }));

    // Trimmed prompt is sent with the route's session id.
    await waitFor(() => expect(sendSpy).toHaveBeenCalledWith('sess-1', 'hello there'));
    // On success the textarea is cleared and a confirmation shows.
    await waitFor(() => expect(textarea.value).toBe(''));
    expect(screen.getByText(/Sent — turn queued/)).toBeInTheDocument();
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
});
