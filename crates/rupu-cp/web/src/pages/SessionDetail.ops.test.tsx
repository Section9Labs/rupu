// @vitest-environment jsdom
// Session Compact and Stop (operator MCP Plan 3b, UI-1, mock §3). Compact
// opens a popover with the window field; while the session detail reports
// `compacting: true` the header shows a `compacting` chip, Compact and Stop
// are disabled and the composer is locked. When it ends, the conversation
// shows the result line. Stop confirms first.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeAll, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, ApiError, type SessionRunRow, type SessionSummary } from '../lib/api';
import { runsApi, type SessionCompaction } from '../lib/api3b/runs';

vi.mock('../components/charts/RunUsageTimeline', () => ({ __esModule: true, default: () => <div /> }));
vi.mock('../components/TranscriptPanel', () => ({
  __esModule: true,
  default: ({ path }: { path: string }) => <div data-testid="transcript-panel">transcript:{path}</div>,
}));

import SessionDetailPage from './SessionDetail';

beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

type Detail = SessionSummary & SessionCompaction;

const IDLE: Detail = {
  codename: 'cobalt-harbor/heron#1',
  codename_derived: false,
  session_id: 'ses_01JC',
  agent_name: 'refactor-loop',
  model: 'claude-opus-5-5',
  status: 'idle',
  total_turns: 23,
  created_at: '2026-10-20T14:00:00Z',
  updated_at: '2026-10-20T14:30:00Z',
  scope: 'active',
  compacting: false,
  compaction: null,
  last_compaction: null,
};

const COMPACTING: Detail = {
  ...IDLE,
  compacting: true,
  compaction: { id: 'cmp_1', pid: 4242, started_at: '2026-10-20T14:31:00Z' },
};

const RUN: SessionRunRow = {
  run_id: 'run_t1',
  prompt: 'refactor the parser',
  transcript_path: '/t/run_t1.jsonl',
  status: 'ok',
  started_at: '2026-10-20T14:20:00Z',
  completed_at: '2026-10-20T14:22:00Z',
  tokens_in: 1,
  tokens_out: 1,
  tokens_cached: 0,
  duration_ms: 1,
};

function stub(sessions: Detail[], runs: SessionRunRow[] = [RUN]) {
  const queue = [...sessions];
  vi.spyOn(api, 'getSession').mockImplementation(async () => (queue.length > 1 ? queue.shift()! : queue[0]));
  vi.spyOn(api, 'getSessionUsageTimeline').mockResolvedValue([]);
  vi.spyOn(api, 'getSessionRuns').mockResolvedValue(runs);
  vi.spyOn(api, 'getSkills').mockResolvedValue([]);
}

function renderPage(search = '') {
  return render(
    <MemoryRouter initialEntries={[`/sessions/ses_01JC${search}`]}>
      <Routes>
        <Route path="/sessions/:id" element={<SessionDetailPage />} />
        <Route path="/sessions" element={<div data-testid="sessions-list" />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('SessionDetail compact', () => {
  it('opens a popover whose window defaults to the session\'s own, and compacts', async () => {
    stub([IDLE, COMPACTING]);
    const compact = vi
      .spyOn(runsApi, 'compactSession')
      .mockResolvedValue({ ok: true, id: 'ses_01JC', compaction_id: 'cmp_1', log: '/l' });
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Compact' }));
    const pop = screen.getByRole('dialog', { name: 'Compact this session' });
    const field = within(pop).getByLabelText('Context window') as HTMLInputElement;
    expect(field.value).toBe('');
    expect(field.placeholder).toBe("the session's own");
    fireEvent.click(within(pop).getByRole('button', { name: 'Compact' }));
    await waitFor(() => expect(compact).toHaveBeenCalledWith('ses_01JC', undefined));
    // The chip comes from the detail saying `compacting`, read right after the 202.
    expect(await screen.findByText('compacting')).toBeInTheDocument();
  });

  it('sends a typed window', async () => {
    stub([IDLE]);
    const compact = vi
      .spyOn(runsApi, 'compactSession')
      .mockResolvedValue({ ok: true, id: 'ses_01JC', compaction_id: 'cmp_1', log: '/l' });
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Compact' }));
    const pop = screen.getByRole('dialog', { name: 'Compact this session' });
    fireEvent.change(within(pop).getByLabelText('Context window'), { target: { value: '120000' } });
    fireEvent.click(within(pop).getByRole('button', { name: 'Compact' }));
    await waitFor(() => expect(compact).toHaveBeenCalledWith('ses_01JC', 120000));
  });

  it('while compacting: chip, disabled Compact and Stop, locked composer', async () => {
    stub([COMPACTING]);
    renderPage();
    expect(await screen.findByText('compacting')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Compact' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Stop' })).toBeDisabled();
    const box = screen.getByLabelText('Message this session') as HTMLTextAreaElement;
    expect(box).toBeDisabled();
    expect(box.placeholder).toBe("The session is compacting; you can send again when it's done.");
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    expect(screen.getByText('Compacting — sending is disabled until it ends.')).toBeInTheDocument();
  });

  it('shows the finished compaction as a line in the conversation', async () => {
    stub([
      {
        ...IDLE,
        last_compaction: {
          id: 'cmp_1',
          outcome: 'compacted',
          detail: 'compacted session ses_01JC: summarized 30 messages, history 34 → 5 messages',
          finished_at: '2026-10-20T14:32:00Z',
        },
      },
    ]);
    renderPage();
    expect(await screen.findByText('Compacted 30 messages into 1: history 34 → 5 messages')).toBeInTheDocument();
  });

  it('does not repeat an old compaction once a later turn ran', async () => {
    stub([
      {
        ...IDLE,
        last_compaction: {
          id: 'cmp_0',
          outcome: 'discarded',
          detail: 'compaction of session ses_01JC discarded: the session changed while it was summarized',
          finished_at: '2026-10-20T14:00:00Z',
        },
      },
    ]);
    renderPage();
    await screen.findByTestId('transcript-panel');
    expect(screen.queryByText(/Compaction discarded/)).not.toBeInTheDocument();
  });

  it('shows a 409 inline under the actions', async () => {
    stub([IDLE]);
    vi.spyOn(runsApi, 'compactSession').mockRejectedValue(
      new ApiError(409, 'conflict', JSON.stringify({ error: 'session ses_01JC has a turn running' })),
    );
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Compact' }));
    fireEvent.click(within(screen.getByRole('dialog', { name: 'Compact this session' })).getByRole('button', { name: 'Compact' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('session ses_01JC has a turn running');
  });
});

describe('SessionDetail stop', () => {
  it('confirms, then stops and re-reads the session', async () => {
    stub([IDLE, { ...IDLE, status: 'stopped' }]);
    const stop = vi.spyOn(runsApi, 'stopSession').mockResolvedValue({ ok: true, id: 'ses_01JC' });
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Stop' }));
    const dlg = screen.getByRole('dialog', { name: 'Stop this session?' });
    expect(stop).not.toHaveBeenCalled();
    fireEvent.click(within(dlg).getByRole('button', { name: 'Stop session' }));
    await waitFor(() => expect(stop).toHaveBeenCalledWith('ses_01JC'));
    expect(await screen.findByText('Session is stopped — sending is disabled.')).toBeInTheDocument();
  });

  it('keeps going when the confirm is dismissed', async () => {
    stub([IDLE]);
    const stop = vi.spyOn(runsApi, 'stopSession');
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Stop' }));
    fireEvent.click(within(screen.getByRole('dialog', { name: 'Stop this session?' })).getByRole('button', { name: 'Keep it running' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(stop).not.toHaveBeenCalled();
  });

  it('a stopped session has Stop disabled', async () => {
    stub([{ ...IDLE, status: 'stopped' }]);
    renderPage();
    expect(await screen.findByRole('button', { name: 'Stop' })).toBeDisabled();
  });

  it('a remote session has Compact and Stop disabled, with the reason', async () => {
    stub([IDLE]);
    renderPage('?host=h1');
    const compact = await screen.findByRole('button', { name: 'Compact' });
    expect(compact).toBeDisabled();
    expect(compact).toHaveAttribute('title', expect.stringContaining('local-only'));
    expect(screen.getByRole('button', { name: 'Stop' })).toBeDisabled();
  });

  it('an archived session offers neither', async () => {
    stub([{ ...IDLE, scope: 'archived' }]);
    renderPage();
    await screen.findByRole('button', { name: /restore/i });
    expect(screen.queryByRole('button', { name: 'Compact' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Stop' })).not.toBeInTheDocument();
  });
});
