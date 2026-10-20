// @vitest-environment jsdom
// SessionDetail keeps its Conversation / Messages view in `?tab=`.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeAll, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { api, type SessionSummary } from '../lib/api';
import type { MessageSpace } from '../lib/useMessageSpace';

vi.mock('../components/charts/RunUsageTimeline', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-timeline-mock">chart</div>,
}));

vi.mock('../components/TranscriptPanel', () => ({
  __esModule: true,
  default: ({ path }: { path: string }) => <div data-testid="transcript-panel">transcript:{path}</div>,
}));

vi.mock('../components/messages/MessageFeed', () => ({
  __esModule: true,
  default: () => <div data-testid="message-feed">feed</div>,
}));

const PRESENT_SPACE = {
  status: 'ready',
  reason: null,
  space: 'present',
  legacy: false,
  participants: [],
  loaded: true,
  streaming: false,
  refreshable: true,
  refresh: () => {},
  pending: [],
  sendError: null,
  blocked: null,
  send: async () => true,
  unread: 0,
} as unknown as MessageSpace;

vi.mock('../lib/useMessageSpace', () => ({
  useMessageSpace: () => PRESENT_SPACE,
}));

import SessionDetailPage from './SessionDetail';

beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const SESSION: SessionSummary = {
  codename: 'amber-quay/wren#1',
  codename_derived: false,
  session_id: 'sess-7',
  agent_name: 'helper',
  model: 'opus',
  status: 'running',
  total_turns: 1,
  created_at: '2026-07-01T00:00:00Z',
  updated_at: '2026-07-01T00:02:00Z',
  scope: 'project',
};

function SearchProbe() {
  return <p data-testid="location-search">{useLocation().search}</p>;
}

describe('SessionDetail ?tab=', () => {
  it('opens the view ?tab= names and writes the view it switches to', async () => {
    vi.spyOn(api, 'getSession').mockResolvedValue(SESSION);
    vi.spyOn(api, 'getSessionUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getSessionRuns').mockResolvedValue([]);
    vi.spyOn(api, 'getSkills').mockResolvedValue([]);

    render(
      <MemoryRouter initialEntries={['/sessions/sess-7?host=h1&tab=messages']}>
        <Routes>
          <Route
            path="/sessions/:id"
            element={
              <>
                <SessionDetailPage />
                <SearchProbe />
              </>
            }
          />
        </Routes>
      </MemoryRouter>,
    );

    expect(await screen.findByTestId('message-feed')).toBeInTheDocument();
    expect(screen.queryByLabelText('Message this session')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Conversation' }));
    expect(screen.queryByTestId('message-feed')).not.toBeInTheDocument();
    expect(await screen.findByLabelText('Message this session')).toBeInTheDocument();
    expect(screen.getByTestId('location-search')).toHaveTextContent('?host=h1&tab=conversation');
  });
});
