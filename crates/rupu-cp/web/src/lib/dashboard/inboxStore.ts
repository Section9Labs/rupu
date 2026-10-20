// inboxStore — the Inbox's per-operator "snooze" and "dismiss" state.
//
// There is NO server state for snooze/dismiss (they're a view preference, not a
// run fact), so they live in localStorage, keyed by run id, on this browser
// only. Every read/write is wrapped in try/catch: storage can throw (private
// mode, blocked cookies) or come back empty, and the Inbox must render and work
// without it — a browser that can't persist simply never hides anything.
//
// Shapes:
//   dismissed  →  string[]              (run ids the operator cleared)
//   snoozed    →  Record<id, untilMs>   (run id → epoch ms it reappears)
//
// A snooze is just a dismiss with an expiry; `activeSnoozes` prunes any whose
// time has passed (and persists the pruned map) so an expired item comes back.

const DISMISS_KEY = 'rupu.cp.inbox.dismissed.v1';
const SNOOZE_KEY = 'rupu.cp.inbox.snoozed.v1';

export type SnoozeDuration = '1h' | 'tomorrow' | '1w';

/** The three fixed snooze choices, in menu order. */
export const SNOOZE_CHOICES: { key: SnoozeDuration; label: string }[] = [
  { key: '1h', label: 'for 1 hour' },
  { key: 'tomorrow', label: 'until tomorrow' },
  { key: '1w', label: 'for 1 week' },
];

/** When a snooze of `dur` taken at `now` should reappear (epoch ms). "tomorrow"
 *  is the next day at 09:00 local — a start-of-workday anchor, not now+24h. */
export function snoozeUntil(dur: SnoozeDuration, now: number): number {
  if (dur === '1h') return now + 3_600_000;
  if (dur === '1w') return now + 7 * 86_400_000;
  const d = new Date(now);
  d.setDate(d.getDate() + 1);
  d.setHours(9, 0, 0, 0);
  return d.getTime();
}

function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return fallback;
    const parsed = JSON.parse(raw) as unknown;
    return parsed == null ? fallback : (parsed as T);
  } catch {
    return fallback;
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // storage unavailable/full — the Inbox degrades to not persisting
  }
}

/** Run ids the operator has dismissed. Deduped; order is not meaningful. */
export function readDismissed(): string[] {
  const arr = readJson<string[]>(DISMISS_KEY, []);
  return Array.isArray(arr) ? arr.filter((x): x is string => typeof x === 'string') : [];
}

export function writeDismissed(ids: string[]): void {
  writeJson(DISMISS_KEY, [...new Set(ids)]);
}

/** Add `id` to the dismissed set and persist; returns the new set. */
export function addDismissed(id: string): string[] {
  const next = [...new Set([...readDismissed(), id])];
  writeDismissed(next);
  return next;
}

/** Remove `id` from the dismissed set and persist; returns the new set. */
export function removeDismissed(id: string): string[] {
  const next = readDismissed().filter((x) => x !== id);
  writeDismissed(next);
  return next;
}

/** The raw snooze map (id → reappear-at ms), unpruned. */
export function readSnoozed(): Record<string, number> {
  const obj = readJson<Record<string, number>>(SNOOZE_KEY, {});
  if (obj == null || typeof obj !== 'object' || Array.isArray(obj)) return {};
  const out: Record<string, number> = {};
  for (const [k, v] of Object.entries(obj)) {
    if (typeof v === 'number' && Number.isFinite(v)) out[k] = v;
  }
  return out;
}

export function writeSnoozed(map: Record<string, number>): void {
  writeJson(SNOOZE_KEY, map);
}

/** Snooze `id` until `until` (ms) and persist; returns the new map. */
export function addSnooze(id: string, until: number): Record<string, number> {
  const next = { ...readSnoozed(), [id]: until };
  writeSnoozed(next);
  return next;
}

/** Un-snooze `id` and persist; returns the new map. */
export function removeSnooze(id: string): Record<string, number> {
  const next = { ...readSnoozed() };
  delete next[id];
  writeSnoozed(next);
  return next;
}

/** The snoozes still in effect at `now`, with expired ones pruned and the
 *  pruned map persisted (so a lapsed item reappears exactly once). */
export function activeSnoozes(now: number): Record<string, number> {
  const all = readSnoozed();
  const active: Record<string, number> = {};
  let changed = false;
  for (const [id, until] of Object.entries(all)) {
    if (until > now) active[id] = until;
    else changed = true;
  }
  if (changed) writeSnoozed(active);
  return active;
}
