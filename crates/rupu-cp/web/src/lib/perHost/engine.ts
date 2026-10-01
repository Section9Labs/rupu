// PerHostListEngine — per-host list state outside React (spec §6.3).
//
// - Serializes work per host: at most one request in flight per host, with
//   a refresh that comes due while one is in flight coalesced behind it.
// - A host that answers late (first page, or recovering) JOINS by catching
//   up to the floor before it gates, so the visible list never gets shorter.
// - Splices refreshes with spliceHead and pages with a 5-row overlap.
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

export interface PerHostFetchParams {
  host: string;
  offset: number;
  limit: number;
}

export class PerHostListEngine<T extends HostTagged> {
  private slices: HostSlice<T>[] = [];
  private readonly queues = new Map<string, Promise<void>>();
  private readonly headQueued = new Set<string>();
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
  }

  /** Seed one `loading` slice per host and fire every host's page 0 independently. */
  start(hosts: readonly HostSeed[]): void {
    this.commit(hosts.map((h) => emptySlice<T>(h)));
    for (const h of hosts) void this.scheduleHead(h.id);
  }

  /** Manual Refresh: add new hosts, drop removed ones, refresh every host (unavailable ones included). */
  reconcile(hosts: readonly HostSeed[]): void {
    const byId = new Map(this.slices.map((s) => [s.hostId, s]));
    this.commit(hosts.map((h) => byId.get(h.id) ?? emptySlice<T>(h)));
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

  retryPaging(id: string): Promise<void> {
    return this.enqueue(id, () => this.join(id, { pagingFailed: false, reason: null }));
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
    const next = prev.then(() => (this.disposed ? undefined : job())).catch(() => undefined);
    this.queues.set(id, next);
    return next;
  }

  private async head(id: string): Promise<void> {
    if (!this.find(id)) return;
    let page: T[];
    try {
      page = withHost(await this.fetchPage({ host: id, offset: 0, limit: PAGE }), id);
    } catch (e) {
      this.fail(id, e);
      return;
    }
    const cur = this.find(id);
    if (this.disposed || !cur) return;
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
      return;
    }
    const { rows, fullyListed, replaced } = spliceHead(cur.rows, page, PAGE, this.access, id);
    this.patch(id, (s) => ({
      ...s,
      rows,
      hasMore: fullyListed ? false : replaced ? true : s.hasMore,
      reason: null,
      receivedAt: Date.now(),
    }));
  }

  /**
   * Bring a host into the merge without shortening the visible list: fetch
   * at least as many rows as it would push below the floor (the budget),
   * stopping early once it reaches the floor or runs out of rows. Then it
   * gates.
   */
  private async join(id: string, prePatch: Partial<HostSlice<T>> = {}): Promise<void> {
    const start = this.find(id);
    if (!start) return;
    this.patch(id, (s) => ({ ...s, ...prePatch, catchingUp: s.hasMore }));
    const others = () => this.slices.filter((o) => o.hostId !== id);
    const budget = displacedCount(
      watermarkMerge(others(), this.access).visible,
      coveredOf({ ...start, ...prePatch } as HostSlice<T>, this.access.timeOf),
      this.access.timeOf,
    );
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
    this.patch(id, (s) => ({ ...s, catchingUp: false }));
  }

  /** Fetch `want` more rows (+ overlap). Returns the count of NEW rows, or null on failure. */
  private async next(id: string, want: number): Promise<number | null> {
    const cur = this.find(id);
    if (!cur) return 0;
    const limit = want + OVERLAP;
    let page: T[];
    try {
      page = withHost(
        await this.fetchPage({ host: id, offset: Math.max(0, cur.rows.length - OVERLAP), limit }),
        id,
      );
    } catch (e) {
      if (!this.disposed) {
        this.patch(id, (s) => ({ ...s, pagingFailed: true, catchingUp: false, reason: apiErrorMessage(e) }));
      }
      return null;
    }
    if (this.disposed) return null;
    let added = 0;
    this.patch(id, (s) => {
      const rows = appendPage(s.rows, page, this.access, id);
      added = rows.length - s.rows.length;
      // A full page of nothing new means the offsets drifted (many rows
      // inserted above). Stop rather than re-request the same offset forever.
      return { ...s, rows, hasMore: page.length === limit && added > 0 };
    });
    return added;
  }

  private fail(id: string, e: unknown): void {
    if (this.disposed) return;
    const f = classifyFailure(e);
    if (f.kind === 'gone' && this.allHosts && id !== 'local') {
      this.commit(this.slices.filter((s) => s.hostId !== id));
      return;
    }
    this.patch(id, (s) =>
      s.state === 'ok'
        ? { ...s, reason: f.reason } // stale-on-error: keep last-good rows
        : { ...s, state: f.kind === 'unavailable' ? 'unavailable' : 'offline', reason: f.reason, catchingUp: false },
    );
  }
}
