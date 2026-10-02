// PerHostListEngine — per-host list state outside React (spec §6.3).
//
// - Serializes work per host: at most one request in flight per host, with
//   a refresh that comes due while one is in flight coalesced behind it.
// - A host that answers late (first page, or recovering) JOINS by catching
//   up to the floor before it gates, so the visible list never gets shorter.
// - Splices refreshes with spliceHead and pages with a 5-row overlap. A full
//   page that adds nothing means rows landed on top and shifted every offset:
//   the host re-anchors on a fresh page 0 and retries once, never silently
//   ending its list.
// - Bounds every fetch (FETCH_TIMEOUT_MS), so one host that never answers
//   cannot hold its own queue, or loadMore's scroll lock, forever. A fetch the
//   engine stops waiting for (timeout, dispose) is ABORTED through the signal
//   it was handed, so abandoned requests do not pile up in the browser's
//   per-origin connection pool.
//
// The React hook (usePerHostPagedList.ts) owns one engine per filter
// generation and disposes it on change. A disposed engine ignores every
// late answer.

import { apiErrorMessage } from '../api';
import { emptySlice, type HostSeed, type HostSlice, type HostTagged, type RowAccess } from './types';
import { coveredOf, displacedCount, floorOf, isGating, watermarkMerge } from './watermarkMerge';
import { appendPage, sortSlice, spliceHead, withHost } from './spliceHead';
import { classifyFailure } from './status';

export const PAGE = 20;
export const OVERLAP = 5;
export const MAX_LIMIT = 200;
/** A host that has not answered by then is treated as failed (offline / paging failure). */
export const FETCH_TIMEOUT_MS = 45_000;

export interface PerHostFetchParams {
  host: string;
  offset: number;
  limit: number;
  /** Aborted when the engine stops waiting (timeout or dispose); pass it to fetch. */
  signal?: AbortSignal;
}

/** What `head()` reports to a caller that needs to know whether it got an answer. */
/** `joined`: head() already ran the catch-up (a first answer, or a replaced slice). */
type HeadResult = { ok: true; joined: boolean } | { ok: false; reason: string };

export class PerHostListEngine<T extends HostTagged> {
  private slices: HostSlice<T>[] = [];
  private readonly queues = new Map<string, Promise<void>>();
  private readonly headQueued = new Set<string>();
  /** One controller per fetch still awaited; `dispose()` aborts them all. */
  private readonly inflight = new Set<AbortController>();
  /**
   * Catch-ups running per host. A re-anchor inside a catch-up can run one of its own (a replaced
   * page 0); only the outermost may end the host's `catchingUp`.
   */
  private readonly joins = new Map<string, number>();
  private disposed = false;

  constructor(
    private readonly fetchPage: (p: PerHostFetchParams) => Promise<T[]>,
    private readonly access: RowAccess<T>,
    private readonly onChange: (slices: HostSlice<T>[]) => void,
    /** Listing every registered host (vs one picked host). Only then does a 404 drop a host. */
    private readonly allHosts: boolean,
  ) {}

  get current(): HostSlice<T>[] {
    return this.slices;
  }

  dispose(): void {
    this.disposed = true;
    for (const c of [...this.inflight]) c.abort();
    this.inflight.clear();
  }

  /** Seed one `loading` slice per host and fire every host's page 0 independently. */
  start(hosts: readonly HostSeed[]): void {
    this.commit(hosts.map((h) => emptySlice<T>(h)));
    for (const h of hosts) void this.scheduleHead(h.id);
  }

  /** Manual Refresh: add new hosts, drop removed ones, refresh every host (unavailable ones included). */
  reconcile(hosts: readonly HostSeed[]): void {
    const byId = new Map(this.slices.map((s) => [s.hostId, s]));
    this.commit(
      hosts.map((h) => {
        const kept = byId.get(h.id);
        return kept ? { ...kept, name: h.name, transportKind: h.transport_kind } : emptySlice<T>(h);
      }),
    );
    for (const h of hosts) void this.scheduleHead(h.id);
  }

  /** Coalesced page-0 load: a first load, a refresh or a recovery. */
  scheduleHead(id: string): Promise<void> {
    if (this.headQueued.has(id)) return Promise.resolve();
    this.headQueued.add(id);
    return this.enqueue(id, async () => {
      this.headQueued.delete(id);
      await this.head(id);
    });
  }

  refreshHost(id: string): void {
    void this.scheduleHead(id);
  }

  /** Scroll: next page from each host sitting at the floor. */
  loadMore(): Promise<void> {
    const { gatingHosts } = watermarkMerge(this.slices, this.access);
    return Promise.all(
      gatingHosts.map((id) =>
        this.enqueue(id, async () => {
          const s = this.find(id);
          if (s && isGating(s)) await this.next(id, PAGE);
        }),
      ),
    ).then(() => undefined);
  }

  /**
   * Resume a host whose paging failed: re-anchor on a fresh page 0 (the list
   * may have shifted since), then catch up again. A no-op for any other host.
   */
  retryPaging(id: string): Promise<void> {
    if (!this.find(id)?.pagingFailed) return Promise.resolve();
    return this.enqueue(id, async () => {
      if (!this.find(id)?.pagingFailed) return;
      // Stay out of gating (catchingUp) while re-anchoring: gating again at this host's
      // old, shallower coverage would lift the floor and hide rows right where Retry was clicked.
      this.patch(id, (s) => ({ ...s, pagingFailed: false, reason: null, catchingUp: s.hasMore }));
      const head = await this.head(id);
      if (!head.ok) {
        // The re-anchor itself failed: still paging-failed, now for the head's own reason. Do not
        // spend a catch-up request on a host that could not even answer page 0.
        this.patch(id, (s) => ({ ...s, pagingFailed: true, catchingUp: false, reason: head.reason }));
        return;
      }
      if (!head.joined) await this.join(id);
    });
  }

  /** A row action (archive/restore/delete) took this row out of the list. */
  removeRow(hostId: string, rowId: string): void {
    this.patch(hostId, (s) => ({ ...s, rows: s.rows.filter((r) => this.access.idOf(r) !== rowId) }));
  }

  /** The local cadence (5 s on polling tables). */
  pollLocal(): void {
    for (const s of this.slices) if (s.hostId === 'local' && s.state !== 'unavailable') void this.scheduleHead(s.hostId);
  }

  /** The remote cadence (60 s visible, and on focus). Unavailable hosts wait for a manual Refresh. */
  pollRemote(): void {
    for (const s of this.slices) if (s.hostId !== 'local' && s.state !== 'unavailable') void this.scheduleHead(s.hostId);
  }

  // ── internals ──────────────────────────────────────────────────────────

  private commit(next: HostSlice<T>[]): void {
    if (this.disposed) return;
    this.slices = next;
    this.onChange(next);
  }

  private find(id: string): HostSlice<T> | undefined {
    return this.slices.find((s) => s.hostId === id);
  }

  private patch(id: string, f: (s: HostSlice<T>) => HostSlice<T>): void {
    this.commit(this.slices.map((s) => (s.hostId === id ? f(s) : s)));
  }

  private enqueue(id: string, job: () => Promise<void>): Promise<void> {
    const prev = this.queues.get(id) ?? Promise.resolve();
    const next = prev
      .then(() => (this.disposed ? undefined : job()))
      .catch((e) => {
        // Fetch failures are handled inside the jobs; this is a bug. Keep the queue alive, but say so.
        console.error('PerHostListEngine: unexpected job error', e);
      });
    this.queues.set(id, next);
    return next;
  }

  /**
   * One fetch, bounded: rejects with a plain Error if the host has not answered in
   * FETCH_TIMEOUT_MS, and aborts the request then (the timeout error is rejected
   * first, so the AbortError the fetch answers with afterwards loses the race).
   */
  private async fetchBounded(p: Omit<PerHostFetchParams, 'signal'>): Promise<T[]> {
    const controller = new AbortController();
    this.inflight.add(controller);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        reject(new Error(`no answer after ${FETCH_TIMEOUT_MS / 1000}s`));
        controller.abort();
      }, FETCH_TIMEOUT_MS);
    });
    try {
      return await Promise.race([this.fetchPage({ ...p, signal: controller.signal }), timeout]);
    } finally {
      clearTimeout(timer);
      this.inflight.delete(controller);
    }
  }

  /** Load page 0. `ok: false` carries the failure reason; a disposed engine or a vanished host is `ok`. */
  private async head(id: string): Promise<HeadResult> {
    if (this.disposed || !this.find(id)) return { ok: true, joined: false };
    let page: T[];
    try {
      page = withHost(await this.fetchBounded({ host: id, offset: 0, limit: PAGE }), id);
    } catch (e) {
      return { ok: false, reason: this.fail(id, e) };
    }
    const cur = this.find(id);
    if (this.disposed || !cur) return { ok: true, joined: false };
    if (cur.state !== 'ok') {
      // First answer or recovery: hold it out of gating until join() decides.
      const hasMore = page.length === PAGE;
      this.patch(id, (s) => ({
        ...s,
        state: 'ok',
        rows: sortSlice(page, this.access, id),
        hasMore,
        catchingUp: hasMore,
        pagingFailed: false,
        reason: null,
        receivedAt: Date.now(),
      }));
      await this.join(id);
      return { ok: true, joined: true };
    }
    const { rows, fullyListed, replaced } = spliceHead(cur.rows, page, PAGE, this.access, id);
    this.patch(id, (s) => ({
      ...s,
      rows,
      hasMore: fullyListed ? false : replaced ? true : s.hasMore,
      // A replaced slice is shallower than before; it stays out of gating until join() below has
      // paged it back down. Gating at page 0's coverage would lift the floor and hold back every
      // other host's older rows.
      catchingUp: replaced ? true : s.catchingUp,
      reason: null,
      receivedAt: Date.now(),
    }));
    // Page 0 did not reach the old rows, so the slice just lost every row below it. Page back down
    // at least that far (in single-host mode there is no other host to displace, so the lost count
    // is the whole budget): the list must not get shorter under the reader (spec §6.3).
    if (replaced) await this.join(id, cur.rows.length - rows.length);
    return { ok: true, joined: replaced };
  }

  /**
   * Bring a host into the merge without shortening the visible list: fetch
   * at least as many rows as it would push below the floor (the budget, and
   * never less than `minBudget`), stopping early once it reaches the floor or
   * runs out of rows. Then it gates.
   */
  private async join(id: string, minBudget = 0): Promise<void> {
    const start = this.find(id);
    if (!start) return;
    const others = () => this.slices.filter((o) => o.hostId !== id);
    this.joins.set(id, (this.joins.get(id) ?? 0) + 1);
    try {
      this.patch(id, (s) => ({ ...s, catchingUp: s.hasMore }));
      const displaced = displacedCount(
        watermarkMerge(others(), this.access).visible,
        coveredOf(start, this.access.timeOf),
        this.access.timeOf,
      );
      const budget = Math.max(displaced, minBudget);
      let added = 0;
      for (;;) {
        const s = this.find(id);
        if (this.disposed || !s) return;
        const floor = floorOf(others(), this.access.timeOf);
        if (!s.hasMore || coveredOf(s, this.access.timeOf) <= floor || added >= budget) break;
        const got = await this.next(id, Math.min(MAX_LIMIT - OVERLAP, Math.max(PAGE, budget - added)));
        if (got === null) return; // next() marked pagingFailed and cleared catchingUp
        if (got === 0) break;
        added += got;
      }
    } finally {
      const depth = (this.joins.get(id) ?? 1) - 1;
      if (depth > 0) this.joins.set(id, depth);
      else this.joins.delete(id);
      // Whatever happened (even an unexpected throw), never leave a host catching up forever.
      if (depth === 0 && this.find(id)?.catchingUp) this.patch(id, (s) => ({ ...s, catchingUp: false }));
    }
  }

  /**
   * Fetch `want` more rows (+ overlap). Returns the count of NEW rows, or null on failure.
   *
   * A FULL page that adds nothing means the list shifted under us (rows landed
   * on top, so the offset now points at rows we already hold). Re-anchor with a
   * fresh page 0 and try once more; if that also adds nothing, fail loudly. A
   * SHORT page that adds nothing is the real end of the list.
   */
  private async next(id: string, want: number, reanchored = false): Promise<number | null> {
    const cur = this.find(id);
    if (!cur) return 0;
    const limit = want + OVERLAP;
    let page: T[];
    try {
      page = withHost(
        await this.fetchBounded({ host: id, offset: Math.max(0, cur.rows.length - OVERLAP), limit }),
        id,
      );
    } catch (e) {
      if (!this.disposed) {
        this.patch(id, (s) => ({ ...s, pagingFailed: true, catchingUp: false, reason: apiErrorMessage(e) }));
      }
      return null;
    }
    if (this.disposed) return null;
    const held = this.find(id);
    if (!held) return 0;
    const rows = appendPage(held.rows, page, this.access, id);
    const added = rows.length - held.rows.length;
    const full = page.length === limit;
    if (full && added === 0) {
      if (reanchored) {
        this.patch(id, (s) => ({ ...s, pagingFailed: true, catchingUp: false, reason: 'list shifted while paging' }));
        return null;
      }
      const head = await this.head(id);
      if (this.disposed || !this.find(id)) return this.disposed ? null : 0;
      if (!head.ok) {
        // The host could not even answer page 0: say why, and do not spend another request on the page.
        this.patch(id, (s) => ({ ...s, pagingFailed: true, catchingUp: false, reason: head.reason }));
        return null;
      }
      return this.next(id, want, true);
    }
    this.patch(id, (s) => ({ ...s, rows, hasMore: full && added > 0 }));
    return added;
  }

  /** Record a failed page 0 and return its reason. */
  private fail(id: string, e: unknown): string {
    const f = classifyFailure(e);
    if (this.disposed) return f.reason;
    if (f.kind === 'gone' && this.allHosts && id !== 'local') {
      this.commit(this.slices.filter((s) => s.hostId !== id));
      return f.reason;
    }
    this.patch(id, (s) =>
      s.state === 'ok'
        ? { ...s, reason: f.reason } // stale-on-error: keep last-good rows
        : { ...s, state: f.kind === 'unavailable' ? 'unavailable' : 'offline', reason: f.reason, catchingUp: false },
    );
    return f.reason;
  }
}
