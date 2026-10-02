// Watermark merge — the k-way merge behind per-host infinite scroll (spec §6.2).
//
// Each host is paged independently, so a row from one host can only be
// placed once every other host that still has more pages has loaded down to
// at least that row's time. The FLOOR is the newest of those "loaded down
// to" points. Rows below it are held back until the host(s) sitting at the
// floor load their next page.

import type { HostSlice, RowAccess } from './types';

/** Oldest instant this host has loaded, or `-Infinity` when it has nothing more to give. */
export function coveredOf<T>(s: HostSlice<T>, timeOf: (r: T) => number): number {
  if (!s.hasMore || s.rows.length === 0) return -Infinity;
  let min = Infinity;
  for (const r of s.rows) min = Math.min(min, timeOf(r));
  return min;
}

/** A host decides where the list currently ends only when it is healthy and has more. */
export function isGating<T>(s: HostSlice<T>): boolean {
  return s.state === 'ok' && s.hasMore && !s.catchingUp && !s.pagingFailed;
}

export function floorOf<T>(slices: readonly HostSlice<T>[], timeOf: (r: T) => number): number {
  let floor = -Infinity;
  for (const s of slices) if (isGating(s)) floor = Math.max(floor, coveredOf(s, timeOf));
  return floor;
}

/** Newest first; ties by merge key, i.e. (host_id, id). Equal `-Infinity` stamps tie too. */
export function compareNewestFirst(a: { t: number; key: string }, b: { t: number; key: string }): number {
  if (a.t !== b.t) return a.t > b.t ? -1 : 1;
  return a.key < b.key ? -1 : a.key > b.key ? 1 : 0;
}

export interface MergeResult<T> {
  /** Correctly-placed rows, newest first. */
  visible: T[];
  floor: number;
  /** Some host can still load older rows (drives the scroll sentinel). */
  hasMore: boolean;
  /** Nothing gates, and no host is still loading or catching up. */
  ended: boolean;
  /** The gating hosts sitting exactly at the floor: the next page comes from these only. */
  gatingHosts: string[];
}

export function watermarkMerge<T>(slices: readonly HostSlice<T>[], access: RowAccess<T>): MergeResult<T> {
  const floor = floorOf(slices, access.timeOf);
  const seen = new Set<string>();
  const entries: { row: T; t: number; key: string }[] = [];
  for (const s of slices) {
    for (const row of s.rows) {
      const t = access.timeOf(row);
      if (t < floor) continue;
      const key = access.keyOf(row, s.hostId);
      if (seen.has(key)) continue;
      seen.add(key);
      entries.push({ row, t, key });
    }
  }
  entries.sort(compareNewestFirst);
  const gating = slices.filter(isGating);
  return {
    visible: entries.map((e) => e.row),
    floor,
    hasMore: gating.length > 0,
    ended: slices.length > 0 && gating.length === 0 && !slices.some((s) => s.state === 'loading' || s.catchingUp),
    gatingHosts: gating.filter((s) => coveredOf(s, access.timeOf) === floor).map((s) => s.hostId),
  };
}

/** How many of `visible` would drop below the floor if a host covered to `covered` started gating. */
export function displacedCount<T>(visible: readonly T[], covered: number, timeOf: (r: T) => number): number {
  let n = 0;
  for (const r of visible) if (timeOf(r) < covered) n++;
  return n;
}
