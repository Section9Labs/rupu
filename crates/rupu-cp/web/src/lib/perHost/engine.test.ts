import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '../api';
import { PerHostListEngine, type PerHostFetchParams } from './engine';
import { rowAccess, type HostSlice } from './types';
import { watermarkMerge } from './watermarkMerge';
import { deferred, flush } from './testUtils';

interface Row { id: string; started_at: string; host_id?: string }
const access = rowAccess<Row>('started_at', 'id');
const T0 = Date.parse('2026-09-30T12:00:00Z');
const at = (minAgo: number) => new Date(T0 - minAgo * 60_000).toISOString();
/** `n` rows `${host}-0…`, `step` minutes apart, the first `from` minutes ago. */
function rows(host: string, n: number, step: number, from = 0): Row[] {
  return Array.from({ length: n }, (_, i) => ({ id: `${host}-${i}`, host_id: host, started_at: at(from + i * step) }));
}
const LOCAL = { id: 'local', name: 'Local', transport_kind: 'local' };
const REMOTE = { id: 'remote', name: 'remote', transport_kind: 'ssh' };

type Script = (p: PerHostFetchParams) => Promise<Row[]>;
function harness(script: Script) {
  const history: HostSlice<Row>[][] = [];
  const fetch = vi.fn(script);
  const engine = new PerHostListEngine<Row>(fetch, access, (s) => history.push(s), true);
  const visible = () => watermarkMerge(engine.current, access).visible;
  return { engine, fetch, history, visible };
}
/** Serve `all` (newest first) by offset/limit, like a real host. */
const pager = (all: Row[]) => (p: PerHostFetchParams) => Promise.resolve(all.slice(p.offset, p.offset + p.limit));

afterEach(() => vi.restoreAllMocks());

describe('PerHostListEngine', () => {
  it('paints local while a remote hangs, then merges the remote in', async () => {
    const remote = deferred<Row[]>();
    const { engine, visible } = harness((p) => (p.host === 'local' ? Promise.resolve(rows('local', 3, 10)) : remote.promise));
    engine.start([LOCAL, REMOTE]);
    await flush();
    expect(visible().map((r) => r.id)).toEqual(['local-0', 'local-1', 'local-2']);
    expect(engine.current.find((s) => s.hostId === 'remote')?.state).toBe('loading');

    remote.resolve(rows('remote', 2, 10, 5));
    await flush();
    expect(visible().map((r) => r.id)).toEqual(['local-0', 'remote-0', 'local-1', 'remote-1', 'local-2']);
  });

  it('classifies failures: 502 offline, 501 unavailable, 404 removes the host', async () => {
    const fail = (status: number) => Promise.reject(new ApiError(status, 'x', `{"error":"e${status}"}`));
    const { engine } = harness((p) =>
      p.host === 'a' ? fail(502) : p.host === 'b' ? fail(501) : p.host === 'c' ? fail(404) : Promise.resolve([]),
    );
    engine.start([LOCAL, { ...REMOTE, id: 'a' }, { ...REMOTE, id: 'b' }, { ...REMOTE, id: 'c' }]);
    await flush();
    const by = (id: string) => engine.current.find((s) => s.hostId === id);
    expect(by('a')).toMatchObject({ state: 'offline', reason: 'e502' });
    expect(by('b')).toMatchObject({ state: 'unavailable', reason: 'e501' });
    expect(by('c')).toBeUndefined();
  });

  it('keeps last-good rows when a refresh fails (stale-on-error)', async () => {
    let fail = false;
    const { engine, visible } = harness(() => (fail ? Promise.reject(new ApiError(502, 'x', '{"error":"down"}')) : Promise.resolve(rows('local', 2, 1))));
    engine.start([LOCAL]);
    await flush();
    fail = true;
    await engine.scheduleHead('local');
    expect(visible()).toHaveLength(2);
    expect(engine.current[0]).toMatchObject({ state: 'ok', reason: 'down' });
  });

  it('loads the next page only from the host sitting at the floor', async () => {
    const local = rows('local', 60, 1); // one per minute
    const remote = rows('remote', 60, 10); // one per 10 minutes
    const { engine, fetch } = harness((p) => pager(p.host === 'local' ? local : remote)(p));
    engine.start([LOCAL, REMOTE]);
    await flush();
    fetch.mockClear();
    await engine.loadMore();
    // local covers 19 min, remote 190 min → the floor is local's; only local pages.
    expect(fetch.mock.calls.map((c) => c[0].host)).toEqual(['local']);
    expect(fetch.mock.calls[0][0]).toEqual({ host: 'local', offset: 15, limit: 25 });
  });

  it('a late host catches up instead of shrinking the visible list', async () => {
    const local = rows('local', 60, 1);
    const remote = rows('remote', 200, 0.1); // busy: 10 rows a minute
    const remoteGate = deferred<void>();
    const { engine, history } = harness(async (p) => {
      if (p.host === 'remote') await remoteGate.promise;
      return pager(p.host === 'local' ? local : remote)(p);
    });
    engine.start([LOCAL, REMOTE]);
    await flush();
    const before = watermarkMerge(engine.current, access).visible.length;
    expect(before).toBe(20);

    remoteGate.resolve();
    for (let i = 0; i < 10; i++) await flush();
    const counts = history.map((h) => watermarkMerge(h, access).visible.length);
    const afterArrival = counts.slice(counts.findIndex((_, i) => history[i].some((s) => s.hostId === 'remote' && s.state === 'ok')));
    for (const n of afterArrival) expect(n).toBeGreaterThanOrEqual(before);
    expect(engine.current.find((s) => s.hostId === 'remote')?.catchingUp).toBe(false);
  });

  it('never has more than one request in flight per host, and coalesces refreshes', async () => {
    let inflight = 0;
    let peak = 0;
    const gate = deferred<void>();
    const { engine, fetch } = harness(async () => {
      inflight++;
      peak = Math.max(peak, inflight);
      await gate.promise;
      inflight--;
      return rows('local', 3, 1);
    });
    engine.start([LOCAL]);
    await flush(); // page 0 is now in flight (a schedule BEFORE it starts would coalesce into it)
    void engine.scheduleHead('local'); // queued behind it
    void engine.scheduleHead('local'); // coalesced into the queued one
    gate.resolve();
    for (let i = 0; i < 5; i++) await flush();
    expect(peak).toBe(1);
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it('a paging failure stops the host gating; retry clears it', async () => {
    const local = rows('local', 60, 1);
    let failNext = true;
    const { engine } = harness((p) => {
      if (p.offset > 0 && failNext) return Promise.reject(new ApiError(502, 'x', '{"error":"down"}'));
      return pager(local)(p);
    });
    engine.start([LOCAL]);
    await flush();
    await engine.loadMore();
    expect(engine.current[0]).toMatchObject({ pagingFailed: true, reason: 'down' });
    expect(watermarkMerge(engine.current, access).hasMore).toBe(false);
    failNext = false;
    await engine.retryPaging('local');
    expect(engine.current[0].pagingFailed).toBe(false);
  });

  it('removeRow drops one row from its host', async () => {
    const { engine, visible } = harness(() => Promise.resolve(rows('local', 3, 1)));
    engine.start([LOCAL]);
    await flush();
    engine.removeRow('local', 'local-1');
    expect(visible().map((r) => r.id)).toEqual(['local-0', 'local-2']);
  });

  it('ignores answers after dispose', async () => {
    const d = deferred<Row[]>();
    const { engine, history } = harness(() => d.promise);
    engine.start([LOCAL]);
    const n = history.length;
    engine.dispose();
    d.resolve(rows('local', 1, 1));
    await flush();
    expect(history.length).toBe(n);
  });

  it('pollLocal refreshes only local; pollRemote skips unavailable hosts', async () => {
    const { engine, fetch } = harness((p) =>
      p.host === 'old' ? Promise.reject(new ApiError(501, 'x', '{"error":"old"}')) : Promise.resolve([]),
    );
    engine.start([LOCAL, REMOTE, { ...REMOTE, id: 'old' }]);
    await flush();
    fetch.mockClear();
    engine.pollLocal();
    await flush();
    expect(fetch.mock.calls.map((c) => c[0].host)).toEqual(['local']);
    fetch.mockClear();
    engine.pollRemote();
    await flush();
    expect(fetch.mock.calls.map((c) => c[0].host)).toEqual(['remote']);
  });

  it('a fully listed host refreshed with a full non-overlapping page 0 is replaced and has more below', async () => {
    let answer: Row[] = rows('local', 3, 1); // short page 0: the host's whole list, hasMore false
    const { engine } = harness(() => Promise.resolve(answer));
    engine.start([LOCAL]);
    await flush();
    expect(engine.current[0]).toMatchObject({ state: 'ok', hasMore: false });

    // 20 rows all newer than anything held: page 0 does not overlap the old rows.
    answer = rows('local', 20, 1, -100).map((r) => ({ ...r, id: `new-${r.id}` }));
    await engine.scheduleHead('local');
    expect(engine.current[0].hasMore).toBe(true);
    expect(engine.current[0].rows.map((r) => r.id)).toEqual(answer.map((r) => r.id));
  });
});
