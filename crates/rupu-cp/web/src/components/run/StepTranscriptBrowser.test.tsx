// @vitest-environment jsdom
// StepTranscriptBrowser — units listed left, selected unit's transcript right.
// TranscriptPanel is mocked to surface its received `path` as a test marker.

import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import StepTranscriptBrowser from './StepTranscriptBrowser';
import type { UnitView } from '../../lib/runGraphModel';

// Capture TranscriptPanel's props by rendering them as a marker element.
vi.mock('../TranscriptPanel', () => ({
  default: ({ path, live }: { path: string; live: boolean }) => (
    <div data-testid="transcript-panel" data-path={path} data-live={String(live)} />
  ),
}));

afterEach(cleanup);

const UNITS: UnitView[] = [
  { index: 0, key: 'item-a', state: 'done', transcriptPath: '/runs/r1/units/0.jsonl' },
  { index: 1, key: 'item-b', state: 'running', transcriptPath: '/runs/r1/units/1.jsonl' },
  { index: 2, key: 'item-c', state: 'failed', transcriptPath: '/runs/r1/units/2.jsonl' },
  { index: 3, key: 'item-d', state: 'failed', transcriptPath: '/runs/r1/units/3.jsonl' },
];

describe('StepTranscriptBrowser', () => {
  it('lists all units on the left', () => {
    render(<StepTranscriptBrowser stepId="process_items" units={UNITS} />);
    for (const u of UNITS) {
      expect(screen.getByTitle(u.key)).toBeTruthy();
    }
  });

  it('auto-selects the first unit and shows its transcript on the right', () => {
    render(<StepTranscriptBrowser stepId="process_items" units={UNITS} />);
    const panel = screen.getByTestId('transcript-panel');
    expect(panel.getAttribute('data-path')).toBe('/runs/r1/units/0.jsonl');
  });

  it('renders the selected unit transcript when a unit row is clicked', () => {
    render(<StepTranscriptBrowser stepId="process_items" units={UNITS} />);

    fireEvent.click(screen.getByTitle('item-b').closest('button')!);
    let panel = screen.getByTestId('transcript-panel');
    expect(panel.getAttribute('data-path')).toBe('/runs/r1/units/1.jsonl');
    // running unit ⇒ live tail
    expect(panel.getAttribute('data-live')).toBe('true');

    fireEvent.click(screen.getByTitle('item-c').closest('button')!);
    panel = screen.getByTestId('transcript-panel');
    expect(panel.getAttribute('data-path')).toBe('/runs/r1/units/2.jsonl');
    expect(panel.getAttribute('data-live')).toBe('false');
  });

  it('narrows the left list to failed units when the failed pill is clicked', () => {
    render(<StepTranscriptBrowser stepId="process_items" units={UNITS} />);

    fireEvent.click(screen.getByText('failed (2)'));

    // failed units remain
    expect(screen.getByTitle('item-c')).toBeTruthy();
    expect(screen.getByTitle('item-d')).toBeTruthy();
    // non-failed units are filtered out
    expect(screen.queryByTitle('item-a')).toBeNull();
    expect(screen.queryByTitle('item-b')).toBeNull();
  });

  it('re-selects the first visible unit after a filter removes the selection', () => {
    render(<StepTranscriptBrowser stepId="process_items" units={UNITS} />);
    // first selection is item-a (done, /0.jsonl); filtering to failed removes it
    fireEvent.click(screen.getByText('failed (2)'));
    const panel = screen.getByTestId('transcript-panel');
    expect(panel.getAttribute('data-path')).toBe('/runs/r1/units/2.jsonl');
  });
});
