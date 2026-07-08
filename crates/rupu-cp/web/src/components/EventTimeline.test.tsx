// @vitest-environment jsdom
// EventTimeline — the grouped, filterable global Live Events feed.
//
// Covers the pure grouping/filter helpers directly (fast, no DOM), plus a
// rendering pass that asserts: a group row collapses repeated same-(type,
// run) events, lifecycle events never group, the filter narrows visible
// rows, and each row links to /runs/:runId.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import EventTimeline, { groupEvents, matchesFilter, tsOf } from './EventTimeline';
import { type SeqEvent } from './RunEventFeed';
import type { RunEvent, StepStartedEvent, RunStartedEvent, StepFailedEvent } from '../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function seq(seqId: number, ev: RunEvent, ts: number): SeqEvent {
  return { seq: seqId, event: { ...ev, ts } as RunEvent };
}

function stepStarted(runId: string, stepId: string): StepStartedEvent {
  return { type: 'step_started', run_id: runId, step_id: stepId, kind: 'linear', agent: null };
}
function runStarted(runId: string): RunStartedEvent {
  return { type: 'run_started', run_id: runId, event_version: 1, workflow_path: 'wf.yaml', started_at: 'x' };
}
function stepFailed(runId: string, stepId: string): StepFailedEvent {
  return { type: 'step_failed', run_id: runId, step_id: stepId, error: 'boom' };
}

describe('groupEvents — pure grouping', () => {
  it('collapses repeated same-(type, run) events within the rolling window', () => {
    const events: SeqEvent[] = [
      seq(3, stepStarted('run_a', 'c'), 3_000),
      seq(2, stepStarted('run_a', 'b'), 2_000),
      seq(1, stepStarted('run_a', 'a'), 1_000),
    ];
    const rows = groupEvents(events, 20_000);
    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe('group');
    if (rows[0].kind === 'group') {
      expect(rows[0].count).toBe(3);
      expect(rows[0].type).toBe('step_started');
    }
  });

  it('never groups lifecycle events even when repeated back-to-back', () => {
    const events: SeqEvent[] = [
      seq(2, runStarted('run_a'), 2_000),
      seq(1, runStarted('run_b'), 1_000),
    ];
    const rows = groupEvents(events, 20_000);
    expect(rows).toHaveLength(2);
    expect(rows.every((r) => r.kind === 'single')).toBe(true);
  });

  it('never groups error events (step_failed)', () => {
    const events: SeqEvent[] = [
      seq(2, stepFailed('run_a', 'x'), 2_000),
      seq(1, stepFailed('run_a', 'x'), 1_000),
    ];
    const rows = groupEvents(events, 20_000);
    expect(rows).toHaveLength(2);
  });

  it('does not group same-type events from different runs', () => {
    const events: SeqEvent[] = [
      seq(2, stepStarted('run_b', 'x'), 2_000),
      seq(1, stepStarted('run_a', 'x'), 1_000),
    ];
    const rows = groupEvents(events, 20_000);
    expect(rows).toHaveLength(2);
  });

  it('splits into separate groups once the gap exceeds the window', () => {
    const events: SeqEvent[] = [
      seq(2, stepStarted('run_a', 'y'), 100_000),
      seq(1, stepStarted('run_a', 'x'), 1_000),
    ];
    const rows = groupEvents(events, 20_000);
    expect(rows).toHaveLength(2);
  });

  it('a lone occurrence renders as a single row, not a group of 1', () => {
    const events: SeqEvent[] = [seq(1, stepStarted('run_a', 'x'), 1_000)];
    const rows = groupEvents(events, 20_000);
    expect(rows).toEqual([{ kind: 'single', item: events[0] }]);
  });
});

describe('tsOf', () => {
  it('reads the stamped ts field', () => {
    expect(tsOf(seq(1, stepStarted('r', 's'), 1234))).toBe(1234);
  });
  it('falls back to 0 when ts is missing', () => {
    expect(tsOf({ seq: 1, event: stepStarted('r', 's') })).toBe(0);
  });
});

describe('matchesFilter', () => {
  it('matches on run_id', () => {
    expect(matchesFilter(stepStarted('run_abc', 'build'), 'run_abc')).toBe(true);
    expect(matchesFilter(stepStarted('run_abc', 'build'), 'run_xyz')).toBe(false);
  });
  it('matches on event type', () => {
    expect(matchesFilter(stepStarted('r', 's'), 'step_started')).toBe(true);
  });
  it('is case-insensitive', () => {
    expect(matchesFilter(stepStarted('RUN_ABC', 's'), 'run_abc')).toBe(true);
  });
  it('an empty needle matches everything', () => {
    expect(matchesFilter(stepStarted('r', 's'), '')).toBe(true);
  });
});

function renderTimeline(events: SeqEvent[]) {
  return render(
    <MemoryRouter>
      <EventTimeline
        events={events}
        liveIDs={new Set()}
        hasMoreOlder={false}
        loadingOlder={false}
        onLoadOlder={vi.fn()}
      />
    </MemoryRouter>,
  );
}

describe('EventTimeline — rendering', () => {
  it('renders historical rows (not the empty state) when events are present', () => {
    renderTimeline([seq(1, runStarted('run_1'), Date.now())]);
    expect(screen.queryByText(/No events yet/)).not.toBeInTheDocument();
    expect(screen.getByText('Run started')).toBeInTheDocument();
  });

  it('shows the empty state when there truly are no events', () => {
    renderTimeline([]);
    expect(screen.getByText(/No events yet/)).toBeInTheDocument();
  });

  it('renders a collapsed group row for repeated same-(type, run) events', () => {
    const now = Date.now();
    renderTimeline([
      seq(3, stepStarted('run_a', 'c'), now),
      seq(2, stepStarted('run_a', 'b'), now - 1000),
      seq(1, stepStarted('run_a', 'a'), now - 2000),
    ]);
    expect(screen.getByText('×3')).toBeInTheDocument();
  });

  it('expanding a group reveals its individual members', () => {
    const now = Date.now();
    renderTimeline([
      seq(3, stepStarted('run_a', 'c'), now),
      seq(2, stepStarted('run_a', 'b'), now - 1000),
      seq(1, stepStarted('run_a', 'a'), now - 2000),
    ]);
    // Collapsed: only the newest member ("c", its step_id is the row title)
    // is visible; the other two members are folded into the group row.
    expect(screen.getByText('c')).toBeInTheDocument();
    expect(screen.queryByText('a')).not.toBeInTheDocument();
    expect(screen.queryByText('b')).not.toBeInTheDocument();

    fireEvent.click(screen.getByText('×3').closest('[role="button"]')!);

    expect(screen.getByText('a')).toBeInTheDocument();
    expect(screen.getByText('b')).toBeInTheDocument();
  });

  it('the filter narrows visible rows', () => {
    renderTimeline([
      seq(1, runStarted('run_a'), Date.now()),
      seq(2, runStarted('run_b'), Date.now() - 1000),
    ]);
    // Both run_started rows visible initially.
    expect(screen.getAllByText('Run started')).toHaveLength(2);

    fireEvent.change(screen.getByPlaceholderText('Filter events…'), { target: { value: 'run_a' } });
    expect(screen.getAllByText('Run started')).toHaveLength(1);
  });

  it('each row links to /runs/:runId', () => {
    renderTimeline([seq(1, runStarted('run_xyz'), Date.now())]);
    const link = screen.getByTitle('Open run run_xyz');
    expect(link).toHaveAttribute('href', '/runs/run_xyz');
  });

  it('calls onLoadOlder when the "Load older events" control is clicked', () => {
    const onLoadOlder = vi.fn();
    render(
      <MemoryRouter>
        <EventTimeline
          events={[seq(1, runStarted('run_a'), Date.now())]}
          liveIDs={new Set()}
          hasMoreOlder
          loadingOlder={false}
          onLoadOlder={onLoadOlder}
        />
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByText('Load older events'));
    expect(onLoadOlder).toHaveBeenCalled();
  });
});
