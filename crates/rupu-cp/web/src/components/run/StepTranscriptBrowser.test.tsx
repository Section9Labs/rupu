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

  it('shows the unit codename with provider/model next to its key', () => {
    const units: UnitView[] = [
      { ...UNITS[0], codename: 'jade-reef/heron#0', provider: 'anthropic', model: 'claude-opus-5-5' },
      { ...UNITS[1], codename: 'jade-reef/heron#1' },
    ];
    render(<StepTranscriptBrowser stepId="process_items" units={units} agent="scanner" />);
    expect(screen.getByText('heron#0 · scanner · anthropic/claude-opus-5-5')).toBeTruthy();
    // placed unit: provider/model absent → just leaf · agent
    expect(screen.getByText('heron#1 · scanner')).toBeTruthy();
    // key stays visible for rows
    expect(screen.getByTitle('item-a')).toBeTruthy();
  });

  it('orders a named row identity-first, item key second (spec §9)', () => {
    const units: UnitView[] = [{ ...UNITS[0], codename: 'jade-reef/heron#0' }];
    render(<StepTranscriptBrowser stepId="process_items" units={units} agent="scanner" />);
    const ident = screen.getByTestId('unit-identity');
    const key = screen.getByTestId('unit-key');
    // DOCUMENT_POSITION_FOLLOWING: key comes after identity in the row
    expect(ident.compareDocumentPosition(key) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(key.textContent).toBe('item-a');
  });

  it('renders a derived unit name muted', () => {
    const units: UnitView[] = [
      { ...UNITS[0], codename: 'jade-reef/heron#1', codenameDerived: true },
      { ...UNITS[1], codename: 'jade-reef/heron#2' },
    ];
    render(<StepTranscriptBrowser stepId="process_items" units={units} agent="scanner" />);
    const derived = screen.getByText('heron#1 · scanner').closest('[title]') as HTMLElement;
    expect(derived.className).toMatch(/opacity-60/);
    expect(derived.getAttribute('title')).toMatch(/derived for a run recorded before codenames/);
    const stored = screen.getByText('heron#2 · scanner').closest('[title]') as HTMLElement;
    expect(stored.className).not.toMatch(/opacity-60/);
  });
});
