// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';

vi.mock('../TranscriptPanel', () => ({
  default: ({ path, calls }: { path: string; calls?: [number, number] }) => (
    <div data-testid="panel">
      {path}:{calls?.join('-')}
    </div>
  ),
}));

import WindowTranscripts, { transcriptLabel } from './WindowTranscripts';
import TurnWindowBar from './TurnWindowBar';
import { turnWindow } from '../../lib/run/turnWindow';
import type { GraphNode, RunGraphModel } from '../../lib/runGraphModel';
import type { UsageTimelinePoint } from '../../lib/api';

afterEach(cleanup);

const nodes = [
  { id: 'plan', kind: 'step', state: 'done', transcriptPath: '/t/plan.jsonl' },
  {
    id: 'assess',
    kind: 'for_each',
    state: 'running',
    fanout: {
      total: 2,
      byState: {},
      units: [
        { index: 0, key: 'a', state: 'done', transcriptPath: '/t/u0.jsonl', codename: 'jade-reef/lynx#1' },
        { index: 1, key: 'b', state: 'running', transcriptPath: '/t/u1.jsonl' },
      ],
    },
  },
] as unknown as GraphNode[];
const model = { nodes, edges: [], nodeById: (id: string) => nodes.find((n) => n.id === id) } as RunGraphModel;

const pt = (turn: number, label: string, transcript: string, transcript_turn: number, s: string): UsageTimelinePoint => ({
  turn,
  label,
  tokens_in: 1,
  tokens_out: 1,
  tokens_cached: 0,
  transcript,
  transcript_turn,
  at: `2026-10-21T10:00:${s}Z`,
});
const POINTS = [
  pt(1, 'plan', '/t/plan.jsonl', 1, '00'),
  pt(2, 'assess', '/t/u0.jsonl', 1, '05'),
  pt(3, 'assess', '/t/u1.jsonl', 1, '07'),
  pt(4, 'assess', '/t/u0.jsonl', 2, '09'),
];

describe('transcriptLabel', () => {
  it('names a step transcript by its step and a unit by step · unit', () => {
    expect(transcriptLabel(model, '/t/plan.jsonl')).toBe('plan');
    expect(transcriptLabel(model, '/t/u0.jsonl')).toBe('assess · lynx#1');
    expect(transcriptLabel(model, '/t/u1.jsonl')).toBe('assess · b');
    expect(transcriptLabel(model, '/elsewhere/run_9.jsonl')).toBe('run_9');
  });
});

describe('WindowTranscripts', () => {
  it('offers every transcript in the window and shows the chosen one narrowed to its calls', () => {
    const w = turnWindow(POINTS, 1, 3)!;
    render(<WindowTranscripts window={w} model={model} preferred={null} live={false} runId="run_1" />);
    const tabs = screen.getAllByRole('tab');
    expect(tabs.map((t) => t.textContent)).toEqual(['assess · lynx#1calls 1–2', 'assess · bcall 1']);
    expect(screen.getByTestId('panel')).toHaveTextContent('/t/u0.jsonl:1-2');
    fireEvent.click(tabs[1]);
    expect(screen.getByTestId('panel')).toHaveTextContent('/t/u1.jsonl:1-1');
  });

  it('opens on the page transcript when the window has it', () => {
    const w = turnWindow(POINTS, 1, 3)!;
    render(<WindowTranscripts window={w} model={model} preferred="/t/u1.jsonl" live={false} runId="run_1" />);
    expect(screen.getByTestId('panel')).toHaveTextContent('/t/u1.jsonl:1-1');
  });

  it('says so when the control plane gives no transcript per turn', () => {
    const bare = POINTS.map(({ transcript: _t, transcript_turn: _n, ...rest }) => rest);
    render(<WindowTranscripts window={turnWindow(bare, 0, 3)!} model={model} preferred={null} live={false} runId="run_1" />);
    expect(screen.getByText(/doesn.t say which transcript each turn is in/)).toBeInTheDocument();
    expect(screen.queryByTestId('panel')).toBeNull();
  });
});

describe('TurnWindowBar', () => {
  it('says what the window covers and clears it', () => {
    const onClear = vi.fn();
    render(<TurnWindowBar window={turnWindow(POINTS, 1, 3)!} onClear={onClear} />);
    expect(screen.getByTestId('turn-window')).toHaveTextContent('Turns 2–4');
    expect(screen.getByTestId('turn-window')).toHaveTextContent('step assess');
    fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(onClear).toHaveBeenCalled();
  });

  it('says how each tab follows the window, and when no times were recorded', () => {
    render(<TurnWindowBar window={turnWindow(POINTS, 1, 3)!} onClear={() => {}} />);
    expect(screen.getByTestId('turn-window-scope')).toHaveTextContent(
      'transcript: these turns · events: these steps · findings, messages: this time',
    );
    cleanup();
    const untimed = POINTS.map(({ at: _a, ...rest }) => rest);
    render(<TurnWindowBar window={turnWindow(untimed, 0, 1)!} onClear={() => {}} />);
    expect(screen.getByTestId('turn-window-scope')).toHaveTextContent(/^no times recorded for these turns/);
  });
});
