import { describe, expect, it } from 'vitest';
import { emptySlice, rowAccess, type HostSlice } from './types';
import { displacedCount, floorOf, watermarkMerge } from './watermarkMerge';

interface Row { id: string; started_at: string; host_id?: string }
const access = rowAccess<Row>('started_at', 'id');
const T0 = Date.parse('2026-09-30T12:00:00Z');
const at = (minAgo: number) => new Date(T0 - minAgo * 60_000).toISOString();

function slice(hostId: string, mins: number[], over: Partial<HostSlice<Row>> = {}): HostSlice<Row> {
  return {
    ...emptySlice<Row>({ id: hostId, name: hostId, transport_kind: 'ssh' }),
    state: 'ok',
    rows: mins.map((m, i) => ({ id: `${hostId}-${i}`, started_at: at(m), host_id: hostId })),
    hasMore: false,
    ...over,
  };
}

describe('watermarkMerge', () => {
  it('orders by parsed instant across Z / +00:00 / fractional-second stamps', () => {
    const a = slice('a', []);
    a.rows = [{ id: 'z', started_at: '2026-09-30T11:00:00Z', host_id: 'a' }];
    const b = slice('b', []);
    b.rows = [
      { id: 'offset', started_at: '2026-09-30T11:30:00+00:00', host_id: 'b' },
      { id: 'frac', started_at: '2026-09-30T10:59:59.500Z', host_id: 'b' },
    ];
    expect(watermarkMerge([a, b], access).visible.map((r) => r.id)).toEqual(['offset', 'z', 'frac']);
  });

  it('sorts an unparseable stamp last', () => {
    const a = slice('a', [5]);
    a.rows.push({ id: 'bad', started_at: 'not-a-date', host_id: 'a' });
    expect(watermarkMerge([a], access).visible.map((r) => r.id)).toEqual(['a-0', 'bad']);
  });

  it('holds back rows older than the newest covered point of a host that still has more', () => {
    const local = slice('local', [1, 2, 3, 4], { hasMore: false });
    const remote = slice('remote', [0, 2.5], { hasMore: true }); // covered at 2.5 min ago
    const m = watermarkMerge([local, remote], access);
    expect(m.floor).toBe(T0 - 2.5 * 60_000);
    expect(m.visible.map((r) => r.id)).toEqual(['remote-0', 'local-0', 'local-1', 'remote-1']);
    expect(m.hasMore).toBe(true);
    expect(m.gatingHosts).toEqual(['remote']);
  });

  it('does not let loading, catching-up, offline or paging-failed hosts gate the floor', () => {
    const local = slice('local', [1, 9], { hasMore: true }); // covered at 9
    const gates = (over: Partial<HostSlice<Row>>) =>
      floorOf([local, slice('r', [0], { hasMore: true, ...over })], access.timeOf);
    expect(gates({ state: 'loading' })).toBe(T0 - 9 * 60_000);
    expect(gates({ catchingUp: true })).toBe(T0 - 9 * 60_000);
    expect(gates({ state: 'offline' })).toBe(T0 - 9 * 60_000);
    expect(gates({ pagingFailed: true })).toBe(T0 - 9 * 60_000);
    expect(gates({})).toBe(T0); // the healthy remote gates at its own covered point
  });

  it('de-duplicates by (host_id, id), the row\'s own tag winning over the slice', () => {
    const a = slice('local', [1]);
    const b = slice('host_prod', []);
    b.rows = [{ id: 'local-0', started_at: at(1), host_id: 'local' }]; // a mock answering with local rows
    expect(watermarkMerge([a, b], access).visible).toHaveLength(1);
  });

  it('is ended only when nothing gates and no host is loading or catching up', () => {
    expect(watermarkMerge([slice('a', [1])], access).ended).toBe(true);
    expect(watermarkMerge([slice('a', [1]), slice('b', [], { state: 'loading' })], access).ended).toBe(false);
    expect(watermarkMerge([slice('a', [1]), slice('b', [2], { catchingUp: true, hasMore: true })], access).ended).toBe(false);
    expect(watermarkMerge([slice('a', [1], { hasMore: true })], access).ended).toBe(false);
    expect(watermarkMerge([], access).ended).toBe(false);
  });

  it('counts the visible rows a host would push below the floor', () => {
    const visible = slice('local', [1, 2, 3, 4]).rows;
    expect(displacedCount(visible, T0 - 2.5 * 60_000, access.timeOf)).toBe(2);
    expect(displacedCount(visible, -Infinity, access.timeOf)).toBe(0);
  });
});
