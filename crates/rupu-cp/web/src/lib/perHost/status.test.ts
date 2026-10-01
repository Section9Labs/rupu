import { describe, expect, it } from 'vitest';
import { ApiError } from '../api';
import { emptySlice, type HostSlice } from './types';
import { classifyFailure, notIncluded, pagingFailedHosts, toFreshnessEntries, waitingLabel } from './status';

const s = (id: string, over: Partial<HostSlice<unknown>> = {}): HostSlice<unknown> => ({
  ...emptySlice({ id, name: id, transport_kind: 'ssh' }),
  ...over,
});

describe('classifyFailure', () => {
  it('maps 501 → unavailable, 404 → gone, anything else → offline, with the server reason', () => {
    expect(classifyFailure(new ApiError(501, 'x', '{"error":"needs a newer rupu"}'))).toEqual({
      kind: 'unavailable',
      reason: 'needs a newer rupu',
    });
    expect(classifyFailure(new ApiError(404, 'x', '{"error":"host gone"}')).kind).toBe('gone');
    expect(classifyFailure(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}'))).toEqual({
      kind: 'offline',
      reason: 'host unreachable: timed out',
    });
    expect(classifyFailure(new TypeError('Failed to fetch')).kind).toBe('offline');
  });
});

describe('labels', () => {
  it('names hosts still loading or catching up', () => {
    expect(waitingLabel([s('local', { state: 'ok' }), s('mini'), s('kuki', { state: 'ok', catchingUp: true })])).toBe(
      'Waiting on mini, kuki…',
    );
    expect(waitingLabel([s('local', { state: 'ok' })])).toBeNull();
  });

  it('names hosts not included and why', () => {
    expect(notIncluded([s('a', { state: 'offline' }), s('b', { state: 'unavailable' }), s('c', { state: 'ok' })])).toBe(
      'a (offline), b (unavailable)',
    );
    expect(notIncluded([s('c', { state: 'ok' })])).toBeNull();
  });

  it('lists paging failures', () => {
    expect(pagingFailedHosts([s('a', { state: 'ok', pagingFailed: true }), s('b', { state: 'ok' })])).toEqual([
      { hostId: 'a', name: 'a' },
    ]);
  });
});

describe('toFreshnessEntries', () => {
  it('shows catching-up as loading and stamps ok hosts with their receipt time', () => {
    const [a, b, c] = toFreshnessEntries([
      s('a', { state: 'ok', receivedAt: Date.parse('2026-09-30T12:00:00Z') }),
      s('b', { state: 'ok', catchingUp: true, receivedAt: 1 }),
      s('c', { state: 'offline', reason: 'down' }),
    ]);
    expect(a).toMatchObject({ host_id: 'a', state: 'ok', captured_at: '2026-09-30T12:00:00.000Z' });
    expect(b.state).toBe('loading');
    expect(c).toMatchObject({ state: 'offline', captured_at: null, reason: 'down' });
  });
});
