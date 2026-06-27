// @vitest-environment jsdom
// SessionConversation — renders a session's turn-runs as a chat: a "You" bubble
// per prompt + an embedded TranscriptPanel per agent response. The recent-turns
// window shows the newest 10; "Load older turns" widens it. The active turn
// (run_id === active_run_id, or null status) streams live.
//
// TranscriptPanel is mocked to a marker exposing its path/live props.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeAll, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import type { SessionSummary, SessionRunRow } from '../../lib/api';

vi.mock('../TranscriptPanel', () => ({
  __esModule: true,
  default: ({ path, live, onComplete }: { path: string; live: boolean; onComplete?: () => void }) => (
    <div
      data-testid="transcript-panel"
      data-path={path}
      data-live={String(live)}
      data-has-on-complete={onComplete != null ? 'true' : 'false'}
    >
      transcript:{path}
    </div>
  ),
}));

import SessionConversation from './SessionConversation';

// jsdom doesn't implement scrollIntoView.
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const SESSION: SessionSummary = {
  session_id: 'sess-1',
  agent_name: 'reviewer',
  model: 'opus',
  status: 'running',
  total_turns: 12,
  created_at: '2026-06-01T00:00:00Z',
  updated_at: '2026-06-01T00:05:00Z',
  scope: 'project',
  active_run_id: 'run-12',
};

/** Build N runs (oldest→newest): run-1 … run-N, each with a unique prompt. */
function makeRuns(n: number): SessionRunRow[] {
  return Array.from({ length: n }, (_, i) => {
    const k = i + 1;
    return {
      run_id: `run-${k}`,
      prompt: `prompt number ${k}`,
      transcript_path: `/t/run-${k}.jsonl`,
      status: k === n ? null : 'ok', // newest is in flight
      tokens_in: 0,
      tokens_out: 0,
      tokens_cached: 0,
      duration_ms: 0,
    };
  });
}

function panels(): HTMLElement[] {
  return screen.queryAllByTestId('transcript-panel');
}

describe('SessionConversation', () => {
  it('shows the newest 10 turns with a "Load older" button, then reveals the rest', () => {
    render(<SessionConversation session={SESSION} runs={makeRuns(12)} />);

    // Only the most recent 10 turns render initially.
    expect(panels()).toHaveLength(10);
    // The two oldest (run-1, run-2) are hidden; the newest (run-12) is shown.
    expect(screen.queryByText('prompt number 1')).not.toBeInTheDocument();
    expect(screen.getByText('prompt number 3')).toBeInTheDocument();
    expect(screen.getByText('prompt number 12')).toBeInTheDocument();

    // Click "Load older turns" → all 12 turns visible.
    fireEvent.click(screen.getByRole('button', { name: 'Load older turns' }));
    expect(panels()).toHaveLength(12);
    expect(screen.getByText('prompt number 1')).toBeInTheDocument();
    // Button disappears once nothing older remains.
    expect(screen.queryByRole('button', { name: 'Load older turns' })).not.toBeInTheDocument();
  });

  it('renders a "You" bubble with the prompt and streams the active run live', () => {
    render(<SessionConversation session={SESSION} runs={makeRuns(3)} />);

    // User bubble carries the operator's prompt text.
    expect(screen.getByText('prompt number 3')).toBeInTheDocument();
    expect(screen.getAllByText('You').length).toBe(3);

    // run-12 is the active_run_id but isn't in this 3-run set; instead the
    // newest run (run-3) has null status → live=true. run-1/run-2 are terminal.
    const byPath = (p: string) => panels().find((el) => el.dataset.path === p)!;
    expect(byPath('/t/run-3.jsonl').dataset.live).toBe('true');
    expect(byPath('/t/run-1.jsonl').dataset.live).toBe('false');
  });

  it('marks the active_run_id turn as live even when its status is terminal', () => {
    // run-2 is the active run despite an "ok" status.
    const runs = makeRuns(3);
    const session: SessionSummary = { ...SESSION, active_run_id: 'run-2' };
    render(<SessionConversation session={session} runs={runs} />);

    const byPath = (p: string) => panels().find((el) => el.dataset.path === p)!;
    expect(byPath('/t/run-2.jsonl').dataset.live).toBe('true');
  });

  it('shows an empty state when there are no turns', () => {
    render(<SessionConversation session={SESSION} runs={[]} />);
    expect(screen.getByText(/No messages yet/)).toBeInTheDocument();
    expect(panels()).toHaveLength(0);
  });

  it('renders a per-turn error line when a run has an error field', () => {
    const runs: SessionRunRow[] = [
      {
        run_id: 'run-1',
        prompt: 'do something',
        transcript_path: '/t/run-1.jsonl',
        status: 'error',
        error: 'provider: API error 401',
        tokens_in: 0,
        tokens_out: 0,
        tokens_cached: 0,
        duration_ms: 0,
      },
    ];
    render(<SessionConversation session={SESSION} runs={runs} />);
    expect(screen.getByText('provider: API error 401')).toBeInTheDocument();
  });

  it('passes onComplete to the active turn panel and not to terminal turns', () => {
    const onTurnComplete = vi.fn();
    const runs = makeRuns(2); // run-2 has null status → active
    render(<SessionConversation session={SESSION} runs={runs} onTurnComplete={onTurnComplete} />);

    const byPath = (p: string) => panels().find((el) => el.dataset.path === p)!;
    // Active turn (null status) gets onComplete wired up.
    expect(byPath('/t/run-2.jsonl').dataset.hasOnComplete).toBe('true');
    // Terminal turn does not.
    expect(byPath('/t/run-1.jsonl').dataset.hasOnComplete).toBe('false');
  });
});
