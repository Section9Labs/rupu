// @vitest-environment jsdom
// FanoutDrill — unit-row click fires onSelectUnit with correct {path,live,label}.

import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import FanoutDrill from './FanoutDrill';
import type { UnitView } from '../lib/runGraphModel';

afterEach(cleanup);

const UNITS: UnitView[] = [
  { index: 0, key: 'item-a', state: 'done', transcriptPath: '/runs/r1/units/0.jsonl' },
  { index: 1, key: 'item-b', state: 'running', transcriptPath: '/runs/r1/units/1.jsonl' },
  { index: 2, key: 'item-c', state: 'failed' },
];

describe('FanoutDrill', () => {
  it('calls onSelectUnit with path+live+label when a unit row is clicked', () => {
    const onSelectUnit = vi.fn();
    const onClose = vi.fn();

    render(
      <FanoutDrill
        stepId="process_items"
        units={UNITS}
        onClose={onClose}
        onSelectUnit={onSelectUnit}
      />,
    );

    // Click the first unit row (done, has transcript)
    const rowA = screen.getByTitle('item-a').closest('button')!;
    fireEvent.click(rowA);
    expect(onSelectUnit).toHaveBeenCalledTimes(1);
    expect(onSelectUnit).toHaveBeenCalledWith({
      path: '/runs/r1/units/0.jsonl',
      live: false,
      label: 'item-a',
    });

    // Click the running unit row — live should be true
    const rowB = screen.getByTitle('item-b').closest('button')!;
    fireEvent.click(rowB);
    expect(onSelectUnit).toHaveBeenCalledTimes(2);
    expect(onSelectUnit).toHaveBeenLastCalledWith({
      path: '/runs/r1/units/1.jsonl',
      live: true,
      label: 'item-b',
    });

    // Click the failed unit with no transcriptPath — path should be null
    const rowC = screen.getByTitle('item-c').closest('button')!;
    fireEvent.click(rowC);
    expect(onSelectUnit).toHaveBeenCalledTimes(3);
    expect(onSelectUnit).toHaveBeenLastCalledWith({
      path: null,
      live: false,
      label: 'item-c',
    });
  });

  it('does not call onClose when a unit row is clicked', () => {
    const onSelectUnit = vi.fn();
    const onClose = vi.fn();

    render(
      <FanoutDrill units={UNITS} onClose={onClose} onSelectUnit={onSelectUnit} />,
    );

    const rowA = screen.getByTitle('item-a').closest('button')!;
    fireEvent.click(rowA);
    expect(onClose).not.toHaveBeenCalled();
  });
});
