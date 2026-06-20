import { describe, it, expect } from 'vitest';
import { buildTurnSeries } from './turnSeries';
import type { TranscriptEvent } from '../../lib/transcript';

describe('buildTurnSeries', () => {
  it('maps usage events to ordered per-turn points', () => {
    const events = [
      { type: 'run_start', data: {} },
      { type: 'usage', data: { input_tokens: 1000, output_tokens: 200, cached_tokens: 0 } },
      { type: 'usage', data: { input_tokens: 800, output_tokens: 150, cached_tokens: 50 } },
    ] as unknown as TranscriptEvent[];
    expect(buildTurnSeries(events)).toEqual([
      { turn: 1, tokens_in: 1000, tokens_out: 200, tokens_cached: 0 },
      { turn: 2, tokens_in: 800, tokens_out: 150, tokens_cached: 50 },
    ]);
  });
  it('returns [] when no usage events', () => {
    expect(buildTurnSeries([{ type: 'run_start', data: {} }] as unknown as TranscriptEvent[])).toEqual([]);
  });
});
