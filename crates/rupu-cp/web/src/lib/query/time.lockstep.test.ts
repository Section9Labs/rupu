// The TypeScript half of the time-value lockstep: the fixture rupu-query's
// `time::lockstep` test runs. Fix the parser, never the fixture, unless the
// spec changed.
import { describe, expect, it } from 'vitest';
import { admitsSince, admitsUntil, normalizeTime, parseRfc3339, parseTimeValue } from './time';
import fixture from '../../../../../rupu-query/tests/fixtures/time_values.json';

describe('time value lockstep', () => {
  const now = parseRfc3339(fixture.now)!;

  it.each(fixture.values.map((c) => [c.v, c] as const))('%j', (_v, c) => {
    const n = normalizeTime(c.v);
    expect(n.ok ? n.value : null).toBe(c.normalized);
  });

  it.each(fixture.bounds.map((c) => [`${c.v} vs ${c.ts}`, c] as const))('%s', (_n, c) => {
    const t = parseTimeValue(c.v)!;
    const ts = parseRfc3339(c.ts);
    const got = ts === null ? [false, false] : [admitsSince(t, ts, now), admitsUntil(t, ts, now)];
    expect(got).toEqual([c.since, c.until]);
  });
});
