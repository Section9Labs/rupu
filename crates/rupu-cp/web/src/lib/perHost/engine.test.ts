import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '../api';
import { FETCH_TIMEOUT_MS, PerHostListEngine, type PerHostFetchParams } from './engine';
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

/** Like `pager`, but re-reads the server's rows on every call (the list changes under paging). */
const pager0 = (all: () => Row[]) => (p: PerHostFetchParams) => pager(all())(p);

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

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
    expect(fetch.mock.calls[0][0]).toMatchObject({ host: 'local', offset: 15, limit: 25 });
  });

  it('a late host catches up instead of shrinking the visible list', async () => {
    const local = rows('local', 120, 1);
    const remote = rows('remote', 400, 0.1); // busy: 10 rows a minute
    const remoteGate = deferred<void>();
    const { engine, fetch, history } = harness(async (p) => {
      if (p.host === 'remote') await remoteGate.promise;
      return pager(p.host === 'local' ? local : remote)(p);
    });
    engine.start([LOCAL, REMOTE]);
    await flush();
    await engine.loadMore();
    await engine.loadMore();
    const before = watermarkMerge(engine.current, access).visible.length;
    expect(before).toBe(60); // three local pages are on screen when the remote finally answers

    remoteGate.resolve();
    for (let i = 0; i < 10; i++) await flush();
    const counts = history.map((h) => watermarkMerge(h, access).visible.length);
    const arrival = history.findIndex((h) => h.some((s) => s.hostId === 'remote' && s.state === 'ok'));
    expect(arrival).toBeGreaterThanOrEqual(0);
    for (const n of counts.slice(arrival)) expect(n).toBeGreaterThanOrEqual(before);
    expect(engine.current.find((s) => s.hostId === 'remote')?.catchingUp).toBe(false);
    // Catch-up is a real batch: more rows than one ordinary next page (PAGE + OVERLAP = 25).
    expect(fetch.mock.calls.some((c) => c[0].host === 'remote' && c[0].limit > 25)).toBe(true);
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
    const { engine, fetch, history } = harness(() => d.promise);
    engine.start([LOCAL]);
    await flush(); // the request is really in flight now
    expect(fetch).toHaveBeenCalledTimes(1);
    const n = history.length;
    engine.dispose();
    d.resolve(rows('local', 1, 1));
    await flush();
    expect(history.length).toBe(n);
    expect(engine.current[0].state).toBe('loading');
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

  it('a page of nothing new after rows land on top re-anchors instead of ending the host', async () => {
    let server = rows('local', 40, 1); // two pages' worth
    const { engine } = harness(pager0(() => server));
    engine.start([LOCAL]);
    await flush();
    await engine.loadMore();
    expect(engine.current[0].rows).toHaveLength(40);

    // 50 runs land on top: the next offsets now point at rows this slice already holds.
    server = [...rows('new', 50, 1, -200), ...server];
    await engine.loadMore();
    await engine.loadMore();
    const s = engine.current[0];
    expect(s.pagingFailed).toBe(false);
    expect(s.hasMore).toBe(true);
    expect(s.rows.some((r) => r.id === 'new-0')).toBe(true);

    for (let i = 0; i < 20 && watermarkMerge(engine.current, access).hasMore; i++) await engine.loadMore();
    const done = engine.current[0];
    expect(done.hasMore).toBe(false);
    expect(done.rows.map((r) => r.id).sort()).toEqual(server.map((r) => r.id).sort());
  });

  it('a page that keeps adding nothing is a paging failure, not a silent end', async () => {
    const local = rows('local', 40, 1);
    let stuck = false;
    const { engine, fetch } = harness((p) => {
      // Stuck: every later page is a FULL page of rows the slice already holds.
      if (stuck && p.offset > 0) return Promise.resolve(rows('local', p.limit, 1));
      return pager(local)(p);
    });
    engine.start([LOCAL]);
    await flush();
    await engine.loadMore();
    expect(engine.current[0].rows).toHaveLength(40);
    stuck = true;
    fetch.mockClear();
    await engine.loadMore();
    // page (nothing new) -> re-anchor head -> page again (still nothing new) -> give up.
    expect(fetch.mock.calls.map((c) => c[0].offset)).toEqual([35, 0, 35]);
    expect(engine.current[0]).toMatchObject({ pagingFailed: true, hasMore: true, reason: 'list shifted while paging' });
  });

  it('retryPaging is a no-op unless the host failed paging', async () => {
    const { engine, fetch } = harness(pager0(() => rows('local', 60, 1)));
    engine.start([LOCAL]);
    await flush();
    fetch.mockClear();
    await engine.retryPaging('local');
    expect(fetch).not.toHaveBeenCalled();
  });

  it('retryPaging re-anchors page 0 before it resumes', async () => {
    let server = rows('local', 60, 1);
    let failNext = true;
    const { engine, fetch } = harness((p) => {
      if (p.offset > 0 && failNext) return Promise.reject(new ApiError(502, 'x', '{"error":"down"}'));
      return pager(server)(p);
    });
    engine.start([LOCAL]);
    await flush();
    await engine.loadMore();
    expect(engine.current[0].pagingFailed).toBe(true);
    failNext = false;
    server = [...rows('new', 2, 1, -10), ...server];
    fetch.mockClear();
    await engine.retryPaging('local');
    expect(fetch.mock.calls[0][0]).toMatchObject({ host: 'local', offset: 0 });
    expect(engine.current[0].rows.some((r) => r.id === 'new-0')).toBe(true);
    expect(engine.current[0]).toMatchObject({ pagingFailed: false, reason: null, catchingUp: false });
  });

  it('a refresh queued behind an in-flight page splices against the post-page rows', async () => {
    let server = rows('local', 60, 1);
    const gate = deferred<void>();
    const log: string[] = [];
    const { engine } = harness(async (p) => {
      log.push(`start:${p.offset}`);
      const page = server.slice(p.offset, p.offset + p.limit); // what the host said when asked
      if (p.offset > 0) await gate.promise;
      log.push(`end:${p.offset}`);
      return page;
    });
    engine.start([LOCAL]);
    await flush();
    const more = engine.loadMore();
    await flush(); // the page request is in flight
    server = [...rows('new', 2, 1, -5), ...server];
    void engine.scheduleHead('local'); // queued behind the page
    gate.resolve();
    await more;
    for (let i = 0; i < 3; i++) await flush();

    expect(log.join()).toBe('start:0,end:0,start:15,end:15,start:0,end:0');
    const ids = engine.current[0].rows.map((r) => r.id);
    expect(ids).toEqual(['new-0', 'new-1', ...Array.from({ length: 40 }, (_, i) => `local-${i}`)]);
  });

  it('a fetch that never answers times out: offline, and other hosts are unaffected', async () => {
    expect(FETCH_TIMEOUT_MS).toBe(45_000);
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const local = rows('local', 60, 1);
    const { engine, visible } = harness((p) => (p.host === 'hang' ? new Promise<Row[]>(() => {}) : pager(local)(p)));
    engine.start([LOCAL, { ...REMOTE, id: 'hang' }]);
    await vi.advanceTimersByTimeAsync(0);
    expect(engine.current.find((s) => s.hostId === 'hang')?.state).toBe('loading');
    expect(visible()).toHaveLength(20);

    await vi.advanceTimersByTimeAsync(45_000);
    expect(engine.current.find((s) => s.hostId === 'hang')).toMatchObject({ state: 'offline', reason: 'no answer after 45s' });
    expect(visible()).toHaveLength(20);
    await engine.loadMore(); // not held up by the dead host
    expect(visible()).toHaveLength(40);
  });

  it('a hung page times out as a paging failure instead of freezing loadMore', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const local = rows('local', 60, 1);
    const { engine } = harness((p) => (p.offset > 0 ? new Promise<Row[]>(() => {}) : pager(local)(p)));
    engine.start([LOCAL]);
    await vi.advanceTimersByTimeAsync(0);
    const more = engine.loadMore();
    await vi.advanceTimersByTimeAsync(45_000);
    await more;
    expect(engine.current[0]).toMatchObject({ state: 'ok', pagingFailed: true, reason: 'no answer after 45s' });
    expect(watermarkMerge(engine.current, access).hasMore).toBe(false);
  });

  it('a hung refresh keeps last-good rows and records why', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    let hang = false;
    const { engine, visible } = harness(() => (hang ? new Promise<Row[]>(() => {}) : Promise.resolve(rows('local', 3, 1))));
    engine.start([LOCAL]);
    await vi.advanceTimersByTimeAsync(0);
    hang = true;
    const refresh = engine.scheduleHead('local');
    await vi.advanceTimersByTimeAsync(45_000);
    await refresh;
    expect(visible()).toHaveLength(3);
    expect(engine.current[0]).toMatchObject({ state: 'ok', reason: 'no answer after 45s' });
  });

  it('an unexpected job error is logged and cannot leave a host catching up forever', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {});
    let boom = false;
    const throwing = {
      ...access,
      keyOf: (r: Row, h: string) => {
        if (boom) throw new Error('boom');
        return access.keyOf(r, h);
      },
    };
    const local = rows('local', 60, 1);
    const remote = rows('remote', 200, 0.1);
    const remoteGate = deferred<void>();
    const pageGate = deferred<void>();
    const fetch = vi.fn(async (p: PerHostFetchParams) => {
      if (p.host === 'remote') {
        await remoteGate.promise;
        if (p.offset > 0) await pageGate.promise;
      }
      return pager(p.host === 'local' ? local : remote)(p);
    });
    const engine = new PerHostListEngine<Row>(fetch, throwing, () => {}, true);
    engine.start([LOCAL, REMOTE]);
    await flush();
    remoteGate.resolve();
    for (let i = 0; i < 5; i++) await flush();
    // The remote answered and is catching up: its next page is in flight.
    expect(engine.current.find((s) => s.hostId === 'remote')?.catchingUp).toBe(true);
    expect(fetch.mock.calls.some((c) => c[0].host === 'remote' && c[0].offset > 0)).toBe(true);

    boom = true;
    pageGate.resolve();
    for (let i = 0; i < 5; i++) await flush();
    expect(err).toHaveBeenCalled();
    expect(engine.current.find((s) => s.hostId === 'remote')?.catchingUp).toBe(false);
  });

  it('reconcile keeps rows and state but takes the new name and transport', async () => {
    const { engine } = harness((p) => Promise.resolve(rows(p.host, 3, 1)));
    engine.start([LOCAL, REMOTE, { ...REMOTE, id: 'gone' }]);
    await flush();
    engine.reconcile([LOCAL, { ...REMOTE, name: 'renamed', transport_kind: 'http_cp' }, { ...REMOTE, id: 'added' }]);
    const by = (id: string) => engine.current.find((s) => s.hostId === id);
    expect(by('gone')).toBeUndefined();
    expect(by('added')?.state).toBe('loading');
    expect(by('remote')).toMatchObject({ name: 'renamed', transportKind: 'http_cp', state: 'ok' });
    expect(by('remote')?.rows).toHaveLength(3);
  });

  it('retry stays out of gating while it re-anchors, so the visible list never shrinks', async () => {
    const local = rows('local', 60, 1); // one a minute: a page covers 19 min
    const remote = rows('remote', 200, 5); // one per 5 min: a page covers 95 min
    let failNext = true;
    let headGate: Promise<void> | null = null;
    const { engine, history, visible } = harness(async (p) => {
      if (p.host === 'local') {
        if (p.offset > 0 && failNext) throw new ApiError(502, 'x', '{"error":"down"}');
        if (p.offset === 0 && headGate) await headGate;
      }
      return pager(p.host === 'local' ? local : remote)(p);
    });
    engine.start([LOCAL, REMOTE]);
    await flush();
    await engine.loadMore(); // local sits at the floor and fails: it stops gating
    expect(engine.current.find((s) => s.hostId === 'local')?.pagingFailed).toBe(true);
    const before = visible().length;
    expect(before).toBe(40); // local's 20 + remote's 20: the floor is remote's deeper coverage

    failNext = false;
    const gate = deferred<void>();
    headGate = gate.promise;
    const base = history.length;
    const retry = engine.retryPaging('local');
    await flush(); // the re-anchor page 0 is in flight
    expect(engine.current.find((s) => s.hostId === 'local')).toMatchObject({ pagingFailed: false, catchingUp: true });
    expect(visible().length).toBeGreaterThanOrEqual(before); // NOT gating at local's shallow coverage

    gate.resolve();
    await retry;
    for (const h of history.slice(base)) expect(watermarkMerge(h, access).visible.length).toBeGreaterThanOrEqual(before);
    expect(engine.current.find((s) => s.hostId === 'local')).toMatchObject({ pagingFailed: false, catchingUp: false });
  });

  it('a failed re-anchor ends the page with the head\'s own reason and makes no third request', async () => {
    let server = rows('local', 40, 1);
    let headDown = false;
    const { engine, fetch } = harness((p) => {
      if (p.offset === 0 && headDown) return Promise.reject(new ApiError(502, 'x', '{"error":"down"}'));
      return pager(server)(p);
    });
    engine.start([LOCAL]);
    await flush();
    await engine.loadMore();
    expect(engine.current[0].rows).toHaveLength(40);

    // 25 runs land on top: the next window falls entirely on rows this slice already holds.
    server = [...rows('new', 25, 1, -100), ...server];
    headDown = true;
    fetch.mockClear();
    await engine.loadMore();
    expect(fetch.mock.calls.map((c) => c[0].offset)).toEqual([35, 0]);
    expect(engine.current[0]).toMatchObject({ pagingFailed: true, catchingUp: false, hasMore: true, reason: 'down' });
  });

  it('a timed-out fetch is aborted, and the AbortError it then rejects with changes nothing', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const signals: (AbortSignal | undefined)[] = [];
    const { engine } = harness(
      (p) =>
        new Promise<Row[]>((_, reject) => {
          signals.push(p.signal);
          p.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
        }),
    );
    engine.start([LOCAL]);
    await vi.advanceTimersByTimeAsync(0);
    expect(signals).toHaveLength(1);
    expect(signals[0]?.aborted).toBe(false);

    await vi.advanceTimersByTimeAsync(45_000);
    expect(signals[0]?.aborted).toBe(true);
    expect(engine.current[0]).toMatchObject({ state: 'offline', reason: 'no answer after 45s' });
  });

  it('dispose aborts the fetches still in flight, and only those', async () => {
    const signals: Record<string, AbortSignal | undefined> = {};
    const hang = deferred<Row[]>();
    const { engine, history } = harness((p) => {
      signals[p.host] = p.signal;
      return p.host === 'done' ? Promise.resolve(rows('done', 2, 1)) : hang.promise;
    });
    engine.start([LOCAL, { ...REMOTE, id: 'done' }]);
    await flush();
    expect(signals.local?.aborted).toBe(false);
    expect(signals.done?.aborted).toBe(false); // a settled fetch is never aborted later
    const n = history.length;

    engine.dispose();
    expect(signals.local?.aborted).toBe(true);
    expect(signals.done?.aborted).toBe(false);
    hang.reject(new DOMException('aborted', 'AbortError'));
    await flush();
    expect(history.length).toBe(n);
  });
});
