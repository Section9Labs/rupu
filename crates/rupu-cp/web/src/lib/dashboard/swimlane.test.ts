import { describe, it, expect } from 'vitest';
import { autoFitRange, assignLanes } from './swimlane';
import type { ActiveRunBar } from '../api';

const NOW = Date.parse('2026-07-16T12:00:00Z');

const bar = (over: Partial<ActiveRunBar> = {}): ActiveRunBar => ({
  run_id: 'r1',
  workflow_name: 'wf',
  status: 'running',
  started_at: '2026-07-16T11:55:00Z',
  trigger: 'manual',
  cycle_id: null,
  ...over,
});

describe('autoFitRange', () => {
  it('fits to the 5th/95th percentile, not min/max', () => {
    // Nineteen ~5-minute runs and one 6-hour outlier. Fitting to min/max would
    // crush the cluster into ~1% of the width.
    const bars = [
      ...Array.from({ length: 19 }, (_, i) =>
        bar({ run_id: `r${i}`, started_at: '2026-07-16T11:55:00Z' }),
      ),
      bar({ run_id: 'outlier', started_at: '2026-07-16T06:00:00Z' }),
    ];
    const { start, end } = autoFitRange(bars, NOW);
    const spanMinutes = (end - start) / 60_000;
    expect(spanMinutes).toBeLessThan(120);
  });

  it('always ends at now — the right edge is the present', () => {
    const { end } = autoFitRange([bar()], NOW);
    expect(end).toBe(NOW);
  });

  it('handles an empty bar list without producing NaN', () => {
    const { start, end } = autoFitRange([], NOW);
    expect(Number.isFinite(start)).toBe(true);
    expect(Number.isFinite(end)).toBe(true);
    expect(end).toBeGreaterThan(start);
  });
});

describe('assignLanes', () => {
  it('groups by workflow', () => {
    const lanes = assignLanes(
      [bar({ workflow_name: 'a' }), bar({ workflow_name: 'b' }), bar({ workflow_name: 'a' })],
      'workflow',
      NOW,
    );
    expect(lanes).toHaveLength(2);
    expect(lanes.find((l) => l.key === 'a')!.bars).toHaveLength(2);
  });

  it('groups by host', () => {
    const lanes = assignLanes(
      [bar({ host_id: 'local' }), bar({ host_id: 'builder-01' })],
      'host',
      NOW,
    );
    expect(lanes.map((l) => l.key).sort()).toEqual(['builder-01', 'local']);
  });

  it('positions bars as 0..1 fractions of the fitted range', () => {
    const lanes = assignLanes([bar()], 'workflow', NOW);
    const b = lanes[0].bars[0];
    expect(b.x0).toBeGreaterThanOrEqual(0);
    expect(b.x1).toBeLessThanOrEqual(1);
    expect(b.x1).toBeGreaterThan(b.x0);
  });

  it('clamps a bar that started before the fitted window to x0 = 0', () => {
    // The outlier is fitted out of the window but must still be drawn —
    // clipped to the left edge, not dropped or given a negative x.
    const bars = [
      ...Array.from({ length: 19 }, (_, i) => bar({ run_id: `r${i}` })),
      bar({ run_id: 'outlier', started_at: '2026-07-16T06:00:00Z' }),
    ];
    const lanes = assignLanes(bars, 'workflow', NOW);
    const outlier = lanes.flatMap((l) => l.bars).find((b) => b.bar.run_id === 'outlier')!;
    expect(outlier.x0).toBe(0);
  });
});
