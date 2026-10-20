// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  activeSnoozes,
  addDismissed,
  addSnooze,
  readDismissed,
  readSnoozed,
  removeDismissed,
  removeSnooze,
  snoozeUntil,
} from './inboxStore';

// jsdom's localStorage is unreliable under this Node version — install a
// simple in-memory implementation we fully control (same discipline as
// shellState.test.tsx / ThemeProvider.test.tsx).
function installLocalStorage() {
  const store = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => store.set(k, String(v)),
    removeItem: (k: string) => store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => Array.from(store.keys())[i] ?? null,
    get length() {
      return store.size;
    },
  });
}

beforeEach(() => {
  installLocalStorage();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('inboxStore dismiss', () => {
  it('adds, dedupes and removes dismissed ids, persisting across reads', () => {
    expect(readDismissed()).toEqual([]);
    addDismissed('run_a');
    addDismissed('run_a'); // dedupe
    addDismissed('run_b');
    expect(new Set(readDismissed())).toEqual(new Set(['run_a', 'run_b']));
    removeDismissed('run_a');
    expect(readDismissed()).toEqual(['run_b']);
  });
});

describe('inboxStore snooze', () => {
  it('stores a reappear time and removes it', () => {
    const until = Date.now() + 10_000;
    addSnooze('run_x', until);
    expect(readSnoozed()).toEqual({ run_x: until });
    removeSnooze('run_x');
    expect(readSnoozed()).toEqual({});
  });

  it('activeSnoozes prunes expired entries and persists the pruned map', () => {
    const now = 1_000_000;
    addSnooze('live', now + 5_000);
    addSnooze('stale', now - 5_000);
    const active = activeSnoozes(now);
    expect(active).toEqual({ live: now + 5_000 });
    // the stale one is gone from storage, not just from the returned view
    expect(readSnoozed()).toEqual({ live: now + 5_000 });
  });
});

describe('snoozeUntil', () => {
  it('1h is one hour out', () => {
    const now = Date.UTC(2026, 0, 2, 12, 0, 0);
    expect(snoozeUntil('1h', now)).toBe(now + 3_600_000);
  });

  it('1w is seven days out', () => {
    const now = Date.UTC(2026, 0, 2, 12, 0, 0);
    expect(snoozeUntil('1w', now)).toBe(now + 7 * 86_400_000);
  });

  it('tomorrow is the next calendar day at 09:00 local', () => {
    const now = new Date(2026, 0, 2, 15, 30, 0).getTime(); // Jan 2, 15:30 local
    const out = new Date(snoozeUntil('tomorrow', now));
    expect(out.getDate()).toBe(3);
    expect(out.getHours()).toBe(9);
    expect(out.getMinutes()).toBe(0);
  });
});
