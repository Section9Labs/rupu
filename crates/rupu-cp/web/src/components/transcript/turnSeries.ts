import type { TranscriptEvent } from '../../lib/transcript';

export interface TurnUsagePoint {
  turn: number;
  tokens_in: number;
  tokens_out: number;
  tokens_cached: number;
}

/** Read a numeric field off an event `data` bag, defaulting to 0.
 *  `TranscriptEvent` has a `string`-typed catch-all member, so narrowing on
 *  `type === 'usage'` widens the field values to `unknown`; guard explicitly. */
function num(data: Record<string, unknown>, key: string): number {
  const v = data[key];
  return typeof v === 'number' ? v : 0;
}

/** Build an ordered per-turn token series from a transcript's events.
 *  One `usage` event = one turn; x-axis is the 1-based turn index. */
export function buildTurnSeries(events: TranscriptEvent[]): TurnUsagePoint[] {
  const out: TurnUsagePoint[] = [];
  for (const ev of events) {
    if (ev.type === 'usage') {
      const d = ev.data;
      out.push({
        turn: out.length + 1,
        tokens_in: num(d, 'input_tokens'),
        tokens_out: num(d, 'output_tokens'),
        tokens_cached: num(d, 'cached_tokens'),
      });
    }
  }
  return out;
}
