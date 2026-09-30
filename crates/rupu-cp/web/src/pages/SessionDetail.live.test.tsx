// @vitest-environment jsdom
// SessionDetail live refresh — while a turn is in flight the fast (1.5s) poll
// also refreshes the header's usage chip and the by-turn usage chart, not just
// the conversation. An idle session keeps polling only the conversation.
//
// Only timer primitives are faked (not promises), so mocked API replies still
// resolve on the real microtask queue.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeAll, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, act } from '@testing-library/react';
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

beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function session(overrides: Partial<SessionSummary> = {}, totalTokens = 1): SessionSummary {
  return {
    session_id: 'sess-1',
    agent_name: 'reviewer',
    model: 'opus',
    status: 'running',
    active_run_id: 'run_x',
    total_turns: 1,
    created_at: '2026-06-01T00:00:00Z',
    updated_at: '2026-06-01T00:05:00Z',
    scope: 'project',
    usage: {
      input_tokens: totalTokens,
      output_tokens: 0,
      cached_tokens: 0,
      total_tokens: totalTokens,
      cost_usd: null,
      priced: false,
      runs: 1,
    },
    ...overrides,
  };
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

/** Flush pending promise callbacks (mock API replies) inside act(). */
async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe('SessionDetail live usage', () => {
  it('refreshes the header usage and the usage timeline on the fast poll while a turn is in flight', async () => {
    const getSession = vi
      .spyOn(api, 'getSession')
      .mockResolvedValueOnce(session({}, 1))
      .mockResolvedValue(session({}, 2));
    const getTimeline = vi.spyOn(api, 'getSessionUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getSessionRuns').mockResolvedValue([]);

    renderPage();
    await flush();

    // First load: header shows the initial total.
    expect(screen.getByText('1 tok')).toBeInTheDocument();
    const sessionCallsBefore = getSession.mock.calls.length;
    const timelineCallsBefore = getTimeline.mock.calls.length;

    // The session is active, so the cadence re-arms to 1.5s; one tick later
    // both fetches fire again and the header carries the new total.
    await advance(1600);
    await flush();

    expect(getSession.mock.calls.length).toBeGreaterThan(sessionCallsBefore);
    expect(getTimeline.mock.calls.length).toBeGreaterThan(timelineCallsBefore);
    expect(screen.getByText('2 tok')).toBeInTheDocument();
  });

  it('does not refetch the session or timeline on the idle poll', async () => {
    const getSession = vi
      .spyOn(api, 'getSession')
      .mockResolvedValue(session({ status: 'idle', active_run_id: null }, 5));
    const getTimeline = vi.spyOn(api, 'getSessionUsageTimeline').mockResolvedValue([]);
    const getRuns = vi.spyOn(api, 'getSessionRuns').mockResolvedValue([]);

    renderPage();
    await flush();
    const sessionCalls = getSession.mock.calls.length;
    const timelineCalls = getTimeline.mock.calls.length;
    const runsCalls = getRuns.mock.calls.length;

    await advance(5100);
    await flush();

    // The conversation still polls…
    expect(getRuns.mock.calls.length).toBeGreaterThan(runsCalls);
    // …but an idle session's identity/usage/chart are not re-fetched.
    expect(getSession.mock.calls.length).toBe(sessionCalls);
    expect(getTimeline.mock.calls.length).toBe(timelineCalls);
  });

  it('skips the usage refresh while the tab is hidden', async () => {
    const getSession = vi.spyOn(api, 'getSession').mockResolvedValue(session({}, 1));
    const getTimeline = vi.spyOn(api, 'getSessionUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getSessionRuns').mockResolvedValue([]);
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');

    renderPage();
    await flush();
    const sessionCalls = getSession.mock.calls.length;
    const timelineCalls = getTimeline.mock.calls.length;

    await advance(3200);
    await flush();
    expect(getSession.mock.calls.length).toBe(sessionCalls);
    expect(getTimeline.mock.calls.length).toBe(timelineCalls);

    visibility.mockReturnValue('visible');
    await advance(1600);
    await flush();
    expect(getSession.mock.calls.length).toBeGreaterThan(sessionCalls);
    expect(getTimeline.mock.calls.length).toBeGreaterThan(timelineCalls);
  });
});
