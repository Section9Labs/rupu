import { describe, expect, it } from 'vitest';
import { rowAccess } from './types';
import { appendPage, spliceHead, withHost } from './spliceHead';

interface Row { id: string; started_at: string; status?: string; host_id?: string }
const access = rowAccess<Row>('started_at', 'id');
const T0 = Date.parse('2026-09-30T12:00:00Z');
const r = (id: string, minAgo: number, status = 'running'): Row => ({
  id, status, host_id: 'h', started_at: new Date(T0 - minAgo * 60_000).toISOString(),
});
/** r1..rN, one minute apart, newest first. */
const seq = (from: number, to: number) => Array.from({ length: to - from + 1 }, (_, i) => r(`r${from + i}`, from + i));
const ids = (rows: Row[]) => rows.map((x) => x.id);

describe('spliceHead', () => {
  it('keeps the boundary row when a new run arrives at the top (usePagedList dropped it)', () => {
    const old = seq(1, 40);
    const fresh = [r('n1', 0), ...seq(1, 19)]; // page 0 now ends at r19
    const { rows, fullyListed, replaced } = spliceHead(old, fresh, 20, access, 'h');
    expect(fullyListed).toBe(false);
    expect(replaced).toBe(false);
    expect(ids(rows)).toEqual(['n1', ...ids(seq(1, 40))]);
  });

  it('drops a run that left the list instead of duplicating its neighbour', () => {
    const old = seq(1, 25);
    const fresh = seq(1, 21).filter((x) => x.id !== 'r3'); // r3 finished
    const { rows } = spliceHead(old, fresh, 20, access, 'h');
    expect(ids(rows)).toEqual(ids(seq(1, 25).filter((x) => x.id !== 'r3')));
    expect(new Set(ids(rows)).size).toBe(rows.length);
  });

  it('takes the fresh copy of a row present in both (status update lands)', () => {
    const old = seq(1, 20);
    const fresh = seq(1, 20).map((x) => (x.id === 'r2' ? { ...x, status: 'completed' } : x));
    expect(spliceHead(old, fresh, 20, access, 'h').rows.find((x) => x.id === 'r2')?.status).toBe('completed');
  });

  it('replaces the slice when page 0 is short (the host is fully listed)', () => {
    const { rows, fullyListed } = spliceHead(seq(1, 30), seq(1, 7), 20, access, 'h');
    expect(fullyListed).toBe(true);
    expect(ids(rows)).toEqual(ids(seq(1, 7)));
  });

  it('keeps a timestamp tie at the cutoff', () => {
    const tie = { ...r('tie', 20) };
    const old = [...seq(1, 20), tie];
    const fresh = seq(1, 20); // cutoff = r20's time = tie's time
    const result = spliceHead(old, fresh, 20, access, 'h');
    expect(ids(result.rows)).toContain('tie');
    expect(ids(result.rows)[ids(result.rows).length - 1]).toBe('tie'); // 'tie' is the last (oldest) row
  });

  it('replaces the slice when fresh does not overlap old, marking replaced: true', () => {
    const old = seq(30, 49); // 30..49 minutes ago
    const fresh = seq(1, 20); // 1..20 minutes ago (no overlap)
    const result = spliceHead(old, fresh, 20, access, 'h');
    expect(ids(result.rows)).toEqual(ids(seq(1, 20)));
    expect(result.fullyListed).toBe(false);
    expect(result.replaced).toBe(true);
  });
});

describe('appendPage / withHost', () => {
  it('appends with de-dup, the page copy winning', () => {
    const old = seq(1, 5);
    const page = [...seq(4, 8)].map((x) => (x.id === 'r4' ? { ...x, status: 'completed' } : x));
    const rows = appendPage(old, page, access, 'h');
    expect(ids(rows)).toEqual(ids(seq(1, 8)));
    expect(rows.find((x) => x.id === 'r4')?.status).toBe('completed');
  });

  it('fills host_id only when the server did not tag the row', () => {
    const rows = withHost<Row>([{ id: 'a', started_at: '' }, { id: 'b', started_at: '', host_id: 'x' }], 'h');
    expect(rows.map((x) => x.host_id)).toEqual(['h', 'x']);
  });

  it('deduplicates a page containing the same key twice, keeping the first copy', () => {
    const page = [
      r('x', 1, 'running'),
      r('x', 2, 'completed'), // duplicate key with different time and status
    ];
    const result = spliceHead([], page, 20, access, 'h');
    expect(ids(result.rows)).toEqual(['x']);
    expect(result.rows[0].status).toBe('running'); // first copy wins
  });
});
